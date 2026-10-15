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
        self.sample_cell(x0 as i64, y0 as i64, fx - x0, fy - y0)
    }

    /// The bilinear blend of the four pixels whose top left one is (`x0`, `y0`),
    /// `tx` and `ty` of the way across (what [`Frame::sample`] works out first).
    #[inline(always)]
    fn sample_cell(&self, x0: i64, y0: i64, tx: f32, ty: f32) -> [f32; 4] {
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

/// Put one layer pixel onto `px` (4 bytes): blended over what is there, or, on a
/// recording canvas, stored as colour and alpha.
#[inline(always)]
fn put(px: &mut [u8], colour: [f32; 3], alpha: f32, store: bool) {
    if store {
        for c in 0..3 {
            px[c] = (colour[c].clamp(0.0, 255.0) + 0.5) as u8;
        }
        px[3] = (alpha.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    } else {
        for c in 0..3 {
            let below = f32::from(px[c]);
            px[c] = ((colour[c] * alpha + below * (1.0 - alpha)).clamp(0.0, 255.0) + 0.5) as u8;
        }
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

/// Does an opaque picture of `frame_w` x `frame_h` pixels, placed like this on a
/// `width` x `height` canvas, hide everything under it? True only when it is
/// unturned, unslanted, fully solid and reaches past every edge.
pub fn covers_canvas(width: u32, height: u32, frame_w: u32, frame_h: u32, p: &Placement) -> bool {
    if p.natural
        || p.opacity < 1.0
        || p.scale <= 0.0
        || p.rotation != 0.0
        || p.is_warped()
        || frame_w == 0
        || frame_h == 0
    {
        return false;
    }
    let (cw, ch) = (width as f32, height as f32);
    let (sw, sh) = (frame_w as f32, frame_h as f32);
    let scale = (cw / sw).min(ch / sh) * p.scale;
    let (half_w, half_h) = (sw * scale / 2.0, sh * scale / 2.0);
    let (cx, cy) = (cw / 2.0 + p.x * cw, ch / 2.0 + p.y * ch);
    cx - half_w <= 0.0 && cx + half_w >= cw && cy - half_h <= 0.0 && cy + half_h >= ch
}

/// Is this layer a picture that lands pixel for pixel on the canvas (unturned,
/// unslanted, unscaled, on whole pixels)? Those are copied row by row and are
/// not worth sending anywhere else.
pub fn is_plain_copy(width: u32, height: u32, frame_w: u32, frame_h: u32, p: &Placement) -> bool {
    if frame_w == 0 || frame_h == 0 {
        return false;
    }
    if p.is_warped() || p.rotation != 0.0 || p.flip_h || p.flip_v {
        return false;
    }
    let (cw, ch) = (width as f32, height as f32);
    let (sw, sh) = (frame_w as f32, frame_h as f32);
    let fit = if p.natural {
        1.0
    } else {
        (cw / sw).min(ch / sh)
    };
    if (fit * p.scale - 1.0).abs() >= 1e-6 {
        return false;
    }
    let (left, top) = (
        cw / 2.0 + p.x * cw - sw / 2.0,
        ch / 2.0 + p.y * ch - sh / 2.0,
    );
    p.natural || ((left - left.round()).abs() < 1e-3 && (top - top.round()).abs() < 1e-3)
}

/// The finished picture. It starts opaque black.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// A canvas that keeps what a single layer looks like (colour and alpha)
    /// instead of blending it onto black; see [`render_layer`].
    store: bool,
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
            store: false,
        }
    }

    /// A fully clear canvas that records layers drawn on it as colour + alpha.
    fn clear(width: u32, height: u32) -> Canvas {
        Canvas {
            width,
            height,
            rgba: vec![0u8; width as usize * height as usize * 4],
            store: true,
        }
    }

    /// Lay a pre-rendered layer over what is there (a whole-pixel blend).
    pub fn blit(&mut self, layer: &Layer) {
        self.draw_aligned(
            &layer.frame,
            i64::from(layer.left),
            i64::from(layer.top),
            1.0,
        );
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
        let store = self.store;
        for y in y0..y1 {
            let src_row = ((y - top) * fw) as usize * 4;
            let dst_row = (y * cw) as usize * 4;
            // Video is opaque: when a whole row is, copy it in one go. (The
            // canvas is opaque too, so copying the alpha bytes changes nothing.)
            if opacity >= 1.0 {
                let from = src_row + ((x0 - left) as usize) * 4;
                let len = (x1 - x0) as usize * 4;
                let src = &frame.rgba[from..from + len];
                if src.chunks_exact(4).all(|p| p[3] == 255) {
                    let to = dst_row + x0 as usize * 4;
                    self.rgba[to..to + len].copy_from_slice(src);
                    continue;
                }
            }
            // Text and captions are mostly clear: find the part of the row that
            // has anything in it and only look there.
            let from = src_row + ((x0 - left) as usize) * 4;
            let row = &frame.rgba[from..from + (x1 - x0) as usize * 4];
            let Some(first) = row.chunks_exact(4).position(|p| p[3] != 0) else {
                continue;
            };
            let last = row
                .chunks_exact(4)
                .rposition(|p| p[3] != 0)
                .unwrap_or(first);
            let mut x = x0 + first as i64;
            let end = x0 + last as i64;
            while x <= end {
                let s = src_row + ((x - left) as usize) * 4;
                let src = &frame.rgba[s..s + 4];
                if src[3] == 0 || opacity <= 0.0 {
                    x += 1;
                    continue;
                }
                let o = dst_row + x as usize * 4;
                // A run of solid pixels (the inside of a turned picture) is one copy.
                if opacity >= 1.0 && src[3] == 255 && !store {
                    let mut run = 1i64;
                    while x + run <= end
                        && frame.rgba[src_row + ((x + run - left) as usize) * 4 + 3] == 255
                    {
                        run += 1;
                    }
                    let len = run as usize * 4;
                    self.rgba[o..o + len].copy_from_slice(&frame.rgba[s..s + len]);
                    x += run;
                    continue;
                }
                let alpha = opacity * (f32::from(src[3]) / 255.0);
                if alpha >= 1.0 && !store {
                    self.rgba[o..o + 3].copy_from_slice(&src[..3]);
                    x += 1;
                    continue;
                }
                put(
                    &mut self.rgba[o..o + 4],
                    [f32::from(src[0]), f32::from(src[1]), f32::from(src[2])],
                    alpha,
                    store,
                );
                x += 1;
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
        // whole canvas pixels, unturned and unscaled (a clip that is already the
        // canvas size). No resampling is needed. Text and captions (`natural`)
        // are drawn this way too, moved to the nearest whole pixel (at most half
        // a pixel off, and crisper): their default places are fractional pixels,
        // which would otherwise send every caption through the slow path.
        let (left, top) = (cx - sw / 2.0, cy - sh / 2.0);
        if !placement.is_warped()
            && placement.rotation == 0.0
            && !placement.flip_h
            && !placement.flip_v
            && (scale - 1.0).abs() < 1e-6
            && (placement.natural
                || ((left - left.round()).abs() < 1e-3 && (top - top.round()).abs() < 1e-3))
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
            // A slanted or tilted picture: where its four corners land bounds it
            // (it falls back to the whole canvas when they cannot be placed).
            let whole = (0, self.width as usize, 0, self.height as usize);
            #[cfg(test)]
            let search_all = SEARCH_ALL.with(|f| f.get());
            #[cfg(not(test))]
            let search_all = false;
            if search_all {
                whole
            } else {
                warped_bounds(placement, sw, sh, scale, (sin, cos), (cx, cy), (cw, ch))
                    .unwrap_or(whole)
            }
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
        let store = self.store;
        let rows = &mut self.rgba[y0 * width * 4..y1 * width * 4];
        let threads = std::thread::available_parallelism()
            .map_or(1, |n| n.get())
            .min(8)
            .min(y1 - y0);
        let per_chunk = (y1 - y0).div_ceil(threads.max(1));
        // Scaled and moved but not turned or slanted (a clip that is not the
        // canvas size, a zoomed clip): everything that depends on the column is
        // worked out once here, and everything that depends on the row once per row.
        let columns: Option<Vec<(i64, f32, f32, u32)>> = (!warped && placement.rotation == 0.0)
            .then(|| {
                (x0..x1)
                    .map(|px| {
                        let mut ux = (px as f32 + 0.5 - cx) / scale;
                        if placement.flip_h {
                            ux = -ux;
                        }
                        let sx = ux + sw / 2.0;
                        let cover = ((sx.min(sw - sx)) * scale + 0.5).clamp(0.0, 1.0);
                        let fx = sx - 0.5;
                        let xi = fx.floor();
                        (xi as i64, fx - xi, cover, ((fx - xi) * 256.0) as u32)
                    })
                    .collect()
            });
        // Turned and/or slanted but with no perspective: where a canvas pixel
        // lands in the picture is a straight-line (affine) function of its
        // position, so work it out at three points and step along each row
        // instead of redoing the geometry (and the divide) for every pixel.
        #[cfg(test)]
        let allow_affine = !NO_AFFINE.with(|f| f.get());
        #[cfg(not(test))]
        let allow_affine = true;
        let affine = (allow_affine
            && columns.is_none()
            && placement.persp_x == 0.0
            && placement.persp_y == 0.0)
            .then(|| {
                let back = |qx: f32, qy: f32| -> (f32, f32) {
                    let (dx, dy) = (qx - cx, qy - cy);
                    let mut ux = (dx * cos + dy * sin) / scale;
                    let mut uy = (-dx * sin + dy * cos) / scale;
                    if warped {
                        uy -= skew_y_tan * ux;
                        ux += skew_x_tan * uy;
                    }
                    if placement.flip_h {
                        ux = -ux;
                    }
                    if placement.flip_v {
                        uy = -uy;
                    }
                    (ux + sw / 2.0, uy + sh / 2.0)
                };
                let origin = back(0.0, 0.0);
                let step_x = back(1.0, 0.0);
                let step_y = back(0.0, 1.0);
                (
                    origin,
                    (step_x.0 - origin.0, step_x.1 - origin.1),
                    (step_y.0 - origin.0, step_y.1 - origin.1),
                )
            });
        let opacity = placement.opacity.min(1.0);
        // A picture with no see-through pixel, drawn at full strength, is just
        // resampled where it is well inside its edges: no blending at all.
        let opaque_picture = (columns.is_some() || affine.is_some())
            && opacity >= 1.0
            && frame.rgba.chunks_exact(4).all(|p| p[3] == 255);
        let (fw, fh) = (frame.width as i64, frame.height as i64);
        let render_rows = |first_row: usize, chunk: &mut [u8]| {
            for (r, row) in chunk.chunks_exact_mut(width * 4).enumerate() {
                let py = (first_row + r) as f32 + 0.5;
                if let Some(columns) = &columns {
                    let mut uy = (py - cy) / scale;
                    if placement.flip_v {
                        uy = -uy;
                    }
                    let sy = uy + sh / 2.0;
                    let cover_y = ((sy.min(sh - sy)) * scale + 0.5).clamp(0.0, 1.0);
                    if cover_y <= 0.0 {
                        continue;
                    }
                    let fy = sy - 0.5;
                    let yi = fy.floor();
                    let ty = fy - yi;
                    let ty8 = (ty * 256.0) as u32;
                    let row_inside =
                        opaque_picture && cover_y >= 1.0 && yi >= 0.0 && (yi as i64) + 1 < fh;
                    let stride = fw as usize * 4;
                    for (i, &(xi, tx, cover_x, tx8)) in columns.iter().enumerate() {
                        if row_inside && cover_x >= 1.0 && xi >= 0 && xi + 1 < fw {
                            // Whole numbers all the way: four pixels, four weights
                            // that add up to 65536, nothing to blend.
                            let base = yi as usize * stride + xi as usize * 4;
                            let (w00, w01) = ((256 - tx8) * (256 - ty8), tx8 * (256 - ty8));
                            let (w10, w11) = ((256 - tx8) * ty8, tx8 * ty8);
                            let p = &frame.rgba[base..base + 8];
                            let q = &frame.rgba[base + stride..base + stride + 8];
                            let o = (x0 + i) * 4;
                            for c in 0..3 {
                                row[o + c] = ((u32::from(p[c]) * w00
                                    + u32::from(p[c + 4]) * w01
                                    + u32::from(q[c]) * w10
                                    + u32::from(q[c + 4]) * w11
                                    + 32768)
                                    >> 16) as u8;
                            }
                            row[o + 3] = 255;
                            continue;
                        }
                        let coverage = cover_x.min(cover_y);
                        if coverage <= 0.0 {
                            continue;
                        }
                        let src = frame.sample_cell(xi, yi as i64, tx, ty);
                        let alpha = coverage * opacity * (src[3] / 255.0);
                        if alpha <= 0.0 {
                            continue;
                        }
                        let o = (x0 + i) * 4;
                        put(&mut row[o..o + 4], [src[0], src[1], src[2]], alpha, store);
                    }
                    continue;
                }
                if let Some((origin, along_x, along_y)) = affine {
                    let (row_x, row_y) = (origin.0 + along_y.0 * py, origin.1 + along_y.1 * py);
                    let stride = fw as usize * 4;
                    for px in x0..x1 {
                        let centre = px as f32 + 0.5;
                        let (sx, sy) = (row_x + along_x.0 * centre, row_y + along_x.1 * centre);
                        let inside = sx.min(sw - sx).min(sy.min(sh - sy)) * scale;
                        let coverage = (inside + 0.5).clamp(0.0, 1.0);
                        if coverage <= 0.0 {
                            continue;
                        }
                        let o = px * 4;
                        if opaque_picture && coverage >= 1.0 {
                            let (fx, fy) = (sx - 0.5, sy - 0.5);
                            let (xf, yf) = (fx.floor(), fy.floor());
                            let (xi, yi) = (xf as i64, yf as i64);
                            if xi >= 0 && yi >= 0 && xi + 1 < fw && yi + 1 < fh {
                                let (tx8, ty8) =
                                    (((fx - xf) * 256.0) as u32, ((fy - yf) * 256.0) as u32);
                                let base = yi as usize * stride + xi as usize * 4;
                                let (w00, w01) = ((256 - tx8) * (256 - ty8), tx8 * (256 - ty8));
                                let (w10, w11) = ((256 - tx8) * ty8, tx8 * ty8);
                                let p = &frame.rgba[base..base + 8];
                                let q = &frame.rgba[base + stride..base + stride + 8];
                                for c in 0..3 {
                                    row[o + c] = ((u32::from(p[c]) * w00
                                        + u32::from(p[c + 4]) * w01
                                        + u32::from(q[c]) * w10
                                        + u32::from(q[c + 4]) * w11
                                        + 32768)
                                        >> 16)
                                        as u8;
                                }
                                row[o + 3] = 255;
                                continue;
                            }
                        }
                        let src = frame.sample(sx, sy);
                        let alpha = coverage * opacity * (src[3] / 255.0);
                        if alpha <= 0.0 {
                            continue;
                        }
                        put(&mut row[o..o + 4], [src[0], src[1], src[2]], alpha, store);
                    }
                    continue;
                }
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
                    put(&mut row[o..o + 4], [src[0], src[1], src[2]], alpha, store);
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

#[cfg(test)]
thread_local! {
    /// Tests turn this on to force the per-pixel geometry (no stepping along rows).
    static NO_AFFINE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
thread_local! {
    /// Tests turn this on to compare the bounded search with the whole canvas.
    static SEARCH_ALL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The canvas rows and columns a slanted or tilted picture can touch, from where
/// its four corners land (the picture is a convex four-sided shape, and the
/// perspective is a projective map, so the corners bound it). `None` when a
/// corner is too close to the horizon of the perspective to be placed.
fn warped_bounds(
    placement: &Placement,
    sw: f32,
    sh: f32,
    scale: f32,
    (sin, cos): (f32, f32),
    (cx, cy): (f32, f32),
    (cw, ch): (f32, f32),
) -> Option<(usize, usize, usize, usize)> {
    let (skew_x, skew_y) = (
        placement.skew_x.to_radians().tan(),
        placement.skew_y.to_radians().tan(),
    );
    let (mut lo_x, mut hi_x, mut lo_y, mut hi_y) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    for (ux, uy) in [
        (-sw / 2.0, -sh / 2.0),
        (sw / 2.0, -sh / 2.0),
        (-sw / 2.0, sh / 2.0),
        (sw / 2.0, sh / 2.0),
    ] {
        // Forward through the same steps `draw` undoes: slant, then perspective,
        // then turn and scale.
        let qx = ux - skew_x * uy;
        let qy = uy + skew_y * qx;
        let (qnx, qny) = (qx / sw, qy / sh);
        let divisor = 1.0 + placement.persp_x * qnx + placement.persp_y * qny;
        if !divisor.is_finite() || divisor < 0.05 {
            return None;
        }
        let (u0x, u0y) = (qnx / divisor * sw, qny / divisor * sh);
        let (dx, dy) = (
            scale * (u0x * cos - u0y * sin),
            scale * (u0x * sin + u0y * cos),
        );
        let (x, y) = (cx + dx, cy + dy);
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        lo_x = lo_x.min(x);
        hi_x = hi_x.max(x);
        lo_y = lo_y.min(y);
        hi_y = hi_y.max(y);
    }
    // A little room for the soft edge and rounding.
    let margin = 3.0;
    Some((
        (lo_x - margin).floor().max(0.0) as usize,
        (hi_x + margin).ceil().min(cw).max(0.0) as usize,
        (lo_y - margin).floor().max(0.0) as usize,
        (hi_y + margin).ceil().min(ch).max(0.0) as usize,
    ))
}

/// A layer drawn once, ready to be laid over a canvas again and again: for
/// pictures that do not change from one frame to the next (stills, text).
#[derive(Debug, Clone)]
pub struct Layer {
    /// Colour and alpha of the part of the canvas the layer covers.
    pub frame: Frame,
    pub left: i32,
    pub top: i32,
}

/// Draw one layer on its own and keep the part of the canvas it touches. Laying
/// the result over a canvas with [`Canvas::blit`] gives the same picture as
/// [`Canvas::draw`] (to within one level of rounding). `None` when it covers nothing.
pub fn render_layer(
    width: u32,
    height: u32,
    frame: &Frame,
    placement: &Placement,
) -> Option<Layer> {
    let mut canvas = Canvas::clear(width, height);
    canvas.draw(frame, placement);
    let (w, h) = (width as usize, height as usize);
    let rows: Vec<usize> = (0..h)
        .filter(|&y| {
            canvas.rgba[y * w * 4..(y + 1) * w * 4]
                .chunks_exact(4)
                .any(|p| p[3] != 0)
        })
        .collect();
    let (&top, &bottom) = (rows.first()?, rows.last()?);
    let mut left = w;
    let mut right = 0;
    for &y in &rows {
        let row = &canvas.rgba[y * w * 4..(y + 1) * w * 4];
        if let Some(first) = row.chunks_exact(4).position(|p| p[3] != 0) {
            left = left.min(first);
        }
        if let Some(last) = row.chunks_exact(4).rposition(|p| p[3] != 0) {
            right = right.max(last);
        }
    }
    let (fw, fh) = (right - left + 1, bottom - top + 1);
    let mut rgba = Vec::with_capacity(fw * fh * 4);
    for y in top..=bottom {
        rgba.extend_from_slice(&canvas.rgba[(y * w + left) * 4..(y * w + right + 1) * 4]);
    }
    Some(Layer {
        frame: Frame {
            width: fw as u32,
            height: fh as u32,
            rgba,
        },
        left: left as i32,
        top: top as i32,
    })
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
    #[test]
    fn only_a_solid_unturned_picture_reaching_every_edge_covers_the_canvas() {
        let p = Placement::default();
        // Fitted 100x100 on a 100x100 canvas: covers.
        assert!(covers_canvas(100, 100, 100, 100, &p));
        // Fitted wide picture on a tall canvas leaves bars.
        assert!(!covers_canvas(100, 200, 200, 100, &p));
        // Scaled up until it reaches the top and bottom: covers.
        assert!(covers_canvas(
            100,
            200,
            200,
            100,
            &Placement { scale: 4.0, ..p }
        ));
        // Moved off to one side, turned, slanted or see-through: does not.
        assert!(!covers_canvas(
            100,
            100,
            100,
            100,
            &Placement { x: 0.1, ..p }
        ));
        assert!(!covers_canvas(
            100,
            100,
            100,
            100,
            &Placement { rotation: 5.0, ..p }
        ));
        assert!(!covers_canvas(
            100,
            100,
            100,
            100,
            &Placement { skew_x: 5.0, ..p }
        ));
        assert!(!covers_canvas(
            100,
            100,
            100,
            100,
            &Placement { opacity: 0.9, ..p }
        ));
    }

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
    fn text_between_pixels_is_moved_to_the_nearest_whole_pixel() {
        // Centred, the picture's left edge is at pixel 1; x = 0.3 of the 4-pixel
        // canvas adds 1.2, so the edge is at 2.2 and lands on pixel 2.
        let placement = Placement {
            natural: true,
            x: 0.3,
            ..Placement::default()
        };
        let c = draw((4, 4), &Frame::solid(2, 2, RED), placement);
        assert_eq!(count(&c, RED), 4, "four whole red pixels, no blending");
        for (x, y) in [(2, 1), (3, 1), (2, 2), (3, 2)] {
            assert_eq!(c.pixel(x, y), RED, "{x},{y}");
        }
    }

    #[test]
    fn a_turned_and_slanted_picture_steps_along_rows_like_the_general_path() {
        let (w, h) = (41usize, 33usize);
        let mut frame = Frame::solid(w as u32, h as u32, [0, 0, 0, 255]);
        for i in 0..w * h {
            let v = (i * 61 % 253) as u8;
            frame.rgba[i * 4..i * 4 + 3].copy_from_slice(&[v, v.wrapping_mul(5), 255 - v]);
        }
        let mut see_through = frame.clone();
        for i in 0..w * h {
            see_through.rgba[i * 4 + 3] = 120 + (i % 130) as u8;
        }
        for (picture, opacity) in [(&frame, 1.0), (&see_through, 0.8)] {
            for (rotation, skew_x, skew_y, flip) in [
                (0.0, 12.0, 0.0, false),
                (0.0, 0.0, -9.0, true),
                (23.0, 10.0, 6.0, false),
                (-40.0, 0.0, 0.0, true),
            ] {
                let p = Placement {
                    x: 0.04,
                    y: -0.02,
                    scale: 1.4,
                    rotation,
                    skew_x,
                    skew_y,
                    flip_h: flip,
                    opacity,
                    ..Placement::default()
                };
                NO_AFFINE.with(|f| f.set(false));
                let fast = draw((90, 120), picture, p);
                NO_AFFINE.with(|f| f.set(true));
                let slow = draw((90, 120), picture, p);
                NO_AFFINE.with(|f| f.set(false));
                let worst = fast
                    .rgba
                    .iter()
                    .zip(&slow.rgba)
                    .map(|(a, b)| a.abs_diff(*b))
                    .max()
                    .unwrap();
                assert!(
                    worst <= 2,
                    "rot {rotation} skew {skew_x},{skew_y}: differs by {worst}"
                );
            }
        }
    }

    #[test]
    fn the_scaled_fast_path_agrees_with_the_general_one() {
        // A made-up picture, scaled, moved, mirrored and a little see-through,
        // once as it is and once with a turn too small to see (which takes the
        // general path).
        let (w, h) = (37usize, 29usize);
        let mut frame = Frame::solid(w as u32, h as u32, [0, 0, 0, 255]);
        for i in 0..w * h {
            let v = (i * 73 % 251) as u8;
            frame.rgba[i * 4..i * 4 + 4].copy_from_slice(&[
                v,
                v.wrapping_mul(3),
                255 - v,
                200 + (i % 56) as u8,
            ]);
        }
        for flip_h in [false, true] {
            for flip_v in [false, true] {
                let fast = Placement {
                    x: 0.07,
                    y: -0.03,
                    scale: 1.3,
                    opacity: 0.9,
                    flip_h,
                    flip_v,
                    ..Placement::default()
                };
                let general = Placement {
                    rotation: 0.0001,
                    ..fast
                };
                let a = draw((50, 80), &frame, fast);
                let b = draw((50, 80), &frame, general);
                let worst = a
                    .rgba
                    .iter()
                    .zip(&b.rgba)
                    .map(|(p, q)| p.abs_diff(*q))
                    .max()
                    .unwrap();
                assert!(worst <= 1, "flip {flip_h} {flip_v}: differs by {worst}");
            }
        }
    }

    #[test]
    fn an_opaque_scaled_picture_matches_the_general_path() {
        let (w, h) = (53usize, 41usize);
        let mut frame = Frame::solid(w as u32, h as u32, [0, 0, 0, 255]);
        for i in 0..w * h {
            let v = (i * 97 % 251) as u8;
            frame.rgba[i * 4..i * 4 + 4].copy_from_slice(&[v, v.wrapping_mul(5), 255 - v, 255]);
        }
        for (scale, x, y, flip_h, flip_v) in [
            (1.0f32, 0.0f32, 0.0f32, false, false),
            (1.371, 0.05, -0.04, false, true),
            (2.554, -0.1, 0.1, true, false),
            (0.6, 0.2, 0.0, true, true),
        ] {
            let fast = Placement {
                scale,
                x,
                y,
                flip_h,
                flip_v,
                ..Placement::default()
            };
            let general = Placement {
                rotation: 0.0001,
                ..fast
            };
            let a = draw((60, 90), &frame, fast);
            let b = draw((60, 90), &frame, general);
            let worst = a
                .rgba
                .iter()
                .zip(&b.rgba)
                .map(|(p, q)| p.abs_diff(*q))
                .max()
                .unwrap();
            assert!(worst <= 2, "scale {scale}: differs by {worst}");
        }
    }

    #[test]
    fn a_warped_picture_is_found_inside_its_bounds() {
        // The bounded search must give exactly what searching the whole canvas gives.
        let (w, h) = (40usize, 24usize);
        let mut frame = Frame::solid(w as u32, h as u32, [0, 0, 0, 255]);
        for i in 0..w * h {
            let v = (i * 91 % 253) as u8;
            frame.rgba[i * 4..i * 4 + 4].copy_from_slice(&[v, 255 - v, v / 2, 255]);
        }
        let cases = [
            (200.0, 0.0, 0.0, 0.0, 0.0, false),
            (0.0, 25.0, 0.0, 0.0, 0.0, false),
            (-30.0, 20.0, 0.0, 0.0, 0.0, false),
            (0.0, 0.0, 0.0, 0.4, 0.0, false),
            (0.0, 0.0, 0.0, 0.0, -0.5, false),
            (15.0, -10.0, 33.0, 0.3, 0.2, true),
            (0.0, 0.0, 90.0, 0.2, 0.0, true),
        ];
        for (skew_x, skew_y, rotation, persp_x, persp_y, flip) in cases {
            let placement = Placement {
                x: 0.1,
                y: -0.15,
                scale: 0.7,
                rotation,
                skew_x: skew_x / 10.0,
                skew_y: skew_y / 10.0,
                persp_x,
                persp_y,
                flip_h: flip,
                natural: true,
                ..Placement::default()
            };
            SEARCH_ALL.with(|f| f.set(false));
            let bounded = draw((120, 200), &frame, placement);
            SEARCH_ALL.with(|f| f.set(true));
            let whole = draw((120, 200), &frame, placement);
            SEARCH_ALL.with(|f| f.set(false));
            assert!(
                bounded.rgba == whole.rgba,
                "differs for skew {skew_x},{skew_y} rotation {rotation} perspective {persp_x},{persp_y}"
            );
            assert!(count(&whole, BLACK) < 120 * 200, "something was drawn");
        }
    }

    #[test]
    fn a_rendered_layer_laid_over_a_canvas_matches_drawing_it_directly() {
        let (w, h) = (45usize, 31usize);
        let mut frame = Frame::solid(w as u32, h as u32, [0, 0, 0, 255]);
        for i in 0..w * h {
            let v = (i * 59 % 251) as u8;
            frame.rgba[i * 4..i * 4 + 4].copy_from_slice(&[
                v,
                255 - v,
                v / 3,
                120 + (i % 130) as u8,
            ]);
        }
        let cases = [
            Placement {
                natural: true,
                x: 0.1,
                y: 0.2,
                ..Placement::default()
            },
            Placement {
                x: -0.1,
                scale: 0.8,
                opacity: 0.7,
                ..Placement::default()
            },
            Placement {
                scale: 1.6,
                flip_h: true,
                ..Placement::default()
            },
            Placement {
                rotation: 20.0,
                scale: 0.9,
                ..Placement::default()
            },
            Placement {
                skew_x: 12.0,
                persp_x: 0.2,
                natural: true,
                ..Placement::default()
            },
        ];
        for placement in cases {
            let mut direct = Canvas::new(90, 120);
            for px in direct.rgba.chunks_exact_mut(4) {
                px[..3].copy_from_slice(&[40, 90, 200]);
            }
            let mut cached = direct.clone();
            direct.draw(&frame, &placement);
            let layer = render_layer(90, 120, &frame, &placement).expect("it covers something");
            cached.blit(&layer);
            let worst = direct
                .rgba
                .iter()
                .zip(&cached.rgba)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap();
            assert!(worst <= 2, "{placement:?}: differs by {worst}");
        }
        let nothing = Placement {
            opacity: 0.0,
            ..Placement::default()
        };
        assert!(render_layer(10, 10, &frame, &nothing).is_none());
    }

    /// Speed check (run with `cargo test --release -p cutroom-compose -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn bench_scaled_video_layer() {
        let (w, h) = (1440usize, 1080usize);
        let mut frame = Frame::solid(w as u32, h as u32, [0, 0, 0, 255]);
        for i in 0..w * h {
            let v = (i * 73 % 251) as u8;
            frame.rgba[i * 4..i * 4 + 4].copy_from_slice(&[v, v.wrapping_mul(3), 255 - v, 255]);
        }
        for (scale, skew_x, slow) in [
            (1.0f32, 0.0f32, false),
            (1.371, 0.0, false),
            (2.554, 0.0, false),
            (1.371, 8.0, true),
            (1.371, 8.0, false),
        ] {
            NO_AFFINE.with(|f| f.set(slow));
            let placement = Placement {
                scale,
                skew_x,
                ..Placement::default()
            };
            let mut canvas = Canvas::new(1080, 1920);
            let start = std::time::Instant::now();
            let runs = 20;
            for _ in 0..runs {
                canvas.draw(&frame, &placement);
            }
            eprintln!(
                "BENCH scale {scale} skew {skew_x} general path {slow}: {:.2} ms per draw",
                start.elapsed().as_secs_f64() * 1000.0 / f64::from(runs)
            );
        }
    }

    #[test]
    fn only_the_filled_part_of_a_mostly_clear_row_is_drawn() {
        // A wide clear picture with one red pixel in the middle of a row.
        let mut frame = Frame::solid(9, 3, [0, 0, 0, 0]);
        frame.rgba[(9 + 4) * 4..(9 + 4) * 4 + 4].copy_from_slice(&RED);
        let placement = Placement {
            natural: true,
            ..Placement::default()
        };
        let c = draw((9, 3), &frame, placement);
        assert_eq!(count(&c, RED), 1);
        assert_eq!(c.pixel(4, 1), RED);
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
