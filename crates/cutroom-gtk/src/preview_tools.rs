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
use std::cell::{Cell, RefCell};
use std::rc::Rc;

// ----- snapping -------------------------------------------------------------------

/// How near (in screen pixels) an edge or the middle of a moved picture must
/// come to a line to snap onto it.
pub const SNAP_REACH: f64 = 6.0;

thread_local! {
    /// The lines a moved picture is snapped to right now, in canvas pixels
    /// (a vertical line's x, a horizontal line's y), drawn lit.
    static SNAPPED: Cell<(Option<f32>, Option<f32>)> = const { Cell::new((None, None)) };
}

/// Show (or clear) the lines a picture is snapped to.
pub fn set_snapped(lines: (Option<f32>, Option<f32>)) {
    SNAPPED.with(|s| s.set(lines));
}

/// The shift that brings the nearest of `edges` onto one of `lines`, and that
/// line, when one is within `reach`.
pub fn snap_shift(edges: &[f32], lines: &[f32], reach: f32) -> Option<(f32, f32)> {
    let mut best: Option<(f32, f32)> = None;
    for e in edges {
        for l in lines {
            let d = l - e;
            if d.abs() <= reach && best.is_none_or(|(b, _)| d.abs() < b.abs()) {
                best = Some((d, *l));
            }
        }
    }
    best
}

/// The lines a moved picture may snap to, in canvas pixels: the picture's
/// edges and middle always, the guides while they are shown, and the grid
/// lines while the grid is shown. (vertical lines' x, horizontal lines' y)
pub fn snap_lines(app: &App, canvas: (f32, f32)) -> (Vec<f32>, Vec<f32>) {
    let st = app.state.borrow();
    let mut xs = vec![0.0, canvas.0 / 2.0, canvas.0];
    let mut ys = vec![0.0, canvas.1 / 2.0, canvas.1];
    if st.settings.show_guides {
        if let Some(s) = st.session.as_ref() {
            for g in &s.doc.guides {
                if g.vertical {
                    xs.push(g.pos as f32);
                } else {
                    ys.push(g.pos as f32);
                }
            }
        }
    }
    if st.settings.show_grid {
        let cell = canvas.0 / GRID_CELLS as f32;
        if cell > 1.0 {
            let mut v = cell;
            while v < canvas.0 {
                xs.push(v);
                v += cell;
            }
            let mut v = cell;
            while v < canvas.1 {
                ys.push(v);
                v += cell;
            }
        }
    }
    (xs, ys)
}

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
    // The lines a picture being moved has snapped to.
    let (sx, sy) = SNAPPED.with(|s| s.get());
    if sx.is_some() || sy.is_some() {
        cr.set_line_width(1.0);
        cr.set_source_rgba(1.0, 0.25, 0.75, 0.95);
        if let Some(x) = sx {
            let x = (ox + f64::from(x) * k).round() + 0.5;
            cr.move_to(x, oy);
            cr.line_to(x, oy + ch);
        }
        if let Some(y) = sy {
            let y = (oy + f64::from(y) * k).round() + 0.5;
            cr.move_to(ox, y);
            cr.line_to(ox + cw, y);
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
    let rows: [(&str, &str, Get, Set); 4] = [
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
        (
            "Lock guides",
            "Guides stay where they are: they cannot be moved, made or removed until unlocked.",
            |s| s.lock_guides,
            |s, v| s.lock_guides = v,
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
    if app.state.borrow().settings.lock_guides {
        app.set_status("Guides are locked: unlock them in the Player menu first");
        app.refresh();
        return;
    }
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

/// Open the preview full screen on black, or close it. A note at the top
/// says how to leave; the controls along the bottom (play, the position
/// slider, the time and an exit icon) show while the pointer moves.
/// Esc, F11 or a double click close it; Space plays and pauses.
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
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&picture));

    // The note at the top, for three seconds.
    let toast = gtk::Label::new(Some("Entering full screen preview. Press Esc to exit"));
    toast.add_css_class("cc-toast");
    let toast_holder = gtk::Revealer::new();
    toast_holder.set_transition_type(gtk::RevealerTransitionType::Crossfade);
    toast_holder.set_child(Some(&toast));
    toast_holder.set_halign(gtk::Align::Center);
    toast_holder.set_valign(gtk::Align::Start);
    toast_holder.set_margin_top(24);
    toast_holder.set_can_target(false);
    toast_holder.set_reveal_child(true);
    overlay.add_overlay(&toast_holder);
    {
        let toast_holder = toast_holder.clone();
        gtk::glib::timeout_add_local_once(std::time::Duration::from_secs(3), move || {
            toast_holder.set_reveal_child(false);
        });
    }

    // The controls along the bottom.
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    bar.add_css_class("cc-fullscreen-bar");
    let play = icons::button(icons::PLAY, "Play / pause (Space)");
    play.set_widget_name(icons::PLAY);
    let seek = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.001);
    seek.set_draw_value(false);
    seek.set_hexpand(true);
    let time = gtk::Label::new(None);
    time.add_css_class("timecode");
    // The same icon as the Full screen button in the player.
    let exit = icons::button(icons::FULLSCREEN, "Exit full screen (Esc)");
    // Keys go to the window (Space plays), not to these controls.
    play.set_focusable(false);
    seek.set_focusable(false);
    exit.set_focusable(false);
    bar.append(&play);
    bar.append(&seek);
    bar.append(&time);
    bar.append(&exit);
    let controls = gtk::Revealer::new();
    controls.set_transition_type(gtk::RevealerTransitionType::Crossfade);
    controls.set_child(Some(&bar));
    controls.set_valign(gtk::Align::End);
    controls.set_reveal_child(true);
    overlay.add_overlay(&controls);

    let window = gtk::Window::builder()
        .title("Kani - full screen preview")
        .transient_for(&app.window)
        .child(&overlay)
        .build();
    window.add_css_class("preview-fullscreen");
    {
        let app = app.clone();
        play.connect_clicked(move |_| app.run_action(Action::TogglePlay));
    }
    {
        let window = window.clone();
        exit.connect_clicked(move |_| window.close());
    }
    // Moving the slider moves the playhead (the update below must not).
    let updating = Rc::new(std::cell::Cell::new(false));
    {
        let (app, updating) = (app.clone(), updating.clone());
        seek.connect_value_changed(move |scale| {
            if updating.get() {
                return;
            }
            {
                let mut st = app.state.borrow_mut();
                let Some(s) = st.session.as_mut() else { return };
                let end = s.timeline_end();
                let t = cutroom_timeline::Ticks((end.0 as f64 * scale.value()).round() as i64);
                s.set_playhead(t);
            }
            app.sync_preview(true);
            crate::editor::on_tick(&app);
        });
    }
    // Keep the slider, time and play button in step; hide the controls when
    // the pointer rests.
    let idle_since = Rc::new(std::cell::Cell::new(std::time::Instant::now()));
    {
        let (app, seek, time, play, controls, updating, idle_since) = (
            app.clone(),
            seek.clone(),
            time.clone(),
            play.clone(),
            controls.clone(),
            updating.clone(),
            idle_since.clone(),
        );
        gtk::glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
            if !fullscreen_open() {
                return gtk::glib::ControlFlow::Break;
            }
            let facts = {
                let st = app.state.borrow();
                st.session.as_ref().map(|s| {
                    (
                        s.playhead,
                        s.timeline_end(),
                        s.playing,
                        s.doc.settings.frame_rate,
                    )
                })
            };
            if let Some((at, end, playing, rate)) = facts {
                updating.set(true);
                seek.set_value(if end.0 > 0 {
                    at.0 as f64 / end.0 as f64
                } else {
                    0.0
                });
                updating.set(false);
                time.set_text(&format!(
                    "{} / {}",
                    cutroom_core::view::timecode(at, rate),
                    cutroom_core::view::timecode(end, rate)
                ));
                icons::set_button_icon(&play, if playing { icons::PAUSE } else { icons::PLAY }, 16);
            }
            controls
                .set_reveal_child(idle_since.get().elapsed() < std::time::Duration::from_secs(3));
            gtk::glib::ControlFlow::Continue
        });
    }
    let motion = gtk::EventControllerMotion::new();
    {
        let (idle_since, controls) = (idle_since.clone(), controls.clone());
        motion.connect_motion(move |_, _, _| {
            idle_since.set(std::time::Instant::now());
            controls.set_reveal_child(true);
        });
    }
    window.add_controller(motion);
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
    picture.add_controller(click);
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
    fn the_nearest_edge_snaps_to_the_nearest_line_within_reach() {
        // Edges at 100, 150, 200; lines at 0, 154, 540.
        assert_eq!(
            snap_shift(&[100.0, 150.0, 200.0], &[0.0, 154.0, 540.0], 6.0),
            Some((4.0, 154.0))
        );
        assert_eq!(
            snap_shift(&[100.0, 150.0, 200.0], &[0.0, 160.0, 540.0], 6.0),
            None
        );
        assert_eq!(
            snap_shift(&[100.0], &[97.0, 104.0], 6.0),
            Some((-3.0, 97.0))
        );
    }

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
            Guide {
                vertical: true,
                pos: 100,
            },
            Guide {
                vertical: false,
                pos: 200,
            },
        ];
        let frame = (10.0, 10.0, 0.5);
        // The vertical guide is at x = 10 + 50 = 60.
        assert_eq!(guide_at(&guides, frame, 62.0, 500.0), Some(0));
        // The horizontal one is at y = 10 + 100 = 110.
        assert_eq!(guide_at(&guides, frame, 500.0, 108.0), Some(1));
        assert_eq!(guide_at(&guides, frame, 70.0, 60.0), None);
    }
}
