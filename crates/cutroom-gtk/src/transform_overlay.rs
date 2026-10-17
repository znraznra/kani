//! The box over the preview: drag the picture to move it, a corner to scale
//! it, the knob above it to turn it (hold Shift to turn in 15 degree steps).

use crate::app::App;
use crate::theme;
use cutroom_core::transform_box::{BoxGeom, Grab, Handle};
use cutroom_timeline::{ClipId, Transform};
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

/// Pointer distance, in screen pixels, that still counts as touching a handle.
const REACH: f64 = 11.0;
/// Where the knob floats above the picture on screen, in pixels.
const KNOB_PX: f64 = 28.0;

/// Which bending handles the preview shows over the selected clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Distort {
    /// The usual box: move, scale, turn.
    #[default]
    Off,
    /// Four handles, one per corner (corner pin).
    Pin,
    /// A 4 x 4 grid of handles (mesh warp).
    Mesh,
}

const SQUARE: [(f32, f32); 4] = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];

#[derive(Clone, Copy)]
struct Shown {
    clip: ClipId,
    canvas: (f32, f32),
    geom: BoxGeom,
    transform: Transform,
}

fn media_size(
    session: &cutroom_core::state::Session,
    media: cutroom_timeline::MediaId,
) -> Option<(f32, f32)> {
    let probe = session
        .doc
        .media_item(media)
        .ok()
        .and_then(|m| m.probe.as_ref())
        .filter(|p| p.width > 0 && p.height > 0);
    if let Some(p) = probe {
        return Some((p.width as f32, p.height as f32));
    }
    // Photos are not probed: ask the file.
    let path = session.media_path(media)?;
    let (_, w, h) = gtk::gdk_pixbuf::Pixbuf::file_info(path)?;
    (w > 0 && h > 0).then_some((w as f32, h as f32))
}

/// The box to show now, if the selected clip's picture is on screen.
fn current(app: &App) -> Option<Shown> {
    let st = app.state.borrow();
    let session = st.session.as_ref()?;
    let id = session.selected?;
    if !session.can_transform(id) {
        return None;
    }
    let (track, clip) = session.doc.timeline.find_clip(id)?;
    if track.hidden
        || clip.disabled
        || session.playhead < clip.start
        || session.playhead >= clip.end()
    {
        return None;
    }
    let canvas = (
        session.doc.settings.width as f32,
        session.doc.settings.height as f32,
    );
    let transform = match st.transform_preview {
        Some((c, t)) if c == id => t,
        _ => clip.transform_at(session.playhead),
    };
    let geom = match &clip.text_at(session.playhead) {
        Some(spec) => {
            let b = crate::text_render::bounds(spec, canvas.0 as u32, canvas.1 as u32)?;
            BoxGeom::unfitted(canvas, (b.w as f32, b.h as f32), &transform)?
        }
        None => {
            let media = media_size(session, clip.media)?;
            BoxGeom::new(canvas, media, &transform)?
        }
    };
    Some(Shown {
        clip: id,
        canvas,
        geom,
        transform,
    })
}

fn unit_to_canvas(shown: &Shown, u: f32, v: f32) -> (f32, f32) {
    let u = if shown.transform.flip_h { 1.0 - u } else { u };
    let v = if shown.transform.flip_v { 1.0 - v } else { v };
    shown.geom.at_unit(u, v)
}

fn canvas_to_unit(shown: &Shown, p: (f32, f32)) -> (f32, f32) {
    let (u, v) = shown.geom.to_unit(p.0, p.1);
    (
        if shown.transform.flip_h { 1.0 - u } else { u },
        if shown.transform.flip_v { 1.0 - v } else { v },
    )
}

fn warp_floats(w: &cutroom_timeline::Warp) -> ([f32; 8], [f32; 32]) {
    (
        w.pin.map(|v| v as f32 / 1000.0),
        w.mesh.map(|v| v as f32 / 1000.0),
    )
}

/// Where the handles are, as points of the picture's unit square.
fn distort_points(shown: &Shown, mode: Distort) -> Vec<(f32, f32)> {
    let (pin, mesh) = warp_floats(&shown.transform.warp);
    match mode {
        Distort::Off => Vec::new(),
        Distort::Pin => (0..4)
            .map(|i| (SQUARE[i].0 + pin[i * 2], SQUARE[i].1 + pin[i * 2 + 1]))
            .collect(),
        Distort::Mesh => {
            let Some(bend) = cutroom_compose::warp::Bend::new(&pin, &mesh) else {
                return Vec::new();
            };
            let side = cutroom_compose::warp::SIDE;
            (0..side * side)
                .map(|k| {
                    let (i, j) = ((k % side) as f32, (k / side) as f32);
                    let n = (side - 1) as f32;
                    bend.forward(i / n, j / n).unwrap_or((i / n, j / n))
                })
                .collect()
        }
    }
}

/// The warp with handle `idx` pulled to the point `unit` of the picture.
fn pulled(shown: &Shown, mode: Distort, idx: usize, unit: (f32, f32)) -> cutroom_timeline::Warp {
    let mut w = shown.transform.warp;
    match mode {
        Distort::Off => {}
        Distort::Pin => {
            w.pin[idx * 2] = ((unit.0 - SQUARE[idx].0) * 1000.0).round() as i32;
            w.pin[idx * 2 + 1] = ((unit.1 - SQUARE[idx].1) * 1000.0).round() as i32;
        }
        Distort::Mesh => {
            let (pin, mesh) = warp_floats(&w);
            // The pin sits on top of the mesh: undo it to find the mesh's own spot.
            let at = cutroom_compose::warp::Bend::new(&pin, &mesh)
                .and_then(|b| b.unpin(unit.0, unit.1))
                .unwrap_or(unit);
            let side = cutroom_compose::warp::SIDE;
            let n = (side - 1) as f32;
            let (i, j) = ((idx % side) as f32, (idx / side) as f32);
            w.mesh[idx * 2] = ((at.0 - i / n) * 1000.0).round() as i32;
            w.mesh[idx * 2 + 1] = ((at.1 - j / n) * 1000.0).round() as i32;
        }
    }
    w.clamped()
}

/// Where the canvas sits inside the overlay: (left, top, on-screen width).
fn canvas_frame(area: &gtk::DrawingArea, canvas: &gtk::Widget) -> Option<(f64, f64, f64)> {
    let b = canvas.compute_bounds(area)?;
    (b.width() > 1.0).then(|| (f64::from(b.x()), f64::from(b.y()), f64::from(b.width())))
}

pub fn build(app: &Rc<App>, canvas_widget: &gtk::Widget) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_hexpand(true);
    area.set_vexpand(true);

    {
        let app = app.clone();
        let canvas_widget = canvas_widget.clone();
        area.set_draw_func(move |a, cr, _, _| {
            let Some(shown) = current(&app) else {
                return;
            };
            let Some((ox, oy, sw)) = canvas_frame(a, &canvas_widget) else {
                return;
            };
            let k = sw / f64::from(shown.canvas.0);
            let at = |p: (f32, f32)| (ox + f64::from(p.0) * k, oy + f64::from(p.1) * k);
            let corners = shown.geom.corners().map(at);
            let accent = theme::ACCENT;
            let mode = app.state.borrow().distort_mode;
            if mode != Distort::Off {
                let pts: Vec<(f64, f64)> = distort_points(&shown, mode)
                    .into_iter()
                    .map(|(u, v)| at(unit_to_canvas(&shown, u, v)))
                    .collect();
                cr.save().ok();
                cr.set_line_width(1.5);
                cr.set_source_rgb(accent.0, accent.1, accent.2);
                let line = |cr: &gtk::cairo::Context, ids: &[usize]| {
                    for (n, id) in ids.iter().enumerate() {
                        if let Some(p) = pts.get(*id) {
                            if n == 0 {
                                cr.move_to(p.0, p.1);
                            } else {
                                cr.line_to(p.0, p.1);
                            }
                        }
                    }
                };
                if mode == Distort::Pin {
                    line(cr, &[0, 1, 2, 3, 0]);
                    let _ = cr.stroke();
                } else {
                    let side = cutroom_compose::warp::SIDE;
                    for a in 0..side {
                        let row: Vec<usize> = (0..side).map(|i| a * side + i).collect();
                        let col: Vec<usize> = (0..side).map(|j| j * side + a).collect();
                        line(cr, &row);
                        let _ = cr.stroke();
                        line(cr, &col);
                        let _ = cr.stroke();
                    }
                }
                for p in &pts {
                    cr.arc(p.0, p.1, 5.5, 0.0, std::f64::consts::TAU);
                    cr.set_source_rgb(1.0, 1.0, 1.0);
                    let _ = cr.fill_preserve();
                    cr.set_source_rgb(accent.0, accent.1, accent.2);
                    let _ = cr.stroke();
                }
                cr.restore().ok();
                return;
            }
            cr.save().ok();
            cr.set_line_width(1.5);
            cr.set_source_rgb(accent.0, accent.1, accent.2);
            cr.move_to(corners[0].0, corners[0].1);
            for c in &corners[1..] {
                cr.line_to(c.0, c.1);
            }
            cr.close_path();
            let _ = cr.stroke();

            // The knob and its stem, a fixed distance above the top edge on screen.
            let offset = (KNOB_PX / k) as f32;
            let top_mid = at(shown.geom.rotate_knob(0.0));
            let knob = at(shown.geom.rotate_knob(offset));
            cr.move_to(top_mid.0, top_mid.1);
            cr.line_to(knob.0, knob.1);
            let _ = cr.stroke();
            cr.arc(knob.0, knob.1, 6.0, 0.0, std::f64::consts::TAU);
            cr.set_source_rgb(1.0, 1.0, 1.0);
            let _ = cr.fill_preserve();
            cr.set_source_rgb(accent.0, accent.1, accent.2);
            let _ = cr.stroke();

            for c in corners {
                cr.rectangle(c.0 - 5.0, c.1 - 5.0, 10.0, 10.0);
                cr.set_source_rgb(1.0, 1.0, 1.0);
                let _ = cr.fill_preserve();
                cr.set_source_rgb(accent.0, accent.1, accent.2);
                let _ = cr.stroke();
            }
            cr.restore().ok();
        });
    }

    // What is under the pointer, for the cursor and for starting a drag.
    let hit_at = {
        let app = app.clone();
        let area = area.clone();
        let canvas_widget = canvas_widget.clone();
        move |x: f64, y: f64| -> Option<(Shown, Handle, (f32, f32))> {
            let shown = current(&app)?;
            let (ox, oy, sw) = canvas_frame(&area, &canvas_widget)?;
            let k = f64::from(shown.canvas.0) / sw;
            let pos = (((x - ox) * k) as f32, ((y - oy) * k) as f32);
            let offset = (KNOB_PX * k) as f32;
            let reach = (REACH * k) as f32;
            let handle = shown.geom.hit(pos, reach, offset)?;
            Some((shown, handle, pos))
        }
    };
    let hit_at = Rc::new(hit_at);

    // A bending handle under the pointer, while a bending mode is on.
    let distort_hit = {
        let app = app.clone();
        let area = area.clone();
        let canvas_widget = canvas_widget.clone();
        move |x: f64, y: f64| -> Option<(Shown, usize, (f32, f32))> {
            let mode = app.state.borrow().distort_mode;
            if mode == Distort::Off {
                return None;
            }
            let shown = current(&app)?;
            let (ox, oy, sw) = canvas_frame(&area, &canvas_widget)?;
            let k = f64::from(shown.canvas.0) / sw;
            let pos = (((x - ox) * k) as f32, ((y - oy) * k) as f32);
            let reach = (REACH * k) as f32;
            let mut best: Option<(usize, f32)> = None;
            for (i, (u, v)) in distort_points(&shown, mode).into_iter().enumerate() {
                let c = unit_to_canvas(&shown, u, v);
                let d = (c.0 - pos.0).hypot(c.1 - pos.1);
                if d <= reach && best.map_or(true, |b| d < b.1) {
                    best = Some((i, d));
                }
            }
            best.map(|(i, _)| (shown, i, pos))
        }
    };
    let distort_hit = Rc::new(distort_hit);

    let motion = gtk::EventControllerMotion::new();
    {
        let (area, hit_at, distort_hit) = (area.clone(), hit_at.clone(), distort_hit.clone());
        motion.connect_motion(move |_, x, y| {
            if distort_hit(x, y).is_some() {
                area.set_cursor_from_name(Some("pointer"));
                return;
            }
            let name = match hit_at(x, y).map(|h| h.1) {
                Some(Handle::Body) => "move",
                Some(Handle::Corner(i)) => {
                    if i % 2 == 0 {
                        "nwse-resize"
                    } else {
                        "nesw-resize"
                    }
                }
                Some(Handle::Rotate) => "crosshair",
                None => "default",
            };
            area.set_cursor_from_name(Some(name));
        });
    }
    area.add_controller(motion);

    // clip, what was grabbed, canvas size, and where on screen the drag began
    type Held = (ClipId, Grab, (f32, f32), (f64, f64));
    let grab: Rc<Cell<Option<Held>>> = Rc::new(Cell::new(None));
    let last: Rc<Cell<Option<Transform>>> = Rc::new(Cell::new(None));
    // A bending handle being dragged: the picture as it was, and which handle.
    let dgrab: Rc<Cell<Option<(Shown, usize)>>> = Rc::new(Cell::new(None));
    let drag = gtk::GestureDrag::builder().button(1).build();
    {
        let (grab, last, hit_at) = (grab.clone(), last.clone(), hit_at.clone());
        let (dgrab, distort_hit) = (dgrab.clone(), distort_hit.clone());
        drag.connect_drag_begin(move |_, x, y| {
            last.set(None);
            dgrab.set(None);
            if let Some((shown, idx, pos)) = distort_hit(x, y) {
                dgrab.set(Some((shown, idx)));
                grab.set(Some((
                    shown.clip,
                    Grab::begin(Handle::Body, pos, shown.transform, shown.geom),
                    shown.canvas,
                    (x, y),
                )));
                return;
            }
            grab.set(hit_at(x, y).map(|(shown, handle, pos)| {
                (
                    shown.clip,
                    Grab::begin(handle, pos, shown.transform, shown.geom),
                    shown.canvas,
                    (x, y),
                )
            }));
        });
    }
    {
        let (app, area, grab, last) = (app.clone(), area.clone(), grab.clone(), last.clone());
        let canvas_widget = canvas_widget.clone();
        let dgrab = dgrab.clone();
        drag.connect_drag_update(move |gesture, dx, dy| {
            let Some((clip, g, canvas, (sx, sy))) = grab.get() else {
                return;
            };
            let Some((ox, oy, sw)) = canvas_frame(&area, &canvas_widget) else {
                return;
            };
            let k = f64::from(canvas.0) / sw;
            let pos = (((sx + dx - ox) * k) as f32, ((sy + dy - oy) * k) as f32);
            if let Some((shown, idx)) = dgrab.get() {
                let mode = app.state.borrow().distort_mode;
                let unit = canvas_to_unit(&shown, pos);
                let mut t = shown.transform;
                t.warp = pulled(&shown, mode, idx, unit);
                last.set(Some(t));
                app.state.borrow_mut().transform_preview = Some((clip, t));
                app.render_preview();
                area.queue_draw();
                return;
            }
            let shift = gesture
                .current_event_state()
                .contains(gdk::ModifierType::SHIFT_MASK);
            let t = g.apply(pos, canvas, shift);
            last.set(Some(t));
            app.state.borrow_mut().transform_preview = Some((clip, t));
            app.render_preview();
            area.queue_draw();
        });
    }
    {
        let (app, area, grab, last) = (app.clone(), area.clone(), grab.clone(), last.clone());
        let dgrab = dgrab.clone();
        drag.connect_drag_end(move |_, _, _| {
            dgrab.set(None);
            let (Some((clip, ..)), Some(t)) = (grab.take(), last.take()) else {
                return;
            };
            {
                let mut st = app.state.borrow_mut();
                st.transform_preview = None;
                let all = st.apply_all_captions;
                if let Some(session) = st.session.as_mut() {
                    // Shared with the other selected clips of the same type,
                    // or with every caption while "Apply to all captions" is on.
                    let targets = session.group_targets(clip, all);
                    session.edit_transform_for(clip, t, &targets);
                }
            }
            app.sync_preview(false);
            app.refresh();
            area.queue_draw();
        });
    }
    area.add_controller(drag);
    area
}
