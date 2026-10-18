//! Where the preview's sound goes.
//!
//! GStreamer's playbin normally picks a sound output by itself. On Windows
//! that choice gave silence in Kani while GStreamer's own tools played sound,
//! so Kani now names the output itself, from a list per system (WASAPI first
//! on Windows), and when an output fails it moves to the next one instead of
//! muting the preview. `KANI_AUDIO_SINK=<element>` puts one first. The output
//! in use, and every sound error, is printed on the console.

use gstreamer as gst;
use gstreamer::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Which entry of [`candidates`] is in use.
static CURRENT: AtomicUsize = AtomicUsize::new(0);
static ANNOUNCED: AtomicBool = AtomicBool::new(false);

/// The sound outputs to try, best first, for `os` ("windows", "macos", "linux"...).
pub fn candidates_for(os: &str, wanted: Option<&str>) -> Vec<String> {
    let mut list: Vec<String> = Vec::new();
    if let Some(w) = wanted.map(str::trim).filter(|w| !w.is_empty()) {
        list.push(w.to_string());
    }
    let defaults: &[&str] = match os {
        "windows" => &[
            "wasapi2sink",
            "wasapisink",
            "directsoundsink",
            "autoaudiosink",
        ],
        "macos" => &["osxaudiosink", "autoaudiosink"],
        _ => &["autoaudiosink", "pulsesink", "pipewiresink", "alsasink"],
    };
    for d in defaults {
        if !list.iter().any(|l| l == d) {
            list.push((*d).to_string());
        }
    }
    list
}

fn candidates() -> Vec<String> {
    candidates_for(
        std::env::consts::OS,
        std::env::var("KANI_AUDIO_SINK").ok().as_deref(),
    )
}

/// The output in use now (one that exists on this machine), if any.
pub fn current_name() -> Option<String> {
    let list = candidates();
    let start = CURRENT.load(Ordering::Relaxed);
    let found = list
        .iter()
        .enumerate()
        .skip(start)
        .find(|(_, n)| gst::ElementFactory::find(n).is_some());
    if let Some((i, name)) = found {
        if i != start {
            CURRENT.store(i, Ordering::Relaxed);
        }
        Some(name.clone())
    } else {
        None
    }
}

/// A new sound output element for one player.
pub fn make_sink() -> Option<gst::Element> {
    let name = current_name()?;
    let sink = gst::ElementFactory::make(&name).build().ok()?;
    if !ANNOUNCED.swap(true, Ordering::Relaxed) {
        eprintln!("kani: preview sound goes to {name}");
    }
    Some(sink)
}

/// The output in use failed: move to the next one. Returns its name, or
/// `None` when there is nothing left to try.
pub fn next_after_failure(why: &str) -> Option<String> {
    let failed = current_name().unwrap_or_default();
    CURRENT.fetch_add(1, Ordering::Relaxed);
    let next = current_name();
    match &next {
        Some(n) => eprintln!("kani: sound output {failed} failed ({why}); trying {n}"),
        None => {
            eprintln!("kani: sound output {failed} failed ({why}); no other sound output to try")
        }
    }
    ANNOUNCED.store(false, Ordering::Relaxed);
    next
}

/// Did this error come from the sound side of a pipeline?
pub fn is_sound_error(src: Option<&gst::Object>) -> bool {
    let mut object = src.cloned();
    while let Some(o) = object {
        if let Some(element) = o.downcast_ref::<gst::Element>() {
            if let Some(f) = element.factory() {
                let klass = f.klass();
                if klass.contains("Audio") && klass.contains("Sink") {
                    return true;
                }
            }
        }
        let name = o.name().to_lowercase();
        // playbin's sound branch: "abin", "aconv", "audiosink", "asink"...
        if name.starts_with("abin")
            || name.starts_with("aconv")
            || name.contains("audiosink")
            || name.starts_with("asink")
        {
            return true;
        }
        object = o.parent();
    }
    false
}

/// Play a short tone through the sound output in use. Blocks up to 4 s.
pub fn test_tone() -> Result<String, String> {
    gst::init().map_err(|e| e.to_string())?;
    let name = current_name().ok_or("no sound output element is installed")?;
    let pipeline = gst::parse::launch(&format!(
        "audiotestsrc num-buffers=40 freq=660 volume=0.3 ! audioconvert ! audioresample ! {name}"
    ))
    .map_err(|e| format!("{name}: {e}"))?;
    pipeline
        .set_state(gst::State::Playing)
        .map_err(|e| format!("{name} could not start: {e}"))?;
    let bus = pipeline.bus().ok_or("no bus")?;
    let result = match bus.timed_pop_filtered(
        gst::ClockTime::from_seconds(4),
        &[gst::MessageType::Eos, gst::MessageType::Error],
    ) {
        Some(m) => match m.view() {
            gst::MessageView::Error(e) => Err(format!("{name}: {}", e.error())),
            _ => Ok(name.clone()),
        },
        None => Err(format!("{name}: the tone did not finish in time")),
    };
    let _ = pipeline.set_state(gst::State::Null);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_system_tries_its_own_outputs_first() {
        assert_eq!(candidates_for("windows", None)[0], "wasapi2sink");
        assert!(candidates_for("windows", None).contains(&"directsoundsink".to_string()));
        assert_eq!(candidates_for("linux", None)[0], "autoaudiosink");
        assert_eq!(candidates_for("macos", None)[0], "osxaudiosink");
        let wanted = candidates_for("windows", Some("directsoundsink"));
        assert_eq!(wanted[0], "directsoundsink");
        assert_eq!(wanted.iter().filter(|n| *n == "directsoundsink").count(), 1);
    }

    /// Not a check: what this machine's sound output does.
    /// `cargo test -p cutroom-gtk sound::tests::tone -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn tone() {
        eprintln!("candidates {:?}", candidates());
        eprintln!("tone: {:?}", test_tone());
        let _ = next_after_failure("pretend");
        eprintln!("after one failure: {:?}", current_name());
    }

    #[test]
    fn errors_from_the_sound_branch_are_told_apart() {
        let _ = gst::init();
        let Ok(sink) = gst::ElementFactory::make("fakesink")
            .name("audiosink-actual")
            .build()
        else {
            return;
        };
        assert!(is_sound_error(Some(sink.upcast_ref())));
        let video = gst::ElementFactory::make("fakesink")
            .name("videosink")
            .build()
            .unwrap();
        assert!(!is_sound_error(Some(video.upcast_ref())));
        assert!(!is_sound_error(None));
    }
}
