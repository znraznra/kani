//! Is the graphics card usable for putting a picture together?
//!
//! Export composites on the processor today. This module finds out whether
//! GStreamer's OpenGL plug-ins can run here (cheap check, `available`) and, on
//! request, times a real two-layer composite on the card against the
//! processor path (`test`), so the result can be reported before anything
//! depends on it.

use cutroom_compose::{Canvas, Frame, Placement};
use gstreamer as gst;
use gstreamer::prelude::*;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// The GStreamer elements GPU compositing would use.
const NEEDED: [&str; 4] = ["glupload", "glcolorconvert", "glvideomixer", "gldownload"];

/// Frames put together in the timing test.
const FRAMES: u32 = 120;

/// What the test found.
#[derive(Debug, Clone)]
pub struct GpuReport {
    pub usable: bool,
    /// One line for the window.
    pub summary: String,
    /// Detail for the console.
    pub details: Vec<String>,
}

/// The needed GStreamer OpenGL elements that are missing (empty when all there).
pub fn missing_elements() -> Vec<&'static str> {
    NEEDED
        .iter()
        .copied()
        .filter(|name| gst::ElementFactory::find(name).is_none())
        .collect()
}

/// Whether the OpenGL plug-ins are installed. This does not prove a card
/// works - `test` does - but it is what a quick check can tell.
pub fn available() -> bool {
    static CACHE: OnceLock<bool> = OnceLock::new();
    *CACHE.get_or_init(|| {
        let _ = gst::init();
        missing_elements().is_empty()
    })
}

/// Time `FRAMES` composites of a 1440x1080 picture over a second one on a
/// 1080x1920 canvas, on the card and on the processor.
pub fn test() -> GpuReport {
    let _ = gst::init();
    let mut details = Vec::new();
    let missing = missing_elements();
    if !missing.is_empty() {
        let names = missing.join(", ");
        details.push(format!("GStreamer has no {names} plug-in"));
        return GpuReport {
            usable: false,
            summary: format!(
                "Not available: GStreamer's OpenGL plug-ins are missing ({names}). Kani composites on the processor."
            ),
            details,
        };
    }
    let cpu = cpu_time();
    details.push(format!(
        "processor: {FRAMES} composites in {:.2} s ({:.1} ms each)",
        cpu.as_secs_f64(),
        cpu.as_secs_f64() * 1000.0 / f64::from(FRAMES)
    ));
    match gpu_time() {
        Ok(gpu) => {
            details.push(format!(
                "graphics card: {FRAMES} composites in {:.2} s ({:.1} ms each, including upload and read-back)",
                gpu.as_secs_f64(),
                gpu.as_secs_f64() * 1000.0 / f64::from(FRAMES)
            ));
            let ratio = cpu.as_secs_f64() / gpu.as_secs_f64().max(1e-6);
            GpuReport {
                usable: true,
                summary: format!(
                    "Works: {:.1} ms per picture on the card, {:.1} ms on the processor ({ratio:.1}x). Not used for export yet.",
                    gpu.as_secs_f64() * 1000.0 / f64::from(FRAMES),
                    cpu.as_secs_f64() * 1000.0 / f64::from(FRAMES)
                ),
                details,
            }
        }
        Err(why) => {
            details.push(format!("graphics card test failed: {why}"));
            GpuReport {
                usable: false,
                summary: format!(
                    "The OpenGL plug-ins are installed but a test failed ({why}). Kani composites on the processor."
                ),
                details,
            }
        }
    }
}

fn cpu_time() -> Duration {
    let (w, h) = (1080u32, 1920u32);
    let picture = |shade: u8| Frame {
        width: 1440,
        height: 1080,
        rgba: [shade, 90, 200 - shade / 2, 255]
            .iter()
            .copied()
            .cycle()
            .take(1440 * 1080 * 4)
            .collect(),
    };
    let (a, b) = (picture(40), picture(160));
    let under = Placement {
        scale: 2.554,
        ..Placement::default()
    };
    let over = Placement {
        scale: 1.0,
        y: 0.15,
        ..Placement::default()
    };
    let started = Instant::now();
    for _ in 0..FRAMES {
        let mut canvas = Canvas::new(w, h);
        canvas.draw(&a, &under);
        canvas.draw(&b, &over);
        std::hint::black_box(&canvas.rgba);
    }
    started.elapsed()
}

fn gpu_time() -> Result<Duration, String> {
    let layer = |pattern: &str| {
        format!(
            "videotestsrc num-buffers={FRAMES} pattern={pattern} ! video/x-raw,format=RGBA,width=1440,height=1080,framerate=30/1 \
             ! glupload ! glcolorconvert ! mix."
        )
    };
    let description = format!(
        "glvideomixer name=mix background=black sink_0::width=1080 sink_0::height=1920 \
         sink_1::width=1080 sink_1::height=810 sink_1::ypos=555 \
         ! video/x-raw(memory:GLMemory),format=RGBA,width=1080,height=1920 \
         ! glcolorconvert ! gldownload ! video/x-raw,format=RGBA \
         ! fakesink sync=false {} {}",
        layer("smpte"),
        layer("ball")
    );
    let pipeline =
        gst::parse::launch(&description).map_err(|e| format!("could not build the test: {e}"))?;
    let bus = pipeline.bus().ok_or("no message bus")?;
    let started = Instant::now();
    pipeline
        .set_state(gst::State::Playing)
        .map_err(|_| "could not start the OpenGL pipeline".to_string())?;
    let mut outcome = Err("timed out after 30 s".to_string());
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        let Some(message) = bus.timed_pop(gst::ClockTime::from_mseconds(200)) else {
            continue;
        };
        match message.view() {
            gst::MessageView::Eos(_) => {
                outcome = Ok(started.elapsed());
                break;
            }
            gst::MessageView::Error(e) => {
                outcome = Err(e.error().to_string());
                break;
            }
            _ => {}
        }
    }
    let _ = pipeline.set_state(gst::State::Null);
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_test_reports_clearly_whether_or_not_opengl_is_there() {
        let report = test();
        assert!(!report.summary.is_empty());
        assert!(!report.details.is_empty());
        if !report.usable {
            // Without the plug-ins (or a card) it says so and names the cause.
            assert!(
                report.summary.contains("Not available") || report.summary.contains("failed"),
                "{}",
                report.summary
            );
        }
    }
}
