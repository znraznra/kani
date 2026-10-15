//! The compositor: stacks the pictures of the clips under the playhead into
//! one canvas image, each moved, scaled, turned, flipped and faded as the
//! clip says. It works on plain pixel buffers, so the preview and the exporter
//! can share it, and it can be tested exactly. (A GPU backend can later be
//! put behind the same interface.)

use cutroom_timeline::Transform;

/// A picture: tightly packed RGBA, 8 bits per channel, straight (not
/// premultiplied) alpha.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Frame {
    /// A frame of one colour.
    pub fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Frame {
        Frame {
            width,
            height,
            rgba: rgba
                .iter()
                .copied()
                .cycle()
                .take(width as usize * height as usize * 4)
                .collect(),
        }
    }

    fn pixel(&self, x: i64, y: i64) -> [f32; 4] {
        let x = x.clamp(0, i64::from(self.width) - 1) as usize;
        let y = y.clamp(0, i64::from(self.height) - 1) as usize;
        let o = (y * self.width as usize + x) * 4;
        [
            f32::from(self.rgba[o]),
            f32::from(self.rgba[o + 1]),
            f32::from(self.rgba[o + 2]),
            f32::from(self.rgba[o + 3]),
        ]
    }

    /// Bilinear sample at continuous coordinates where pixel `i` covers `[i, i+1)`.
    fn sample(&self, sx: f32, sy: f32) -> [f32; 4] {
        let (fx, fy) = (sx - 0.5, sy - 0.5);
        let (x0, y0) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - x0, fy - y0);
        let (x0, y0) = (x0 as i64, y0 as i64);
        // Well inside the picture: no edge clamping needed.
        if x0 >= 0 && y0 >= 0 && x0 + 1 < i64::from(self.width) && y0 + 1 < i64::from(self.height) {
            let stride = self.width as usize * 4;
            let o = y0 as usize * stride + x0 as usize * 4;
            let p = &self.rgba[o..o + 4];
            let q = &self.rgba[o + 4..o + 8];
            let r = &self.rgba[o + stride..o + stride + 4];
            let t = &self.rgba[o + stride + 4..o + stride + 8];
            let mut out = [0.0; 4];
            for i in 0..4 {
                let (a, b, c, d) = (
                    f32::from(p[i]),
                    f32::from(q[i]),
                    f32::from(r[i]),
                    f32::from(t[i]),
                );
                let top = a + (b - a) * tx;
                let bottom = c + (d - c) * tx;
                out[i] = top + (bottom - top) * ty;
            }
            return out;
        }
        let (a, b) = (self.pixel(x0, y0), self.pixel(x0 + 1, y0));
        let (c, d) = (self.pixel(x0, y0 + 1), self.pixel(x0 + 1, y0 + 1));
        let mut out = [0.0; 4];
        for i in 0..4 {
            let top = a[i] + (b[i] - a[i]) * tx;
            let bottom = c[i] + (d[i] - c[i]) * tx;
            out[i] = top + (bottom - top) * ty;
        }
        out
    }
}

/// Where a layer sits, in resolution-independent units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// Offset of the picture's centre from the canvas centre, as a fraction of
    /// the canvas width.
    pub x: f32,
    /// Same, as a fraction of the canvas height (down is positive).
    pub y: f32,
    /// Relative to the picture fitted inside the canvas (1.0 = fitted).
    pub scale: f32,
    /// Degrees, clockwise.
    pub rotation: f32,
    pub opacity: f32,
    pub flip_h: bool,
    pub flip_v: bool,
    /// Slant along x and y, in degrees.
    pub skew_x: f32,
    pub skew_y: f32,
    /// Perspective along x and y (see `Transform::persp_x`); 0.0 is none.
    pub persp_x: f32,
    pub persp_y: f32,
    /// Draw the picture at its own pixel size (scale 1.0 = one picture pixel per
    /// canvas pixel) instead of fitting it inside the canvas. Used for text.
    pub natural: bool,
}

impl Placement {
    fn is_warped(&self) -> bool {
        self.skew_x != 0.0 || self.skew_y != 0.0 || self.persp_x != 0.0 || self.persp_y != 0.0
    }
}

impl Default for Placement {
    fn default() -> Placement {
        Placement {
            x: 0.0,
            y: 0.0,
            scale: 1.0,
            rotation: 0.0,
            opacity: 1.0,
            flip_h: false,
            flip_v: false,
            skew_x: 0.0,
            skew_y: 0.0,
            persp_x: 0.0,
            persp_y: 0.0,
            natural: false,
        }
    }
}

impl From<&Transform> for Placement {
    fn from(t: &Transform) -> Placement {
        Placement {
            x: t.x as f32 / 1000.0,
            y: t.y as f32 / 1000.0,
            scale: t.scale as f32 / 1000.0,
            rotation: t.rotation as f32 / 10.0,
            opacity: t.opacity as f32 / 1000.0,
            flip_h: t.flip_h,
            flip_v: t.flip_v,
            skew_x: t.skew_x as f32 / 10.0,
            skew_y: t.skew_y as f32 / 10.0,
            persp_x: t.persp_x as f32 / 1000.0,
            persp_y: t.persp_y as f32 / 1000.0,
            natural: false,
        }
    }
}

/// The finished picture. It starts opaque black.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Canvas {
    pub fn new(width: u32, height: u32) -> Canvas {
        let mut rgba = vec![0u8; width as usize * height as usize * 4];
        for pixel in rgba.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        Canvas {
            width,
            height,
            rgba,
        }
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let o = (y as usize * self.width as usize + x as usize) * 4;
        [
            self.rgba[o],
            self.rgba[o + 1],
            self.rgba[o + 2],
            self.rgba[o + 3],
        ]
    }

    /// Lay a picture over the canvas with its top left corner at (`left`, `top`),
    /// pixel for pixel (see the fast path in [`Canvas::draw`]).
    fn draw_aligned(&mut self, frame: &Frame, left: i64, top: i64, opacity: f32) {
        let (cw, ch) = (i64::from(self.width), i64::from(self.height));
        let (fw, fh) = (i64::from(frame.width), i64::from(frame.height));
        let (x0, x1) = (left.max(0), (left + fw).min(cw));
        let (y0, y1) = (top.max(0), (top + fh).min(ch));
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        for y in y0..y1 {
            let src_row = ((y - top) * fw) as usize * 4;
            let dst_row = (y * cw) as usize * 4;
            for x in x0..x1 {
                let s = src_row + ((x - left) as usize) * 4;
                let src = &frame.rgba[s..s + 4];
                if src[3] == 0 || opacity <= 0.0 {
                    continue;
                }
                let o = dst_row + x as usize * 4;
                let alpha = opacity * (f32::from(src[3]) / 255.0);
                if alpha >= 1.0 {
                    self.rgba[o..o + 3].copy_from_slice(&src[..3]);
                    continue;
                }
                for c in 0..3 {
                    let below = f32::from(self.rgba[o + c]);
                    self.rgba[o + c] = (f32::from(src[c]) * alpha + below * (1.0 - alpha))
                        .round()
                        .clamp(0.0, 255.0) as u8;
                }
            }
        }
    }

    /// Draw one layer over what is already there.
    pub fn draw(&mut self, frame: &Frame, placement: &Placement) {
        if frame.width == 0
            || frame.height == 0
            || placement.opacity <= 0.0
            || placement.scale <= 0.0
        {
            return;
        }
        let (cw, ch) = (self.width as f32, self.height as f32);
        let (sw, sh) = (frame.width as f32, frame.height as f32);
        let fit = if placement.natural {
            1.0
        } else {
            (cw / sw).min(ch / sh)
        };
        let scale = fit * placement.scale;
        let (cx, cy) = (cw / 2.0 + placement.x * cw, ch / 2.0 + placement.y * ch);
        let radians = placement.rotation.to_radians();
        let (sin, cos) = radians.sin_cos();
        let skew_x_tan = placement.skew_x.to_radians().tan();
        let skew_y_tan = placement.skew_y.to_radians().tan();

        // The common case, and the cheap one: a picture that lands exactly on
        // whole canvas pixels, unturned and unscaled (captions, a clip that is
        // already the canvas size). No resampling is needed.
        let (left, top) = (cx - sw / 2.0, cy - sh / 2.0);
        if !placement.is_warped()
            && placement.rotation == 0.0
            && !placement.flip_h
            && !placement.flip_v
            && (scale - 1.0).abs() < 1e-6
            && (left - left.round()).abs() < 1e-3
            && (top - top.round()).abs() < 1e-3
        {
            self.draw_aligned(
                frame,
                left.round() as i64,
                top.round() as i64,
                placement.opacity.min(1.0),
            );
            return;
        }

        // Only the rows and columns the turned rectangle can touch.
        let (hw, hh) = (sw * scale / 2.0, sh * scale / 2.0);
        let reach_x = hw * cos.abs() + hh * sin.abs();
        let reach_y = hw * sin.abs() + hh * cos.abs();
        let warped = placement.is_warped();
        let (x0, x1, y0, y1) = if warped {
            // A slanted or tilted picture can reach anywhere: look at it all.
            (0, self.width as usize, 0, self.height as usize)
        } else {
            (
                ((cx - reach_x - 1.0).floor().max(0.0)) as usize,
                ((cx + reach_x + 1.0).ceil().min(cw)).max(0.0) as usize,
                ((cy - reach_y - 1.0).floor().max(0.0)) as usize,
                ((cy + reach_y + 1.0).ceil().min(ch)).max(0.0) as usize,
            )
        };
        if x0 >= x1 || y0 >= y1 {
            return;
        }

        let width = self.width as usize;
        let rows = &mut self.rgba[y0 * width * 4..y1 * width * 4];
        let threads = std::thread::available_parallelism()
            .map_or(1, |n| n.get())
            .min(8)
            .min(y1 - y0);
        let per_chunk = (y1 - y0).div_ceil(threads.max(1));
        let render_rows = |first_row: usize, chunk: &mut [u8]| {
            for (r, row) in chunk.chunks_exact_mut(width * 4).enumerate() {
                let py = (first_row + r) as f32 + 0.5;
                for px in x0..x1 {
                    let (dx, dy) = (px as f32 + 0.5 - cx, py - cy);
                    // Back into the picture: undo the turn, then the scale.
                    let mut ux = (dx * cos + dy * sin) / scale;
                    let mut uy = (-dx * sin + dy * cos) / scale;
                    if warped {
                        // Undo the perspective, then the slant.
                        let (nx, ny) = (ux / sw, uy / sh);
                        let denom = 1.0 - placement.persp_x * nx - placement.persp_y * ny;
                        if denom < 0.05 {
                            continue;
                        }
                        let (qx, qy) = (nx / denom * sw, ny / denom * sh);
                        uy = qy - skew_y_tan * qx;
                        ux = qx + skew_x_tan * uy;
                    }
                    if placement.flip_h {
                        ux = -ux;
                    }
                    if placement.flip_v {
                        uy = -uy;
                    }
                    let (sx, sy) = (ux + sw / 2.0, uy + sh / 2.0);
                    // How far inside the picture's edge this point is, in canvas pixels.
                    let inside = sx.min(sw - sx).min(sy.min(sh - sy)) * scale;
                    let coverage = (inside + 0.5).clamp(0.0, 1.0);
                    if coverage <= 0.0 {
                        continue;
                    }
                    let src = frame.sample(sx, sy);
                    let alpha = coverage * placement.opacity.min(1.0) * (src[3] / 255.0);
                    if alpha <= 0.0 {
                        continue;
                    }
                    let o = px * 4;
                    for c in 0..3 {
                        let below = f32::from(row[o + c]);
                        row[o + c] = (src[c] * alpha + below * (1.0 - alpha))
                            .round()
                            .clamp(0.0, 255.0) as u8;
                    }
                }
            }
        };
        if threads <= 1 {
            render_rows(y0, rows);
        } else {
            std::thread::scope(|scope| {
                for (i, chunk) in rows.chunks_mut(per_chunk * width * 4).enumerate() {
                    let render_rows = &render_rows;
                    scope.spawn(move || render_rows(y0 + i * per_chunk, chunk));
                }
            });
        }
    }
}

/// Stack layers, bottom first, onto a black canvas.
pub fn compose<'a>(
    width: u32,
    height: u32,
    layers: impl IntoIterator<Item = (&'a Frame, &'a Placement)>,
) -> Canvas {
    let mut canvas = Canvas::new(width, height);
    for (frame, placement) in layers {
        canvas.draw(frame, placement);
    }
    canvas
}

/// The size to render the preview at: the canvas shrunk so its longer side is
/// at most `max_edge` (never enlarged), both sides at least 2 and even.
pub fn preview_size(width: u32, height: u32, max_edge: u32) -> (u32, u32) {
    let longest = width.max(height).max(1);
    let scale = (f64::from(max_edge) / f64::from(longest)).min(1.0);
    let even = |v: f64| ((v.round() as u32).max(2) / 2) * 2;
    (
        even(f64::from(width) * scale),
        even(f64::from(height) * scale),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: [u8; 4] = [255, 0, 0, 255];
    const GREEN: [u8; 4] = [0, 255, 0, 255];
    const BLACK: [u8; 4] = [0, 0, 0, 255];

    fn draw(canvas: (u32, u32), frame: &Frame, placement: Placement) -> Canvas {
        let mut c = Canvas::new(canvas.0, canvas.1);
        c.draw(frame, &placement);
        c
    }

    fn count(c: &Canvas, colour: [u8; 4]) -> usize {
        (0..c.height)
            .flat_map(|y| (0..c.width).map(move |x| (x, y)))
            .filter(|&(x, y)| c.pixel(x, y) == colour)
            .count()
    }

    #[test]
    fn a_picture_that_lands_on_whole_pixels_is_copied_exactly() {
        // A 4x4 canvas with a 2x2 red picture, natural size, shifted right by one
        // pixel (x = 0.25 of the canvas width is exactly one pixel).
        let placement = Placement {
            natural: true,
            x: 0.25,
            ..Placement::default()
        };
        let c = draw((4, 4), &Frame::solid(2, 2, RED), placement);
        assert_eq!(count(&c, RED), 4);
        for (x, y) in [(2, 1), (3, 1), (2, 2), (3, 2)] {
            assert_eq!(c.pixel(x, y), RED, "{x},{y}");
        }
    }

    #[test]
    fn a_clear_pixel_leaves_the_canvas_and_a_half_clear_one_blends() {
        let mut frame = Frame::solid(2, 1, [255, 255, 255, 0]);
        frame.rgba[4..8].copy_from_slice(&[255, 255, 255, 128]);
        let placement = Placement {
            natural: true,
            ..Placement::default()
        };
        let c = draw((2, 1), &frame, placement);
        assert_eq!(c.pixel(0, 0), BLACK, "fully clear");
        let half = c.pixel(1, 0);
        assert!((127..=129).contains(&half[0]), "{half:?}");
    }

    #[test]
    fn the_fast_path_agrees_with_the_general_one() {
        // Same picture, once exactly aligned and once with a turn too small to see.
        let mut frame = Frame::solid(8, 8, [10, 200, 30, 255]);
        for i in 0..8usize {
            frame.rgba[(i * 8 + i) * 4..(i * 8 + i) * 4 + 4].copy_from_slice(&[250, 20, 20, 200]);
        }
        let aligned = Placement {
            natural: true,
            opacity: 0.8,
            ..Placement::default()
        };
        let general = Placement {
            rotation: 0.001,
            ..aligned
        };
        let (a, g) = (draw((8, 8), &frame, aligned), draw((8, 8), &frame, general));
        for y in 1..7 {
            for x in 1..7 {
                for k in 0..3 {
                    let diff = i32::from(a.pixel(x, y)[k]) - i32::from(g.pixel(x, y)[k]);
                    assert!(
                        diff.abs() <= 2,
                        "{x},{y}: {:?} vs {:?}",
                        a.pixel(x, y),
                        g.pixel(x, y)
                    );
                }
            }
        }
    }

    #[test]
    fn a_new_canvas_is_opaque_black() {
        let c = Canvas::new(3, 2);
        assert_eq!(count(&c, BLACK), 6);
    }

    #[test]
    fn a_picture_with_the_canvas_shape_fills_it() {
        let c = draw((8, 8), &Frame::solid(4, 4, RED), Placement::default());
        assert_eq!(count(&c, RED), 64, "scaled up to fit");
    }

    #[test]
    fn a_wide_picture_is_letterboxed_and_never_cropped() {
        // 8x8 canvas, 8x4 picture: fitted at scale 1, centred, bars above and below.
        let c = draw((8, 8), &Frame::solid(8, 4, RED), Placement::default());
        assert_eq!(count(&c, RED), 32);
        assert_eq!(c.pixel(4, 3), RED);
        assert_eq!(c.pixel(4, 1), BLACK);
        assert_eq!(c.pixel(4, 6), BLACK);
    }

    #[test]
    fn a_tall_picture_is_pillarboxed() {
        let c = draw((8, 8), &Frame::solid(4, 8, RED), Placement::default());
        assert_eq!(count(&c, RED), 32);
        assert_eq!(c.pixel(1, 4), BLACK);
        assert_eq!(c.pixel(3, 4), RED);
    }

    #[test]
    fn scale_changes_the_size_around_the_centre() {
        let half = draw(
            (8, 8),
            &Frame::solid(8, 8, RED),
            Placement {
                scale: 0.5,
                ..Default::default()
            },
        );
        assert_eq!(count(&half, RED), 16, "a 4x4 square");
        assert_eq!(
            (half.pixel(2, 2), half.pixel(5, 5), half.pixel(1, 1)),
            (RED, RED, BLACK)
        );
        let big = draw(
            (8, 8),
            &Frame::solid(8, 8, RED),
            Placement {
                scale: 3.0,
                ..Default::default()
            },
        );
        assert_eq!(
            count(&big, RED),
            64,
            "covers everything and is cut off by the canvas"
        );
    }

    #[test]
    fn position_is_a_fraction_of_the_canvas() {
        let right = draw(
            (8, 8),
            &Frame::solid(8, 8, RED),
            Placement {
                scale: 0.5,
                x: 0.25,
                ..Default::default()
            },
        );
        // The 4x4 square's centre moved from x=4 to x=6, so it spans 4..8.
        assert_eq!(
            (right.pixel(3, 4), right.pixel(4, 4), right.pixel(7, 4)),
            (BLACK, RED, RED)
        );
        let down = draw(
            (8, 8),
            &Frame::solid(8, 8, RED),
            Placement {
                scale: 0.5,
                y: -0.25,
                ..Default::default()
            },
        );
        assert_eq!(
            (down.pixel(4, 3), down.pixel(4, 0), down.pixel(4, 4)),
            (RED, RED, BLACK),
            "negative y moves up"
        );
    }

    #[test]
    fn a_picture_moved_wholly_off_the_canvas_changes_nothing() {
        let c = draw(
            (8, 8),
            &Frame::solid(8, 8, RED),
            Placement {
                x: 3.0,
                ..Default::default()
            },
        );
        assert_eq!(count(&c, BLACK), 64);
    }

    #[test]
    fn a_quarter_turn_swaps_the_sides_of_a_wide_picture() {
        // A 2:1 picture on a 16x16 canvas is 16x8; turned 90 degrees it is 8x16.
        let c = draw(
            (16, 16),
            &Frame::solid(16, 8, RED),
            Placement {
                rotation: 90.0,
                ..Default::default()
            },
        );
        assert_eq!((c.pixel(8, 1), c.pixel(8, 14)), (RED, RED), "tall now");
        assert_eq!(
            (c.pixel(2, 8), c.pixel(13, 8)),
            (BLACK, BLACK),
            "no longer wide"
        );
    }

    #[test]
    fn turning_is_clockwise() {
        // Left half red, right half green. A quarter turn clockwise puts red on top.
        let mut frame = Frame::solid(8, 8, GREEN);
        for y in 0..8 {
            for x in 0..4 {
                let o = (y * 8 + x) * 4;
                frame.rgba[o..o + 4].copy_from_slice(&RED);
            }
        }
        let c = draw(
            (8, 8),
            &frame,
            Placement {
                rotation: 90.0,
                ..Default::default()
            },
        );
        assert_eq!((c.pixel(4, 1), c.pixel(4, 6)), (RED, GREEN));
    }

    #[test]
    fn flips_mirror_the_picture() {
        let mut frame = Frame::solid(8, 8, GREEN);
        for y in 0..8 {
            for x in 0..4 {
                let o = (y * 8 + x) * 4;
                frame.rgba[o..o + 4].copy_from_slice(&RED);
            }
        }
        let normal = draw((8, 8), &frame, Placement::default());
        assert_eq!((normal.pixel(1, 4), normal.pixel(6, 4)), (RED, GREEN));
        let flipped = draw(
            (8, 8),
            &frame,
            Placement {
                flip_h: true,
                ..Default::default()
            },
        );
        assert_eq!((flipped.pixel(1, 4), flipped.pixel(6, 4)), (GREEN, RED));
        let vertical = draw(
            (8, 8),
            &frame,
            Placement {
                flip_v: true,
                ..Default::default()
            },
        );
        assert_eq!(
            (vertical.pixel(1, 4), vertical.pixel(6, 4)),
            (RED, GREEN),
            "left-right unchanged"
        );
    }

    #[test]
    fn opacity_blends_with_what_is_below() {
        let half = draw(
            (4, 4),
            &Frame::solid(4, 4, RED),
            Placement {
                opacity: 0.5,
                ..Default::default()
            },
        );
        assert_eq!(half.pixel(2, 2), [128, 0, 0, 255]);
        let none = draw(
            (4, 4),
            &Frame::solid(4, 4, RED),
            Placement {
                opacity: 0.0,
                ..Default::default()
            },
        );
        assert_eq!(count(&none, BLACK), 16);
    }

    #[test]
    fn a_pictures_own_transparency_shows_what_is_below() {
        let clear = Frame::solid(4, 4, [255, 0, 0, 0]);
        assert_eq!(
            count(&draw((4, 4), &clear, Placement::default()), BLACK),
            16
        );
        let half = Frame::solid(4, 4, [255, 0, 0, 128]);
        let c = draw((4, 4), &half, Placement::default());
        assert_eq!(c.pixel(1, 1)[0], 128);
    }

    #[test]
    fn layers_stack_bottom_to_top() {
        let (red, green) = (Frame::solid(4, 4, RED), Frame::solid(4, 4, GREEN));
        let small = Placement {
            scale: 0.5,
            ..Default::default()
        };
        let full = Placement::default();
        let c = compose(8, 8, [(&red, &full), (&green, &small)]);
        assert_eq!(
            (c.pixel(0, 0), c.pixel(4, 4)),
            (RED, GREEN),
            "green sits on top of red"
        );
        let reversed = compose(8, 8, [(&green, &small), (&red, &full)]);
        assert_eq!(
            count(&reversed, GREEN),
            0,
            "red now covers green completely"
        );
    }

    #[test]
    fn a_fractional_edge_is_antialiased() {
        // A 3x3 red square centred in an 8x8 canvas spans 2.5 to 5.5: its edge pixels are half covered.
        let c = draw(
            (8, 8),
            &Frame::solid(8, 8, RED),
            Placement {
                scale: 3.0 / 8.0,
                ..Default::default()
            },
        );
        let edge = c.pixel(2, 4)[0];
        assert!(
            edge > 0 && edge < 255,
            "partial coverage at the edge, got {edge}"
        );
        assert_eq!(c.pixel(4, 4), RED);
    }

    #[test]
    fn many_threads_give_the_same_answer_as_one() {
        // A canvas tall enough that the rows are split between threads.
        let mut frame = Frame::solid(64, 64, RED);
        for (i, px) in frame.rgba.chunks_exact_mut(4).enumerate() {
            px[1] = (i % 251) as u8;
        }
        let a = draw(
            (200, 300),
            &frame,
            Placement {
                rotation: 17.0,
                scale: 0.8,
                x: 0.1,
                ..Default::default()
            },
        );
        let b = draw(
            (200, 300),
            &frame,
            Placement {
                rotation: 17.0,
                scale: 0.8,
                x: 0.1,
                ..Default::default()
            },
        );
        assert_eq!(a, b, "deterministic");
        assert!(count(&a, BLACK) < 200 * 300, "something was drawn");
    }

    #[test]
    fn placements_come_from_transforms_exactly() {
        let t = Transform {
            x: 250,
            y: -500,
            scale: 1500,
            rotation: 450,
            opacity: 800,
            flip_h: true,
            flip_v: false,
            skew_x: 100,
            skew_y: 0,
            persp_x: 0,
            persp_y: -500,
        };
        let p = Placement::from(&t);
        assert_eq!(
            (p.x, p.y, p.scale, p.rotation, p.opacity),
            (0.25, -0.5, 1.5, 45.0, 0.8)
        );
        assert!(p.flip_h && !p.flip_v);
        assert_eq!((p.skew_x, p.persp_y), (10.0, -0.5));
        assert_eq!(Placement::from(&Transform::default()), Placement::default());
    }

    #[test]
    fn preview_sizes_shrink_and_stay_even() {
        assert_eq!(preview_size(1080, 1920, 960), (540, 960));
        assert_eq!(preview_size(1920, 1080, 960), (960, 540));
        assert_eq!(preview_size(640, 360, 960), (640, 360), "never enlarged");
        assert_eq!(preview_size(1001, 563, 500).0 % 2, 0);
        assert_eq!(preview_size(0, 0, 960), (2, 2));
    }

    #[test]
    fn degenerate_inputs_do_nothing() {
        let mut c = Canvas::new(4, 4);
        c.draw(
            &Frame {
                width: 0,
                height: 0,
                rgba: vec![],
            },
            &Placement::default(),
        );
        c.draw(
            &Frame::solid(2, 2, RED),
            &Placement {
                scale: 0.0,
                ..Default::default()
            },
        );
        assert_eq!(count(&c, BLACK), 16);
    }
    #[test]
    fn skewing_slides_the_top_one_way_and_the_bottom_the_other() {
        let placement = Placement {
            scale: 0.5,
            skew_x: 30.0,
            ..Default::default()
        };
        let c = draw((40, 40), &Frame::solid(40, 40, RED), placement);
        // A 20x20 square centred; a positive skew leans the top to the right.
        let first = |y: u32| (0..40).find(|&x| c.pixel(x, y) == RED).unwrap();
        assert!(
            first(11) > first(28),
            "top starts further right than the bottom"
        );
        assert_eq!(count(&c, RED) > 300, true, "area is kept");
    }

    #[test]
    fn perspective_shrinks_the_far_side() {
        let placement = Placement {
            scale: 0.5,
            persp_x: 0.6,
            ..Default::default()
        };
        let c = draw((40, 40), &Frame::solid(40, 40, RED), placement);
        let height = |x: u32| (0..40).filter(|&y| c.pixel(x, y) == RED).count();
        assert!(height(11) > height(28), "the left is taller than the right");
    }

    #[test]
    fn no_warp_matches_the_plain_path() {
        let a = draw((16, 16), &Frame::solid(16, 16, RED), Placement::default());
        let b = draw(
            (16, 16),
            &Frame::solid(16, 16, RED),
            Placement {
                skew_x: 0.0,
                ..Default::default()
            },
        );
        assert_eq!(a, b);
    }

    #[test]
    fn a_natural_size_picture_is_not_fitted_to_the_canvas() {
        let placement = Placement {
            natural: true,
            ..Default::default()
        };
        let c = draw((20, 20), &Frame::solid(4, 2, RED), placement);
        assert_eq!(count(&c, RED), 8, "4 x 2 pixels, not stretched to fill");
        assert_eq!(c.pixel(10, 10), RED);
        assert_eq!(c.pixel(5, 10), BLACK);
    }
}
