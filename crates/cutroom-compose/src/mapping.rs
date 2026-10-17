//! Where the points of a picture land on the canvas (and back), for drawing
//! the box and handles over the preview. It follows the same steps as
//! `Canvas::draw`: bend (corner pin, mesh), flip, slant, perspective, then
//! turn, scale and move.

use crate::warp::Bend;
use crate::Placement;

type P = (f32, f32);

pub struct Mapper {
    placement: Placement,
    sw: f32,
    sh: f32,
    scale: f32,
    cx: f32,
    cy: f32,
    sin: f32,
    cos: f32,
    skew_x: f32,
    skew_y: f32,
    bend: Option<Bend>,
}

impl Mapper {
    /// `canvas` and `media` are (width, height) in pixels; `media` is the picture's own size.
    pub fn new(placement: &Placement, canvas: (f32, f32), media: (f32, f32)) -> Mapper {
        let fit = if placement.natural {
            1.0
        } else {
            (canvas.0 / media.0).min(canvas.1 / media.1)
        };
        let (sin, cos) = placement.rotation.to_radians().sin_cos();
        Mapper {
            placement: *placement,
            sw: media.0,
            sh: media.1,
            scale: fit * placement.scale,
            cx: canvas.0 / 2.0 + placement.x * canvas.0,
            cy: canvas.1 / 2.0 + placement.y * canvas.1,
            sin,
            cos,
            skew_x: placement.skew_x.to_radians().tan(),
            skew_y: placement.skew_y.to_radians().tan(),
            bend: if placement.has_distort() {
                Bend::new(&placement.pin, &placement.mesh)
            } else {
                None
            },
        }
    }

    /// A point of the picture's unit square (before any bend), to the canvas.
    pub fn point(&self, u: f32, v: f32) -> Option<P> {
        let (u, v) = match &self.bend {
            Some(b) => b.forward(u, v)?,
            None => (u, v),
        };
        self.plain(u, v)
    }

    /// A point of the picture's unit square after the bend, to the canvas.
    pub fn plain(&self, u: f32, v: f32) -> Option<P> {
        let (mut ux, mut uy) = ((u - 0.5) * self.sw, (v - 0.5) * self.sh);
        if self.placement.flip_h {
            ux = -ux;
        }
        if self.placement.flip_v {
            uy = -uy;
        }
        let qx = ux - self.skew_x * uy;
        let qy = uy + self.skew_y * qx;
        let (nx, ny) = (qx / self.sw, qy / self.sh);
        let divisor = 1.0 + self.placement.persp_x * nx + self.placement.persp_y * ny;
        if !divisor.is_finite() || divisor < 0.05 {
            return None;
        }
        let (u0x, u0y) = (nx / divisor * self.sw, ny / divisor * self.sh);
        Some((
            self.cx + self.scale * (u0x * self.cos - u0y * self.sin),
            self.cy + self.scale * (u0x * self.sin + u0y * self.cos),
        ))
    }

    /// A canvas point back to the picture's unit square (after the bend).
    pub fn unplain(&self, x: f32, y: f32) -> Option<P> {
        let (dx, dy) = (x - self.cx, y - self.cy);
        let ux = (dx * self.cos + dy * self.sin) / self.scale;
        let uy = (-dx * self.sin + dy * self.cos) / self.scale;
        let (nx, ny) = (ux / self.sw, uy / self.sh);
        let denom = 1.0 - self.placement.persp_x * nx - self.placement.persp_y * ny;
        if !denom.is_finite() || denom < 0.05 {
            return None;
        }
        let (qx, qy) = (nx / denom * self.sw, ny / denom * self.sh);
        let mut uy = qy - self.skew_y * qx;
        let mut ux = qx + self.skew_x * uy;
        if self.placement.flip_h {
            ux = -ux;
        }
        if self.placement.flip_v {
            uy = -uy;
        }
        Some((ux / self.sw + 0.5, uy / self.sh + 0.5))
    }

    /// The outline of the picture as it is drawn: `per_side` points along each
    /// edge, clockwise from the top left (follows the bend).
    pub fn outline(&self, per_side: usize) -> Vec<P> {
        let n = per_side.max(1);
        let mut out = Vec::with_capacity(n * 4);
        let edge = |from: P, to: P, out: &mut Vec<P>| {
            for i in 0..n {
                let t = i as f32 / n as f32;
                if let Some(p) = self.point(from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t) {
                    out.push(p);
                }
            }
        };
        edge((0.0, 0.0), (1.0, 0.0), &mut out);
        edge((1.0, 0.0), (1.0, 1.0), &mut out);
        edge((1.0, 1.0), (0.0, 1.0), &mut out);
        edge((0.0, 1.0), (0.0, 0.0), &mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_picture_has_its_corners_on_the_fitted_rectangle() {
        let m = Mapper::new(&Placement::default(), (200.0, 100.0), (100.0, 100.0));
        let tl = m.point(0.0, 0.0).unwrap();
        let br = m.point(1.0, 1.0).unwrap();
        assert!((tl.0 - 50.0).abs() < 1e-3 && tl.1.abs() < 1e-3, "{tl:?}");
        assert!((br.0 - 150.0).abs() < 1e-3 && (br.1 - 100.0).abs() < 1e-3, "{br:?}");
    }

    #[test]
    fn mapping_there_and_back_finds_the_same_point_with_slant_perspective_and_turn() {
        let p = Placement {
            x: 0.1,
            y: -0.05,
            scale: 0.8,
            rotation: 25.0,
            skew_x: 12.0,
            skew_y: -7.0,
            persp_x: 0.3,
            persp_y: -0.2,
            flip_h: true,
            ..Default::default()
        };
        let m = Mapper::new(&p, (1000.0, 600.0), (640.0, 360.0));
        for (u, v) in [(0.1, 0.2), (0.9, 0.3), (0.5, 0.5), (0.2, 0.95)] {
            let at = m.plain(u, v).unwrap();
            let back = m.unplain(at.0, at.1).unwrap();
            assert!((back.0 - u).abs() < 1e-3 && (back.1 - v).abs() < 1e-3, "{u},{v} -> {back:?}");
        }
    }

    #[test]
    fn the_outline_follows_a_corner_pin() {
        let mut p = Placement::default();
        p.pin[2] = -0.5; // top right corner pulled left by half the width
        let m = Mapper::new(&p, (100.0, 100.0), (100.0, 100.0));
        let tr = m.point(1.0, 0.0).unwrap();
        assert!((tr.0 - 50.0).abs() < 0.5, "{tr:?}");
        assert!(m.outline(8).len() >= 32);
    }
}
