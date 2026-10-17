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
use std::any::Any;
use std::collections::VecDeque;
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

/// How many pictures stay uploaded between calls, at most, and how many bytes
/// of them (cached text and stills stay; video pictures drift out).
const MAX_TEXTURES: usize = 96;
const TEXTURE_BYTES: usize = 160 * 1024 * 1024;

/// A picture that is on the card.
struct Upload {
    /// Keeps the picture alive so its address cannot be reused while it is here.
    _hold: Arc<dyn Any + Send + Sync>,
    key: usize,
    texture: glow::Texture,
    width: u32,
    height: u32,
    bytes: usize,
}

/// Where the corners of one layer land.
struct Quad {
    /// Four corners, each: clip position (x, y, z, w) and picture coordinate (u, v).
    vertices: [f32; 24],
    /// The canvas rectangle the layer can touch: left, top, right, bottom.
    bounds: (i32, i32, i32, i32),
    /// The picture's size, and how many canvas pixels one picture pixel is.
    size: (f32, f32),
    scale: f32,
}

/// Errors starting with this are about one layer the card path cannot place
/// (the processor does that picture); anything else is the card failing.
/// Pictures that can be on their way back from the card at once.
pub const PACKS: usize = 3;

pub const UNSUPPORTED: &str = "unsupported: ";

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
    uploaded: Vec<Upload>,
    uploaded_bytes: usize,
    /// Textures no longer used, kept to be filled again when a picture of the same size comes.
    spare: Vec<(u32, u32, glow::Texture)>,
    width: u32,
    height: u32,
    /// Buffers the card copies finished pictures into while it carries on drawing.
    packs: Vec<glow::Buffer>,
    /// Which of them hold a picture that was asked for and not yet taken, oldest first.
    in_flight: VecDeque<usize>,
    next_pack: usize,
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
                uploaded_bytes: 0,
                spare: Vec::new(),
                width: 0,
                height: 0,
                packs: Vec::new(),
                in_flight: VecDeque::new(),
                next_pack: 0,
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
        let size = width as i32 * height as i32 * 4;
        for pack in self.packs.drain(..) {
            unsafe { self.gl.delete_buffer(pack) };
        }
        self.in_flight.clear();
        for _ in 0..PACKS {
            unsafe {
                let pack = self.gl.create_buffer()?;
                self.gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(pack));
                self.gl
                    .buffer_data_size(glow::PIXEL_PACK_BUFFER, size, glow::STREAM_READ);
                self.gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
                self.packs.push(pack);
            }
        }
        Ok(())
    }

    /// The card's copy of a picture, uploading it if it is not there already.
    fn texture_for(
        &mut self,
        hold: Arc<dyn Any + Send + Sync>,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> glow::Texture {
        let key = Arc::as_ptr(&hold).cast::<()>() as usize;
        if let Some(i) = self.uploaded.iter().position(|u| u.key == key) {
            let used = self.uploaded.remove(i);
            let texture = used.texture;
            self.uploaded.push(used);
            return texture;
        }
        let bytes = rgba.len();
        let gl = &self.gl;
        // Make room: the least recently used pictures go first.
        while !self.uploaded.is_empty()
            && (self.uploaded.len() >= MAX_TEXTURES || self.uploaded_bytes + bytes > TEXTURE_BYTES)
        {
            let old = self.uploaded.remove(0);
            self.uploaded_bytes -= old.bytes;
            self.spare.push((old.width, old.height, old.texture));
        }
        while self.spare.len() > 6 {
            let (_, _, texture) = self.spare.remove(0);
            unsafe { gl.delete_texture(texture) };
        }
        let (texture, same_size) = match self
            .spare
            .iter()
            .position(|(w, h, _)| *w == width && *h == height)
        {
            Some(i) => (self.spare.remove(i).2, true),
            None => (unsafe { gl.create_texture().expect("texture") }, false),
        };
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
            if same_size {
                gl.tex_sub_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    0,
                    0,
                    width as i32,
                    height as i32,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(rgba)),
                );
            } else {
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA8 as i32,
                    width as i32,
                    height as i32,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(rgba)),
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
        self.uploaded_bytes += bytes;
        self.uploaded.push(Upload {
            _hold: hold,
            key,
            texture,
            width,
            height,
            bytes,
        });
        texture
    }

    /// Where the four corners of `frame` land when placed like this. `Ok(None)`:
    /// it shows nothing.
    fn quad(&self, frame: &Frame, placement: &Placement) -> Result<Option<Quad>, String> {
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
                return Err(format!(
                    "{UNSUPPORTED}perspective too strong for the card path"
                ));
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
        let bounds = (
            (lo_x - 2.0).floor().max(0.0) as i32,
            (lo_y - 2.0).floor().max(0.0) as i32,
            (hi_x + 2.0).ceil().min(cw) as i32,
            (hi_y + 2.0).ceil().min(ch) as i32,
        );
        if bounds.0 >= bounds.2 || bounds.1 >= bounds.3 {
            return Ok(None);
        }
        Ok(Some(Quad {
            vertices,
            bounds,
            size: (sw, sh),
            scale,
        }))
    }

    /// Draw one quad with the bound state (the caller has set up the target).
    fn draw_quad(&mut self, texture: glow::Texture, quad: &Quad, opacity: f32) {
        self.draw_vertices(
            texture,
            &quad.vertices,
            glow::TRIANGLE_STRIP,
            quad.size,
            quad.scale,
            opacity,
        );
    }

    /// Draw triangles (six floats a corner: clip x, y, z, w, then u, v).
    fn draw_vertices(
        &mut self,
        texture: glow::Texture,
        vertices: &[f32],
        mode: u32,
        size: (f32, f32),
        shown: f32,
        opacity: f32,
    ) {
        let gl = &self.gl;
        unsafe {
            gl.use_program(Some(self.program));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.uniform_1_i32(gl.get_uniform_location(self.program, "picture").as_ref(), 0);
            gl.uniform_2_f32(
                gl.get_uniform_location(self.program, "size").as_ref(),
                size.0,
                size.1,
            );
            gl.uniform_1_f32(gl.get_uniform_location(self.program, "shown").as_ref(), shown);
            gl.uniform_1_f32(
                gl.get_uniform_location(self.program, "opacity").as_ref(),
                opacity,
            );
            gl.bind_vertex_array(Some(self.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes_of(vertices), glow::STREAM_DRAW);
            gl.draw_arrays(mode, 0, (vertices.len() / 6) as i32);
        }
    }

    /// A bent picture (corner pin, mesh warp) as a fine grid of triangles:
    /// each grid point goes where [`Mapper`] says the picture's point lands,
    /// which follows the same steps as the processor's drawing.
    fn draw_warped(&mut self, frame: &Arc<Frame>, placement: &Placement) -> Result<(), String> {
        const CELLS: usize = 24;
        let (cw, ch) = (self.width as f32, self.height as f32);
        let (sw, sh) = (frame.width as f32, frame.height as f32);
        let mapper = cutroom_compose::mapping::Mapper::new(placement, (cw, ch), (sw, sh));
        let n = CELLS + 1;
        let mut points = Vec::with_capacity(n * n);
        for j in 0..n {
            for i in 0..n {
                let (u, v) = (i as f32 / CELLS as f32, j as f32 / CELLS as f32);
                let (px, py) = mapper.point(u, v).ok_or_else(|| {
                    format!("{UNSUPPORTED}perspective too strong for the card path")
                })?;
                points.push((2.0 * px / cw - 1.0, 2.0 * py / ch - 1.0, u, v));
            }
        }
        let mut vertices = Vec::with_capacity(CELLS * CELLS * 36);
        for j in 0..CELLS {
            for i in 0..CELLS {
                let at = |a: usize, b: usize| points[b * n + a];
                for (a, b) in [
                    (i, j),
                    (i + 1, j),
                    (i, j + 1),
                    (i + 1, j),
                    (i + 1, j + 1),
                    (i, j + 1),
                ] {
                    let (x, y, u, v) = at(a, b);
                    vertices.extend_from_slice(&[x, y, 0.0, 1.0, u, v]);
                }
            }
        }
        let fit = if placement.natural {
            1.0
        } else {
            (cw / sw).min(ch / sh)
        };
        let texture = self.texture_for(frame.clone(), frame.width, frame.height, &frame.rgba);
        self.draw_vertices(
            texture,
            &vertices,
            glow::TRIANGLES,
            (sw, sh),
            fit * placement.scale,
            placement.opacity.min(1.0),
        );
        Ok(())
    }

    /// Draw `frame` like [`cutroom_compose::Canvas::draw`] would and return the
    /// part of the canvas it touches. `Ok(None)`: it touches nothing. `Err`:
    /// the card cannot do this one (use the processor).
    pub fn render(
        &mut self,
        frame: &Arc<Frame>,
        placement: &Placement,
    ) -> Result<Option<Layer>, String> {
        let Some(quad) = self.quad(frame, placement)? else {
            return Ok(None);
        };
        let (x0, y0, x1, y1) = quad.bounds;
        let (rw, rh) = ((x1 - x0) as usize, (y1 - y0) as usize);
        let texture = self.texture_for(frame.clone(), frame.width, frame.height, &frame.rgba);
        let mut rgba = vec![0u8; rw * rh * 4];
        unsafe {
            let gl = &self.gl;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbo));
            gl.viewport(0, 0, self.width as i32, self.height as i32);
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(x0, y0, rw as i32, rh as i32);
            gl.clear_color(0.0, 0.0, 0.0, 0.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.disable(glow::BLEND);
            gl.disable(glow::CULL_FACE);
        }
        self.draw_quad(texture, &quad, placement.opacity.min(1.0));
        unsafe {
            let gl = &self.gl;
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

    // ------------------------------------------------ whole pictures

    /// Start a picture: an opaque black canvas that stays on the card while the
    /// layers are drawn over it, bottom first (see `draw`, `draw_pixels`,
    /// `finish_picture`).
    pub fn begin_picture(&mut self) {
        let gl = &self.gl;
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbo));
            gl.viewport(0, 0, self.width as i32, self.height as i32);
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::CULL_FACE);
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.enable(glow::BLEND);
            gl.blend_func_separate(
                glow::SRC_ALPHA,
                glow::ONE_MINUS_SRC_ALPHA,
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
            );
        }
    }

    /// Lay `frame` over the picture like [`cutroom_compose::Canvas::draw`] would.
    pub fn draw(&mut self, frame: &Arc<Frame>, placement: &Placement) -> Result<(), String> {
        if placement.has_distort() {
            // Corner pin and mesh warp: a grid of triangles bends the picture.
            if frame.width == 0 || frame.height == 0 || placement.opacity <= 0.0 || placement.scale <= 0.0 {
                return Ok(());
            }
            return self.draw_warped(frame, placement);
        }
        let Some(quad) = self.quad(frame, placement)? else {
            return Ok(());
        };
        let texture = self.texture_for(frame.clone(), frame.width, frame.height, &frame.rgba);
        self.draw_quad(texture, &quad, placement.opacity.min(1.0));
        Ok(())
    }

    /// Lay a picture over the canvas pixel for pixel with its top left corner
    /// at (`left`, `top`): cached text and stills, and pictures that already
    /// are the canvas size. `hold` is what the picture belongs to (its address
    /// names the card's copy).
    pub fn draw_pixels(
        &mut self,
        hold: Arc<dyn Any + Send + Sync>,
        frame: &Frame,
        left: i32,
        top: i32,
    ) {
        if frame.width == 0 || frame.height == 0 {
            return;
        }
        let (cw, ch) = (self.width as f32, self.height as f32);
        let (l, t) = (left as f32, top as f32);
        let (r, b) = (l + frame.width as f32, t + frame.height as f32);
        let to_clip = |x: f32, y: f32| (2.0 * x / cw - 1.0, 2.0 * y / ch - 1.0);
        let mut vertices = [0.0f32; 24];
        for (i, (x, y, u, v)) in [
            (l, t, 0.0, 0.0),
            (r, t, 1.0, 0.0),
            (l, b, 0.0, 1.0),
            (r, b, 1.0, 1.0),
        ]
        .into_iter()
        .enumerate()
        {
            let (cx, cy) = to_clip(x, y);
            vertices[i * 6..i * 6 + 6].copy_from_slice(&[cx, cy, 0.0, 1.0, u, v]);
        }
        let quad = Quad {
            vertices,
            bounds: (
                left,
                top,
                left + frame.width as i32,
                top + frame.height as i32,
            ),
            size: (frame.width as f32, frame.height as f32),
            scale: 1.0,
        };
        let texture = self.texture_for(hold, frame.width, frame.height, &frame.rgba);
        self.draw_quad(texture, &quad, 1.0);
    }

    /// Ask the card to copy the finished picture into a buffer of its own and
    /// carry on; nothing waits. Take it later with [`take_picture`]. At most
    /// [`PACKS`] pictures can be on their way at once.
    pub fn start_picture_read(&mut self) -> Result<(), String> {
        if self.in_flight.len() >= self.packs.len() {
            return Err("too many pictures waiting to be read back".into());
        }
        let slot = self.next_pack;
        self.next_pack = (self.next_pack + 1) % self.packs.len();
        let gl = &self.gl;
        unsafe {
            gl.disable(glow::BLEND);
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(self.packs[slot]));
            gl.read_pixels(
                0,
                0,
                self.width as i32,
                self.height as i32,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::BufferOffset(0),
            );
            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
            let error = gl.get_error();
            if error != glow::NO_ERROR {
                return Err(format!("OpenGL error 0x{error:x}"));
            }
        }
        self.in_flight.push_back(slot);
        Ok(())
    }

    /// How many pictures were asked for and not yet taken.
    pub fn pictures_waiting(&self) -> usize {
        self.in_flight.len()
    }

    /// The oldest picture asked for with [`start_picture_read`], top row first,
    /// as straight RGBA bytes; waits until the card has finished it.
    pub fn take_picture(&mut self) -> Result<Vec<u8>, String> {
        let slot = self
            .in_flight
            .pop_front()
            .ok_or("no picture is waiting to be read back")?;
        let length = self.width as usize * self.height as usize * 4;
        let gl = &self.gl;
        unsafe {
            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(self.packs[slot]));
            let mapped = gl.map_buffer_range(
                glow::PIXEL_PACK_BUFFER,
                0,
                length as i32,
                glow::MAP_READ_BIT,
            );
            if mapped.is_null() {
                gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
                return Err("the card would not hand the picture over".into());
            }
            let rgba = std::slice::from_raw_parts(mapped, length).to_vec();
            gl.unmap_buffer(glow::PIXEL_PACK_BUFFER);
            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
            let error = gl.get_error();
            if error != glow::NO_ERROR {
                return Err(format!("OpenGL error 0x{error:x}"));
            }
            Ok(rgba)
        }
    }

    /// The finished picture, top row first, as straight RGBA bytes.
    #[cfg(test)]
    pub fn finish_picture(&mut self) -> Result<Vec<u8>, String> {
        let mut rgba = vec![0u8; self.width as usize * self.height as usize * 4];
        let gl = &self.gl;
        unsafe {
            gl.disable(glow::BLEND);
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
            gl.read_pixels(
                0,
                0,
                self.width as i32,
                self.height as i32,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut rgba)),
            );
            let error = gl.get_error();
            if error != glow::NO_ERROR {
                return Err(format!("OpenGL error 0x{error:x}"));
            }
        }
        Ok(rgba)
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
            for upload in self.uploaded.drain(..) {
                gl.delete_texture(upload.texture);
            }
            for (_, _, texture) in self.spare.drain(..) {
                gl.delete_texture(texture);
            }
            for pack in self.packs.drain(..) {
                gl.delete_buffer(pack);
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
    use cutroom_compose::{render_layer, Canvas, Layer};

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
    fn a_whole_picture_built_on_the_card_matches_the_processor() {
        let Ok(mut card) = GpuCompositor::new(120, 200) else {
            eprintln!("no OpenGL here: skipping");
            return;
        };
        let frame = picture();
        // A caption-like layer: mostly clear, with a half see-through bar.
        let mut caption = Frame::solid(70, 20, [0, 0, 0, 0]);
        for y in 5..15 {
            for x in 4..66 {
                let o = (y * 70 + x) * 4;
                caption.rgba[o..o + 4].copy_from_slice(&[
                    250,
                    240,
                    20,
                    if x < 30 { 255 } else { 140 },
                ]);
            }
        }
        let layer = Arc::new(Layer {
            frame: caption,
            left: 25,
            top: 150,
        });
        let back = Placement {
            scale: 4.0,
            ..Placement::default()
        };
        let skewed = Placement {
            scale: 1.5,
            skew_x: 9.0,
            y: -0.1,
            opacity: 0.9,
            ..Placement::default()
        };
        let mut cpu = Canvas::new(120, 200);
        cpu.draw(&frame, &back);
        cpu.draw(&frame, &skewed);
        cpu.blit(&layer);
        card.begin_picture();
        card.draw(&frame, &back).unwrap();
        card.draw(&frame, &skewed).unwrap();
        card.draw_pixels(layer.clone(), &layer.frame, layer.left, layer.top);
        let on_card = card.finish_picture().unwrap();
        let mut off = 0usize;
        let mut worst = 0u8;
        for (p, q) in on_card.chunks_exact(4).zip(cpu.rgba.chunks_exact(4)) {
            let diff = (0..3).map(|c| p[c].abs_diff(q[c])).max().unwrap();
            worst = worst.max(diff);
            if diff > 12 {
                off += 1;
            }
            assert_eq!(p[3], 255, "the picture stays opaque");
        }
        eprintln!(
            "whole picture: {off} of {} pixels differ by more than 12, worst {worst}",
            120 * 200
        );
        assert!(off < 120 * 200 / 50, "{off} pixels differ (worst {worst})");
    }

    #[test]
    fn bent_pictures_on_the_card_match_the_processor() {
        let Ok(mut card) = GpuCompositor::new(120, 200) else {
            eprintln!("no OpenGL here: skipping");
            return;
        };
        let frame = picture();
        let mut mesh = [0.0f32; 32];
        mesh[10] = 0.08; // a middle point pushed right
        mesh[13] = -0.06; // another pulled up
        let mut pin = [0.0f32; 8];
        pin[2] = -0.1; // top right corner in
        pin[5] = 0.05; // bottom right corner down
        for (name, placement) in [
            ("mesh", Placement { scale: 1.1, mesh, ..Placement::default() }),
            ("pin", Placement { scale: 1.0, pin, ..Placement::default() }),
            (
                "pin, flipped and turned",
                Placement {
                    scale: 0.9,
                    pin,
                    mesh,
                    flip_h: true,
                    rotation: 12.0,
                    ..Placement::default()
                },
            ),
        ] {
            let mut cpu = Canvas::new(120, 200);
            cpu.draw(&frame, &placement);
            card.begin_picture();
            card.draw(&frame, &placement).unwrap();
            let on_card = card.finish_picture().unwrap();
            let off = on_card
                .chunks_exact(4)
                .zip(cpu.rgba.chunks_exact(4))
                .filter(|(p, q)| (0..3).any(|c| p[c].abs_diff(q[c]) > 24))
                .count();
            eprintln!("{name}: {off} of {} pixels differ by more than 24", 120 * 200);
            assert!(off < 120 * 200 / 25, "{name}: {off} pixels differ");
        }
    }

    #[test]
    fn pictures_read_back_without_waiting_arrive_whole_and_in_order() {
        let Ok(mut card) = GpuCompositor::new(120, 200) else {
            eprintln!("no OpenGL here: skipping");
            return;
        };
        let frame = picture();
        let mut expected = VecDeque::new();
        let mut taken = 0;
        for i in 0..9 {
            let placement = Placement {
                scale: 1.0 + i as f32 * 0.3,
                ..Placement::default()
            };
            card.begin_picture();
            card.draw(&frame, &placement).unwrap();
            // The same picture read the slow way, to compare with.
            expected.push_back(card.finish_picture().unwrap());
            card.start_picture_read().unwrap();
            // Keep two on their way, like export does.
            while card.pictures_waiting() > 2 {
                let got = card.take_picture().unwrap();
                assert!(got == expected.pop_front().unwrap(), "picture {taken} differs");
                taken += 1;
            }
        }
        while card.pictures_waiting() > 0 {
            let got = card.take_picture().unwrap();
            assert!(got == expected.pop_front().unwrap(), "picture {taken} differs");
            taken += 1;
        }
        assert_eq!(taken, 9);
        assert!(card.take_picture().is_err());
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
