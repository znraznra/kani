//! The inspector's "Transform" controls: move, scale, turn and fade the
//! selected clip's picture. Dragging a slider previews the change at once;
//! the edit is recorded (one undo step) when the slider is let go.

use crate::app::App;
use cutroom_timeline::{ClipId, Prop, Transform};
use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

/// The slider values the controls show (percent and degrees).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sliders {
    pub x: f64,
    pub y: f64,
    pub scale: f64,
    pub rotation: f64,
    pub opacity: f64,
    pub skew_x: f64,
    pub skew_y: f64,
    pub persp_x: f64,
    pub persp_y: f64,
}

pub const SKEW_RANGE: (f64, f64) = (-60.0, 60.0);
pub const PERSP_RANGE: (f64, f64) = (-80.0, 80.0);
pub const X_RANGE: (f64, f64) = (-100.0, 100.0);
pub const Y_RANGE: (f64, f64) = (-100.0, 100.0);
pub const SCALE_RANGE: (f64, f64) = (10.0, 400.0);
pub const ROTATION_RANGE: (f64, f64) = (-180.0, 180.0);
pub const OPACITY_RANGE: (f64, f64) = (0.0, 100.0);

pub fn to_sliders(t: &Transform) -> Sliders {
    Sliders {
        x: f64::from(t.x) / 10.0,
        y: f64::from(t.y) / 10.0,
        scale: f64::from(t.scale) / 10.0,
        rotation: f64::from(t.rotation) / 10.0,
        opacity: f64::from(t.opacity) / 10.0,
        skew_x: f64::from(t.skew_x) / 10.0,
        skew_y: f64::from(t.skew_y) / 10.0,
        persp_x: f64::from(t.persp_x) / 10.0,
        persp_y: f64::from(t.persp_y) / 10.0,
    }
}

/// The transform for slider values (the flips come from `base`).
pub fn from_sliders(s: &Sliders, base: &Transform) -> Transform {
    Transform {
        x: (s.x * 10.0).round() as i32,
        y: (s.y * 10.0).round() as i32,
        scale: (s.scale * 10.0).round() as i32,
        rotation: (s.rotation * 10.0).round() as i32,
        opacity: (s.opacity * 10.0).round() as i32,
        skew_x: (s.skew_x * 10.0).round() as i32,
        skew_y: (s.skew_y * 10.0).round() as i32,
        persp_x: (s.persp_x * 10.0).round() as i32,
        persp_y: (s.persp_y * 10.0).round() as i32,
        ..*base
    }
    .clamped()
}

/// The diamond: a keyframe for these settings at the playhead, on or off.
/// Filled when there is a keyframe here, accent-coloured when the settings
/// are animated at all.
pub fn key_button(app: &Rc<App>, clip: ClipId, props: &[Prop]) -> gtk::ToggleButton {
    let (on, animated) = app
        .state
        .borrow()
        .session
        .as_ref()
        .map_or((false, false), |s| {
            (
                props.iter().all(|p| s.has_key_at_playhead(clip, *p)),
                props.iter().any(|p| s.is_animated(clip, *p)),
            )
        });
    let b = crate::icons::toggle(
        if on {
            crate::icons::KEYFRAME_ON
        } else {
            crate::icons::KEYFRAME
        },
        "Add or remove a keyframe at the playhead",
    );
    b.set_active(on);
    if animated {
        b.add_css_class("animated");
    }
    let props = props.to_vec();
    let app = app.clone();
    b.connect_clicked(move |_| {
        if let Some(s) = app.state.borrow_mut().session.as_mut() {
            s.toggle_keyframes(clip, &props);
        }
        app.sync_preview(true);
        app.refresh();
    });
    b
}

/// The controls for `clip`, starting from `initial`.
pub fn controls(app: &Rc<App>, clip: ClipId, initial: Transform) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
    column.add_css_class("cc-section");
    let grid = gtk::Grid::new();
    grid.set_column_spacing(8);
    grid.set_row_spacing(6);
    let next_row = Cell::new(0i32);
    let current = Rc::new(Cell::new(initial));
    let building = Rc::new(Cell::new(true));
    let dragging = Rc::new(Cell::new(false));
    let generation = Rc::new(Cell::new(0u32));

    let commit = {
        let (app, current) = (app.clone(), current.clone());
        Rc::new(move || {
            {
                let mut st = app.state.borrow_mut();
                st.transform_preview = None;
                let all = st.apply_all_captions;
                if let Some(session) = st.session.as_mut() {
                    let targets = session.group_targets(clip, all);
                    session.edit_transform_for(clip, current.get(), &targets);
                }
            }
            app.sync_preview(false);
            app.refresh();
        })
    };
    // Rebuilding the inspector while one of its own controls is still handling
    // an event would pull the widget out from under it, so recording waits for
    // the event to finish.
    let commit_soon: Rc<dyn Fn()> = {
        let commit = commit.clone();
        Rc::new(move || {
            let commit = commit.clone();
            glib::idle_add_local_once(move || commit());
        })
    };
    // Called with the new transform whenever a control changes.
    let changed = {
        let (app, current, building, dragging, generation, commit) = (
            app.clone(),
            current.clone(),
            building.clone(),
            dragging.clone(),
            generation.clone(),
            commit.clone(),
        );
        Rc::new(move |new: Transform| {
            if building.get() {
                return;
            }
            current.set(new);
            app.state.borrow_mut().transform_preview = Some((clip, new));
            app.render_preview();
            // The box on the picture follows the values as they change.
            app.ui().editor.queue_overlay();
            if !dragging.get() {
                // Keys and the mouse wheel: record once things go quiet.
                let id = generation.get().wrapping_add(1);
                generation.set(id);
                let (generation, dragging, commit) =
                    (generation.clone(), dragging.clone(), commit.clone());
                glib::timeout_add_local_once(Duration::from_millis(500), move || {
                    if generation.get() == id && !dragging.get() {
                        commit();
                    }
                });
            }
        })
    };

    let diamond = {
        let app = app.clone();
        move |props: &[Prop]| key_button(&app, clip, props)
    };
    let sliders = to_sliders(&initial);
    let add_slider = |label: &str,
                      range: (f64, f64),
                      value: f64,
                      unit: &'static str,
                      prop: Option<Prop>,
                      set: fn(&mut Sliders, f64)| {
        let caption = gtk::Label::new(Some(label));
        caption.set_xalign(0.0);
        caption.add_css_class("cc-field-label");
        let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, range.0, range.1, 1.0);
        scale.set_hexpand(true);
        scale.set_size_request(96, -1);
        scale.set_draw_value(false);
        scale.set_value(value);
        // The number beside the slider can be clicked and typed into.
        let spin = gtk::SpinButton::with_range(range.0, range.1, 1.0);
        spin.set_digits(0);
        spin.set_numeric(false);
        spin.set_width_chars(5);
        spin.set_max_width_chars(5);
        spin.add_css_class("cc-spin");
        spin.set_value(value);
        spin.connect_output(move |s| {
            s.set_text(&format!("{:.0}{unit}", s.value()));
            glib::Propagation::Stop
        });
        spin.connect_input(move |s| {
            let text = s.text();
            let cleaned: String = text
                .chars()
                .filter(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | '.'))
                .collect();
            let Ok(typed) = cleaned.parse::<f64>() else {
                return Some(Err(()));
            };
            // The box shows whole numbers; losing focus must not round away
            // the fraction a drag on the picture left behind.
            if (typed - s.value()).abs() < 0.5
                && format!("{:.0}", s.value()) == format!("{typed:.0}")
            {
                Some(Ok(s.value()))
            } else {
                Some(Ok(typed))
            }
        });
        // Watch the mouse without claiming it, so the slider's own drag works.
        let watch = gtk::GestureClick::new();
        watch.set_propagation_phase(gtk::PropagationPhase::Capture);
        {
            let dragging = dragging.clone();
            watch.connect_pressed(move |_, _, _, _| dragging.set(true));
        }
        {
            let (dragging, commit, building) =
                (dragging.clone(), commit_soon.clone(), building.clone());
            watch.connect_released(move |_, _, _, _| {
                dragging.set(false);
                if !building.get() {
                    commit();
                }
            });
        }
        {
            // The slider claims the mouse for its own drag, which cancels this
            // watcher instead of releasing it: record the value here too.
            let (dragging, commit, building) =
                (dragging.clone(), commit_soon.clone(), building.clone());
            watch.connect_cancel(move |_, _| {
                dragging.set(false);
                if !building.get() {
                    commit();
                }
            });
        }
        scale.add_controller(watch);
        {
            let spin = spin.clone();
            scale.connect_value_changed(move |s| {
                if (spin.value() - s.value()).abs() > f64::EPSILON {
                    spin.set_value(s.value());
                }
            });
        }
        {
            let scale = scale.clone();
            spin.connect_value_changed(move |s| {
                if (scale.value() - s.value()).abs() > f64::EPSILON {
                    scale.set_value(s.value());
                }
            });
        }
        {
            let (changed, current) = (changed.clone(), current.clone());
            scale.connect_value_changed(move |s| {
                let mut values = to_sliders(&current.get());
                set(&mut values, s.value());
                changed(from_sliders(&values, &current.get()));
            });
        }
        let r = next_row.get();
        next_row.set(r + 1);
        grid.attach(&caption, 0, r, 1, 1);
        grid.attach(&scale, 1, r, 1, 1);
        grid.attach(&spin, 2, r, 1, 1);
        if let Some(prop) = prop {
            grid.attach(&diamond(&[prop]), 3, r, 1, 1);
        }
    };
    add_slider(
        "Position X",
        X_RANGE,
        sliders.x,
        "%",
        Some(Prop::X),
        |s, v| s.x = v,
    );
    add_slider(
        "Position Y",
        Y_RANGE,
        sliders.y,
        "%",
        Some(Prop::Y),
        |s, v| s.y = v,
    );
    add_slider(
        "Scale",
        SCALE_RANGE,
        sliders.scale,
        "%",
        Some(Prop::Scale),
        |s, v| s.scale = v,
    );
    add_slider(
        "Rotation",
        ROTATION_RANGE,
        sliders.rotation,
        "\u{b0}",
        Some(Prop::Rotation),
        |s, v| s.rotation = v,
    );
    add_slider(
        "Opacity",
        OPACITY_RANGE,
        sliders.opacity,
        "%",
        Some(Prop::Opacity),
        |s, v| s.opacity = v,
    );
    // Slant and perspective.
    add_slider(
        "Skew X",
        SKEW_RANGE,
        sliders.skew_x,
        "\u{b0}",
        Some(Prop::SkewX),
        |s, v| s.skew_x = v,
    );
    add_slider(
        "Skew Y",
        SKEW_RANGE,
        sliders.skew_y,
        "\u{b0}",
        Some(Prop::SkewY),
        |s, v| s.skew_y = v,
    );
    add_slider(
        "Perspective X",
        PERSP_RANGE,
        sliders.persp_x,
        "%",
        Some(Prop::PerspX),
        |s, v| s.persp_x = v,
    );
    add_slider(
        "Perspective Y",
        PERSP_RANGE,
        sliders.persp_y,
        "%",
        Some(Prop::PerspY),
        |s, v| s.persp_y = v,
    );

    let flips = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    for (icon, tip, horizontal) in [
        (crate::icons::MIRROR, "Mirror (left and right)", true),
        (crate::icons::FLIP, "Flip upside down", false),
    ] {
        let check = crate::icons::toggle(icon, tip);
        check.set_active(if horizontal {
            initial.flip_h
        } else {
            initial.flip_v
        });
        let (current, app, commit) = (current.clone(), app.clone(), commit_soon.clone());
        let building = building.clone();
        check.connect_toggled(move |c| {
            if building.get() {
                return;
            }
            let mut t = current.get();
            if horizontal {
                t.flip_h = c.is_active();
            } else {
                t.flip_v = c.is_active();
            }
            current.set(t);
            app.state.borrow_mut().transform_preview = Some((clip, t));
            commit();
        });
        flips.append(&check);
    }
    column.append(&flips);
    column.append(&grid);

    let reset = crate::icons::button(crate::icons::RESET, "Reset position, size, rotation, opacity, skew and perspective (and remove their keyframes)");
    {
        let app = app.clone();
        reset.connect_clicked(move |_| {
            glib::idle_add_local_once({
                let app = app.clone();
                move || {
                    {
                        let mut st = app.state.borrow_mut();
                        st.transform_preview = None;
                        if let Some(session) = st.session.as_mut() {
                            session.reset_transform(clip);
                        }
                    }
                    app.sync_preview(false);
                    app.refresh();
                }
            });
        });
    }
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let title = gtk::Label::new(Some("Position & Size"));
    title.add_css_class("cc-section-title");
    title.set_xalign(0.0);
    title.set_hexpand(true);
    header.append(&title);
    header.append(&reset);
    header.append(&diamond(&Prop::TRANSFORM));
    column.prepend(&header);
    building.set(false);
    column
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sliders_and_transforms_convert_both_ways() {
        let t = Transform {
            x: -250,
            y: 125,
            scale: 1500,
            rotation: -450,
            opacity: 800,
            flip_h: true,
            flip_v: false,
            skew_x: 150,
            skew_y: -300,
            persp_x: 400,
            persp_y: -250,
        };
        let s = to_sliders(&t);
        assert_eq!(
            (s.x, s.y, s.scale, s.rotation, s.opacity),
            (-25.0, 12.5, 150.0, -45.0, 80.0)
        );
        assert_eq!(from_sliders(&s, &t), t, "round trip, flips kept");
        assert_eq!(
            from_sliders(&to_sliders(&Transform::default()), &Transform::default()),
            Transform::default()
        );
    }

    #[test]
    fn slider_values_are_clamped_into_the_legal_range() {
        let s = Sliders {
            x: 9999.0,
            y: -9999.0,
            scale: 0.0,
            rotation: 540.0,
            opacity: 400.0,
            skew_x: 0.0,
            skew_y: 0.0,
            persp_x: 0.0,
            persp_y: 0.0,
        };
        let t = from_sliders(&s, &Transform::default());
        assert_eq!((t.x, t.y), (5000, -5000));
        assert_eq!(t.scale, Transform::SCALE_MIN);
        assert_eq!(t.rotation, -1800, "540 degrees is a turn and a half");
        assert_eq!(t.opacity, 1000);
    }

    #[test]
    fn the_slider_ranges_cover_the_everyday_values() {
        let d = to_sliders(&Transform::default());
        for (value, range) in [
            (d.x, X_RANGE),
            (d.y, Y_RANGE),
            (d.scale, SCALE_RANGE),
            (d.rotation, ROTATION_RANGE),
            (d.opacity, OPACITY_RANGE),
        ] {
            assert!(value >= range.0 && value <= range.1, "{value} in {range:?}");
        }
    }
}
