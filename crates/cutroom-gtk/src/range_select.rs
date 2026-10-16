//! Picking the part of the timeline to export (Shift+Ctrl+X).
//!
//! The timeline shows two handles and dims everything outside them. Drag a
//! handle to move that end, drag between them to move the whole part. "Export
//! range" (or Enter) opens the Export window with that part filled in; Cancel
//! (or Esc) leaves without doing anything.

use crate::app::App;
use crate::theme::{self, Rgb};
use cutroom_core::geom::Rect;
use cutroom_core::timeline_logic::{HEADER_W, RULER_H};
use cutroom_core::view::TimelineView;
use cutroom_timeline::{Ticks, TICKS_PER_SECOND};
use gtk4::cairo;
use gtk4::prelude::*;
use std::rc::Rc;

/// What the pointer holds right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grab {
    Nothing,
    Start,
    End,
    /// The whole part; the number is how far into it the pointer took hold.
    Body(Ticks),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportRange {
    pub start: Ticks,
    pub end: Ticks,
    pub grab: Grab,
}

/// How close (in pixels) the pointer must be to a handle to take it.
const HANDLE_REACH: f64 = 9.0;
const BUTTON_H: f64 = 28.0;

pub fn active(app: &App) -> bool {
    app.state.borrow().export_range.is_some()
}

fn redraw(app: &App) {
    app.ui().editor.timeline.area.queue_draw();
}

/// One frame, the least a part can be.
fn frame_of(app: &App) -> Ticks {
    let st = app.state.borrow();
    let ticks = st
        .session
        .as_ref()
        .and_then(|s| s.doc.settings.frame_rate.ticks_per_frame())
        .unwrap_or(TICKS_PER_SECOND / 30);
    Ticks(ticks)
}

/// Start picking. The part starts as the selected clips, else ten seconds from the playhead.
pub fn enter(app: &Rc<App>) {
    if active(app) {
        cancel(app);
        return;
    }
    let range = {
        let st = app.state.borrow();
        let Some(session) = st.session.as_ref() else {
            return;
        };
        let duration = session.doc.timeline.duration();
        if duration <= Ticks::ZERO {
            drop(st);
            app.set_status("There is nothing on the timeline to export yet.");
            app.update_status();
            return;
        }
        let spans: Vec<(Ticks, Ticks)> = session
            .selection()
            .into_iter()
            .filter_map(|id| session.doc.timeline.find_clip(id).map(|(_, c)| (c.start, c.end())))
            .collect();
        let ten = Ticks(10 * TICKS_PER_SECOND);
        match (spans.iter().map(|s| s.0).min(), spans.iter().map(|s| s.1).max()) {
            (Some(a), Some(b)) if b > a => (a, b),
            _ => {
                let a = session.playhead.min(duration);
                let b = (a + ten).min(duration);
                if b - a >= Ticks(TICKS_PER_SECOND) {
                    (a, b)
                } else {
                    ((duration - ten).max(Ticks::ZERO), duration)
                }
            }
        }
    };
    {
        let mut st = app.state.borrow_mut();
        // Bring the part into view if it is off screen.
        let visible = ((st.timeline_width - f64::from(HEADER_W)) / st.view.pixels_per_second).max(1.0);
        let (a, b) = (range.0.as_seconds_f64(), range.1.as_seconds_f64());
        if a < st.view.scroll_seconds || a > st.view.scroll_seconds + visible * 0.9 {
            st.view.scroll_seconds = (a - visible * 0.1).max(0.0);
        } else if b > st.view.scroll_seconds + visible && b - a < visible {
            st.view.scroll_seconds = (b - visible * 0.9).max(0.0);
        }
        st.export_range = Some(ExportRange {
            start: range.0,
            end: range.1,
            grab: Grab::Nothing,
        });
    }
    app.set_status("Drag the handles to choose what to export. Enter or Export range to continue, Esc to cancel.");
    app.update_status();
    redraw(app);
}

pub fn cancel(app: &Rc<App>) {
    if app.state.borrow_mut().export_range.take().is_some() {
        app.set_status("");
        app.update_status();
        redraw(app);
    }
}

/// Leave the picking and open Export with the part set.
pub fn export(app: &Rc<App>) {
    let Some(r) = app.state.borrow_mut().export_range.take() else {
        return;
    };
    redraw(app);
    crate::export_ui::open_range(app, Some((r.start, r.end)));
}

// ----- geometry ----------------------------------------------------------------------

fn x_of(view: &TimelineView, t: Ticks) -> f64 {
    f64::from(HEADER_W + view.x_for(t))
}

fn time_at(view: &TimelineView, x: f64) -> Ticks {
    view.time_for((x - f64::from(HEADER_W)) as f32)
}

/// The two buttons, as (x, y, width) with the widget's width and the vertical scroll in view.
fn buttons(view: &TimelineView, r: &ExportRange, w: f64, off: f64) -> [Rect; 2] {
    let left = f64::from(HEADER_W);
    let (xs, xe) = (x_of(view, r.start).max(left), x_of(view, r.end).min(w));
    let (export_w, cancel_w, gap) = (112.0, 112.0, 8.0);
    let total = export_w + gap + cancel_w;
    let mid = (xs + xe) / 2.0;
    let x = (mid - total / 2.0).clamp(left + 6.0, (w - total - 6.0).max(left + 6.0));
    let y = off + f64::from(RULER_H) + 10.0;
    let rect = |x: f64, width: f64| {
        Rect::from_min_size(
            cutroom_core::geom::pos2(x as f32, y as f32),
            cutroom_core::geom::vec2(width as f32, BUTTON_H as f32),
        )
    };
    [rect(x, export_w), rect(x + export_w + gap, cancel_w)]
}

fn inside(rect: &Rect, x: f64, y: f64) -> bool {
    x >= f64::from(rect.left())
        && x <= f64::from(rect.right())
        && y >= f64::from(rect.top())
        && y <= f64::from(rect.bottom())
}

// ----- mouse ---------------------------------------------------------------------------

/// A press on the timeline while picking: take a handle, or the part between them.
pub fn press(app: &Rc<App>, x: f64) {
    let (frame, _) = (frame_of(app), ());
    let mut st = app.state.borrow_mut();
    let view = st.view;
    let Some(mut r) = st.export_range else {
        return;
    };
    let (xs, xe) = (x_of(&view, r.start), x_of(&view, r.end));
    let near_start = (x - xs).abs() <= HANDLE_REACH;
    let near_end = (x - xe).abs() <= HANDLE_REACH;
    r.grab = if near_start && (!near_end || (x - xs).abs() <= (x - xe).abs()) {
        Grab::Start
    } else if near_end {
        Grab::End
    } else if x > xs && x < xe {
        Grab::Body(time_at(&view, x) - r.start)
    } else {
        Grab::Nothing
    };
    let _ = frame;
    st.export_range = Some(r);
}

/// The pointer moved while holding something.
pub fn drag_to(app: &Rc<App>, x: f64) {
    let frame = frame_of(app);
    {
        let mut st = app.state.borrow_mut();
        let view = st.view;
        let snap_to = st.session.as_ref().map(|s| s.playhead);
        let duration = st
            .session
            .as_ref()
            .map_or(Ticks::ZERO, |s| s.doc.timeline.duration());
        let snap = st.snap;
        let Some(mut r) = st.export_range else {
            return;
        };
        let tick = frame.0.max(1);
        let near = view.ticks_for_pixels(8.0);
        let mut t = time_at(&view, x);
        let fit = |t: Ticks| -> Ticks {
            // Whole frames, and onto the playhead when close to it.
            let mut t = Ticks((t.0 as f64 / tick as f64).round() as i64 * tick);
            if let (true, Some(p)) = (snap, snap_to) {
                if (t.0 - p.0).abs() <= near.0 {
                    t = p;
                }
            }
            t
        };
        match r.grab {
            Grab::Nothing => return,
            Grab::Start => {
                t = fit(t).max(Ticks::ZERO);
                r.start = t.min(r.end - frame);
            }
            Grab::End => {
                t = fit(t).min(duration);
                r.end = t.max(r.start + frame);
            }
            Grab::Body(offset) => {
                let len = r.end - r.start;
                let mut start = fit(t - offset).max(Ticks::ZERO);
                if start + len > duration {
                    start = (duration - len).max(Ticks::ZERO);
                }
                r.start = start;
                r.end = start + len;
            }
        }
        st.export_range = Some(r);
    }
    redraw(app);
}

pub fn release(app: &Rc<App>) {
    if let Some(r) = app.state.borrow_mut().export_range.as_mut() {
        r.grab = Grab::Nothing;
    }
}

/// A click while picking: the two buttons. `off` is how far the track list has scrolled.
pub fn click(app: &Rc<App>, x: f64, y: f64, w: f64, off: f64) {
    let hit = {
        let st = app.state.borrow();
        let Some(r) = st.export_range else {
            return;
        };
        let [go, no] = buttons(&st.view, &r, w, off);
        if inside(&go, x, y) {
            1
        } else if inside(&no, x, y) {
            2
        } else {
            0
        }
    };
    match hit {
        1 => export(app),
        2 => cancel(app),
        _ => {}
    }
}

// ----- drawing ---------------------------------------------------------------------------

fn set(cr: &cairo::Context, color: Rgb, alpha: f64) {
    cr.set_source_rgba(color.0, color.1, color.2, alpha);
}

fn pill(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64) {
    let r = h / 2.0;
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -std::f64::consts::FRAC_PI_2, std::f64::consts::FRAC_PI_2);
    cr.arc(x + r, y + r, r, std::f64::consts::FRAC_PI_2, 3.0 * std::f64::consts::FRAC_PI_2);
    cr.close_path();
}

fn centered(cr: &cairo::Context, s: &str, x: f64, y: f64, w: f64, h: f64, size: f64, color: Rgb) {
    cr.select_font_face(crate::fonts::FAMILY, cairo::FontSlant::Normal, cairo::FontWeight::Bold);
    cr.set_font_size(size);
    let e = cr.text_extents(s).ok();
    let tw = e.map_or(0.0, |e| e.width());
    set(cr, color, 1.0);
    cr.move_to(x + (w - tw) / 2.0, y + h / 2.0 + size * 0.35);
    let _ = cr.show_text(s);
}

fn clock(t: Ticks) -> String {
    let s = t.as_seconds_f64();
    format!("{}:{:04.1}", (s / 60.0).floor() as u32, s % 60.0)
}

/// Paint the dimming, handles and buttons over the timeline (widget coordinates).
pub fn paint(app: &App, cr: &cairo::Context, w: f64, h: f64, off: f64) {
    let st = app.state.borrow();
    let Some(r) = st.export_range else {
        return;
    };
    let view = &st.view;
    let left = f64::from(HEADER_W);
    let xs = x_of(view, r.start).clamp(left, w);
    let xe = x_of(view, r.end).clamp(left, w);
    cr.save().ok();
    // Dim everything outside the part.
    set(cr, (0.0, 0.0, 0.0), 0.58);
    cr.rectangle(left, 0.0, xs - left, h);
    cr.rectangle(xe, 0.0, w - xe, h);
    let _ = cr.fill();
    // A soft band on the ruler and a frame around the part.
    set(cr, theme::ACCENT, 0.18);
    cr.rectangle(xs, off, xe - xs, f64::from(RULER_H));
    let _ = cr.fill();
    set(cr, theme::ACCENT, 1.0);
    cr.set_line_width(2.0);
    cr.move_to(xs, off);
    cr.line_to(xe, off);
    cr.move_to(xs, h - 1.0);
    cr.line_to(xe, h - 1.0);
    let _ = cr.stroke();
    // The handles: a bar down the edge with a grip on the ruler.
    for (x, real) in [(xs, x_of(view, r.start)), (xe, x_of(view, r.end))] {
        if real < left - 1.0 || real > w + 1.0 {
            continue;
        }
        set(cr, theme::ACCENT, 1.0);
        cr.set_line_width(2.0);
        cr.move_to(x, 0.0);
        cr.line_to(x, h);
        let _ = cr.stroke();
        cr.new_path();
        pill(cr, x - 5.0, off + 3.0, 10.0, f64::from(RULER_H) + 12.0);
        set(cr, theme::ACCENT, 1.0);
        let _ = cr.fill();
        set(cr, (1.0, 1.0, 1.0), 0.9);
        cr.set_line_width(1.0);
        for dx in [-1.5, 1.5] {
            cr.move_to(x + dx, off + 10.0);
            cr.line_to(x + dx, off + 20.0);
        }
        let _ = cr.stroke();
    }
    // The buttons, and the length of the part.
    let [go, no] = buttons(view, &r, w, off);
    let (gx, gy, gw) = (f64::from(go.left()), f64::from(go.top()), f64::from(go.width()));
    pill(cr, gx, gy, gw, BUTTON_H);
    set(cr, theme::ACCENT, 1.0);
    let _ = cr.fill();
    centered(cr, "Export range", gx, gy, gw, BUTTON_H, 12.0, (0.02, 0.1, 0.1));
    let (nx, ny, nw) = (f64::from(no.left()), f64::from(no.top()), f64::from(no.width()));
    pill(cr, nx, ny, nw, BUTTON_H);
    set(cr, (0.13, 0.14, 0.17), 0.96);
    let _ = cr.fill_preserve();
    set(cr, (1.0, 1.0, 1.0), 0.55);
    cr.set_line_width(1.0);
    let _ = cr.stroke();
    centered(cr, "Cancel (Esc)", nx, ny, nw, BUTTON_H, 12.0, (1.0, 1.0, 1.0));
    let label = format!("{} - {}  ({})", clock(r.start), clock(r.end), clock(r.end - r.start));
    centered(cr, &label, gx, gy + BUTTON_H + 4.0, gx + gw + 8.0 + nw - gx, 18.0, 11.0, (1.0, 1.0, 1.0));
    cr.restore().ok();
}
