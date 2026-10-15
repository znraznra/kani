//! Pure timeline-view math: no egui types, so it is unit-testable.

use cutroom_timeline::{FrameRate, Ticks, TICKS_PER_SECOND};

pub const MIN_PIXELS_PER_SECOND: f64 = 2.0;
pub const MAX_PIXELS_PER_SECOND: f64 = 4000.0;

/// Horizontal zoom and scroll of the timeline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimelineView {
    pub pixels_per_second: f64,
    /// Time at the left edge of the canvas.
    pub scroll_seconds: f64,
}

impl Default for TimelineView {
    fn default() -> Self {
        TimelineView {
            pixels_per_second: 80.0,
            scroll_seconds: 0.0,
        }
    }
}

impl TimelineView {
    /// X offset (in pixels, from the canvas left edge) of a time.
    pub fn x_for(&self, t: Ticks) -> f32 {
        ((t.as_seconds_f64() - self.scroll_seconds) * self.pixels_per_second) as f32
    }

    /// Time under an x offset. Never negative.
    pub fn time_for(&self, x: f32) -> Ticks {
        let seconds = self.scroll_seconds + x as f64 / self.pixels_per_second;
        Ticks::from_seconds_f64(seconds.max(0.0))
    }

    /// A pixel distance expressed as timeline time at the current zoom.
    pub fn ticks_for_pixels(&self, pixels: f32) -> Ticks {
        Ticks((pixels as f64 / self.pixels_per_second * TICKS_PER_SECOND as f64).round() as i64)
    }

    /// Zoom by `factor`, keeping the time under `anchor_x` where it is.
    pub fn zoom_at(&mut self, factor: f64, anchor_x: f32) {
        let anchor_time = self.scroll_seconds + anchor_x as f64 / self.pixels_per_second;
        self.pixels_per_second =
            (self.pixels_per_second * factor).clamp(MIN_PIXELS_PER_SECOND, MAX_PIXELS_PER_SECOND);
        self.scroll_seconds = (anchor_time - anchor_x as f64 / self.pixels_per_second).max(0.0);
    }

    pub fn pan_pixels(&mut self, dx: f32) {
        self.scroll_seconds = (self.scroll_seconds + dx as f64 / self.pixels_per_second).max(0.0);
    }

    /// Zoom so `total` fits in `width` pixels with a little margin.
    pub fn fit(&mut self, total: Ticks, width: f32) {
        let seconds = total.as_seconds_f64().max(1.0);
        self.pixels_per_second =
            ((width as f64 * 0.92) / seconds).clamp(MIN_PIXELS_PER_SECOND, MAX_PIXELS_PER_SECOND);
        self.scroll_seconds = 0.0;
    }

    /// Keep `t` visible in a canvas `width` pixels wide (used while playing).
    pub fn follow(&mut self, t: Ticks, width: f32) {
        let visible = width as f64 / self.pixels_per_second;
        let seconds = t.as_seconds_f64();
        if seconds > self.scroll_seconds + visible * 0.95 || seconds < self.scroll_seconds {
            self.scroll_seconds = (seconds - visible * 0.1).max(0.0);
        }
    }
}

/// Spacing between labelled ruler marks, chosen so labels stay readable.
pub fn ruler_step_seconds(pixels_per_second: f64) -> f64 {
    const STEPS: [f64; 14] = [
        0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0,
    ];
    let min_gap_px = 90.0;
    STEPS
        .iter()
        .copied()
        .find(|step| step * pixels_per_second >= min_gap_px)
        .unwrap_or(3600.0)
}

/// Round to the nearest whole frame. Returns `t` unchanged when the rate has
/// no exact tick length.
pub fn quantize_to_frame(t: Ticks, rate: FrameRate) -> Ticks {
    match rate.ticks_per_frame() {
        Some(per_frame) => Ticks(((t.0 as f64 / per_frame as f64).round() as i64) * per_frame),
        None => t,
    }
}

/// `HH:MM:SS:FF` using the nominal (rounded) frame rate. This is a plain
/// frame counter, not drop-frame timecode.
pub fn timecode(t: Ticks, rate: FrameRate) -> String {
    let Some(frames) = t.frame_floor(rate) else {
        return format!("{:.2}s", t.as_seconds_f64());
    };
    let nominal = (rate.as_f64().round() as i64).max(1);
    let frames = frames.max(0);
    let ff = frames % nominal;
    let total_seconds = frames / nominal;
    format!(
        "{:02}:{:02}:{:02}:{:02}",
        total_seconds / 3600,
        (total_seconds / 60) % 60,
        total_seconds % 60,
        ff
    )
}

/// Short label for ruler marks: `0:05`, `1:30`, `1:02:00`, or `0.5s` for sub-second steps.
pub fn ruler_label(seconds: f64, step: f64) -> String {
    if step < 1.0 {
        return format!("{seconds:.2}s");
    }
    let total = seconds.round() as i64;
    if total >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            total / 3600,
            (total / 60) % 60,
            total % 60
        )
    } else {
        format!("{}:{:02}", total / 60, total % 60)
    }
}

// ----- the timeline navigator (scroll bar that also zooms) ----------------------------

/// Which part of the navigator's handle was grabbed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavGrab {
    /// The middle: slide along the timeline.
    Body,
    /// The left edge: zoom, keeping the right edge of the view still.
    Left,
    /// The right edge: zoom, keeping the left edge of the view still.
    Right,
}

/// Seconds the whole navigator track stands for: the project plus some spare
/// room, or more if the view has been scrolled or zoomed beyond that.
pub fn nav_span(timeline_end_seconds: f64, view: &TimelineView, canvas_width: f64) -> f64 {
    let visible = canvas_width / view.pixels_per_second;
    let spare = (timeline_end_seconds * 0.25).max(5.0);
    (timeline_end_seconds + spare)
        .max(view.scroll_seconds + visible)
        .max(10.0)
}

/// The handle's start and end as fractions (0 to 1) of the track.
pub fn nav_thumb(span: f64, view: &TimelineView, canvas_width: f64) -> (f64, f64) {
    let visible = canvas_width / view.pixels_per_second;
    let start = (view.scroll_seconds / span).clamp(0.0, 1.0);
    let end = ((view.scroll_seconds + visible) / span).clamp(start, 1.0);
    (start, end)
}

/// Which part of the handle a press lands on, given the handle's pixel range.
/// `None` means the press is on the empty track.
pub fn nav_hit(x: f64, thumb_start: f64, thumb_end: f64) -> Option<NavGrab> {
    if x < thumb_start || x > thumb_end {
        return None;
    }
    let grip = ((thumb_end - thumb_start) / 3.0).clamp(0.0, 9.0);
    if x <= thumb_start + grip {
        Some(NavGrab::Left)
    } else if x >= thumb_end - grip {
        Some(NavGrab::Right)
    } else {
        Some(NavGrab::Body)
    }
}

/// The view after dragging a grab by `delta_seconds`, starting from `start`.
pub fn nav_apply(
    grab: NavGrab,
    start: TimelineView,
    delta_seconds: f64,
    canvas_width: f64,
) -> TimelineView {
    let min_visible = canvas_width / MAX_PIXELS_PER_SECOND;
    let max_visible = canvas_width / MIN_PIXELS_PER_SECOND;
    let visible = canvas_width / start.pixels_per_second;
    let left = start.scroll_seconds;
    let right = left + visible;
    let (new_left, new_right) = match grab {
        NavGrab::Body => (
            (left + delta_seconds).max(0.0),
            (left + delta_seconds).max(0.0) + visible,
        ),
        NavGrab::Left => {
            let l =
                (left + delta_seconds).clamp((right - max_visible).max(0.0), right - min_visible);
            (l, right)
        }
        NavGrab::Right => {
            let r = (right + delta_seconds).clamp(left + min_visible, left + max_visible);
            (left, r)
        }
    };
    TimelineView {
        pixels_per_second: (canvas_width / (new_right - new_left))
            .clamp(MIN_PIXELS_PER_SECOND, MAX_PIXELS_PER_SECOND),
        scroll_seconds: new_left,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x_and_time_roundtrip() {
        let view = TimelineView {
            pixels_per_second: 100.0,
            scroll_seconds: 2.0,
        };
        let t = Ticks::from_seconds_f64(5.0);
        assert_eq!(view.x_for(t), 300.0);
        assert_eq!(view.time_for(300.0), t);
    }

    #[test]
    fn time_never_goes_negative() {
        let view = TimelineView::default();
        assert_eq!(view.time_for(-500.0), Ticks::ZERO);
    }

    #[test]
    fn zoom_keeps_the_anchor_time_in_place() {
        let mut view = TimelineView {
            pixels_per_second: 100.0,
            scroll_seconds: 3.0,
        };
        let before = view.time_for(250.0);
        view.zoom_at(2.0, 250.0);
        let after = view.time_for(250.0);
        assert!((before.as_seconds_f64() - after.as_seconds_f64()).abs() < 1e-6);
        assert_eq!(view.pixels_per_second, 200.0);
    }

    #[test]
    fn zoom_is_clamped() {
        let mut view = TimelineView::default();
        view.zoom_at(1e9, 0.0);
        assert_eq!(view.pixels_per_second, MAX_PIXELS_PER_SECOND);
        view.zoom_at(1e-9, 0.0);
        assert_eq!(view.pixels_per_second, MIN_PIXELS_PER_SECOND);
    }

    #[test]
    fn fit_shows_the_whole_timeline() {
        let mut view = TimelineView::default();
        view.fit(Ticks::from_seconds_f64(60.0), 1200.0);
        let end_x = view.x_for(Ticks::from_seconds_f64(60.0));
        assert!(end_x > 0.0 && end_x < 1200.0, "end at {end_x}");
        assert_eq!(view.scroll_seconds, 0.0);
    }

    #[test]
    fn follow_scrolls_only_when_the_playhead_leaves_the_view() {
        let mut view = TimelineView {
            pixels_per_second: 100.0,
            scroll_seconds: 0.0,
        };
        view.follow(Ticks::from_seconds_f64(2.0), 1000.0);
        assert_eq!(view.scroll_seconds, 0.0);
        view.follow(Ticks::from_seconds_f64(30.0), 1000.0);
        assert!(view.scroll_seconds > 20.0);
    }

    #[test]
    fn ruler_labels_get_sparser_when_zoomed_out() {
        assert!(ruler_step_seconds(10.0) > ruler_step_seconds(200.0));
        assert!(ruler_step_seconds(200.0) * 200.0 >= 90.0);
    }

    #[test]
    fn quantize_snaps_to_whole_frames() {
        let rate = FrameRate::FPS_30;
        let per_frame = rate.ticks_per_frame().unwrap();
        let almost = Ticks(per_frame * 7 + per_frame / 3);
        assert_eq!(quantize_to_frame(almost, rate), Ticks(per_frame * 7));
        let past_half = Ticks(per_frame * 7 + per_frame * 2 / 3);
        assert_eq!(quantize_to_frame(past_half, rate), Ticks(per_frame * 8));
    }

    #[test]
    fn timecode_formats_frames() {
        let t = Ticks::from_frames(45, FrameRate::FPS_30).unwrap();
        assert_eq!(timecode(t, FrameRate::FPS_30), "00:00:01:15");
        let t = Ticks::from_frames(30, FrameRate::NTSC_29_97).unwrap();
        assert_eq!(timecode(t, FrameRate::NTSC_29_97), "00:00:01:00");
        let hour = Ticks::from_frames(30 * 3600 + 5, FrameRate::FPS_30).unwrap();
        assert_eq!(timecode(hour, FrameRate::FPS_30), "01:00:00:05");
    }

    #[test]
    fn ruler_labels_read_naturally() {
        assert_eq!(ruler_label(5.0, 5.0), "0:05");
        assert_eq!(ruler_label(90.0, 30.0), "1:30");
        assert_eq!(ruler_label(3720.0, 60.0), "1:02:00");
        assert_eq!(ruler_label(0.5, 0.25), "0.50s");
    }

    #[test]
    fn the_navigator_handle_matches_what_is_visible() {
        let view = TimelineView {
            pixels_per_second: 100.0,
            scroll_seconds: 10.0,
        };
        let span = nav_span(60.0, &view, 1000.0);
        assert_eq!(span, 75.0, "60 s plus a quarter spare");
        let (a, b) = nav_thumb(span, &view, 1000.0);
        assert!((a - 10.0 / 75.0).abs() < 1e-9 && (b - 20.0 / 75.0).abs() < 1e-9);
        // Scrolled past the project: the track grows to hold the view.
        let far = TimelineView {
            pixels_per_second: 100.0,
            scroll_seconds: 200.0,
        };
        let span = nav_span(60.0, &far, 1000.0);
        assert_eq!(nav_thumb(span, &far, 1000.0).1, 1.0);
        assert!(nav_span(0.0, &TimelineView::default(), 800.0) >= 10.0);
    }

    #[test]
    fn navigator_hits_pick_the_edge_or_the_body() {
        assert_eq!(nav_hit(5.0, 10.0, 100.0), None);
        assert_eq!(nav_hit(12.0, 10.0, 100.0), Some(NavGrab::Left));
        assert_eq!(nav_hit(98.0, 10.0, 100.0), Some(NavGrab::Right));
        assert_eq!(nav_hit(50.0, 10.0, 100.0), Some(NavGrab::Body));
        assert_eq!(
            nav_hit(11.0, 10.0, 14.0),
            Some(NavGrab::Left),
            "tiny handles stay grabbable"
        );
    }

    #[test]
    fn dragging_the_body_pans_and_keeps_the_zoom() {
        let start = TimelineView {
            pixels_per_second: 100.0,
            scroll_seconds: 5.0,
        };
        let v = nav_apply(NavGrab::Body, start, 3.0, 1000.0);
        assert_eq!((v.pixels_per_second, v.scroll_seconds), (100.0, 8.0));
        assert_eq!(
            nav_apply(NavGrab::Body, start, -50.0, 1000.0).scroll_seconds,
            0.0
        );
    }

    #[test]
    fn dragging_an_edge_zooms_and_keeps_the_other_edge() {
        let start = TimelineView {
            pixels_per_second: 100.0,
            scroll_seconds: 10.0,
        };
        // View is 10..20 s. Pull the right edge out by 10 s: 10..30 s, half the zoom.
        let v = nav_apply(NavGrab::Right, start, 10.0, 1000.0);
        assert!((v.pixels_per_second - 50.0).abs() < 1e-9 && v.scroll_seconds == 10.0);
        // Push the left edge in by 5 s: 15..20 s, double the zoom, right edge still at 20 s.
        let v = nav_apply(NavGrab::Left, start, 5.0, 1000.0);
        assert!((v.pixels_per_second - 200.0).abs() < 1e-9);
        assert!((v.scroll_seconds + 1000.0 / v.pixels_per_second - 20.0).abs() < 1e-9);
        // Edges cannot cross, and zoom stays inside its limits.
        let v = nav_apply(NavGrab::Left, start, 500.0, 1000.0);
        assert_eq!(v.pixels_per_second, MAX_PIXELS_PER_SECOND);
        let v = nav_apply(NavGrab::Right, start, 1.0e6, 1000.0);
        assert_eq!(v.pixels_per_second, MIN_PIXELS_PER_SECOND);
    }
}
