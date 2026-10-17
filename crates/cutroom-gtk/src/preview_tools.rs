//! The Player's menu (rulers, grid, guides), the aids it switches on, and the
//! full screen preview.
//!
//! Rulers run along the top and left of the player and show canvas pixels.
//! Dragging out of a ruler makes a guide; a guide can be dragged again, and
//! dragged back onto the ruler (or off the picture) to remove it. Guides are
//! kept in the project. The grid is square cells, ten across the picture.

use crate::app::App;
use crate::icons;
use cutroom_core::shortcuts::Action;
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

/// Width of the rulers, in screen pixels.
pub const RULER: f64 = 18.0;
/// How near the pointer must be to a guide to take hold of it, in screen pixels.
pub const GRAB: f64 = 5.0;
const GUIDE_RGB: (f64, f64, f64) = (0.2, 0.8, 1.0);

/// How many cells the grid has across the picture.
pub const GRID_CELLS: u32 = 10;

/// Nice spacing between ruler ticks so they are at least `min_px` apart on screen.
pub fn tick_step(pixels_per_unit: f64, min_px: f64) -> u32 {
    const STEPS: [u32; 12] = [1, 2, 5, 10, 20, 50, 100, 200, 500, 1000, 2000, 5000];
    STEPS
        .iter()
        .copied()
        .find(|s| f64::from(*s) * pixels_per_unit >= min_px)
        .unwrap_or(5000)
}

/// Which guide (if any) is at screen position (`x`, `y`); canvas at (`ox`, `oy`), `k` screen pixels per canvas pixel.
pub fn guide_at(
    guides: &[cutroom_project::Guide],
    (ox, oy, k): (f64, f64, f64),
    x: f64,
    y: f64,
) -> Option<usize> {
    let mut best: Option<(usize, f64)> = None;
    for (i, g) in guides.iter().enumerate() {
        let d = if g.vertical {
            (x - (ox + f64::from(g.pos) * k)).abs()
        } else {
            (y - (oy + f64::from(g.pos) * k)).abs()
        };
        if d <= GRAB && best.is_none_or(|b| d < b.1) {
            best = Some((i, d));
        }
    }
    best.map(|b| b.0)
}

/// Draw the aids that are switched on. `frame` is the canvas's left, top and
/// on-screen width in the drawing area; `canvas` its size in pixels.
pub fn draw(
    app: &App,
    cr: &gtk::cairo::Context,
    area_size: (f64, f64),
    frame: (f64, f64, f64),
    canvas: (u32, u32),
) {
    let (show_rulers, show_grid, show_guides, guides) = {
        let st = app.state.borrow();
        (
            st.settings.show_rulers,
            st.settings.show_grid,
            st.settings.show_guides,
            st.session
                .as_ref()
                .map(|s| s.doc.guides.clone())
                .unwrap_or_default(),
        )
    };
    let (ox, oy, sw) = frame;
    let k = sw / f64::from(canvas.0.max(1));
    let (cw, ch) = (f64::from(canvas.0) * k, f64::from(canvas.1) * k);
    cr.save().ok();
    if show_grid {
        cr.set_line_width(1.0);
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.28);
        let cell = f64::from(canvas.0) / f64::from(GRID_CELLS) * k;
        if cell >= 4.0 {
            let mut x = ox;
            while x <= ox + cw + 0.5 {
                cr.move_to(x.round() + 0.5, oy);
                cr.line_to(x.round() + 0.5, oy + ch);
                x += cell;
            }
            let mut y = oy;
            while y <= oy + ch + 0.5 {
                cr.move_to(ox, y.round() + 0.5);
                cr.line_to(ox + cw, y.round() + 0.5);
                y += cell;
            }
            let _ = cr.stroke();
        }
    }
    if show_guides {
        cr.set_line_width(1.0);
        cr.set_source_rgba(GUIDE_RGB.0, GUIDE_RGB.1, GUIDE_RGB.2, 0.95);
        for g in &guides {
            if g.vertical {
                let x = (ox + f64::from(g.pos) * k).round() + 0.5;
                cr.move_to(x, 0.0);
                cr.line_to(x, area_size.1);
            } else {
                let y = (oy + f64::from(g.pos) * k).round() + 0.5;
                cr.move_to(0.0, y);
                cr.line_to(area_size.0, y);
            }
        }
        let _ = cr.stroke();
    }
    if show_rulers {
        let step = tick_step(k, 56.0);
        cr.set_source_rgba(0.1, 0.1, 0.12, 0.85);
        cr.rectangle(0.0, 0.0, area_size.0, RULER);
        cr.rectangle(0.0, RULER, RULER, area_size.1 - RULER);
        let _ = cr.fill();
        cr.set_source_rgba(0.85, 0.85, 0.88, 1.0);
        cr.set_font_size(9.0);
        cr.set_line_width(1.0);
        let mut v = 0u32;
        while v <= canvas.0 {
            let x = (ox + f64::from(v) * k).round() + 0.5;
            if x > RULER && x < area_size.0 {
                cr.move_to(x, RULER - 6.0);
                cr.line_to(x, RULER);
                let _ = cr.stroke();
                cr.move_to(x + 2.0, RULER - 8.0);
                let _ = cr.show_text(&v.to_string());
            }
            v += step;
        }
        let mut v = 0u32;
        while v <= canvas.1 {
            let y = (oy + f64::from(v) * k).round() + 0.5;
            if y > RULER && y < area_size.1 {
                cr.move_to(RULER - 6.0, y);
                cr.line_to(RULER, y);
                let _ = cr.stroke();
                cr.save().ok();
                cr.translate(RULER - 8.0, y - 2.0);
                cr.rotate(-std::f64::consts::FRAC_PI_2);
                cr.move_to(0.0, 0.0);
                let _ = cr.show_text(&v.to_string());
                cr.restore().ok();
            }
            v += step;
        }
    }
    cr.restore().ok();
}

/// The Player's menu: rulers, grid, guides and a way to clear the guides.
pub fn menu_button(app: &Rc<App>) -> gtk::MenuButton {
    let button = gtk::MenuButton::new();
    button.set_child(Some(&icons::icon(icons::MENU, 16)));
    button.add_css_class("flat");
    button.add_css_class("cc-icon-button");
    button.set_tooltip_text(Some("Player options"));
    let popover = gtk::Popover::new();
    let column = gtk::Box::new(gtk::Orientation::Vertical, 2);
    column.set_margin_top(6);
    column.set_margin_bottom(6);
    column.set_margin_start(8);
    column.set_margin_end(8);

    type Get = fn(&cutroom_core::settings::Settings) -> bool;
    type Set = fn(&mut cutroom_core::settings::Settings, bool);
    let rows: [(&str, &str, Get, Set); 3] = [
        (
            "Show rulers",
            "Rulers along the top and left, in picture pixels. Drag out of a ruler to make a guide.",
            |s| s.show_rulers,
            |s, v| s.show_rulers = v,
        ),
        (
            "Show grid",
            "A grid of square cells, ten across the picture.",
            |s| s.show_grid,
            |s, v| s.show_grid = v,
        ),
        (
            "Show guides",
            "Guide lines dragged out of the rulers. They are kept in the project.",
            |s| s.show_guides,
            |s, v| s.show_guides = v,
        ),
    ];
    for (label, tip, get, set) in rows {
        let check = gtk::CheckButton::with_label(label);
        check.set_tooltip_text(Some(tip));
        check.set_active(get(&app.state.borrow().settings));
        let app2 = app.clone();
        check.connect_toggled(move |c| {
            {
                let mut st = app2.state.borrow_mut();
                set(&mut st.settings, c.is_active());
                st.save_settings();
            }
            app2.ui().editor.queue_overlay();
        });
        column.append(&check);
    }
    column.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    let reset = gtk::Button::with_label("Reset guides");
    reset.add_css_class("flat");
    reset.set_halign(gtk::Align::Start);
    reset.set_tooltip_text(Some("Remove every guide from this project"));
    {
        let (app, popover) = (app.clone(), popover.clone());
        reset.connect_clicked(move |_| {
            reset_guides(&app);
            popover.popdown();
        });
    }
    column.append(&reset);
    popover.set_child(Some(&column));
    button.set_popover(Some(&popover));
    button
}

/// Remove every guide from the project.
pub fn reset_guides(app: &Rc<App>) {
    let removed = {
        let mut st = app.state.borrow_mut();
        st.session.as_mut().is_some_and(|s| s.reset_guides())
    };
    app.set_status(if removed {
        "Guides removed"
    } else {
        "There are no guides to remove"
    });
    app.ui().editor.queue_overlay();
    app.refresh();
}

// ----- full screen -----------------------------------------------------------------

thread_local! {
    static FULL: RefCell<Option<(gtk::Window, gtk::Picture)>> = const { RefCell::new(None) };
}

/// Is the full screen preview open?
pub fn fullscreen_open() -> bool {
    FULL.with(|f| f.borrow().is_some())
}

/// Show the newest picture in the full screen preview too.
pub fn update_texture(texture: &gdk::Texture) {
    FULL.with(|f| {
        if let Some((_, picture)) = f.borrow().as_ref() {
            picture.set_paintable(Some(texture));
        }
    });
}

/// Open the preview full screen on black, or close it. Esc, F11 or a double
/// click close it; Space plays and pauses.
pub fn toggle_fullscreen(app: &Rc<App>) {
    if let Some((window, _)) = FULL.with(|f| f.borrow().clone()) {
        window.close();
        return;
    }
    if app.state.borrow().session.is_none() {
        return;
    }
    let picture = gtk::Picture::new();
    picture.set_content_fit(gtk::ContentFit::Contain);
    picture.set_can_shrink(true);
    picture.set_hexpand(true);
    picture.set_vexpand(true);
    picture.set_paintable(app.ui().editor.current_paintable().as_ref());
    let window = gtk::Window::builder()
        .title("Kani - full screen preview")
        .transient_for(&app.window)
        .child(&picture)
        .build();
    window.add_css_class("preview-fullscreen");
    let keys = gtk::EventControllerKey::new();
    {
        let (app, window) = (app.clone(), window.clone());
        keys.connect_key_pressed(move |_, key, _, _| match key {
            gdk::Key::Escape | gdk::Key::F11 => {
                window.close();
                gtk::glib::Propagation::Stop
            }
            gdk::Key::space => {
                app.run_action(Action::TogglePlay);
                gtk::glib::Propagation::Stop
            }
            _ => gtk::glib::Propagation::Proceed,
        });
    }
    window.add_controller(keys);
    let click = gtk::GestureClick::new();
    {
        let window = window.clone();
        click.connect_pressed(move |_, n, _, _| {
            if n >= 2 {
                window.close();
            }
        });
    }
    window.add_controller(click);
    window.connect_close_request(|_| {
        FULL.with(|f| *f.borrow_mut() = None);
        gtk::glib::Propagation::Proceed
    });
    FULL.with(|f| *f.borrow_mut() = Some((window.clone(), picture)));
    window.fullscreen();
    window.present();
}

#[cfg(test)]
mod tests {
    use super::*;
    use cutroom_project::Guide;

    #[test]
    fn ruler_ticks_are_never_closer_than_asked() {
        assert_eq!(tick_step(1.0, 56.0), 100);
        assert_eq!(tick_step(0.25, 56.0), 500);
        assert_eq!(tick_step(10.0, 56.0), 10);
        assert_eq!(tick_step(0.001, 56.0), 5000);
    }

    #[test]
    fn the_nearest_guide_within_reach_is_picked() {
        let guides = [
            Guide { vertical: true, pos: 100 },
            Guide { vertical: false, pos: 200 },
        ];
        let frame = (10.0, 10.0, 0.5);
        // The vertical guide is at x = 10 + 50 = 60.
        assert_eq!(guide_at(&guides, frame, 62.0, 500.0), Some(0));
        // The horizontal one is at y = 10 + 100 = 110.
        assert_eq!(guide_at(&guides, frame, 500.0, 108.0), Some(1));
        assert_eq!(guide_at(&guides, frame, 70.0, 60.0), None);
    }
}
