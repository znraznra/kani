//! Draws a text clip: the words, outline, shadow and box described by a
//! [`TextSpec`], onto a transparent picture the size of the canvas. That
//! picture goes through the compositor like any other layer, so text can be
//! moved, scaled, turned, slanted and faded with the same controls.

use cutroom_compose::Frame;
use cutroom_timeline::{TextAlign, TextSpec};
use gtk4::cairo;
use gtk4::pango;

/// Where the text block (including its box) sits on the canvas, in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Widest the text may get before it wraps, as a share of the canvas width.
const WRAP_SHARE: f64 = 0.9;

struct Laid {
    layout: pango::Layout,
    /// Where the layout's ink starts inside its own box (Pango's logical offset).
    ink: (f64, f64),
    /// Size of the text itself.
    size: (f64, f64),
    font_px: f64,
}

impl Laid {
    fn px(&self, thousandths: i32) -> f64 {
        f64::from(thousandths) / 1000.0 * self.font_px
    }
}

fn lay_out(cr: &cairo::Context, spec: &TextSpec, width: u32, height: u32, lit: bool) -> Laid {
    let font_px = f64::from(spec.size) / 1000.0 * f64::from(height);
    let layout = pangocairo::functions::create_layout(cr);
    let mut font = pango::FontDescription::new();
    font.set_family(if spec.font.trim().is_empty() {
        crate::fonts::FAMILY
    } else {
        spec.font.as_str()
    });
    font.set_weight(if spec.bold {
        pango::Weight::Bold
    } else {
        pango::Weight::Normal
    });
    font.set_style(if spec.italic {
        pango::Style::Italic
    } else {
        pango::Style::Normal
    });
    font.set_absolute_size(font_px * f64::from(pango::SCALE));
    layout.set_font_description(Some(&font));
    layout.set_alignment(match spec.align {
        TextAlign::Left => pango::Alignment::Left,
        TextAlign::Center => pango::Alignment::Center,
        TextAlign::Right => pango::Alignment::Right,
    });
    layout.set_wrap(pango::WrapMode::WordChar);
    layout.set_width((f64::from(width) * WRAP_SHARE * f64::from(pango::SCALE)) as i32);
    let attrs = pango::AttrList::new();
    if spec.spacing != 0 {
        let units = (f64::from(spec.spacing) / 1000.0 * font_px * f64::from(pango::SCALE)) as i32;
        attrs.insert(pango::AttrInt::new_letter_spacing(units));
    }
    // The word being said gets its own colour (only on the fill layer: the
    // shadow and outline keep theirs).
    if let (true, Some((from, to)), true) = (lit, spec.active, spec.highlight[3] > 0) {
        if from < to && to <= spec.content.len() {
            let c = spec.highlight;
            let up = |v: u8| u16::from(v) * 257;
            let mut fg = pango::AttrColor::new_foreground(up(c[0]), up(c[1]), up(c[2]));
            fg.set_start_index(from as u32);
            fg.set_end_index(to as u32);
            attrs.insert(fg);
            if c[3] < 255 {
                let mut alpha = pango::AttrInt::new_foreground_alpha(up(c[3]));
                alpha.set_start_index(from as u32);
                alpha.set_end_index(to as u32);
                attrs.insert(alpha);
            }
        }
    }
    layout.set_attributes(Some(&attrs));
    layout.set_text(&spec.content);
    let (_, logical) = layout.pixel_extents();
    let (w, h) = (f64::from(logical.width()), f64::from(logical.height()));
    Laid {
        layout,
        ink: (f64::from(logical.x()), f64::from(logical.y())),
        size: (w, h),
        font_px,
    }
}

/// The picture is the text block (with its box) plus room all round for the
/// outline and shadow, so effects are never cut off and the picture's centre
/// is the block's centre.
struct Frame2 {
    /// Room added on every side.
    margin: f64,
    /// The block: text plus padding.
    block: (f64, f64),
}

fn frame_of(spec: &TextSpec, laid: &Laid) -> Frame2 {
    let pad = laid.px(spec.padding);
    let mut margin = 2.0;
    if spec.outline > 0 {
        margin += laid.px(spec.outline);
    }
    if spec.shadow > 0 {
        margin += laid.px(spec.shadow) + laid.px(spec.shadow_blur) + 1.0;
    }
    Frame2 {
        margin,
        block: (laid.size.0 + 2.0 * pad, laid.size.1 + 2.0 * pad),
    }
}

/// The size of the picture [`render`] makes for this text on a canvas of this
/// size, in canvas pixels (the block, its box and room for effects). The
/// picture is placed by its centre, which is the block's centre.
pub fn bounds(spec: &TextSpec, width: u32, height: u32) -> Option<Bounds> {
    let spec = &spec.effective();
    if spec.content.trim().is_empty() {
        return None;
    }
    let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 1, 1).ok()?;
    let cr = cairo::Context::new(&surface).ok()?;
    let laid = lay_out(&cr, spec, width, height, false);
    let f = frame_of(spec, &laid);
    Some(Bounds {
        x: 0.0,
        y: 0.0,
        w: (f.block.0 + 2.0 * f.margin).ceil(),
        h: (f.block.1 + 2.0 * f.margin).ceil(),
    })
}

fn set_source(cr: &cairo::Context, c: [u8; 4], alpha: f64) {
    cr.set_source_rgba(
        f64::from(c[0]) / 255.0,
        f64::from(c[1]) / 255.0,
        f64::from(c[2]) / 255.0,
        f64::from(c[3]) / 255.0 * alpha,
    );
}

fn rounded_rect(cr: &cairo::Context, b: Bounds, radius: f64) {
    let r = radius.min(b.w / 2.0).min(b.h / 2.0).max(0.0);
    let (x, y, w, h) = (b.x, b.y, b.w, b.h);
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -std::f64::consts::FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, std::f64::consts::FRAC_PI_2);
    cr.arc(
        x + r,
        y + h - r,
        r,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
    );
    cr.arc(
        x + r,
        y + r,
        r,
        std::f64::consts::PI,
        1.5 * std::f64::consts::PI,
    );
    cr.close_path();
}

/// Draw the text clip as a transparent picture just big enough for it (see
/// [`bounds`]). It is meant to be placed at its natural size, not fitted to
/// the canvas.
pub fn render(spec: &TextSpec, width: u32, height: u32) -> Frame {
    let spec = &spec.effective();
    let empty = Frame {
        width: 0,
        height: 0,
        rgba: Vec::new(),
    };
    let Some(size) = bounds(spec, width, height) else {
        return empty;
    };
    let (fw, fh) = (size.w.max(1.0) as u32, size.h.max(1.0) as u32);
    if fw > 8192 || fh > 8192 {
        return empty;
    }
    let mut frame = Frame {
        width: fw,
        height: fh,
        rgba: vec![0; fw as usize * fh as usize * 4],
    };
    let Ok(mut surface) = cairo::ImageSurface::create(cairo::Format::ARgb32, fw as i32, fh as i32)
    else {
        return empty;
    };
    {
        let Ok(cr) = cairo::Context::new(&surface) else {
            return empty;
        };
        let laid = lay_out(&cr, spec, width, height, false);
        let fill = lay_out(&cr, spec, width, height, true);
        let f = frame_of(spec, &laid);
        let pad = laid.px(spec.padding);
        // Centre the block in the picture (the size was rounded up).
        let bx = (f64::from(fw) - f.block.0) / 2.0;
        let by = (f64::from(fh) - f.block.1) / 2.0;
        let block = Bounds {
            x: bx,
            y: by,
            w: f.block.0,
            h: f.block.1,
        };

        if spec.background[3] > 0 {
            set_source(&cr, spec.background, 1.0);
            let radius = f64::from(spec.corner) / 1000.0 * block.h;
            rounded_rect(&cr, block, radius);
            let _ = cr.fill();
        }

        let (ox, oy) = (bx + pad - laid.ink.0, by + pad - laid.ink.1);
        let font_px = laid.font_px;
        if spec.shadow > 0 && spec.shadow_color[3] > 0 {
            let dist = f64::from(spec.shadow) / 1000.0 * font_px;
            let blur = f64::from(spec.shadow_blur) / 1000.0 * font_px;
            // A soft shadow is the text drawn several times around its spot.
            let taps: Vec<(f64, f64)> = if blur < 0.5 {
                vec![(0.0, 0.0)]
            } else {
                let mut t = vec![(0.0, 0.0)];
                for i in 0..12 {
                    let a = f64::from(i) / 12.0 * std::f64::consts::TAU;
                    t.push((a.cos() * blur * 0.5, a.sin() * blur * 0.5));
                    t.push((a.cos() * blur, a.sin() * blur));
                }
                t
            };
            let each = 1.0 / (taps.len() as f64).sqrt();
            for (dx, dy) in taps {
                cr.save().ok();
                cr.translate(ox + dist * 0.7071 + dx, oy + dist * 0.7071 + dy);
                set_source(&cr, spec.shadow_color, each);
                pangocairo::functions::show_layout(&cr, &laid.layout);
                cr.restore().ok();
            }
        }

        if spec.outline > 0 {
            cr.save().ok();
            cr.translate(ox, oy);
            pangocairo::functions::layout_path(&cr, &laid.layout);
            set_source(&cr, spec.outline_color, 1.0);
            cr.set_line_join(cairo::LineJoin::Round);
            cr.set_line_width(f64::from(spec.outline) / 1000.0 * font_px * 2.0);
            let _ = cr.stroke();
            cr.restore().ok();
        }

        cr.save().ok();
        cr.translate(ox, oy);
        set_source(&cr, spec.color, 1.0);
        pangocairo::functions::show_layout(&cr, &fill.layout);
        cr.restore().ok();
    }
    surface.flush();
    let stride = surface.stride() as usize;
    let Ok(data) = surface.data() else {
        return frame;
    };
    // Cairo hands over premultiplied BGRA; the compositor wants straight RGBA.
    let (width, height) = (fw, fh);
    for y in 0..height as usize {
        for x in 0..width as usize {
            let s = y * stride + x * 4;
            let (b, g, r, a) = (data[s], data[s + 1], data[s + 2], data[s + 3]);
            let o = (y * width as usize + x) * 4;
            if a == 0 {
                continue;
            }
            let un =
                |v: u8| ((u32::from(v) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8;
            frame.rgba[o..o + 4].copy_from_slice(&[un(r), un(g), un(b), a]);
        }
    }
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opaque_pixels(f: &Frame) -> usize {
        f.rgba.chunks_exact(4).filter(|p| p[3] > 0).count()
    }

    fn spec(text: &str) -> TextSpec {
        TextSpec {
            content: text.into(),
            ..TextSpec::default()
        }
    }

    #[test]
    fn the_picture_is_just_big_enough_for_the_words() {
        let f = render(&spec("Hello"), 400, 300);
        let b = bounds(&spec("Hello"), 400, 300).unwrap();
        assert_eq!((f64::from(f.width), f64::from(f.height)), (b.w, b.h));
        assert!(f.width < 400 && f.height < 300, "smaller than the canvas");
        assert!(opaque_pixels(&f) > 200);
        // The ink is centred in the picture.
        let (mut left, mut right) = (f.width, 0);
        for y in 0..f.height {
            for x in 0..f.width {
                if f.rgba[((y * f.width + x) * 4 + 3) as usize] > 0 {
                    left = left.min(x);
                    right = right.max(x);
                }
            }
        }
        let (gap_l, gap_r) = (left, f.width - 1 - right);
        assert!(gap_l.abs_diff(gap_r) <= 3, "margins {gap_l} and {gap_r}");
    }

    #[test]
    fn the_spoken_word_is_drawn_in_the_highlight_colour() {
        let mut s = spec("AAAA BBBB");
        s.color = [255, 255, 255, 255];
        s.highlight = [255, 0, 0, 255];
        s.active = Some((5, 9));
        let f = render(&s, 600, 300);
        let (mut red_x, mut white_x) = (Vec::new(), Vec::new());
        for y in 0..f.height {
            for x in 0..f.width {
                let p = &f.rgba[((y * f.width + x) * 4) as usize..][..4];
                if p[3] == 255 && p[0] == 255 && p[1] == 0 {
                    red_x.push(x);
                } else if p[3] == 255 && p[1] == 255 {
                    white_x.push(x);
                }
            }
        }
        assert!(
            !red_x.is_empty() && !white_x.is_empty(),
            "both colours drawn"
        );
        // The lit word is the second one, on the right.
        assert!(red_x.iter().min() > white_x.iter().min());
        // Highlighting does not change the size of the picture.
        let mut plain = s.clone();
        plain.active = None;
        assert_eq!(bounds(&plain, 600, 300), bounds(&s, 600, 300));
    }

    #[test]
    fn switched_off_effects_draw_nothing_but_keep_their_settings() {
        let mut s = spec("Hi");
        s.outline = 80;
        s.shadow = 120;
        s.background = [0, 0, 0, 255];
        s.padding = 300;
        s.outline_on = true;
        s.shadow_on = true;
        s.box_on = true;
        let with = opaque_pixels(&render(&s, 300, 200));
        let plain = opaque_pixels(&render(&spec("Hi"), 300, 200));
        assert!(with > plain * 2);
        s.outline_on = false;
        s.shadow_on = false;
        s.box_on = false;
        assert_eq!(opaque_pixels(&render(&s, 300, 200)), plain);
        assert_eq!(bounds(&s, 300, 200), bounds(&spec("Hi"), 300, 200));
        assert_eq!(
            (s.outline, s.shadow, s.padding),
            (80, 120, 300),
            "settings kept"
        );
        s.outline_on = true;
        s.shadow_on = true;
        s.box_on = true;
        assert_eq!(opaque_pixels(&render(&s, 300, 200)), with);
    }

    #[test]
    fn empty_text_draws_nothing() {
        assert_eq!(render(&spec("  "), 100, 100).width, 0);
        assert!(bounds(&spec(""), 100, 100).is_none());
    }

    #[test]
    fn colour_and_straight_alpha_come_through() {
        let mut s = spec("MMMM");
        s.color = [10, 200, 30, 255];
        let f = render(&s, 400, 200);
        let solid = f
            .rgba
            .chunks_exact(4)
            .find(|p| p[3] == 255)
            .expect("a solid pixel");
        assert_eq!(solid, &[10, 200, 30, 255]);
    }

    #[test]
    fn outline_shadow_and_box_add_ink() {
        let plain = opaque_pixels(&render(&spec("Hi"), 300, 200));
        let mut s = spec("Hi");
        s.outline = 80;
        s.outline_on = true;
        assert!(opaque_pixels(&render(&s, 300, 200)) > plain);
        let mut s = spec("Hi");
        s.shadow = 120;
        s.shadow_on = true;
        assert!(opaque_pixels(&render(&s, 300, 200)) > plain);
        let mut s = spec("Hi");
        s.background = [0, 0, 0, 255];
        s.padding = 300;
        s.box_on = true;
        assert!(opaque_pixels(&render(&s, 300, 200)) > plain * 2);
    }

    #[test]
    fn long_text_wraps_inside_the_canvas() {
        let long = spec("one two three four five six seven eight nine ten eleven twelve");
        let b = bounds(&long, 300, 300).unwrap();
        assert!(b.w <= 300.0 * 0.9 + 2.0);
        assert!(b.h > bounds(&spec("one"), 300, 300).unwrap().h * 1.5);
    }

    #[test]
    fn bigger_size_means_bigger_block() {
        let mut big = spec("Hi");
        big.size = 200;
        assert!(bounds(&big, 400, 400).unwrap().h > bounds(&spec("Hi"), 400, 400).unwrap().h * 2.0);
    }

    #[test]
    fn perspective_and_skew_change_how_text_looks_in_both_directions() {
        use cutroom_compose::{Canvas, Placement};
        let frame = render(&spec("MMMM"), 400, 400);
        let ink = |p: Placement| -> Vec<usize> {
            let mut c = Canvas::new(400, 400);
            c.draw(&frame, &Placement { natural: true, ..p });
            // Lit pixels in the left and right halves, top and bottom halves.
            let lit = |x0: u32, x1: u32, y0: u32, y1: u32| {
                (y0..y1)
                    .flat_map(|y| (x0..x1).map(move |x| (x, y)))
                    .filter(|&(x, y)| c.pixel(x, y)[0] > 128)
                    .count()
            };
            vec![
                lit(0, 200, 0, 400),
                lit(200, 400, 0, 400),
                lit(0, 400, 0, 200),
                lit(0, 400, 200, 400),
            ]
        };
        let plain = ink(Placement::default());
        let px = ink(Placement {
            persp_x: 0.6,
            scale: 2.0,
            ..Default::default()
        });
        assert!(
            px[0] > px[1],
            "x perspective: far side (right) is smaller {px:?}"
        );
        let py = ink(Placement {
            persp_y: 0.6,
            scale: 2.0,
            ..Default::default()
        });
        assert!(
            py[2] > py[3],
            "y perspective: far side (bottom) is smaller {py:?}"
        );
        assert!(plain[0] > 0 && plain[2] > 0);
    }
}
