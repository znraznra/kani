//! Reading the sound of a clip as 16 kHz mono floats, which is what speech
//! recognition wants.

use crate::media::path_to_uri;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

pub const RATE: usize = 16_000;

/// The audio of `path` from `from` seconds, `length` seconds long (less if
/// the file ends sooner). Blocks; run it on a worker thread.
pub fn decode_range(
    path: &Path,
    from: f64,
    length: f64,
    cancel: &AtomicBool,
) -> Result<Vec<f32>, String> {
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
        .field("rate", RATE as i32)
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

    let first = (from.max(0.0) * RATE as f64) as usize;
    let last = first + (length.max(0.0) * RATE as f64) as usize;
    let run = || -> Result<Vec<f32>, String> {
        pipeline
            .set_state(gst::State::Playing)
            .map_err(|e| format!("could not start decoding: {e}"))?;
        let mut out = Vec::new();
        let mut at = 0usize;
        let mut got_audio = false;
        while at < last {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            let Some(sample) = sink.try_pull_sample(gst::ClockTime::from_seconds(15)) else {
                break;
            };
            got_audio = true;
            let buffer = sample.buffer().ok_or("empty buffer")?;
            let map = buffer.map_readable().map_err(|e| e.to_string())?;
            for b in map.chunks_exact(4) {
                if at >= first && at < last {
                    out.push(f32::from_le_bytes([b[0], b[1], b[2], b[3]]));
                }
                at += 1;
            }
        }
        if !got_audio {
            return Err("no audio could be read".into());
        }
        Ok(out)
    };
    let result = run();
    let _ = pipeline.set_state(gst::State::Null);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::test_media;

    #[test]
    fn a_stretch_of_sound_comes_out_at_16k_mono() {
        if !test_media::init() {
            eprintln!("GStreamer plugins unavailable: skipping");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("tone.wav");
        test_media::make_wav(&wav, 4);
        let cancel = AtomicBool::new(false);
        let all = decode_range(&wav, 0.0, 100.0, &cancel).unwrap();
        // About four seconds of sound (the test tone's length is only approximate).
        let secs = all.len() as f64 / RATE as f64;
        assert!((3.9..4.6).contains(&secs), "{secs} seconds");
        assert!(all.iter().any(|s| s.abs() > 0.01), "not silence");
        let part = decode_range(&wav, 1.0, 1.5, &cancel).unwrap();
        assert!(
            (part.len() as i64 - (1.5 * RATE as f64) as i64).abs() < 64,
            "{} samples",
            part.len()
        );
        let cancelled = AtomicBool::new(true);
        assert!(decode_range(&wav, 0.0, 4.0, &cancelled).is_err());
        assert!(decode_range(&dir.path().join("nothing.wav"), 0.0, 1.0, &cancel).is_err());
    }
}
