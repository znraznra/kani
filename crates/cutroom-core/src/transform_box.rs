//! The box drawn over the preview for moving, scaling and turning the
//! selected clip's picture. All geometry is in canvas pixels, so it doesn't
//! depend on how big the preview is on screen.

use cutroom_timeline::Transform;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Handle {
    /// Anywhere inside the picture: move it.
    Body,
    /// One of the four corners (0 = top-left, clockwise): scale it.
    Corner(usize),
    /// The knob above the top edge: turn it.
    Rotate,
}

/// Where the picture sits, as a rotated rectangle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoxGeom {
    pub cx: f32,
    pub cy: f32,
    /// Half the picture's size on screen (before turning).
    pub hw: f32,
    pub hh: f32,
    pub radians: f32,
}

/// How far above the top edge the rotate knob floats, in canvas pixels.
pub const ROTATE_OFFSET: f32 = 40.0;

impl BoxGeom {
    /// `canvas` and `media` are (width, height) in pixels.
    pub fn new(canvas: (f32, f32), media: (f32, f32), t: &Transform) -> Option<BoxGeom> {
        let ((cw, ch), (mw, mh)) = (canvas, media);
        if cw <= 0.0 || ch <= 0.0 || mw <= 0.0 || mh <= 0.0 {
            return None;
        }
        Self::with_fit((cw / mw).min(ch / mh), canvas, media, t)
    }

    /// The box for something drawn at its own size, never fitted to the
    /// canvas (text): `size` is its width and height in canvas pixels at scale 100%.
    pub fn unfitted(canvas: (f32, f32), size: (f32, f32), t: &Transform) -> Option<BoxGeom> {
        if canvas.0 <= 0.0 || canvas.1 <= 0.0 || size.0 <= 0.0 || size.1 <= 0.0 {
            return None;
        }
        Self::with_fit(1.0, canvas, size, t)
    }

    fn with_fit(fit: f32, canvas: (f32, f32), media: (f32, f32), t: &Transform) -> Option<BoxGeom> {
        let ((cw, ch), (mw, mh)) = (canvas, media);
        let scale = fit * t.scale as f32 / 1000.0;
        Some(BoxGeom {
            cx: cw / 2.0 + t.x as f32 / 1000.0 * cw,
            cy: ch / 2.0 + t.y as f32 / 1000.0 * ch,
            hw: mw * scale / 2.0,
            hh: mh * scale / 2.0,
            radians: (t.rotation as f32 / 10.0).to_radians(),
        })
    }

    /// A point given relative to the centre, turned and moved into canvas space.
    fn to_canvas(&self, dx: f32, dy: f32) -> (f32, f32) {
        let (s, c) = self.radians.sin_cos();
        (self.cx + dx * c - dy * s, self.cy + dx * s + dy * c)
    }

    /// The four corners, top-left first, going clockwise.
    pub fn corners(&self) -> [(f32, f32); 4] {
        [
            self.to_canvas(-self.hw, -self.hh),
            self.to_canvas(self.hw, -self.hh),
            self.to_canvas(self.hw, self.hh),
            self.to_canvas(-self.hw, self.hh),
        ]
    }

    /// The rotate knob. `offset` is its distance above the top edge.
    pub fn rotate_knob(&self, offset: f32) -> (f32, f32) {
        self.to_canvas(0.0, -self.hh - offset)
    }

    /// The point in the picture's own (unturned) space, relative to its centre.
    fn local(&self, x: f32, y: f32) -> (f32, f32) {
        let (s, c) = self.radians.sin_cos();
        let (dx, dy) = (x - self.cx, y - self.cy);
        (dx * c + dy * s, -dx * s + dy * c)
    }

    /// What is under `pos`? `reach` is how close counts as touching a handle;
    /// `offset` is the rotate knob's distance above the top edge.
    pub fn hit(&self, pos: (f32, f32), reach: f32, offset: f32) -> Option<Handle> {
        let near = |p: (f32, f32)| (p.0 - pos.0).hypot(p.1 - pos.1) <= reach;
        if near(self.rotate_knob(offset)) {
            return Some(Handle::Rotate);
        }
        for (i, corner) in self.corners().into_iter().enumerate() {
            if near(corner) {
                return Some(Handle::Corner(i));
            }
        }
        let (lx, ly) = self.local(pos.0, pos.1);
        (lx.abs() <= self.hw && ly.abs() <= self.hh).then_some(Handle::Body)
    }
}

/// A drag in progress: what was grabbed, where, and how the picture was then.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grab {
    pub handle: Handle,
    pub start: (f32, f32),
    pub from: Transform,
    pub geom: BoxGeom,
}

impl Grab {
    pub fn begin(handle: Handle, start: (f32, f32), from: Transform, geom: BoxGeom) -> Grab {
        Grab {
            handle,
            start,
            from,
            geom,
        }
    }

    /// The transform for the pointer being at `pos`. `canvas` is (width, height).
    /// `snap` turns in 15 degree steps (Shift).
    pub fn apply(&self, pos: (f32, f32), canvas: (f32, f32), snap: bool) -> Transform {
        let mut t = self.from;
        match self.handle {
            Handle::Body => {
                t.x = self.from.x + ((pos.0 - self.start.0) / canvas.0 * 1000.0).round() as i32;
                t.y = self.from.y + ((pos.1 - self.start.1) / canvas.1 * 1000.0).round() as i32;
            }
            Handle::Corner(_) => {
                let c = (self.geom.cx, self.geom.cy);
                let before = (self.start.0 - c.0).hypot(self.start.1 - c.1);
                let after = (pos.0 - c.0).hypot(pos.1 - c.1);
                if before > 1.0 {
                    t.scale = ((self.from.scale as f32 * after / before).round() as i32)
                        .clamp(Transform::SCALE_MIN, Transform::SCALE_MAX);
                }
            }
            Handle::Rotate => {
                let c = (self.geom.cx, self.geom.cy);
                let angle = |p: (f32, f32)| (p.1 - c.1).atan2(p.0 - c.0);
                let turn = (angle(pos) - angle(self.start)).to_degrees();
                let mut degrees = self.from.rotation as f32 / 10.0 + turn;
                if snap {
                    degrees = (degrees / 15.0).round() * 15.0;
                }
                t.rotation = (degrees * 10.0).round() as i32;
            }
        }
        t.clamped()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CANVAS: (f32, f32) = (1000.0, 2000.0);

    fn geom(t: &Transform) -> BoxGeom {
        // A 2000 x 1000 landscape picture fitted into the portrait canvas: 1000 x 500.
        BoxGeom::new(CANVAS, (2000.0, 1000.0), t).unwrap()
    }

    #[test]
    fn the_box_matches_how_the_compositor_fits_and_places_a_picture() {
        let g = geom(&Transform::default());
        assert_eq!((g.cx, g.cy, g.hw, g.hh), (500.0, 1000.0, 500.0, 250.0));
        let moved = Transform {
            x: 100,
            y: -50,
            scale: 500,
            ..Transform::default()
        };
        let g = geom(&moved);
        assert_eq!((g.cx, g.cy, g.hw, g.hh), (600.0, 900.0, 250.0, 125.0));
    }

    #[test]
    fn text_is_boxed_at_its_own_size_not_fitted() {
        let g = BoxGeom::unfitted(CANVAS, (300.0, 100.0), &Transform::default()).unwrap();
        assert_eq!((g.hw, g.hh), (150.0, 50.0));
        let big = Transform {
            scale: 2000,
            ..Transform::default()
        };
        let g = BoxGeom::unfitted(CANVAS, (300.0, 100.0), &big).unwrap();
        assert_eq!((g.hw, g.hh), (300.0, 100.0));
    }

    #[test]
    fn hits_find_corners_the_knob_and_the_body() {
        let g = geom(&Transform::default());
        assert_eq!(
            g.hit((0.0, 750.0), 10.0, ROTATE_OFFSET),
            Some(Handle::Corner(0))
        );
        assert_eq!(
            g.hit((1000.0, 1250.0), 10.0, ROTATE_OFFSET),
            Some(Handle::Corner(2))
        );
        assert_eq!(
            g.hit((500.0, 710.0), 10.0, ROTATE_OFFSET),
            Some(Handle::Rotate)
        );
        assert_eq!(
            g.hit((400.0, 900.0), 10.0, ROTATE_OFFSET),
            Some(Handle::Body)
        );
        assert_eq!(g.hit((400.0, 200.0), 10.0, ROTATE_OFFSET), None);
    }

    #[test]
    fn a_turned_picture_is_hit_where_it_actually_is() {
        let t = Transform {
            rotation: 900, // 90 degrees clockwise: it is now 500 wide and 1000 tall
            ..Transform::default()
        };
        let g = geom(&t);
        assert_eq!(
            g.hit((500.0, 600.0), 5.0, ROTATE_OFFSET),
            Some(Handle::Body)
        );
        assert_eq!(g.hit((900.0, 1000.0), 5.0, ROTATE_OFFSET), None);
    }

    #[test]
    fn dragging_the_body_moves_it_in_thousandths_of_the_canvas() {
        let t = Transform::default();
        let grab = Grab::begin(Handle::Body, (500.0, 1000.0), t, geom(&t));
        let out = grab.apply((600.0, 800.0), CANVAS, false);
        assert_eq!((out.x, out.y), (100, -100));
    }

    #[test]
    fn dragging_a_corner_away_scales_and_back_shrinks() {
        let t = Transform::default();
        let grab = Grab::begin(Handle::Corner(2), (1000.0, 1250.0), t, geom(&t));
        let double = grab.apply((1500.0, 1500.0), CANVAS, false);
        assert_eq!(double.scale, 2000);
        let half = grab.apply((750.0, 1125.0), CANVAS, false);
        assert_eq!(half.scale, 500);
        let tiny = grab.apply((500.0, 1000.0), CANVAS, false);
        assert_eq!(tiny.scale, Transform::SCALE_MIN);
    }

    #[test]
    fn dragging_the_knob_around_the_centre_turns_it() {
        let t = Transform::default();
        let g = geom(&t);
        let grab = Grab::begin(Handle::Rotate, g.rotate_knob(ROTATE_OFFSET), t, g);
        // From straight above the centre to straight right of it: a quarter turn clockwise.
        let out = grab.apply((900.0, 1000.0), CANVAS, false);
        assert_eq!(out.rotation, 900);
        let snapped = grab.apply((900.0, 960.0), CANVAS, true);
        assert_eq!(snapped.rotation % 150, 0);
    }
}
