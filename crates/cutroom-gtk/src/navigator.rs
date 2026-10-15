//! The bar under the timeline: a scroll bar whose handle shows which part of the
//! project is in view. Slide the handle to scroll; drag either end to zoom.

use crate::app::App;
use crate::theme;
use cutroom_core::view::{self, NavGrab, TimelineView};
use gtk4 as gtk;
use gtk4::prelude::*;
use std::rc::Rc;

pub const HEIGHT: i32 = 16;

struct Drag {
    grab: NavGrab,
    start: TimelineView,
    span: f64,
}

/// Seconds the track stands for, and the canvas width, right now.
fn metrics(app: &App) -> Option<(f64, f64, TimelineView)> {
    let st = app.state.borrow();
    let session = st.session.as_ref()?;
    let canvas = st.timeline_width.max(100.0);
    let end = session.timeline_end().as_seconds_f64();
    Some((view::nav_span(end, &st.view, canvas), canvas, st.view))
}

pub fn build(app: &Rc<App>) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_height(HEIGHT);
    area.set_hexpand(true);
    area.set_margin_start(cutroom_core::timeline_logic::HEADER_W as i32);
    area.set_margin_end(4);
    area.set_margin_top(2);
    area.set_margin_bottom(4);
    area.set_tooltip_text(Some(
        "Drag the handle to scroll. Drag its ends to zoom in or out.",
    ));

    {
        let app = app.clone();
        area.set_draw_func(move |a, cr, w, h| {
            let (w, h) = (f64::from(w), f64::from(h));
            let c = a.color();
            let (r, g, b) = (
                f64::from(c.red()),
                f64::from(c.green()),
                f64::from(c.blue()),
            );
            rounded(cr, 0.0, 0.0, w, h);
            cr.set_source_rgba(r, g, b, 0.10);
            let _ = cr.fill();
            let Some((span, canvas, view)) = metrics(&app) else {
                return;
            };
            let (s, e) = view::nav_thumb(span, &view, canvas);
            let (x0, x1) = (s * w, (e * w).max(s * w + 14.0).min(w));
            rounded(cr, x0, 0.0, x1 - x0, h);
            let (ar, ag, ab) = theme::ACCENT;
            cr.set_source_rgba(ar, ag, ab, 0.55);
            let _ = cr.fill();
            // Grips on both ends: these zoom.
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.85);
            for x in [x0 + 4.0, x1 - 4.0] {
                cr.rectangle(x - 1.0, h * 0.28, 2.0, h * 0.44);
                let _ = cr.fill();
            }
        });
    }

    let drag_info: Rc<std::cell::RefCell<Option<Drag>>> = Rc::new(std::cell::RefCell::new(None));
    let gesture = gtk::GestureDrag::new();
    {
        let (app, area, info) = (app.clone(), area.clone(), drag_info.clone());
        gesture.connect_drag_begin(move |_, x, _| {
            let Some((span, canvas, start)) = metrics(&app) else {
                return;
            };
            let w = f64::from(area.width()).max(1.0);
            let (s, e) = view::nav_thumb(span, &start, canvas);
            let grab = view::nav_hit(x, s * w, e * w);
            let mut start = start;
            let grab = match grab {
                Some(g) => g,
                None => {
                    // A click on the empty track centres the handle there.
                    let visible = canvas / start.pixels_per_second;
                    start.scroll_seconds = (x / w * span - visible / 2.0).max(0.0);
                    app.state.borrow_mut().view = start;
                    crate::editor::view_changed(&app);
                    NavGrab::Body
                }
            };
            *info.borrow_mut() = Some(Drag { grab, start, span });
        });
    }
    {
        let (app, area, info) = (app.clone(), area.clone(), drag_info.clone());
        gesture.connect_drag_update(move |_, dx, _| {
            let Some(d) = info.borrow().as_ref().map(|d| (d.grab, d.start, d.span)) else {
                return;
            };
            let canvas = app.state.borrow().timeline_width.max(100.0);
            let w = f64::from(area.width()).max(1.0);
            let new = view::nav_apply(d.0, d.1, dx / w * d.2, canvas);
            app.state.borrow_mut().view = new;
            crate::editor::view_changed(&app);
        });
    }
    {
        let info = drag_info.clone();
        gesture.connect_drag_end(move |_, _, _| *info.borrow_mut() = None);
    }
    area.add_controller(gesture);

    // The pointer shows what a press would do.
    let motion = gtk::EventControllerMotion::new();
    {
        let (app, area) = (app.clone(), area.clone());
        motion.connect_motion(move |_, x, _| {
            let Some((span, canvas, view)) = metrics(&app) else {
                return;
            };
            let w = f64::from(area.width()).max(1.0);
            let (s, e) = view::nav_thumb(span, &view, canvas);
            let name = match view::nav_hit(x, s * w, e * w) {
                Some(NavGrab::Left | NavGrab::Right) => "ew-resize",
                Some(NavGrab::Body) => "grab",
                None => "pointer",
            };
            area.set_cursor_from_name(Some(name));
        });
    }
    area.add_controller(motion);
    area
}

fn rounded(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64) {
    let r = (h / 2.0).min(w / 2.0);
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -std::f64::consts::FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, std::f64::consts::FRAC_PI_2);
    cr.arc(
        x + r,
        y + h - r,
        r,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
    );
    cr.arc(
        x + r,
        y + r,
        r,
        std::f64::consts::PI,
        3.0 * std::f64::consts::FRAC_PI_2,
    );
    cr.close_path();
}
