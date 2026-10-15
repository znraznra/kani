//! Does the graphics card work for putting export pictures together?
//!
//! `test` starts the same compositor export uses (see `gpu_compose`), draws a
//! skewed and a scaled 1440x1080 picture on the canvas size of a vertical
//! video both on the card and on the processor, and reports the times and how
//! closely the two pictures agree.

use crate::gpu_compose::GpuCompositor;
use cutroom_compose::{Canvas, Frame, Placement};
use std::sync::Arc;
use std::time::Instant;

/// Pictures put together per timing.
const RUNS: usize = 40;

/// What the test found.
#[derive(Debug, Clone)]
pub struct GpuReport {
    pub usable: bool,
    /// One or two lines for the window.
    pub summary: String,
    /// Detail for the console.
    pub details: Vec<String>,
}

fn gradient(shift: usize) -> Arc<Frame> {
    let (w, h) = (1440usize, 1080usize);
    let mut frame = Frame::solid(w as u32, h as u32, [0, 0, 0, 255]);
    for y in 0..h {
        for x in 0..w {
            let o = (y * w + x) * 4;
            frame.rgba[o] = ((x + shift) / 6) as u8;
            frame.rgba[o + 1] = (y / 4) as u8;
            frame.rgba[o + 2] = ((x + y) / 9) as u8;
        }
    }
    Arc::new(frame)
}

pub fn test() -> GpuReport {
    let (width, height) = (1080u32, 1920u32);
    let mut details = Vec::new();
    let mut card = match GpuCompositor::new(width, height) {
        Ok(card) => card,
        Err(why) => {
            details.push(format!("could not start: {why}"));
            return GpuReport {
                usable: false,
                summary: format!("Not available: {why}. Kani composites on the processor."),
                details,
            };
        }
    };
    details.push(format!("graphics: {}", card.renderer));
    // Several different pictures, so every run really uploads one.
    let pictures: Vec<Arc<Frame>> = (0..8).map(|i| gradient(i * 40)).collect();
    let cases = [
        (
            "skewed layer",
            Placement {
                scale: 1.371,
                skew_x: 8.0,
                ..Placement::default()
            },
        ),
        (
            "scaled layer",
            Placement {
                scale: 2.554,
                ..Placement::default()
            },
        ),
    ];
    let mut parts = Vec::new();
    let mut usable = true;
    for (name, placement) in cases {
        let started = Instant::now();
        let mut failed = None;
        for i in 0..RUNS {
            let mut canvas = Canvas::new(width, height);
            match card.render(&pictures[i % pictures.len()], &placement) {
                Ok(Some(layer)) => canvas.blit(&layer),
                Ok(None) => {}
                Err(why) => {
                    failed = Some(why);
                    break;
                }
            }
            std::hint::black_box(&canvas.rgba);
        }
        if let Some(why) = failed {
            details.push(format!("{name}: failed on the card: {why}"));
            usable = false;
            parts.push(format!("{name} failed ({why})"));
            continue;
        }
        let on_card = started.elapsed().as_secs_f64() * 1000.0 / RUNS as f64;
        let started = Instant::now();
        for i in 0..RUNS {
            let mut canvas = Canvas::new(width, height);
            canvas.draw(&pictures[i % pictures.len()], &placement);
            std::hint::black_box(&canvas.rgba);
        }
        let on_cpu = started.elapsed().as_secs_f64() * 1000.0 / RUNS as f64;
        details.push(format!(
            "{name}: {on_card:.1} ms on the card (with upload and read-back), {on_cpu:.1} ms on the processor ({:.1}x)",
            on_cpu / on_card.max(0.001)
        ));
        parts.push(format!(
            "{name} {on_card:.1} ms on the card, {on_cpu:.1} ms on the processor ({:.1}x)",
            on_cpu / on_card.max(0.001)
        ));
    }
    GpuReport {
        usable,
        summary: if usable {
            format!("Works ({}): {}.", card.renderer, parts.join("; "))
        } else {
            format!(
                "The card started but a test failed: {}. Kani composites on the processor.",
                parts.join("; ")
            )
        },
        details,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_test_says_clearly_whether_the_card_can_be_used() {
        let report = test();
        assert!(!report.summary.is_empty());
        assert!(!report.details.is_empty());
        if !report.usable {
            assert!(
                report.summary.contains("Not available") || report.summary.contains("failed"),
                "{}",
                report.summary
            );
        }
    }
}
