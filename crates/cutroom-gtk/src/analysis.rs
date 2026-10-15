//! Work GStreamer does for the editor besides playing: waveforms, timeline
//! filmstrip frames, finding out which decoder a file uses, and making
//! low-resolution proxy files. All of it blocks, so callers run it on worker threads.

use crate::media::{path_to_uri, Thumb};
use cutroom_core::waveform::{PeakBuilder, Peaks};
use cutroom_timeline::Ticks;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use std::path::Path;
use std::str::FromStr;
use std::time::{Duration, Instant};

fn clock(t: Ticks) -> gst::ClockTime {
    let ns = t.0.max(0) as u128 * 1_000_000_000 / cutroom_timeline::TICKS_PER_SECOND as u128;
    gst::ClockTime::from_nseconds(ns as u64)
}

// ----- waveform -----------------------------------------------------------

/// Decode all of a file's audio (mixed to mono) and reduce it to peaks.
pub fn compute_peaks(path: &Path) -> Result<Peaks, String> {
    const RATE: i32 = 8000;
    let pipeline = gst::Pipeline::new();
    let make = |name: &str| {
        gst::ElementFactory::make(name)
            .build()
            .map_err(|e| format!("{name}: {e}"))
    };
    let source = gst::ElementFactory::make("uridecodebin")
        .property("uri", path_to_uri(path))
        .build()
        .map_err(|e| e.to_string())?;
    let convert = make("audioconvert")?;
    let resample = make("audioresample")?;
    let caps = gst::Caps::builder("audio/x-raw")
        .field("format", "F32LE")
        .field("channels", 1i32)
        .field("rate", RATE)
        .build();
    let sink = gst_app::AppSink::builder().caps(&caps).sync(false).build();
    pipeline
        .add_many([&source, &convert, &resample, sink.upcast_ref()])
        .map_err(|e| e.to_string())?;
    gst::Element::link_many([&convert, &resample, sink.upcast_ref()]).map_err(|e| e.to_string())?;
    {
        let convert = convert.clone();
        source.connect_pad_added(move |_, pad| {
            let Some(sink_pad) = convert.static_pad("sink") else {
                return;
            };
            let is_audio = pad
                .current_caps()
                .or_else(|| Some(pad.query_caps(None)))
                .and_then(|c| c.structure(0).map(|s| s.name().starts_with("audio/")))
                .unwrap_or(false);
            if is_audio && !sink_pad.is_linked() {
                let _ = pad.link(&sink_pad);
            }
        });
    }

    let run = || -> Result<Peaks, String> {
        pipeline
            .set_state(gst::State::Playing)
            .map_err(|e| format!("could not start decoding: {e}"))?;
        let mut builder = PeakBuilder::new(RATE as u32);
        let mut got_audio = false;
        loop {
            match sink.try_pull_sample(gst::ClockTime::from_seconds(15)) {
                Some(sample) => {
                    let buffer = sample.buffer().ok_or("empty buffer")?;
                    let map = buffer.map_readable().map_err(|e| e.to_string())?;
                    let samples: Vec<f32> = map
                        .chunks_exact(4)
                        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                        .collect();
                    got_audio = true;
                    builder.push(&samples);
                }
                None => break,
            }
        }
        if !sink.is_eos() && !got_audio {
            return Err("no audio could be read".into());
        }
        Ok(builder.finish())
    };
    let result = run();
    let _ = pipeline.set_state(gst::State::Null);
    result
}

// ----- timeline filmstrip -------------------------------------------------

/// Frames of a video at the given times, each `height` pixels tall.
/// Stops early (returning what it has) if decoding takes too long.
pub fn filmstrip_frames(path: &Path, times: &[Ticks], height: i32) -> Vec<(Ticks, Thumb)> {
    let Ok(playbin) = gst::ElementFactory::make("playbin")
        .property("uri", path_to_uri(path))
        .build()
    else {
        return Vec::new();
    };
    let Ok(sink_bin) = gst::parse::bin_from_description(
        &format!(
            "videoconvert ! videoscale ! video/x-raw,format=RGBA,height={height},pixel-aspect-ratio=1/1 \
             ! appsink name=stripsink sync=false max-buffers=1"
        ),
        true,
    ) else {
        return Vec::new();
    };
    let Some(appsink) = sink_bin
        .by_name("stripsink")
        .and_then(|e| e.downcast::<gst_app::AppSink>().ok())
    else {
        return Vec::new();
    };
    playbin.set_property("video-sink", &sink_bin);
    if let Ok(fake) = gst::ElementFactory::make("fakesink").build() {
        playbin.set_property("audio-sink", &fake);
    }
    playbin.set_property_from_str("flags", "video");

    let mut frames = Vec::new();
    let started = Instant::now();
    if playbin.set_state(gst::State::Paused).is_ok()
        && appsink
            .try_pull_preroll(gst::ClockTime::from_seconds(8))
            .is_some()
    {
        for &time in times {
            if started.elapsed() > Duration::from_secs(25) {
                break;
            }
            let flags =
                gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT | gst::SeekFlags::SNAP_NEAREST;
            if playbin.seek_simple(flags, clock(time)).is_err() {
                continue;
            }
            if let Some(sample) = appsink.try_pull_preroll(gst::ClockTime::from_seconds(4)) {
                if let Some(thumb) = crate::media::sample_to_thumb(&sample) {
                    frames.push((time, thumb));
                }
            }
        }
    }
    let _ = playbin.set_state(gst::State::Null);
    frames
}

/// Times to take filmstrip frames at: about one a second, at most `max_frames`.
pub fn filmstrip_times(duration: Ticks, max_frames: usize) -> Vec<Ticks> {
    let seconds = duration.as_seconds_f64();
    if seconds <= 0.0 || max_frames == 0 {
        return Vec::new();
    }
    let step = (seconds / max_frames as f64).max(1.0);
    let mut times = Vec::new();
    let mut t = 0.0;
    while t < seconds && times.len() < max_frames {
        times.push(Ticks::from_seconds_f64(t));
        t += step;
    }
    times
}

// ----- decoders -----------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecoderInfo {
    pub factory: String,
    pub hardware: bool,
}

const HARDWARE_HINTS: [&str; 10] = [
    "vaapi", "nvdec", "nvh26", "nvav1", "d3d11", "d3d12", "qsv", "msdk", "v4l2", "vulkan",
];

fn is_hardware(factory: &gst::ElementFactory) -> bool {
    let name = factory.name().to_string();
    let klass = factory.metadata("klass").unwrap_or_default();
    klass.contains("Hardware")
        || HARDWARE_HINTS.iter().any(|h| name.contains(h))
        || (name.starts_with("va") && name.len() > 2 && name.ends_with("dec"))
}

/// Which video decoder GStreamer actually picks for this file.
pub fn decoder_in_use(path: &Path) -> Option<DecoderInfo> {
    let playbin = gst::ElementFactory::make("playbin")
        .property("uri", path_to_uri(path))
        .build()
        .ok()?;
    for (property, factory) in [("video-sink", "fakesink"), ("audio-sink", "fakesink")] {
        if let Ok(fake) = gst::ElementFactory::make(factory).build() {
            playbin.set_property(property, &fake);
        }
    }
    playbin.set_property_from_str("flags", "video");
    let result = (|| {
        playbin.set_state(gst::State::Paused).ok()?;
        let bus = playbin.bus()?;
        let message = bus.timed_pop_filtered(
            gst::ClockTime::from_seconds(10),
            &[gst::MessageType::AsyncDone, gst::MessageType::Error],
        )?;
        if matches!(message.view(), gst::MessageView::Error(_)) {
            return None;
        }
        let bin = playbin.downcast_ref::<gst::Bin>()?;
        let mut found = None;
        let mut iterator = bin.iterate_recurse();
        while let Ok(Some(element)) = iterator.next() {
            let Some(factory) = element.factory() else {
                continue;
            };
            let klass = factory.metadata("klass").unwrap_or_default();
            if klass.contains("Decoder") && klass.contains("Video") {
                found = Some(DecoderInfo {
                    factory: factory.name().to_string(),
                    hardware: is_hardware(&factory),
                });
                break;
            }
        }
        found
    })();
    let _ = playbin.set_state(gst::State::Null);
    result
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecSupport {
    pub codec: &'static str,
    /// Decoders able to read it, best first.
    pub decoders: Vec<DecoderInfo>,
}

/// For common video codecs, the decoders installed, in the order GStreamer
/// would prefer them.
pub fn decoder_choices() -> Vec<CodecSupport> {
    const CODECS: [(&str, &str); 6] = [
        ("H.264", "video/x-h264"),
        ("H.265 (HEVC)", "video/x-h265"),
        ("VP8", "video/x-vp8"),
        ("VP9", "video/x-vp9"),
        ("AV1", "video/x-av1"),
        ("ProRes", "video/x-prores"),
    ];
    let mut factories = gst::ElementFactory::factories_with_type(
        gst::ElementFactoryType::DECODER | gst::ElementFactoryType::MEDIA_VIDEO,
        gst::Rank::MARGINAL,
    )
    .into_iter()
    .collect::<Vec<_>>();
    factories.sort_by_key(|f| std::cmp::Reverse(f.rank()));
    CODECS
        .iter()
        .map(|(codec, caps)| {
            let caps = gst::Caps::from_str(caps).unwrap_or_else(|_| gst::Caps::new_empty());
            CodecSupport {
                codec,
                decoders: factories
                    .iter()
                    .filter(|f| f.can_sink_any_caps(&caps))
                    .map(|f| DecoderInfo {
                        factory: f.name().to_string(),
                        hardware: is_hardware(f),
                    })
                    .collect(),
            }
        })
        .collect()
}

/// One line per codec: `H.264: vah264dec (hardware), avdec_h264`.
pub fn decoder_lines(choices: &[CodecSupport]) -> Vec<String> {
    choices
        .iter()
        .map(|c| {
            if c.decoders.is_empty() {
                return format!("  {}: no decoder installed", c.codec);
            }
            let list: Vec<String> = c
                .decoders
                .iter()
                .take(3)
                .map(|d| {
                    if d.hardware {
                        format!("{} (hardware)", d.factory)
                    } else {
                        d.factory.clone()
                    }
                })
                .collect();
            format!("  {}: {}", c.codec, list.join(", "))
        })
        .collect()
}

// ----- proxies ------------------------------------------------------------

/// The size of a proxy: the longer side is at most `max_edge` (never enlarged),
/// both sides even, as video encoders want.
pub fn proxy_size(width: u32, height: u32, max_edge: u32) -> (u32, u32) {
    let longest = width.max(height).max(1);
    let scale = (f64::from(max_edge) / f64::from(longest)).min(1.0);
    let even = |v: f64| ((v.round() as u32).max(2) / 2) * 2;
    (
        even(f64::from(width) * scale),
        even(f64::from(height) * scale),
    )
}

/// Can this machine make proxies?
pub fn proxies_supported() -> bool {
    [
        "uridecodebin",
        "videoscale",
        "vp8enc",
        "vorbisenc",
        "webmmux",
        "filesink",
    ]
    .iter()
    .all(|name| gst::ElementFactory::find(name).is_some())
}

/// Re-encode `src` to a small WebM at `dest` (written to a temporary name and
/// renamed when complete). `progress` gets 0.0 to 1.0 about every 200 ms.
pub fn make_proxy(
    src: &Path,
    dest: &Path,
    width: u32,
    height: u32,
    progress: &dyn Fn(f64),
) -> Result<(), String> {
    let temp = dest.with_extension("part");
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let pipeline = gst::Pipeline::new();
    let make = |name: &str| {
        gst::ElementFactory::make(name)
            .build()
            .map_err(|e| format!("{name}: {e}"))
    };
    let source = gst::ElementFactory::make("uridecodebin")
        .property("uri", path_to_uri(src))
        .build()
        .map_err(|e| e.to_string())?;
    let mux = make("webmmux")?;
    let file = gst::ElementFactory::make("filesink")
        .property("location", temp.to_string_lossy().to_string())
        .build()
        .map_err(|e| e.to_string())?;
    pipeline
        .add_many([&source, &mux, &file])
        .map_err(|e| e.to_string())?;
    mux.link(&file).map_err(|e| e.to_string())?;

    // Only the streams the file really has are connected, so the muxer is
    // never left waiting for one that doesn't exist.
    {
        let pipeline = pipeline.downgrade();
        let mux = mux.clone();
        source.connect_pad_added(move |_, pad| {
            let Some(pipeline) = pipeline.upgrade() else {
                return;
            };
            let name = pad
                .current_caps()
                .or_else(|| Some(pad.query_caps(None)))
                .and_then(|c| c.structure(0).map(|s| s.name().to_string()))
                .unwrap_or_default();
            let chain: Vec<&str> = if name.starts_with("video/") {
                vec![
                    "queue",
                    "videoconvert",
                    "videoscale",
                    "capsfilter",
                    "vp8enc",
                    "queue",
                ]
            } else if name.starts_with("audio/") {
                vec![
                    "queue",
                    "audioconvert",
                    "audioresample",
                    "vorbisenc",
                    "queue",
                ]
            } else {
                return;
            };
            let mut elements = Vec::new();
            for factory in chain {
                let Ok(element) = gst::ElementFactory::make(factory).build() else {
                    return;
                };
                if factory == "capsfilter" {
                    let caps = gst::Caps::builder("video/x-raw")
                        .field("width", width as i32)
                        .field("height", height as i32)
                        .build();
                    element.set_property("caps", &caps);
                }
                if factory == "vp8enc" {
                    element.set_property("deadline", 1i64);
                    element.set_property("cpu-used", 8i32);
                    element.set_property("threads", 4i32);
                    element.set_property("keyframe-max-dist", 15i32);
                    element.set_property("target-bitrate", 2_000_000i32);
                }
                elements.push(element);
            }
            if pipeline.add_many(elements.iter()).is_err() {
                return;
            }
            let refs: Vec<&gst::Element> = elements.iter().collect();
            if gst::Element::link_many(refs.iter().copied()).is_err() {
                return;
            }
            let (Some(first), Some(last)) = (elements.first(), elements.last()) else {
                return;
            };
            let Some(sink_pad) = first.static_pad("sink") else {
                return;
            };
            let template = if name.starts_with("video/") {
                "video_%u"
            } else {
                "audio_%u"
            };
            let (Some(mux_pad), Some(src_pad)) =
                (mux.request_pad_simple(template), last.static_pad("src"))
            else {
                return;
            };
            for element in &elements {
                let _ = element.sync_state_with_parent();
            }
            if pad.link(&sink_pad).is_ok() {
                let _ = src_pad.link(&mux_pad);
            }
        });
    }

    let outcome = (|| -> Result<(), String> {
        pipeline
            .set_state(gst::State::Playing)
            .map_err(|e| format!("could not start: {e}"))?;
        let bus = pipeline.bus().ok_or("no message bus")?;
        let started = Instant::now();
        loop {
            if let Some(message) = bus.timed_pop_filtered(
                gst::ClockTime::from_mseconds(200),
                &[gst::MessageType::Eos, gst::MessageType::Error],
            ) {
                return match message.view() {
                    gst::MessageView::Eos(_) => Ok(()),
                    gst::MessageView::Error(e) => {
                        Err(format!("{} ({})", e.error(), e.debug().unwrap_or_default()))
                    }
                    _ => Ok(()),
                };
            }
            let position = pipeline.query_position::<gst::ClockTime>();
            let duration = pipeline.query_duration::<gst::ClockTime>();
            if let (Some(p), Some(d)) = (position, duration) {
                if d.nseconds() > 0 {
                    progress((p.nseconds() as f64 / d.nseconds() as f64).clamp(0.0, 1.0));
                }
            }
            if started.elapsed() > Duration::from_secs(60 * 60 * 3) {
                return Err("took too long".into());
            }
        }
    })();
    let _ = pipeline.set_state(gst::State::Null);
    match outcome {
        Ok(()) => std::fs::rename(&temp, dest).map_err(|e| e.to_string()),
        Err(e) => {
            let _ = std::fs::remove_file(&temp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::{probe_file, test_media::*};
    use tempfile::tempdir;

    #[test]
    fn peaks_come_out_at_a_hundred_a_second_and_are_not_silent() {
        if !init() {
            return;
        }
        let tmp = tempdir().unwrap();
        let wav = tmp.path().join("tone.wav");
        make_wav(&wav, 3);
        let peaks = compute_peaks(&wav).unwrap();
        let real = probe_file(&wav).unwrap().duration.unwrap().as_seconds_f64();
        assert!(
            (peaks.duration_seconds() - real).abs() < 0.1,
            "peaks cover {} s, the file is {real} s",
            peaks.duration_seconds()
        );
        assert!(
            *peaks.data.iter().max().unwrap() > 20,
            "a test tone is not silence"
        );
    }

    #[test]
    fn peaks_also_come_from_the_sound_of_a_video() {
        if !init() {
            return;
        }
        let tmp = tempdir().unwrap();
        let video = tmp.path().join("v.webm");
        make_video(&video, 2);
        let peaks = compute_peaks(&video).unwrap();
        assert!((peaks.duration_seconds() - 2.0).abs() < 0.2);
    }

    #[test]
    fn a_file_without_audio_gives_an_error_not_a_hang() {
        if !init() {
            return;
        }
        let tmp = tempdir().unwrap();
        let png = tmp.path().join("p.png");
        make_png(&png, 32, 32);
        let started = Instant::now();
        let result = compute_peaks(&png);
        assert!(result.is_err() || result.unwrap().data.is_empty());
        assert!(started.elapsed() < Duration::from_secs(20));
    }

    #[test]
    fn filmstrip_times_are_about_a_second_apart_and_capped() {
        let ten = filmstrip_times(Ticks::from_seconds_f64(10.0), 80);
        assert_eq!(ten.len(), 10);
        assert_eq!(ten[1], Ticks::from_seconds_f64(1.0));
        let long = filmstrip_times(Ticks::from_seconds_f64(1000.0), 80);
        assert_eq!(long.len(), 80, "capped; spaced wider");
        assert!(long[1].as_seconds_f64() > 12.0);
        assert!(filmstrip_times(Ticks::ZERO, 80).is_empty());
    }

    #[test]
    fn filmstrip_frames_are_taken_at_the_requested_height() {
        if !init() {
            return;
        }
        let tmp = tempdir().unwrap();
        let video = tmp.path().join("v.webm");
        make_video(&video, 4);
        let times = filmstrip_times(Ticks::from_seconds_f64(4.0), 80);
        let frames = filmstrip_frames(&video, &times, 48);
        assert!(frames.len() >= 3, "got {} of {}", frames.len(), times.len());
        for (_, thumb) in &frames {
            assert_eq!(thumb.height, 48);
            assert_eq!(thumb.width, 64, "320x240 scaled to 48 tall");
            assert_eq!(thumb.rgba.len(), (thumb.width * thumb.height * 4) as usize);
        }
    }

    #[test]
    fn the_decoder_in_use_is_reported_for_a_real_file() {
        if !init() {
            return;
        }
        let tmp = tempdir().unwrap();
        let video = tmp.path().join("v.webm");
        make_video(&video, 1);
        let info = decoder_in_use(&video).expect("a decoder is found");
        assert!(info.factory.contains("vp8"), "{info:?}");
        assert!(!info.hardware, "software decoding of VP8 here: {info:?}");
    }

    #[test]
    fn decoder_choices_list_the_common_codecs() {
        if gst::init().is_err() {
            return;
        }
        let choices = decoder_choices();
        assert_eq!(choices.len(), 6);
        let lines = decoder_lines(&choices);
        assert_eq!(lines.len(), 6);
        assert!(lines.iter().all(|l| l.starts_with("  ")));
        let vp8 = choices.iter().find(|c| c.codec == "VP8").unwrap();
        if gst::ElementFactory::find("vp8dec").is_some() {
            assert!(vp8.decoders.iter().any(|d| d.factory == "vp8dec"));
        }
    }

    #[test]
    fn lines_say_hardware_and_missing() {
        let lines = decoder_lines(&[
            CodecSupport {
                codec: "H.264",
                decoders: vec![
                    DecoderInfo {
                        factory: "vah264dec".into(),
                        hardware: true,
                    },
                    DecoderInfo {
                        factory: "avdec_h264".into(),
                        hardware: false,
                    },
                ],
            },
            CodecSupport {
                codec: "AV1",
                decoders: vec![],
            },
        ]);
        assert_eq!(lines[0], "  H.264: vah264dec (hardware), avdec_h264");
        assert_eq!(lines[1], "  AV1: no decoder installed");
    }

    #[test]
    fn proxy_sizes_shrink_the_long_side_and_stay_even() {
        assert_eq!(proxy_size(1920, 1080, 960), (960, 540));
        assert_eq!(proxy_size(1080, 1920, 960), (540, 960), "vertical video");
        assert_eq!(proxy_size(3840, 2160, 960), (960, 540));
        assert_eq!(proxy_size(640, 360, 960), (640, 360), "never enlarged");
        assert_eq!(proxy_size(1001, 563, 500).0 % 2, 0);
        assert_eq!(
            proxy_size(0, 0, 960),
            (2, 2),
            "degenerate input stays valid"
        );
    }

    #[test]
    fn a_proxy_is_a_smaller_file_with_the_same_length_and_sound() {
        if !init() || !proxies_supported() {
            return;
        }
        let tmp = tempdir().unwrap();
        let (video, proxy) = (
            tmp.path().join("v.webm"),
            tmp.path().join("proxies").join("p.webm"),
        );
        make_video(&video, 3);
        let last = std::cell::Cell::new(0.0);
        make_proxy(&video, &proxy, 160, 120, &|p| last.set(p)).unwrap();
        assert!(proxy.is_file());
        assert!(
            !proxy.with_extension("part").exists(),
            "no temporary file is left"
        );
        let result = probe_file(&proxy).unwrap();
        assert!(result.probe.has_video && result.probe.has_audio);
        assert_eq!((result.probe.width, result.probe.height), (160, 120));
        let seconds = result.duration.unwrap().as_seconds_f64();
        assert!((seconds - 3.0).abs() < 0.3, "length {seconds}");
    }

    #[test]
    fn a_proxy_of_a_video_without_sound_does_not_hang() {
        if !init() || !proxies_supported() {
            return;
        }
        let tmp = tempdir().unwrap();
        let silent = tmp.path().join("silent.webm");
        let status = gst::parse::launch(&format!(
            "webmmux name=mux ! filesink location=\"{}\" videotestsrc num-buffers=60 ! video/x-raw,width=160,height=120,framerate=30/1 ! videoconvert ! vp8enc deadline=1 ! mux.",
            silent.display()
        ))
        .unwrap();
        status.set_state(gst::State::Playing).unwrap();
        status
            .bus()
            .unwrap()
            .timed_pop_filtered(gst::ClockTime::from_seconds(20), &[gst::MessageType::Eos]);
        status.set_state(gst::State::Null).unwrap();
        let proxy = tmp.path().join("silent-proxy.webm");
        make_proxy(&silent, &proxy, 80, 60, &|_| {}).unwrap();
        let result = probe_file(&proxy).unwrap();
        assert!(result.probe.has_video && !result.probe.has_audio);
    }

    #[test]
    fn a_missing_source_fails_cleanly_and_leaves_nothing_behind() {
        if !init() || !proxies_supported() {
            return;
        }
        let tmp = tempdir().unwrap();
        let dest = tmp.path().join("out.webm");
        assert!(make_proxy(&tmp.path().join("nope.mp4"), &dest, 80, 60, &|_| {}).is_err());
        assert!(!dest.exists() && !dest.with_extension("part").exists());
    }
}
