//! Reading media files with GStreamer: probing (length and streams) and
//! thumbnails. Everything here is blocking; callers run it on a worker thread.

use cutroom_project::MediaProbe;
use cutroom_timeline::{FrameRate, Ticks};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_pbutils as pbutils;
use gtk4::gio;
use gtk4::prelude::*;
use std::path::Path;

/// Longest edge of a library thumbnail, in pixels.
pub const THUMB_EDGE: i32 = 192;

pub fn path_to_uri(path: &Path) -> String {
    gio::File::for_path(path).uri().to_string()
}

pub struct ProbeResult {
    /// `None` when the file has no meaningful length (still images).
    pub duration: Option<Ticks>,
    pub probe: MediaProbe,
}

fn ticks_from(time: gst::ClockTime) -> Ticks {
    Ticks::from_seconds_f64(time.nseconds() as f64 / 1_000_000_000.0)
}

/// Read a file's length and streams.
pub fn probe_file(path: &Path) -> Result<ProbeResult, String> {
    let discoverer =
        pbutils::Discoverer::new(gst::ClockTime::from_seconds(20)).map_err(|e| e.to_string())?;
    let info = discoverer
        .discover_uri(&path_to_uri(path))
        .map_err(|e| e.to_string())?;
    let video = info.video_streams();
    let audio = info.audio_streams();
    let first = video.first();
    let frame_rate = first.and_then(|v| {
        let f = v.framerate();
        (f.numer() > 0 && f.denom() > 0).then(|| FrameRate::new(f.numer() as u32, f.denom() as u32))
    });
    Ok(ProbeResult {
        duration: info.duration().map(ticks_from),
        probe: MediaProbe {
            has_video: !video.is_empty(),
            has_audio: !audio.is_empty(),
            width: first.map_or(0, |v| v.width()),
            height: first.map_or(0, |v| v.height()),
            frame_rate,
        },
    })
}

/// A decoded thumbnail: tightly packed RGBA.
pub struct Thumb {
    pub width: i32,
    pub height: i32,
    pub rgba: Vec<u8>,
}

pub(crate) fn sample_to_thumb(sample: &gst::Sample) -> Option<Thumb> {
    let caps = sample.caps()?;
    let info = gstreamer_video::VideoInfo::from_caps(caps).ok()?;
    let buffer = sample.buffer()?;
    let map = buffer.map_readable().ok()?;
    let (width, height) = (info.width() as usize, info.height() as usize);
    let stride = info.stride()[0] as usize;
    let mut rgba = Vec::with_capacity(width * height * 4);
    for row in 0..height {
        let start = row * stride;
        rgba.extend_from_slice(map.get(start..start + width * 4)?);
    }
    Some(Thumb {
        width: width as i32,
        height: height as i32,
        rgba,
    })
}

/// A frame from a video, shrunk to `THUMB_EDGE` wide, taken a little way in.
pub fn video_thumbnail(path: &Path) -> Option<Thumb> {
    let playbin = gst::ElementFactory::make("playbin")
        .property("uri", path_to_uri(path))
        .build()
        .ok()?;
    let sink_bin = gst::parse::bin_from_description(
        &format!(
            "videoconvert ! videoscale ! video/x-raw,format=RGBA,width={THUMB_EDGE},pixel-aspect-ratio=1/1 \
             ! appsink name=thumbsink sync=false max-buffers=1"
        ),
        true,
    )
    .ok()?;
    let appsink = sink_bin
        .by_name("thumbsink")?
        .downcast::<gst_app::AppSink>()
        .ok()?;
    playbin.set_property("video-sink", &sink_bin);
    if let Ok(fake) = gst::ElementFactory::make("fakesink").build() {
        playbin.set_property("audio-sink", &fake);
    }
    playbin.set_property_from_str("flags", "video");

    let result = (|| {
        playbin.set_state(gst::State::Paused).ok()?;
        let first = appsink.try_pull_preroll(gst::ClockTime::from_seconds(8))?;
        // Try for a frame a little way in (the first one is often black).
        let target = playbin
            .query_duration::<gst::ClockTime>()
            .map(|d| std::cmp::min(gst::ClockTime::from_seconds(1), d / 10));
        if let Some(target) = target.filter(|t| t.nseconds() > 0) {
            let flags = gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT;
            if playbin.seek_simple(flags, target).is_ok() {
                if let Some(later) = appsink.try_pull_preroll(gst::ClockTime::from_seconds(4)) {
                    return sample_to_thumb(&later).or_else(|| sample_to_thumb(&first));
                }
            }
        }
        sample_to_thumb(&first)
    })();
    let _ = playbin.set_state(gst::State::Null);
    result
}

/// A still image shrunk to `THUMB_EDGE`.
pub fn image_thumbnail(path: &Path) -> Option<Thumb> {
    let pixbuf =
        gtk4::gdk_pixbuf::Pixbuf::from_file_at_scale(path, THUMB_EDGE, THUMB_EDGE, true).ok()?;
    let pixbuf = if pixbuf.has_alpha() {
        pixbuf
    } else {
        pixbuf.add_alpha(false, 0, 0, 0).ok()?
    };
    let (width, height) = (pixbuf.width() as usize, pixbuf.height() as usize);
    let stride = pixbuf.rowstride() as usize;
    let bytes = pixbuf.read_pixel_bytes();
    let mut rgba = Vec::with_capacity(width * height * 4);
    for row in 0..height {
        let start = row * stride;
        rgba.extend_from_slice(bytes.get(start..start + width * 4)?);
    }
    Some(Thumb {
        width: width as i32,
        height: height as i32,
        rgba,
    })
}

#[cfg(test)]
pub(crate) mod test_media {
    use super::*;
    use std::path::Path;

    pub fn init() -> bool {
        gst::init().is_ok()
            && [
                "vp8enc",
                "webmmux",
                "vorbisenc",
                "audiotestsrc",
                "wavenc",
                "playbin",
                "appsink",
            ]
            .iter()
            .all(|name| gst::ElementFactory::find(name).is_some())
    }

    fn run_to_eos(description: &str) {
        let pipeline = gst::parse::launch(description).expect("test pipeline");
        pipeline.set_state(gst::State::Playing).unwrap();
        let bus = pipeline.bus().unwrap();
        let message = bus.timed_pop_filtered(
            gst::ClockTime::from_seconds(30),
            &[gst::MessageType::Eos, gst::MessageType::Error],
        );
        pipeline.set_state(gst::State::Null).unwrap();
        assert!(
            matches!(
                message.as_ref().map(|m| m.view()),
                Some(gst::MessageView::Eos(_))
            ),
            "test media pipeline failed: {message:?}"
        );
    }

    /// A small WebM with video and audio, `seconds` long (30 fps, 320x240).
    pub fn make_video(path: &Path, seconds: u32) {
        run_to_eos(&format!(
            "webmmux name=mux ! filesink location=\"{}\" \
             videotestsrc num-buffers={} ! video/x-raw,width=320,height=240,framerate=30/1 \
             ! videoconvert ! vp8enc deadline=1 ! mux. \
             audiotestsrc num-buffers={} samplesperbuffer=1024 ! audioconvert ! vorbisenc ! mux.",
            path.display(),
            seconds * 30,
            seconds * 48000 / 1024
        ));
    }

    pub fn make_wav(path: &Path, seconds: u32) {
        run_to_eos(&format!(
            "audiotestsrc num-buffers={} samplesperbuffer=1024 ! audioconvert ! wavenc ! filesink location=\"{}\"",
            seconds * 48000 / 1024,
            path.display()
        ));
    }

    pub fn make_png(path: &Path, w: i32, h: i32) {
        let pixbuf =
            gtk4::gdk_pixbuf::Pixbuf::new(gtk4::gdk_pixbuf::Colorspace::Rgb, false, 8, w, h)
                .unwrap();
        pixbuf.fill(0x3366ccff);
        pixbuf.savev(path, "png", &[]).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::test_media::*;
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn uris_are_built_from_paths() {
        let uri = path_to_uri(Path::new("/tmp/a b.mp4"));
        assert_eq!(uri, "file:///tmp/a%20b.mp4");
    }

    #[test]
    fn probing_a_video_reports_streams_size_and_length() {
        if !init() {
            return;
        }
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("clip.webm");
        make_video(&path, 2);
        let result = probe_file(&path).unwrap();
        assert!(result.probe.has_video && result.probe.has_audio);
        assert_eq!((result.probe.width, result.probe.height), (320, 240));
        assert_eq!(result.probe.frame_rate, Some(FrameRate::FPS_30));
        let seconds = result.duration.unwrap().as_seconds_f64();
        assert!((seconds - 2.0).abs() < 0.3, "duration {seconds}");
    }

    #[test]
    fn probing_audio_finds_no_video() {
        if !init() {
            return;
        }
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("tone.wav");
        make_wav(&path, 3);
        let result = probe_file(&path).unwrap();
        assert!(!result.probe.has_video && result.probe.has_audio);
        assert!((result.duration.unwrap().as_seconds_f64() - 3.0).abs() < 0.3);
    }

    #[test]
    fn probing_a_missing_or_broken_file_fails_cleanly() {
        if !init() {
            return;
        }
        let tmp = tempdir().unwrap();
        assert!(probe_file(&tmp.path().join("nope.mp4")).is_err());
        let junk = tmp.path().join("junk.mp4");
        std::fs::write(&junk, b"this is not media").unwrap();
        let result = probe_file(&junk);
        assert!(
            result.is_err()
                || (!result.as_ref().unwrap().probe.has_video && !result.unwrap().probe.has_audio)
        );
    }

    #[test]
    fn video_thumbnails_are_scaled_rgba() {
        if !init() {
            return;
        }
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("clip.webm");
        make_video(&path, 2);
        let thumb = video_thumbnail(&path).expect("a thumbnail");
        assert_eq!(thumb.width, THUMB_EDGE);
        assert_eq!(thumb.height, 144, "keeps the 4:3 shape");
        assert_eq!(thumb.rgba.len(), (thumb.width * thumb.height * 4) as usize);
        assert!(thumb.rgba.iter().any(|b| *b != 0), "not an empty frame");
    }

    #[test]
    fn image_thumbnails_keep_the_aspect_ratio() {
        if !init() {
            return;
        }
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("wide.png");
        make_png(&path, 400, 200);
        let thumb = image_thumbnail(&path).unwrap();
        assert_eq!((thumb.width, thumb.height), (192, 96));
        assert_eq!(thumb.rgba.len(), 192 * 96 * 4);
        assert!(image_thumbnail(&tmp.path().join("missing.png")).is_none());
    }
}
