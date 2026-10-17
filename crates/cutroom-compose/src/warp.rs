//! Corner pin and mesh warp: bending a picture before it is placed.
//!
//! Everything here works in the picture's own unit square: (0, 0) is its top
//! left corner and (1, 1) its bottom right. The picture is first bent by the
//! mesh (a grid of points, drawn patch by patch), then by the corner pin (the
//! four corners of the result, with true perspective). Drawing goes the other
//! way round: for every pixel of the result, find where in the picture it
//! comes from.

use crate::Frame;

/// Points on a side of the mesh (must match `cutroom_timeline::MESH_SIDE`).
pub const SIDE: usize = 4;
const CELLS: usize = SIDE - 1;

type P = (f32, f32);

fn cross(a: P, b: P) -> f32 {
    a.0 * b.1 - a.1 * b.0
}

/// The picture-to-result bend, ready to run both ways.
pub struct Bend {
    /// Where each mesh point is (grid row by row), or `None` for no mesh.
    mesh: Option<[P; SIDE * SIDE]>,
    /// The corner pin as a matrix taking the unit square to the pinned shape,
    /// and its inverse; `None` for no pin.
    pin: Option<([f32; 9], [f32; 9])>,
}

impl Bend {
    /// From offsets in units of the picture size (`pin`: four corners x, y;
    /// `mesh`: the grid's points x, y). `None` if the pin cannot be undone.
    pub fn new(pin: &[f32; 8], mesh: &[f32; SIDE * SIDE * 2]) -> Option<Bend> {
        let pin = if pin.iter().all(|v| *v == 0.0) {
            None
        } else {
            let corners = [
                (pin[0], pin[1]),
                (1.0 + pin[2], pin[3]),
                (1.0 + pin[4], 1.0 + pin[5]),
                (pin[6], 1.0 + pin[7]),
            ];
            let m = square_to_quad(corners);
            Some((m, invert(&m)?))
        };
        let mesh = if mesh.iter().all(|v| *v == 0.0) {
            None
        } else {
            let mut pts = [(0.0, 0.0); SIDE * SIDE];
            for j in 0..SIDE {
                for i in 0..SIDE {
                    let k = j * SIDE + i;
                    pts[k] = (
                        i as f32 / CELLS as f32 + mesh[k * 2],
                        j as f32 / CELLS as f32 + mesh[k * 2 + 1],
                    );
                }
            }
            Some(pts)
        };
        Some(Bend { mesh, pin })
    }

    /// Where a point of the picture ends up (unit square in, unit square out).
    pub fn forward(&self, a: f32, b: f32) -> Option<P> {
        let mut p = (a, b);
        if let Some(mesh) = &self.mesh {
            let ci = ((a * CELLS as f32).floor().max(0.0) as usize).min(CELLS - 1);
            let cj = ((b * CELLS as f32).floor().max(0.0) as usize).min(CELLS - 1);
            let (s, t) = (a * CELLS as f32 - ci as f32, b * CELLS as f32 - cj as f32);
            let at = |i: usize, j: usize| mesh[j * SIDE + i];
            let (p00, p10, p01, p11) = (at(ci, cj), at(ci + 1, cj), at(ci, cj + 1), at(ci + 1, cj + 1));
            let top = (p00.0 + (p10.0 - p00.0) * s, p00.1 + (p10.1 - p00.1) * s);
            let bottom = (p01.0 + (p11.0 - p01.0) * s, p01.1 + (p11.1 - p01.1) * s);
            p = (top.0 + (bottom.0 - top.0) * t, top.1 + (bottom.1 - top.1) * t);
        }
        if let Some((m, _)) = &self.pin {
            p = apply(m, p)?;
        }
        Some(p)
    }

    /// Undoes the corner pin only: where a point of the pinned result is
    /// before the pin (for dragging mesh points that sit under a pin).
    pub fn unpin(&self, a: f32, b: f32) -> Option<P> {
        match &self.pin {
            Some((_, inv)) => apply(inv, (a, b)),
            None => Some((a, b)),
        }
    }

    /// Where in the picture a point of the result comes from.
    pub fn backward(&self, x: f32, y: f32) -> Option<P> {
        let mut p = (x, y);
        if let Some((_, inv)) = &self.pin {
            p = apply(inv, p)?;
        }
        if let Some(mesh) = &self.mesh {
            p = mesh_back(mesh, p)?;
        }
        Some(p)
    }
}

fn apply(m: &[f32; 9], p: P) -> Option<P> {
    let w = m[6] * p.0 + m[7] * p.1 + m[8];
    if !w.is_finite() || w.abs() < 1e-5 {
        return None;
    }
    let (x, y) = (
        (m[0] * p.0 + m[1] * p.1 + m[2]) / w,
        (m[3] * p.0 + m[4] * p.1 + m[5]) / w,
    );
    (x.is_finite() && y.is_finite()).then_some((x, y))
}

/// The matrix taking (0,0), (1,0), (1,1), (0,1) to the four corners.
fn square_to_quad(c: [P; 4]) -> [f32; 9] {
    let [(x0, y0), (x1, y1), (x2, y2), (x3, y3)] = c;
    let (dx1, dx2, dx3) = (x1 - x2, x3 - x2, x0 - x1 + x2 - x3);
    let (dy1, dy2, dy3) = (y1 - y2, y3 - y2, y0 - y1 + y2 - y3);
    if dx3.abs() < 1e-9 && dy3.abs() < 1e-9 {
        return [x1 - x0, x3 - x0, x0, y1 - y0, y3 - y0, y0, 0.0, 0.0, 1.0];
    }
    let den = dx1 * dy2 - dy1 * dx2;
    let g = (dx3 * dy2 - dy3 * dx2) / den;
    let h = (dx1 * dy3 - dy1 * dx3) / den;
    [
        x1 - x0 + g * x1,
        x3 - x0 + h * x3,
        x0,
        y1 - y0 + g * y1,
        y3 - y0 + h * y3,
        y0,
        g,
        h,
        1.0,
    ]
}

fn invert(m: &[f32; 9]) -> Option<[f32; 9]> {
    let [a, b, c, d, e, f, g, h, i] = *m;
    let (co0, co1, co2) = (e * i - f * h, f * g - d * i, d * h - e * g);
    let det = a * co0 + b * co1 + c * co2;
    if !det.is_finite() || det.abs() < 1e-9 {
        return None;
    }
    let k = 1.0 / det;
    Some([
        co0 * k,
        (c * h - b * i) * k,
        (b * f - c * e) * k,
        co1 * k,
        (a * i - c * g) * k,
        (c * d - a * f) * k,
        co2 * k,
        (b * g - a * h) * k,
        (a * e - b * d) * k,
    ])
}

/// Which point of a four-cornered patch (a, b, c, d going round) is at `p`:
/// how far across (u) and down (v) it is, if `p` is inside.
fn inverse_bilinear(p: P, a: P, b: P, c: P, d: P) -> Option<P> {
    let e = (b.0 - a.0, b.1 - a.1);
    let f = (d.0 - a.0, d.1 - a.1);
    let g = (a.0 - b.0 + c.0 - d.0, a.1 - b.1 + c.1 - d.1);
    let h = (p.0 - a.0, p.1 - a.1);
    let k2 = cross(g, f);
    let k1 = cross(e, f) + cross(h, g);
    let k0 = cross(h, e);
    let along = |v: f32| -> Option<P> {
        let (dx, dy) = (e.0 + g.0 * v, e.1 + g.1 * v);
        // Use the better-conditioned of the two ways to get u.
        let u = if dx.abs() > dy.abs() {
            (h.0 - f.0 * v) / dx
        } else {
            (h.1 - f.1 * v) / dy
        };
        (u.is_finite() && v.is_finite()).then_some((u, v))
    };
    let inside = |q: P| q.0 >= -1e-3 && q.0 <= 1.001 && q.1 >= -1e-3 && q.1 <= 1.001;
    if k2.abs() < 1e-6 {
        if k1.abs() < 1e-9 {
            return None;
        }
        return along(-k0 / k1).filter(|q| inside(*q));
    }
    let w = k1 * k1 - 4.0 * k0 * k2;
    if w < 0.0 {
        return None;
    }
    let w = w.sqrt();
    let ik2 = 0.5 / k2;
    for v in [(-k1 - w) * ik2, (-k1 + w) * ik2] {
        if let Some(q) = along(v).filter(|q| inside(*q)) {
            return Some(q);
        }
    }
    None
}

/// Undo the mesh at `p`: the point of the picture the mesh has put there.
fn mesh_back(mesh: &[P; SIDE * SIDE], p: P) -> Option<P> {
    for cj in 0..CELLS {
        for ci in 0..CELLS {
            let at = |i: usize, j: usize| mesh[j * SIDE + i];
            let (a, b, c, d) = (at(ci, cj), at(ci + 1, cj), at(ci + 1, cj + 1), at(ci, cj + 1));
            // Quick no: outside the patch's bounding box.
            let (lo_x, hi_x) = (
                a.0.min(b.0).min(c.0).min(d.0),
                a.0.max(b.0).max(c.0).max(d.0),
            );
            let (lo_y, hi_y) = (
                a.1.min(b.1).min(c.1).min(d.1),
                a.1.max(b.1).max(c.1).max(d.1),
            );
            if p.0 < lo_x - 1e-4 || p.0 > hi_x + 1e-4 || p.1 < lo_y - 1e-4 || p.1 > hi_y + 1e-4 {
                continue;
            }
            if let Some((u, v)) = inverse_bilinear(p, a, b, c, d) {
                return Some((
                    (ci as f32 + u.clamp(0.0, 1.0)) / CELLS as f32,
                    (cj as f32 + v.clamp(0.0, 1.0)) / CELLS as f32,
                ));
            }
        }
    }
    None
}

/// The picture bent as asked, on a canvas of its own centred where the
/// picture's centre is (so the rest of the placement works as if the bent
/// picture were the picture). `None` when it cannot be bent (a corner pin
/// folded flat, or far too large).
pub fn bend_frame(frame: &Frame, pin: &[f32; 8], mesh: &[f32; SIDE * SIDE * 2]) -> Option<Frame> {
    let bend = Bend::new(pin, mesh)?;
    let (sw, sh) = (frame.width as f32, frame.height as f32);
    // How far from the centre the bent picture reaches: follow its edge.
    let (mut reach_x, mut reach_y) = (0.5f32, 0.5f32);
    const STEPS: usize = 48;
    for k in 0..=STEPS {
        let t = k as f32 / STEPS as f32;
        for (a, b) in [(t, 0.0), (t, 1.0), (0.0, t), (1.0, t)] {
            let (x, y) = bend.forward(a, b)?;
            reach_x = reach_x.max((x - 0.5).abs());
            reach_y = reach_y.max((y - 0.5).abs());
        }
    }
    let (w, h) = (
        ((reach_x * 2.0 * sw).ceil() as i64 + 4).clamp(1, 8192),
        ((reach_y * 2.0 * sh).ceil() as i64 + 4).clamp(1, 8192),
    );
    if w * h > 36_000_000 || w > 4 * frame.width as i64 + 8 || h > 4 * frame.height as i64 + 8 {
        return None;
    }
    let (w, h) = (w as usize, h as usize);
    let mut out = vec![0u8; w * h * 4];
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(8)
        .min(h);
    let per_chunk = h.div_ceil(threads.max(1));
    let bend = &bend;
    let rows = |first: usize, chunk: &mut [u8]| {
        for (r, row) in chunk.chunks_exact_mut(w * 4).enumerate() {
            let y = (first + r) as f32 + 0.5 - h as f32 / 2.0;
            for (x_i, px) in row.chunks_exact_mut(4).enumerate() {
                let x = x_i as f32 + 0.5 - w as f32 / 2.0;
                let Some((a, b)) = bend.backward(x / sw + 0.5, y / sh + 0.5) else {
                    continue;
                };
                let (sx, sy) = (a * sw, b * sh);
                // Soft edge, as the plain drawing has.
                let inside = sx.min(sw - sx).min(sy.min(sh - sy));
                let coverage = (inside + 0.5).clamp(0.0, 1.0);
                if coverage <= 0.0 {
                    continue;
                }
                let s = frame.sample(sx, sy);
                px[0] = (s[0] + 0.5) as u8;
                px[1] = (s[1] + 0.5) as u8;
                px[2] = (s[2] + 0.5) as u8;
                px[3] = (s[3] * coverage + 0.5).clamp(0.0, 255.0) as u8;
            }
        }
    };
    if threads <= 1 {
        rows(0, &mut out);
    } else {
        std::thread::scope(|scope| {
            for (i, chunk) in out.chunks_mut(per_chunk * w * 4).enumerate() {
                let rows = &rows;
                scope.spawn(move || rows(i * per_chunk, chunk));
            }
        });
    }
    Some(Frame {
        width: w as u32,
        height: h as u32,
        rgba: out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: P, b: P) -> bool {
        (a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3
    }

    #[test]
    fn unpin_undoes_the_pin_and_forward_of_a_corner_lands_on_the_pin() {
        let mut pin = [0.0; 8];
        pin[2] = -0.2; // top right corner moves left
        pin[5] = -0.1;
        let b = Bend::new(&pin, &[0.0; 32]).unwrap();
        let f = b.forward(1.0, 0.0).unwrap();
        assert!((f.0 - 0.8).abs() < 1e-4 && f.1.abs() < 1e-4, "{f:?}");
        let back = b.unpin(f.0, f.1).unwrap();
        assert!((back.0 - 1.0).abs() < 1e-3 && back.1.abs() < 1e-3, "{back:?}");
    }

    #[test]
    fn no_bend_changes_nothing() {
        let bend = Bend::new(&[0.0; 8], &[0.0; 32]).unwrap();
        assert!(close(bend.forward(0.3, 0.7).unwrap(), (0.3, 0.7)));
        assert!(close(bend.backward(0.3, 0.7).unwrap(), (0.3, 0.7)));
    }

    #[test]
    fn a_corner_pin_puts_the_corners_where_asked() {
        // Top right corner pulled in and down.
        let pin = [0.0, 0.0, -0.2, 0.1, 0.0, 0.0, 0.0, 0.0];
        let bend = Bend::new(&pin, &[0.0; 32]).unwrap();
        assert!(close(bend.forward(1.0, 0.0).unwrap(), (0.8, 0.1)));
        assert!(close(bend.forward(0.0, 0.0).unwrap(), (0.0, 0.0)));
        assert!(close(bend.forward(1.0, 1.0).unwrap(), (1.0, 1.0)));
        // And back again.
        let back = bend.backward(0.8, 0.1).unwrap();
        assert!(close(back, (1.0, 0.0)), "{back:?}");
        let mid = bend.forward(0.37, 0.61).unwrap();
        assert!(close(bend.backward(mid.0, mid.1).unwrap(), (0.37, 0.61)));
    }

    #[test]
    fn a_moved_mesh_point_drags_the_picture_with_it_and_back() {
        let mut mesh = [0.0f32; 32];
        // The point at column 1, row 1 moves right and down.
        let k = (SIDE + 1) * 2;
        mesh[k] = 0.05;
        mesh[k + 1] = 0.04;
        let bend = Bend::new(&[0.0; 8], &mesh).unwrap();
        let at = (1.0 / 3.0, 1.0 / 3.0);
        let moved = bend.forward(at.0, at.1).unwrap();
        assert!(close(moved, (at.0 + 0.05, at.1 + 0.04)), "{moved:?}");
        // The edges of the picture stay put.
        assert!(close(bend.forward(0.0, 0.0).unwrap(), (0.0, 0.0)));
        assert!(close(bend.forward(1.0, 1.0).unwrap(), (1.0, 1.0)));
        for (a, b) in [(0.2, 0.25), (0.5, 0.5), (0.8, 0.6), (0.1, 0.9)] {
            let f = bend.forward(a, b).unwrap();
            let r = bend.backward(f.0, f.1).unwrap();
            assert!(close(r, (a, b)), "({a},{b}) -> {f:?} -> {r:?}");
        }
    }

    #[test]
    fn a_bent_picture_keeps_its_colours_where_it_is_not_moved() {
        // Left half red, right half blue; pull the top right corner in.
        let mut rgba = Vec::new();
        for _y in 0..40 {
            for x in 0..40 {
                rgba.extend_from_slice(if x < 20 { &[255, 0, 0, 255] } else { &[0, 0, 255, 255] });
            }
        }
        let frame = Frame { width: 40, height: 40, rgba };
        let pin = [0.0, 0.0, -0.3, 0.0, 0.0, 0.0, 0.0, 0.0];
        let out = bend_frame(&frame, &pin, &[0.0; 32]).unwrap();
        let at = |x: u32, y: u32| {
            let o = ((y * out.width + x) * 4) as usize;
            [out.rgba[o], out.rgba[o + 1], out.rgba[o + 2], out.rgba[o + 3]]
        };
        let (cx, cy) = (out.width / 2, out.height / 2);
        // Left of the middle is still red, right still blue, both solid.
        assert_eq!(at(cx - 8, cy)[0], 255);
        assert_eq!(at(cx + 6, cy)[2], 255);
        assert_eq!(at(cx + 6, cy)[3], 255);
        // The pulled-in top right corner is now empty.
        let corner = at(out.width / 2 + 17, out.height / 2 - 18);
        assert_eq!(corner[3], 0, "{corner:?}");
    }

    #[test]
    fn a_folded_pin_is_refused_rather_than_drawn_wrong() {
        // Both right corners dragged across to the left edge: a flat sliver.
        let pin = [0.0, 0.0, -1.0, 0.0, -1.0, 0.0, 0.0, 0.0];
        assert!(Bend::new(&pin, &[0.0; 32]).is_none());
    }
}
