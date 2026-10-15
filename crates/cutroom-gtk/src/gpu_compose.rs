//! Putting one layer together on the graphics card.
//!
//! A headless OpenGL ES 3 context (EGL, loaded at run time) draws a video
//! picture as a four-cornered shape - scaled, turned, slanted, with
//! perspective - into an off-screen image and reads the touched part back as a
//! [`Layer`], which the processor then lays over the canvas exactly as it does
//! for cached stills and text. Anything that goes wrong comes back as an error
//! and the caller falls back to the processor.

use cutroom_compose::{Frame, Layer, Placement};
use glow::HasContext;
use khronos_egl as egl;
use std::sync::Arc;

const VERTEX: &str = "#version 300 es
precision highp float;
layout(location = 0) in vec4 position;
layout(location = 1) in vec2 coord;
out vec2 v_coord;
void main() {
    v_coord = coord;
    gl_Position = position;
}";

const FRAGMENT: &str = "#version 300 es
precision highp float;
uniform sampler2D picture;
uniform vec2 size;
uniform float shown;
uniform float opacity;
in vec2 v_coord;
out vec4 colour;
void main() {
    vec4 c = texture(picture, v_coord);
    float inside = min(min(v_coord.x, 1.0 - v_coord.x) * size.x,
                       min(v_coord.y, 1.0 - v_coord.y) * size.y) * shown;
    float cover = clamp(inside + 0.5, 0.0, 1.0);
    colour = vec4(c.rgb, c.a * opacity * cover);
}";

/// How many pictures stay uploaded between calls.
const TEXTURES: usize = 4;

pub struct GpuCompositor {
    egl: egl::DynamicInstance<egl::EGL1_5>,
    display: egl::Display,
    context: egl::Context,
    surface: egl::Surface,
    gl: glow::Context,
    program: glow::Program,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    fbo: glow::Framebuffer,
    target: glow::Texture,
    uploaded: Vec<(Arc<Frame>, glow::Texture)>,
    width: u32,
    height: u32,
    /// What the card calls itself, for the log.
    pub renderer: String,
}

impl GpuCompositor {
    /// A compositor for a `width` x `height` canvas, or why there is none.
    pub fn new(width: u32, height: u32) -> Result<GpuCompositor, String> {
        // SAFETY: loading the system EGL library; its functions are only used
        // through the typed instance below.
        let egl = unsafe { egl::DynamicInstance::<egl::EGL1_5>::load_required() }
            .map_err(|e| format!("no EGL library ({e})"))?;
        // The surfaceless platform needs no window system; fall back to the
        // default display (what a desktop session offers).
        const PLATFORM_SURFACELESS_MESA: egl::Enum = 0x31DD;
        let display = unsafe {
            egl.get_platform_display(
                PLATFORM_SURFACELESS_MESA,
                egl::DEFAULT_DISPLAY,
                &[egl::ATTRIB_NONE],
            )
        }
        .or_else(|_| unsafe { egl.get_display(egl::DEFAULT_DISPLAY) }.ok_or(()))
        .map_err(|()| "no EGL display".to_string())?;
        egl.initialize(display)
            .map_err(|e| format!("EGL would not start ({e})"))?;
        let config = egl
            .choose_first_config(
                display,
                &[
                    egl::SURFACE_TYPE,
                    egl::PBUFFER_BIT,
                    egl::RENDERABLE_TYPE,
                    egl::OPENGL_ES3_BIT,
                    egl::RED_SIZE,
                    8,
                    egl::GREEN_SIZE,
                    8,
                    egl::BLUE_SIZE,
                    8,
                    egl::ALPHA_SIZE,
                    8,
                    egl::NONE,
                ],
            )
            .map_err(|e| format!("EGL configuration failed ({e})"))?
            .ok_or("no suitable EGL configuration")?;
        egl.bind_api(egl::OPENGL_ES_API)
            .map_err(|e| format!("OpenGL ES unavailable ({e})"))?;
        let context = egl
            .create_context(
                display,
                config,
                None,
                &[
                    egl::CONTEXT_MAJOR_VERSION,
                    3,
                    egl::CONTEXT_MINOR_VERSION,
                    0,
                    egl::NONE,
                ],
            )
            .map_err(|e| format!("no OpenGL ES 3 context ({e})"))?;
        let surface = egl
            .create_pbuffer_surface(
                display,
                config,
                &[egl::WIDTH, 16, egl::HEIGHT, 16, egl::NONE],
            )
            .map_err(|e| format!("no off-screen surface ({e})"))?;
        egl.make_current(display, Some(surface), Some(surface), Some(context))
            .map_err(|e| format!("could not use the context ({e})"))?;
        // SAFETY: the context is current on this thread; the loader returns
        // EGL's own function pointers.
        let gl = unsafe {
            glow::Context::from_loader_function(|name| {
                egl.get_proc_address(name)
                    .map_or(std::ptr::null(), |f| f as *const _)
            })
        };
        let renderer = unsafe { gl.get_parameter_string(glow::RENDERER) };
        let mut this = unsafe {
            let program = Self::program(&gl)?;
            let vao = gl.create_vertex_array()?;
            let vbo = gl.create_buffer()?;
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            let stride = 6 * 4;
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 4, glow::FLOAT, false, stride, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, stride, 16);
            let target = gl.create_texture()?;
            let fbo = gl.create_framebuffer()?;
            GpuCompositor {
                egl,
                display,
                context,
                surface,
                gl,
                program,
                vao,
                vbo,
                fbo,
                target,
                uploaded: Vec::new(),
                width: 0,
                height: 0,
                renderer,
            }
        };
        this.resize(width, height)?;
        Ok(this)
    }

    unsafe fn program(gl: &glow::Context) -> Result<glow::Program, String> {
        let program = gl.create_program()?;
        for (kind, source) in [
            (glow::VERTEX_SHADER, VERTEX),
            (glow::FRAGMENT_SHADER, FRAGMENT),
        ] {
            let shader = gl.create_shader(kind)?;
            gl.shader_source(shader, source);
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                return Err(format!("shader failed: {}", gl.get_shader_info_log(shader)));
            }
            gl.attach_shader(program, shader);
            gl.delete_shader(shader);
        }
        gl.link_program(program);
        if !gl.get_program_link_status(program) {
            return Err(format!(
                "shader link failed: {}",
                gl.get_program_info_log(program)
            ));
        }
        Ok(program)
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<(), String> {
        let gl = &self.gl;
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(self.target));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                width as i32,
                height as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbo));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(self.target),
                0,
            );
            if gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
                return Err("the off-screen image could not be made".into());
            }
        }
        self.width = width;
        self.height = height;
        Ok(())
    }

    /// The card's copy of `frame`, uploading it if it is not there already.
    fn texture(&mut self, frame: &Arc<Frame>) -> glow::Texture {
        if let Some((_, tex)) = self.uploaded.iter().find(|(f, _)| Arc::ptr_eq(f, frame)) {
            return *tex;
        }
        let gl = &self.gl;
        // Reuse the oldest texture's storage when it is the same size.
        let recycled = if self.uploaded.len() >= TEXTURES {
            Some(self.uploaded.remove(0))
        } else {
            None
        };
        let (tex, same_size) = match &recycled {
            Some((old, tex)) => (*tex, old.width == frame.width && old.height == frame.height),
            None => (unsafe { gl.create_texture().expect("texture") }, false),
        };
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            if same_size {
                gl.tex_sub_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    0,
                    0,
                    frame.width as i32,
                    frame.height as i32,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(&frame.rgba)),
                );
            } else {
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA8 as i32,
                    frame.width as i32,
                    frame.height as i32,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(&frame.rgba)),
                );
                for (name, value) in [
                    (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                    (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                    (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                    (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
                ] {
                    gl.tex_parameter_i32(glow::TEXTURE_2D, name, value as i32);
                }
            }
        }
        self.uploaded.push((frame.clone(), tex));
        tex
    }

    /// Draw `frame` like [`cutroom_compose::Canvas::draw`] would and return the
    /// part of the canvas it touches. `Ok(None)`: it touches nothing. `Err`:
    /// the card cannot do this one (use the processor).
    pub fn render(
        &mut self,
        frame: &Arc<Frame>,
        placement: &Placement,
    ) -> Result<Option<Layer>, String> {
        if frame.width == 0
            || frame.height == 0
            || placement.opacity <= 0.0
            || placement.scale <= 0.0
        {
            return Ok(None);
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
        let (sin, cos) = placement.rotation.to_radians().sin_cos();
        let (skew_x, skew_y) = (
            placement.skew_x.to_radians().tan(),
            placement.skew_y.to_radians().tan(),
        );
        // The four corners (top left, top right, bottom left, bottom right):
        // where each lands on the canvas, and which point of the picture it is.
        let mut vertices = [0.0f32; 24];
        let (mut lo_x, mut hi_x, mut lo_y, mut hi_y) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for (i, (u, v)) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)]
            .into_iter()
            .enumerate()
        {
            let (ux, uy) = ((u - 0.5) * sw, (v - 0.5) * sh);
            // Slant, then perspective, then turn and scale (as `draw` undoes them).
            let qx = ux - skew_x * uy;
            let qy = uy + skew_y * qx;
            let d = 1.0 + placement.persp_x * (qx / sw) + placement.persp_y * (qy / sh);
            if !d.is_finite() || d < 0.05 {
                return Err("perspective too strong for the card path".into());
            }
            let (tx, ty) = (scale * (qx * cos - qy * sin), scale * (qx * sin + qy * cos));
            let (px, py) = (cx + tx / d, cy + ty / d);
            lo_x = lo_x.min(px);
            hi_x = hi_x.max(px);
            lo_y = lo_y.min(py);
            hi_y = hi_y.max(py);
            // Clip space, with the canvas's top row first in memory (so the
            // read-back needs no flip): y runs the same way as the canvas.
            let o = i * 6;
            vertices[o] = 2.0 * (cx * d + tx) / cw - d;
            vertices[o + 1] = 2.0 * (cy * d + ty) / ch - d;
            vertices[o + 2] = 0.0;
            vertices[o + 3] = d;
            vertices[o + 4] = if placement.flip_h { 1.0 - u } else { u };
            vertices[o + 5] = if placement.flip_v { 1.0 - v } else { v };
        }
        let x0 = (lo_x - 2.0).floor().max(0.0) as i32;
        let y0 = (lo_y - 2.0).floor().max(0.0) as i32;
        let x1 = (hi_x + 2.0).ceil().min(cw) as i32;
        let y1 = (hi_y + 2.0).ceil().min(ch) as i32;
        if x0 >= x1 || y0 >= y1 {
            return Ok(None);
        }
        let (rw, rh) = ((x1 - x0) as usize, (y1 - y0) as usize);
        let texture = self.texture(frame);
        let gl = &self.gl;
        let mut rgba = vec![0u8; rw * rh * 4];
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbo));
            gl.viewport(0, 0, self.width as i32, self.height as i32);
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(x0, y0, rw as i32, rh as i32);
            gl.clear_color(0.0, 0.0, 0.0, 0.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.disable(glow::BLEND);
            gl.disable(glow::CULL_FACE);
            gl.use_program(Some(self.program));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.uniform_1_i32(gl.get_uniform_location(self.program, "picture").as_ref(), 0);
            gl.uniform_2_f32(
                gl.get_uniform_location(self.program, "size").as_ref(),
                sw,
                sh,
            );
            gl.uniform_1_f32(
                gl.get_uniform_location(self.program, "shown").as_ref(),
                scale,
            );
            gl.uniform_1_f32(
                gl.get_uniform_location(self.program, "opacity").as_ref(),
                placement.opacity.min(1.0),
            );
            gl.bind_vertex_array(Some(self.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes_of(&vertices), glow::STREAM_DRAW);
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
            gl.read_pixels(
                x0,
                y0,
                rw as i32,
                rh as i32,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut rgba)),
            );
            gl.disable(glow::SCISSOR_TEST);
            let error = gl.get_error();
            if error != glow::NO_ERROR {
                return Err(format!("OpenGL error 0x{error:x}"));
            }
        }
        Ok(Some(Layer {
            frame: Frame {
                width: rw as u32,
                height: rh as u32,
                rgba,
            },
            left: x0,
            top: y0,
        }))
    }
}

fn bytes_of(values: &[f32]) -> &[u8] {
    // SAFETY: f32 has no padding; the slice is read-only for the call.
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast(), std::mem::size_of_val(values)) }
}

impl Drop for GpuCompositor {
    fn drop(&mut self) {
        let _ = self.egl.make_current(
            self.display,
            Some(self.surface),
            Some(self.surface),
            Some(self.context),
        );
        let gl = &self.gl;
        unsafe {
            for (_, tex) in self.uploaded.drain(..) {
                gl.delete_texture(tex);
            }
            gl.delete_texture(self.target);
            gl.delete_framebuffer(self.fbo);
            gl.delete_buffer(self.vbo);
            gl.delete_vertex_array(self.vao);
            gl.delete_program(self.program);
        }
        let _ = self.egl.make_current(self.display, None, None, None);
        let _ = self.egl.destroy_surface(self.display, self.surface);
        let _ = self.egl.destroy_context(self.display, self.context);
        // The display is left open on purpose: when the surfaceless platform is
        // not offered, it is the desktop's own display, which GTK is still using,
        // and terminating it would pull it out from under the interface.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cutroom_compose::{render_layer, Canvas};

    fn picture() -> Arc<Frame> {
        let (w, h) = (64usize, 48usize);
        let mut frame = Frame::solid(w as u32, h as u32, [0, 0, 0, 255]);
        for y in 0..h {
            for x in 0..w {
                let o = (y * w + x) * 4;
                frame.rgba[o..o + 3].copy_from_slice(&[
                    (x * 4) as u8,
                    (y * 5) as u8,
                    ((x + y) * 2) as u8,
                ]);
            }
        }
        Arc::new(frame)
    }

    /// Fraction of covered canvas pixels where the card and the processor
    /// disagree by more than `level`, and the largest disagreement.
    fn compare(placement: Placement) -> Option<(f64, u8)> {
        let mut gpu = GpuCompositor::new(120, 200).ok()?;
        let frame = picture();
        let on_card = gpu.render(&frame, &placement).expect("render")?;
        let on_cpu = render_layer(120, 200, &frame, &placement)?;
        let mut a = Canvas::new(120, 200);
        a.blit(&on_card);
        let mut b = Canvas::new(120, 200);
        b.blit(&on_cpu);
        let (mut off, mut covered, mut worst) = (0u32, 0u32, 0u8);
        for (p, q) in a.rgba.chunks_exact(4).zip(b.rgba.chunks_exact(4)) {
            if p[..3] == [0, 0, 0] && q[..3] == [0, 0, 0] {
                continue;
            }
            covered += 1;
            let diff = (0..3).map(|c| p[c].abs_diff(q[c])).max().unwrap();
            worst = worst.max(diff);
            if diff > 12 {
                off += 1;
            }
        }
        Some((f64::from(off) / f64::from(covered.max(1)), worst))
    }

    #[test]
    fn the_card_draws_like_the_processor() {
        for (name, placement) in [
            (
                "plain",
                Placement {
                    scale: 1.2,
                    ..Placement::default()
                },
            ),
            (
                "slanted",
                Placement {
                    scale: 1.1,
                    skew_x: 10.0,
                    ..Placement::default()
                },
            ),
            (
                "turned",
                Placement {
                    scale: 1.0,
                    rotation: 25.0,
                    x: 0.05,
                    ..Placement::default()
                },
            ),
            (
                "flipped",
                Placement {
                    scale: 1.0,
                    flip_h: true,
                    flip_v: true,
                    rotation: 10.0,
                    ..Placement::default()
                },
            ),
            (
                "perspective",
                Placement {
                    scale: 1.0,
                    persp_x: 0.3,
                    skew_y: 5.0,
                    ..Placement::default()
                },
            ),
            (
                "see-through",
                Placement {
                    scale: 1.0,
                    opacity: 0.5,
                    rotation: 8.0,
                    ..Placement::default()
                },
            ),
        ] {
            let Some((off, worst)) = compare(placement) else {
                eprintln!("no OpenGL here: skipping");
                return;
            };
            eprintln!(
                "{name}: {:.2}% of pixels differ by more than 12, worst {worst}",
                off * 100.0
            );
            assert!(
                off < 0.03,
                "{name}: {:.1}% differ (worst {worst})",
                off * 100.0
            );
        }
    }
}
