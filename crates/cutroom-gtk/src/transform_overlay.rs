//! The box over the preview: drag the picture to move it, a corner to scale
//! it, the knob above it to turn it (hold Shift to turn in 15 degree steps).
//! The box follows the picture as it is drawn (slant, perspective, bend).
//! Click a corner to switch between the box, corner pin and mesh warp
//! handles, as in Photoshop's transform box.

use crate::app::App;
use crate::theme;
use cutroom_core::transform_box::{BoxGeom, Grab, Handle};
use cutroom_timeline::{ClipId, Transform};
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::prelude::*;
use cutroom_compose::mapping::Mapper;
use cutroom_compose::Placement;
use std::cell::Cell;
use std::rc::Rc;

/// Pointer distance, in screen pixels, that still counts as touching a handle.
const REACH: f64 = 11.0;
/// Where the knob floats above the picture on screen, in pixels.
const KNOB_PX: f64 = 28.0;

/// Which handles the preview shows over a clip.
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

impl Distort {
    /// What a click on a corner switches to.
    pub fn next(self) -> Distort {
        match self {
            Distort::Off => Distort::Pin,
            Distort::Pin => Distort::Mesh,
            Distort::Mesh => Distort::Off,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Distort::Off => "Transform",
            Distort::Pin => "Corner pin",
            Distort::Mesh => "Mesh warp",
        }
    }
}

const SQUARE: [(f32, f32); 4] = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];

#[derive(Clone, Copy)]
struct Shown {
    clip: ClipId,
    canvas: (f32, f32),
    media: (f32, f32),
    geom: BoxGeom,
    transform: Transform,
    placement: Placement,
}

impl Shown {
    fn mapper(&self) -> Mapper {
        Mapper::new(&self.placement, self.canvas, self.media)
    }
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

/// The handles shown for `clip` (each clip has its own).
pub fn mode_of(app: &App, clip: ClipId) -> Distort {
    app.state
        .borrow()
        .distort_modes
        .get(&clip)
        .copied()
        .unwrap_or_default()
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
    let mut placement = Placement::from(&transform);
    let (geom, media) = match &clip.text_at(session.playhead) {
        Some(spec) => {
            let b = crate::text_render::bounds(spec, canvas.0 as u32, canvas.1 as u32)?;
            let size = (b.w as f32, b.h as f32);
            placement.natural = true;
            (BoxGeom::unfitted(canvas, size, &transform)?, size)
        }
        None => {
            let media = media_size(session, clip.media)?;
            (BoxGeom::new(canvas, media, &transform)?, media)
        }
    };
    Some(Shown {
        clip: id,
        canvas,
        media,
        geom,
        transform,
        placement,
    })
}

/// Where the canvas sits inside the overlay: (left, top, on-screen width).
fn canvas_frame(area: &gtk::DrawingArea, canvas: &gtk::Widget) -> Option<(f64, f64, f64)> {
    let b = canvas.compute_bounds(area)?;
    (b.width() > 1.0).then(|| (f64::from(b.x()), f64::from(b.y()), f64::from(b.width())))
}

/// The project's canvas size in pixels.
fn canvas_size(app: &App) -> Option<(u32, u32)> {
    let st = app.state.borrow();
    let s = st.session.as_ref()?;
    Some((s.doc.settings.width, s.doc.settings.height))
}

/// Where the canvas is (left, top, screen pixels per canvas pixel) and its size.
fn canvas_geometry(
    app: &App,
    area: &gtk::DrawingArea,
    canvas: &gtk::Widget,
) -> Option<((f64, f64, f64), (u32, u32))> {
    let (ox, oy, sw) = canvas_frame(area, canvas)?;
    let size = canvas_size(app)?;
    Some(((ox, oy, sw / f64::from(size.0.max(1))), size))
}

fn warp_floats(w: &cutroom_timeline::Warp) -> ([f32; 8], [f32; 32]) {
    (
        w.pin.map(|v| v as f32 / 1000.0),
        w.mesh.map(|v| v as f32 / 1000.0),
    )
}

/// Where the bending handles are, as points of the picture's (bent) unit square.
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
            let n = (side - 1) as f32;
            (0..side * side)
                .map(|k| {
                    let (i, j) = ((k % side) as f32, (k / side) as f32);
                    bend.forward(i / n, j / n).unwrap_or((i / n, j / n))
                })
                .collect()
        }
    }
}

/// Is handle `idx` of `mode` one of the picture's four corners?
fn is_corner(mode: Distort, idx: usize) -> bool {
    match mode {
        Distort::Off => false,
        Distort::Pin => true,
        Distort::Mesh => {
            let side = cutroom_compose::warp::SIDE;
            matches!(idx, 0 | 3) || idx == side * (side - 1) || idx == side * side - 1
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

/// Is `p` inside the polygon?
fn inside(poly: &[(f32, f32)], p: (f32, f32)) -> bool {
    let mut hit = false;
    let mut j = poly.len().wrapping_sub(1);
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 {
            hit = !hit;
        }
        j = i;
    }
    hit
}

/// Where the corners of the drawn picture are, and the rotate knob (a fixed
/// distance above the top edge, in canvas pixels), all in canvas pixels.
fn box_points(shown: &Shown, knob_offset: f32) -> Option<([(f32, f32); 4], (f32, f32), (f32, f32))> {
    let m = shown.mapper();
    let corners = [
        m.point(0.0, 0.0)?,
        m.point(1.0, 0.0)?,
        m.point(1.0, 1.0)?,
        m.point(0.0, 1.0)?,
    ];
    let top_mid = m.point(0.5, 0.0)?;
    let centre = m.point(0.5, 0.5)?;
    let (dx, dy) = (top_mid.0 - centre.0, top_mid.1 - centre.1);
    let len = dx.hypot(dy).max(1e-3);
    let knob = (
        top_mid.0 + dx / len * knob_offset,
        top_mid.1 + dy / len * knob_offset,
    );
    Some((corners, top_mid, knob))
}

pub fn build(app: &Rc<App>, canvas_widget: &gtk::Widget) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_hexpand(true);
    area.set_vexpand(true);

    {
        let app = app.clone();
        let canvas_widget = canvas_widget.clone();
        area.set_draw_func(move |a, cr, w, h| {
            if let (Some(frame), Some(canvas)) =
                (canvas_frame(a, &canvas_widget), canvas_size(&app))
            {
                crate::preview_tools::draw(&app, cr, (f64::from(w), f64::from(h)), frame, canvas);
            }
            let Some(shown) = current(&app) else {
                return;
            };
            let Some((ox, oy, sw)) = canvas_frame(a, &canvas_widget) else {
                return;
            };
            let k = sw / f64::from(shown.canvas.0);
            let at = |p: (f32, f32)| (ox + f64::from(p.0) * k, oy + f64::from(p.1) * k);
            let accent = theme::ACCENT;
            let mode = mode_of(&app, shown.clip);
            let mapper = shown.mapper();
            cr.save().ok();
            cr.set_line_width(1.5);
            cr.set_source_rgb(accent.0, accent.1, accent.2);

            // The mode, so a click on a corner is understood.
            cr.set_font_size(12.0);
            cr.move_to(ox + 8.0, oy + 18.0);
            let _ = cr.show_text(&format!("{} - click a corner to switch", mode.label()));

            // The outline as the picture is drawn (slant, perspective, bend).
            let outline: Vec<(f64, f64)> = mapper.outline(16).into_iter().map(at).collect();
            if let Some(first) = outline.first() {
                cr.move_to(first.0, first.1);
                for p in &outline[1..] {
                    cr.line_to(p.0, p.1);
                }
                cr.close_path();
                let _ = cr.stroke();
            }

            if mode == Distort::Off {
                if let Some((corners, top_mid, knob)) = box_points(&shown, (KNOB_PX / k) as f32) {
                    let (top_mid, knob) = (at(top_mid), at(knob));
                    cr.move_to(top_mid.0, top_mid.1);
                    cr.line_to(knob.0, knob.1);
                    let _ = cr.stroke();
                    cr.arc(knob.0, knob.1, 6.0, 0.0, std::f64::consts::TAU);
                    cr.set_source_rgb(1.0, 1.0, 1.0);
                    let _ = cr.fill_preserve();
                    cr.set_source_rgb(accent.0, accent.1, accent.2);
                    let _ = cr.stroke();
                    for c in corners.map(at) {
                        cr.rectangle(c.0 - 5.0, c.1 - 5.0, 10.0, 10.0);
                        cr.set_source_rgb(1.0, 1.0, 1.0);
                        let _ = cr.fill_preserve();
                        cr.set_source_rgb(accent.0, accent.1, accent.2);
                        let _ = cr.stroke();
                    }
                }
            } else {
                let pts: Vec<(f64, f64)> = distort_points(&shown, mode)
                    .into_iter()
                    .filter_map(|(u, v)| mapper.plain(u, v))
                    .map(at)
                    .collect();
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
                for (i, p) in pts.iter().enumerate() {
                    if is_corner(mode, i) {
                        cr.rectangle(p.0 - 5.0, p.1 - 5.0, 10.0, 10.0);
                    } else {
                        cr.arc(p.0, p.1, 5.0, 0.0, std::f64::consts::TAU);
                    }
                    cr.set_source_rgb(1.0, 1.0, 1.0);
                    let _ = cr.fill_preserve();
                    cr.set_source_rgb(accent.0, accent.1, accent.2);
                    let _ = cr.stroke();
                }
            }
            cr.restore().ok();
        });
    }

    // What is under the pointer, for the cursor and for starting a drag.
    // `Handle` is the box's; in a bending mode the handle number is separate.
    enum Under {
        Box(Handle),
        Point(usize),
    }
    let hit_at = {
        let app = app.clone();
        let area = area.clone();
        let canvas_widget = canvas_widget.clone();
        move |x: f64, y: f64| -> Option<(Shown, Under, (f32, f32))> {
            let shown = current(&app)?;
            let (ox, oy, sw) = canvas_frame(&area, &canvas_widget)?;
            let k = f64::from(shown.canvas.0) / sw;
            let pos = (((x - ox) * k) as f32, ((y - oy) * k) as f32);
            let reach = (REACH * k) as f32;
            let near = |p: (f32, f32)| (p.0 - pos.0).hypot(p.1 - pos.1) <= reach;
            let mode = mode_of(&app, shown.clip);
            let mapper = shown.mapper();
            if mode == Distort::Off {
                let (corners, _, knob) = box_points(&shown, (KNOB_PX * k) as f32)?;
                if near(knob) {
                    return Some((shown, Under::Box(Handle::Rotate), pos));
                }
                if let Some(i) = corners.iter().position(|c| near(*c)) {
                    return Some((shown, Under::Box(Handle::Corner(i)), pos));
                }
            } else {
                let mut best: Option<(usize, f32)> = None;
                for (i, (u, v)) in distort_points(&shown, mode).into_iter().enumerate() {
                    let Some(c) = mapper.plain(u, v) else {
                        continue;
                    };
                    let d = (c.0 - pos.0).hypot(c.1 - pos.1);
                    if d <= reach && best.map_or(true, |b| d < b.1) {
                        best = Some((i, d));
                    }
                }
                if let Some((i, _)) = best {
                    return Some((shown, Under::Point(i), pos));
                }
            }
            let outline = mapper.outline(16);
            let on_picture = if outline.len() >= 3 {
                inside(&outline, pos)
            } else {
                shown.geom.hit(pos, 0.0, 0.0) == Some(Handle::Body)
            };
            on_picture.then_some((shown, Under::Box(Handle::Body), pos))
        }
    };
    let hit_at = Rc::new(hit_at);

    let motion = gtk::EventControllerMotion::new();
    {
        let (area, hit_at) = (area.clone(), hit_at.clone());
        let (app, canvas_widget) = (app.clone(), canvas_widget.clone());
        motion.connect_motion(move |_, x, y| {
            if let Some((frame, _)) = canvas_geometry(&app, &area, &canvas_widget) {
                let st = app.state.borrow();
                if st.settings.show_guides {
                    if let Some(g) = st.session.as_ref().and_then(|s| {
                        crate::preview_tools::guide_at(&s.doc.guides, frame, x, y)
                            .map(|i| s.doc.guides[i])
                    }) {
                        area.set_cursor_from_name(Some(if g.vertical {
                            "col-resize"
                        } else {
                            "row-resize"
                        }));
                        return;
                    }
                }
            }
            let name = match hit_at(x, y).map(|h| h.1) {
                Some(Under::Box(Handle::Body)) => "move",
                Some(Under::Box(Handle::Corner(i))) => {
                    if i % 2 == 0 {
                        "nwse-resize"
                    } else {
                        "nesw-resize"
                    }
                }
                Some(Under::Box(Handle::Rotate)) => "crosshair",
                Some(Under::Point(_)) => "pointer",
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
    // Was the pointer pressed on a corner, and has it moved since? A press and
    // release on a corner without moving switches the mode.
    let corner_click: Rc<Cell<Option<ClipId>>> = Rc::new(Cell::new(None));
    let moved = Rc::new(Cell::new(false));
    // A guide being made or moved: its index, whether it is vertical, where the
    // press began, and whether letting go now would drop it (back on a ruler or
    // off the picture).
    type GuideHeld = (usize, bool, (f64, f64), bool);
    let guide_drag: Rc<Cell<Option<GuideHeld>>> = Rc::new(Cell::new(None));
    let drag = gtk::GestureDrag::builder().button(1).build();
    {
        let (grab, last, hit_at) = (grab.clone(), last.clone(), hit_at.clone());
        let (dgrab, corner_click, moved) = (dgrab.clone(), corner_click.clone(), moved.clone());
        let app = app.clone();
        let (guide_drag, area, canvas_widget) =
            (guide_drag.clone(), area.clone(), canvas_widget.clone());
        drag.connect_drag_begin(move |_, x, y| {
            last.set(None);
            dgrab.set(None);
            corner_click.set(None);
            moved.set(false);
            grab.set(None);
            guide_drag.set(None);
            // Take hold of a guide, or pull a new one out of a ruler.
            if let Some(((ox, oy, k), _)) = canvas_geometry(&app, &area, &canvas_widget) {
                use crate::preview_tools::{guide_at, RULER};
                let (rulers, guides_on, existing) = {
                    let st = app.state.borrow();
                    (
                        st.settings.show_rulers,
                        st.settings.show_guides,
                        st.session
                            .as_ref()
                            .map(|s| s.doc.guides.clone())
                            .unwrap_or_default(),
                    )
                };
                if guides_on {
                    if let Some(i) = guide_at(&existing, (ox, oy, k), x, y) {
                        guide_drag.set(Some((i, existing[i].vertical, (x, y), false)));
                        return;
                    }
                }
                if rulers && (y < RULER) != (x < RULER) {
                    let vertical = x < RULER;
                    let pos = if vertical { (x - ox) / k } else { (y - oy) / k };
                    let index = {
                        let mut st = app.state.borrow_mut();
                        st.settings.show_guides = true;
                        st.session
                            .as_mut()
                            .map(|s| s.add_guide(vertical, pos.round() as i32))
                    };
                    if let Some(index) = index {
                        guide_drag.set(Some((index, vertical, (x, y), false)));
                        area.queue_draw();
                        return;
                    }
                }
            }
            let Some((shown, under, pos)) = hit_at(x, y) else {
                return;
            };
            let mode = mode_of(&app, shown.clip);
            match under {
                Under::Point(idx) => {
                    dgrab.set(Some((shown, idx)));
                    if is_corner(mode, idx) {
                        corner_click.set(Some(shown.clip));
                    }
                    grab.set(Some((
                        shown.clip,
                        Grab::begin(Handle::Body, pos, shown.transform, shown.geom),
                        shown.canvas,
                        (x, y),
                    )));
                }
                Under::Box(handle) => {
                    if matches!(handle, Handle::Corner(_)) {
                        corner_click.set(Some(shown.clip));
                    }
                    grab.set(Some((
                        shown.clip,
                        Grab::begin(handle, pos, shown.transform, shown.geom),
                        shown.canvas,
                        (x, y),
                    )));
                }
            }
        });
    }
    {
        let (app, area, grab, last) = (app.clone(), area.clone(), grab.clone(), last.clone());
        let canvas_widget = canvas_widget.clone();
        let (dgrab, corner_click, moved) = (dgrab.clone(), corner_click.clone(), moved.clone());
        let guide_drag = guide_drag.clone();
        drag.connect_drag_update(move |gesture, dx, dy| {
            if let Some((index, vertical, (gx, gy), _)) = guide_drag.get() {
                let Some(((ox, oy, k), size)) = canvas_geometry(&app, &area, &canvas_widget)
                else {
                    return;
                };
                let (px, py) = (gx + dx, gy + dy);
                let pos = if vertical { (px - ox) / k } else { (py - oy) / k };
                let limit = if vertical { size.0 } else { size.1 };
                let on_ruler = app.state.borrow().settings.show_rulers
                    && if vertical {
                        px < crate::preview_tools::RULER
                    } else {
                        py < crate::preview_tools::RULER
                    };
                let off = on_ruler || pos < 0.0 || pos > f64::from(limit);
                guide_drag.set(Some((index, vertical, (gx, gy), off)));
                if let Some(s) = app.state.borrow_mut().session.as_mut() {
                    s.set_guide(index, Some(pos.round() as i32));
                }
                area.queue_draw();
                return;
            }
            let Some((clip, g, canvas, (sx, sy))) = grab.get() else {
                return;
            };
            // A corner pressed and barely moved is a click, not a drag.
            if corner_click.get().is_some() && !moved.get() {
                if dx.hypot(dy) < 4.0 {
                    return;
                }
                moved.set(true);
                corner_click.set(None);
            }
            let Some((ox, oy, sw)) = canvas_frame(&area, &canvas_widget) else {
                return;
            };
            let k = f64::from(canvas.0) / sw;
            let pos = (((sx + dx - ox) * k) as f32, ((sy + dy - oy) * k) as f32);
            if let Some((shown, idx)) = dgrab.get() {
                let mode = mode_of(&app, clip);
                let Some(unit) = shown.mapper().unplain(pos.0, pos.1) else {
                    return;
                };
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
        let (dgrab, corner_click) = (dgrab.clone(), corner_click.clone());
        let guide_drag = guide_drag.clone();
        drag.connect_drag_end(move |_, _, _| {
            if let Some((index, _, _, off)) = guide_drag.take() {
                // Dropped back on a ruler or off the picture: the guide is removed.
                if off {
                    if let Some(s) = app.state.borrow_mut().session.as_mut() {
                        s.set_guide(index, None);
                    }
                }
                app.refresh();
                area.queue_draw();
                return;
            }
            dgrab.set(None);
            if let Some(clip) = corner_click.take() {
                // A click on a corner: the next kind of handles for this clip.
                grab.set(None);
                last.set(None);
                {
                    let mut st = app.state.borrow_mut();
                    let next = st.distort_modes.get(&clip).copied().unwrap_or_default().next();
                    st.distort_modes.insert(clip, next);
                }
                app.refresh();
                area.queue_draw();
                return;
            }
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
                    // (The bend is never shared: it is each clip's own.)
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
