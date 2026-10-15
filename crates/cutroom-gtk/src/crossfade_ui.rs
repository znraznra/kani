//! The little popover opened from the box on a cut between two clips: how
//! long the crossfade lasts, or remove it.

use crate::app::App;
use cutroom_timeline::{ClipId, Ticks, TICKS_PER_SECOND};
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::{gdk, glib};
use std::rc::Rc;

fn seconds(t: Ticks) -> f64 {
    t.0 as f64 / TICKS_PER_SECOND as f64
}

fn ticks(seconds: f64) -> Ticks {
    Ticks((seconds * TICKS_PER_SECOND as f64).round() as i64)
}

/// Does the cut before `clip` have spare footage, so both pictures overlap?
fn blends(app: &App, clip: ClipId) -> bool {
    let st = app.state.borrow();
    let Some(session) = st.session.as_ref() else {
        return false;
    };
    let Some((ti, ci)) = session.doc.timeline.locate_clip(clip) else {
        return false;
    };
    let track = &session.doc.timeline.tracks[ti];
    cutroom_timeline::transition::window(track, ci).is_some_and(|(from, _)| {
        cutroom_timeline::transition::mix_at(track, ci, from).is_some_and(|m| m.overlapping)
    })
}

pub fn show(app: &Rc<App>, anchor: &gtk::Widget, clip: ClipId, x: f64, y: f64) {
    // The first click gives the cut a one second crossfade to start from.
    let (max, current) = {
        let mut st = app.state.borrow_mut();
        let Some(session) = st.session.as_mut() else {
            return;
        };
        if session.crossfade_of(clip) == Ticks::ZERO {
            let one = Ticks(TICKS_PER_SECOND);
            let start = Ticks(one.0.min(session.max_crossfade_of(clip).0));
            session.set_clip_crossfade(clip, start);
        }
        (session.max_crossfade_of(clip), session.crossfade_of(clip))
    };
    app.sync_preview(true);
    crate::timeline::queue_draw(app);

    let popover = gtk::Popover::new();
    popover.set_parent(anchor);
    popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
    body.set_margin_top(10);
    body.set_margin_bottom(10);
    body.set_margin_start(12);
    body.set_margin_end(12);
    let title = gtk::Label::new(Some("Crossfade"));
    title.add_css_class("panel-title");
    title.set_xalign(0.0);
    body.append(&title);

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let spin = gtk::SpinButton::with_range(0.0, seconds(max).max(0.1), 0.1);
    spin.set_digits(1);
    spin.set_value(seconds(current));
    row.append(&spin);
    row.append(&gtk::Label::new(Some("seconds")));
    body.append(&row);

    let note = gtk::Label::new(None);
    note.add_css_class("dim-label");
    note.set_xalign(0.0);
    note.set_wrap(true);
    note.set_max_width_chars(34);
    let describe = {
        let (app, note) = (app.clone(), note.clone());
        move || {
            note.set_text(if blends(&app, clip) {
                "The two clips blend into each other."
            } else {
                "No spare footage at this cut, so each clip fades through its own end."
            });
        }
    };
    describe();
    body.append(&note);

    let remove = gtk::Button::with_label("Remove crossfade");
    body.append(&remove);
    popover.set_child(Some(&body));

    {
        let app = app.clone();
        spin.connect_value_changed(move |s| {
            if let Some(session) = app.state.borrow_mut().session.as_mut() {
                session.set_clip_crossfade(clip, ticks(s.value()));
            }
            describe();
            app.sync_preview(true);
            crate::timeline::queue_draw(&app);
        });
    }
    {
        let (app, popover) = (app.clone(), popover.clone());
        remove.connect_clicked(move |_| {
            if let Some(session) = app.state.borrow_mut().session.as_mut() {
                session.set_clip_crossfade(clip, Ticks::ZERO);
            }
            app.sync_preview(true);
            app.refresh();
            popover.popdown();
        });
    }
    popover.connect_closed(|p| {
        let p = p.clone();
        glib::idle_add_local_once(move || p.unparent());
    });
    popover.popup();
}
