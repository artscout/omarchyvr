//! Headless, CPU-only rendering of egui frames to RGBA buffers and PNG files.
//!
//! No GPU, no wgpu/OpenGL, no window system: the triangles that
//! [`egui::Context::tessellate`] produces are rasterised by a small software renderer that
//! reproduces what the egui wgpu backend does with `fs_main_gamma_framebuffer`:
//!
//! * vertex colours (premultiplied sRGBA, treated as gamma-space floats) and UVs are
//!   interpolated affinely over the triangle,
//! * the texture is sampled bilinearly at pixel centres (clamp-to-edge, like the "predictable
//!   texture filtering" path of egui-wgpu), `TextureOptions` of the texture are honoured,
//! * `src = vertex_colour * texel` is blended with premultiplied alpha,
//!   `dst = src + dst * (1 - src.a)`, directly in gamma space,
//! * clip rects are converted to pixels with the same rounding as egui-wgpu/egui_glow.
//!
//! Coverage uses 24.8 fixed-point vertex positions, exact integer edge functions and the
//! top-left fill rule, so triangles that share an edge never leave a gap and never blend twice.
//!
//! Typical use (use a *dedicated* [`egui::Context`], not the one of a live window):
//!
//! ```ignore
//! let ctx = egui::Context::default();
//! snapshot(&ctx, egui::vec2(1280.0, 860.0), 1.25, 3, |ui| { /* build the ui */ },
//!          std::path::Path::new("out/shot.png"))?;
//! ```
//!
//! Needs only `egui` (`default-features = false`, `default_fonts` so there are glyphs) and `png`.

use eframe::egui;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::sync::Arc;

use egui::epaint::{ClippedPrimitive, ImageDelta, Mesh, Primitive};
use egui::{
    Color32, ImageData, TextureFilter, TextureId, TextureOptions, TextureWrapMode, TexturesDelta,
};

/// Sub-pixel precision of vertex positions (like GPU rasterisers: 8 bits).
const SUBPIXEL_BITS: u32 = 8;
/// One pixel in fixed-point units.
const FIX: i64 = 1 << SUBPIXEL_BITS;
/// Half a pixel in fixed-point units: pixel `x` has its centre at `x * FIX + HALF`.
const HALF: i64 = FIX / 2;
/// Vertex coordinates are clamped to +-this many fixed-point units (2^28 = about 1M pixels)
/// so that the `i64` edge-function maths cannot overflow, whatever egui throws at us.
const COORD_LIMIT: f64 = (1_i64 << 28) as f64;
/// Texel coordinates are clamped to this before they are turned into integers.
const TEXEL_LIMIT: f32 = 1.0e7;
/// `u8` -> 0..1.
const INV_255: f32 = 1.0 / 255.0;
/// Largest image side `snapshot` accepts (like a typical GPU texture limit).
const MAX_SIDE_PX: f32 = 16384.0;

// ---------------------------------------------------------------------------------------------
// Textures
// ---------------------------------------------------------------------------------------------

/// A texture that egui asked us to keep (the font atlas, images, ...).
struct Texture {
    /// width, height in texels
    size: [usize; 2],
    /// Premultiplied sRGBA, row by row, `size[0] * size[1]` entries.
    pixels: Vec<Color32>,
    options: TextureOptions,
}

impl Texture {
    #[inline]
    fn texel(&self, x: usize, y: usize) -> [f32; 4] {
        let [r, g, b, a] = self.pixels[y * self.size[0] + x].to_array();
        [
            f32::from(r) * INV_255,
            f32::from(g) * INV_255,
            f32::from(b) * INV_255,
            f32::from(a) * INV_255,
        ]
    }

    /// Bilinear sample at texel-space coordinates `(u, v)` (= uv * texture size).
    #[inline]
    fn sample_linear(&self, u: f32, v: f32) -> [f32; 4] {
        let px = (u - 0.5).clamp(-TEXEL_LIMIT, TEXEL_LIMIT);
        let py = (v - 0.5).clamp(-TEXEL_LIMIT, TEXEL_LIMIT);
        let (fx, fy) = (px.floor(), py.floor());
        let (tx, ty) = (px - fx, py - fy);
        let (x0, y0) = (fx as i32, fy as i32);
        let mode = self.options.wrap_mode;
        let (xa, xb) = (
            wrap(x0, self.size[0], mode),
            wrap(x0 + 1, self.size[0], mode),
        );
        let (ya, yb) = (
            wrap(y0, self.size[1], mode),
            wrap(y0 + 1, self.size[1], mode),
        );
        let (tl, tr) = (self.texel(xa, ya), self.texel(xb, ya));
        let (bl, br) = (self.texel(xa, yb), self.texel(xb, yb));
        let mut out = [0.0; 4];
        for (i, o) in out.iter_mut().enumerate() {
            let top = tl[i] + (tr[i] - tl[i]) * tx;
            let bottom = bl[i] + (br[i] - bl[i]) * tx;
            *o = top + (bottom - top) * ty;
        }
        out
    }

    /// Nearest-texel sample at texel-space coordinates `(u, v)`.
    #[inline]
    fn sample_nearest(&self, u: f32, v: f32) -> [f32; 4] {
        let x = u.clamp(-TEXEL_LIMIT, TEXEL_LIMIT).floor() as i32;
        let y = v.clamp(-TEXEL_LIMIT, TEXEL_LIMIT).floor() as i32;
        let mode = self.options.wrap_mode;
        self.texel(wrap(x, self.size[0], mode), wrap(y, self.size[1], mode))
    }
}

/// Texel index `i` (possibly outside `0..n`) -> index inside `0..n`. Requires `n > 0`.
#[inline]
fn wrap(i: i32, n: usize, mode: TextureWrapMode) -> usize {
    let n = n as i32;
    let wrapped = match mode {
        TextureWrapMode::ClampToEdge => i.clamp(0, n - 1),
        TextureWrapMode::Repeat => i.rem_euclid(n),
        TextureWrapMode::MirroredRepeat => {
            let m = i.rem_euclid(2 * n);
            if m < n { m } else { 2 * n - 1 - m }
        }
    };
    wrapped as usize
}

// ---------------------------------------------------------------------------------------------
// Public renderer
// ---------------------------------------------------------------------------------------------

/// CPU renderer state: the textures egui asked for (font atlas etc.).
///
/// Keep one `Raster` per [`egui::Context`] and feed it the `textures_delta` of *every* pass
/// (see [`Raster::apply_textures_delta`] and [`Raster::paint`]), exactly like a GPU backend
/// would, otherwise glyphs that were added to the font atlas in an earlier pass are missing.
#[derive(Default)]
pub struct Raster {
    textures: HashMap<TextureId, Texture>,
}

impl Raster {
    /// An empty renderer: it knows no texture until it is fed a `textures_delta`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply the texture changes of a pass that is *not* painted (`set`, then `free`).
    ///
    /// The painted pass goes through [`Raster::paint`] instead, which applies `set` before and
    /// `free` after painting.
    pub fn apply_textures_delta(&mut self, mut delta: TexturesDelta) {
        self.apply_set(&mut delta);
        self.apply_free(&mut delta);
    }

    /// Apply `output.textures_delta` (set before painting, free after), tessellate
    /// `output.shapes` with `ctx.tessellate(shapes, output.pixels_per_point)` and rasterise
    /// them into an opaque RGBA8 buffer of `width_px` x `height_px` pixels (row-major, top row
    /// first), cleared to `background` first. `ctx` must be the context that produced `output`.
    ///
    /// The alpha of `background` is ignored (the result is always opaque; a translucent
    /// background is effectively composited over black). Paint callbacks and textures that egui
    /// does not manage (`TextureId::User`) are skipped.
    pub fn paint(
        &mut self,
        ctx: &egui::Context,
        mut output: egui::FullOutput,
        width_px: u32,
        height_px: u32,
        background: Color32,
    ) -> Vec<u8> {
        // `TexturesDelta` panics (debug builds) if dropped with unapplied entries, so take it
        // out of `output` and drain it completely.
        let mut delta = std::mem::take(&mut output.textures_delta);
        let shapes = std::mem::take(&mut output.shapes);
        let pixels_per_point = output.pixels_per_point;

        self.apply_set(&mut delta);
        let primitives = ctx.tessellate(shapes, pixels_per_point);
        let rgba = self.render(
            &primitives,
            pixels_per_point,
            width_px,
            height_px,
            background,
        );
        self.apply_free(&mut delta);
        rgba
    }

    fn apply_set(&mut self, delta: &mut TexturesDelta) {
        for (id, patches) in delta.set.drain() {
            for patch in patches {
                self.apply_image_delta(id, patch);
            }
        }
    }

    fn apply_free(&mut self, delta: &mut TexturesDelta) {
        for id in delta.free.drain() {
            self.textures.remove(&id);
        }
    }

    /// `pos == None`: replace the whole texture; `pos == Some`: patch a sub-rectangle.
    fn apply_image_delta(&mut self, id: TextureId, delta: ImageDelta) {
        let ImageData::Color(image) = delta.image;
        let [w, h] = image.size;
        let pixels = Arc::try_unwrap(image)
            .map_or_else(|shared| shared.pixels.clone(), |image| image.pixels);
        if pixels.len() != w * h {
            return; // malformed image, ignore
        }
        match delta.pos {
            None => {
                self.textures.insert(
                    id,
                    Texture {
                        size: [w, h],
                        pixels,
                        options: delta.options,
                    },
                );
            }
            Some([px, py]) => {
                let Some(texture) = self.textures.get_mut(&id) else {
                    return; // patch for a texture we never got: nothing to patch
                };
                texture.options = delta.options;
                let [tw, th] = texture.size;
                if px >= tw || py >= th {
                    return;
                }
                let columns = w.min(tw - px);
                for row in 0..h.min(th - py) {
                    let src = &pixels[row * w..row * w + columns];
                    let start = (py + row) * tw + px;
                    texture.pixels[start..start + columns].copy_from_slice(src);
                }
            }
        }
    }

    /// Rasterise already tessellated primitives (`pixels_per_point` converts their points to pixels).
    fn render(
        &self,
        primitives: &[ClippedPrimitive],
        pixels_per_point: f32,
        width_px: u32,
        height_px: u32,
        background: Color32,
    ) -> Vec<u8> {
        let (width, height) = (width_px as usize, height_px as usize);
        let [r, g, b, _] = background.to_array();
        let mut target = Target {
            buf: [r, g, b, 255].repeat(width * height),
            width,
            height,
        };
        let ppp = if pixels_per_point.is_finite() && pixels_per_point > 0.0 {
            pixels_per_point
        } else {
            1.0
        };
        let mut verts = Vec::new();
        for primitive in primitives {
            if let Primitive::Mesh(mesh) = &primitive.primitive {
                self.draw_mesh(&mut target, mesh, primitive.clip_rect, ppp, &mut verts);
            }
        }
        target.buf
    }

    fn draw_mesh(
        &self,
        target: &mut Target,
        mesh: &Mesh,
        clip_rect: egui::Rect,
        ppp: f32,
        verts: &mut Vec<Vert>,
    ) {
        let Some(texture) = self.textures.get(&mesh.texture_id) else {
            return;
        };
        let [tw, th] = texture.size;
        if tw == 0 || th == 0 {
            return;
        }
        let clip = PixRect::from_clip_rect(clip_rect, ppp, target.width, target.height);
        if clip.x0 >= clip.x1 || clip.y0 >= clip.y1 {
            return;
        }
        // Per-vertex work once, instead of once per triangle.
        let (scale_u, scale_v) = (tw as f32, th as f32);
        verts.clear();
        verts.extend(mesh.vertices.iter().map(|v| Vert {
            x: to_fixed(v.pos.x, ppp),
            y: to_fixed(v.pos.y, ppp),
            col: color_to_f32(v.color),
            tc: [v.uv.x * scale_u, v.uv.y * scale_v],
        }));
        let (triangles, _) = mesh.indices.as_chunks::<3>();
        for &[i0, i1, i2] in triangles {
            let vertex = |i: u32| verts.get(i as usize);
            if let (Some(a), Some(b), Some(c)) = (vertex(i0), vertex(i1), vertex(i2)) {
                draw_triangle(target, clip, texture, a, b, c);
            }
        }
    }
}

/// Encode `rgba` (`width * height * 4` bytes, row-major, top row first) as an 8-bit RGBA PNG.
/// Missing parent directories of `path` are created.
pub fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let mut out = BufWriter::new(File::create(path)?);
    encode_png(&mut out, width, height, rgba)?;
    out.flush()
}

fn encode_png(out: impl Write, width: u32, height: u32, rgba: &[u8]) -> io::Result<()> {
    if width == 0 || height == 0 || rgba.len() != width as usize * height as usize * 4 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "expected {width}x{height} RGBA = {} bytes, got {}",
                width as usize * height as usize * 4,
                rgba.len()
            ),
        ));
    }
    let mut encoder = png::Encoder::new(out, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(rgba)?;
    writer.finish()?;
    Ok(())
}

/// Run `ui_fn` for `passes` frames (egui needs 2-3 passes to settle layout and load fonts) on
/// a screen of `size_points` at `pixels_per_point`, apply the texture deltas of EVERY pass in
/// order, paint the LAST pass on the active theme's `panel_fill` background, write the PNG to
/// `path` and return its size in pixels, `(size_points * pixels_per_point).round()`
/// (`InvalidInput` for a non-positive / non-finite size or scale, or a side above 16384 px).
///
/// How the screen is described to egui in every pass:
/// * `RawInput::screen_rect = (0,0) .. size_points`,
/// * `RawInput::viewports[ROOT].native_pixels_per_point = pixels_per_point / ctx.zoom_factor()`
///   (so the effective `ctx.pixels_per_point()` is exactly `pixels_per_point`; calling
///   `set_pixels_per_point` would only take effect one pass later),
/// * `RawInput::time` = 1/60 s more per pass (starting from the context's current time), and
///   `Style::animation_time` is forced to 0 for the call (restored afterwards) so animated
///   widgets (collapsing headers, ...) are settled at once.
///
/// Use a dedicated `Context` (e.g. `Context::default()` with the same fonts/style set-up as the
/// app), not the one driving a live window: this runs several passes, consumes every texture
/// delta and calls `ui_fn` several times. With zero input events nothing is hovered or focused.
/// The context may be reused for several snapshots (the font atlas is re-read from it), but
/// images it uploaded earlier (`ctx.load_texture`) are not sent again by egui and would be
/// missing: use a fresh context per snapshot if the ui shows such images.
pub fn snapshot(
    ctx: &egui::Context,
    size_points: egui::Vec2,
    pixels_per_point: f32,
    passes: usize,
    mut ui_fn: impl FnMut(&mut egui::Ui),
    path: &Path,
) -> io::Result<(u32, u32)> {
    let valid = |v: f32| v.is_finite() && v > 0.0;
    let too_big = |points: f32| points * pixels_per_point > MAX_SIDE_PX;
    if !(valid(pixels_per_point) && valid(size_points.x) && valid(size_points.y))
        || too_big(size_points.x)
        || too_big(size_points.y)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "bad snapshot size {size_points:?} points at {pixels_per_point} pixels per point"
            ),
        ));
    }
    let to_px = |points: f32| (points * pixels_per_point).round().max(1.0) as u32;
    let (width_px, height_px) = (to_px(size_points.x), to_px(size_points.y));

    // Settle animations immediately; put the user's value back when we are done.
    let themes = [egui::Theme::Dark, egui::Theme::Light];
    let saved_animation_time = themes.map(|theme| ctx.style_of(theme).animation_time);
    ctx.all_styles_mut(|style| style.animation_time = 0.0);

    let mut raster = Raster::new();
    if ctx.cumulative_pass_nr() > 0 {
        // This context already ran frames, and egui only ever sends *changes* to the font atlas:
        // start from the context's current atlas.
        let atlas = ctx.fonts(|fonts| fonts.image());
        raster.apply_image_delta(
            TextureId::default(),
            ImageDelta::full(atlas, TextureOptions::LINEAR),
        );
    }
    let passes = passes.max(1);
    let start_time = ctx.input(|i| i.time); // 0 for a fresh context; keeps time monotonic if reused
    let mut painted = None;
    for pass in 0..passes {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size_points)),
            time: Some(start_time + (pass + 1) as f64 / 60.0),
            ..Default::default()
        };
        input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(pixels_per_point / ctx.zoom_factor());

        let mut output = ctx.run_ui(input, &mut ui_fn);
        if pass + 1 == passes {
            painted = Some(output);
        } else {
            raster.apply_textures_delta(std::mem::take(&mut output.textures_delta));
        }
    }

    for (theme, animation_time) in themes.into_iter().zip(saved_animation_time) {
        ctx.style_mut_of(theme, |style| style.animation_time = animation_time);
    }

    let background = ctx.global_style().visuals.panel_fill;
    let output = painted.expect("passes >= 1");
    let rgba = raster.paint(ctx, output, width_px, height_px, background);
    write_png(path, width_px, height_px, &rgba)?;
    Ok((width_px, height_px))
}

// ---------------------------------------------------------------------------------------------
// Triangle rasteriser
// ---------------------------------------------------------------------------------------------

/// Output image, RGBA8, always opaque.
struct Target {
    buf: Vec<u8>,
    width: usize,
    height: usize,
}

/// Half-open pixel rectangle `x0..x1`, `y0..y1`.
#[derive(Clone, Copy)]
struct PixRect {
    x0: i64,
    y0: i64,
    x1: i64,
    y1: i64,
}

impl PixRect {
    /// Same conversion as the egui wgpu/glow backends use for their scissor rect: scale to
    /// pixels, round to the nearest pixel edge, clamp to the render target.
    fn from_clip_rect(clip: egui::Rect, ppp: f32, width: usize, height: usize) -> Self {
        let edge = |v: f32, max: usize| (v * ppp).round().clamp(0.0, max as f32) as i64;
        let x0 = edge(clip.min.x, width);
        let y0 = edge(clip.min.y, height);
        Self {
            x0,
            y0,
            x1: edge(clip.max.x, width).max(x0),
            y1: edge(clip.max.y, height).max(y0),
        }
    }
}

/// A vertex, already in pixel space.
#[derive(Clone, Copy)]
struct Vert {
    /// Position in pixels, 24.8 fixed point.
    x: i64,
    y: i64,
    /// Premultiplied sRGBA in 0..1 (gamma space).
    col: [f32; 4],
    /// Texture coordinate in texels (uv * texture size).
    tc: [f32; 2],
}

/// Points -> 24.8 fixed-point pixels.
fn to_fixed(points: f32, ppp: f32) -> i64 {
    (f64::from(points) * f64::from(ppp) * FIX as f64)
        .round()
        .clamp(-COORD_LIMIT, COORD_LIMIT) as i64
}

fn color_to_f32(color: Color32) -> [f32; 4] {
    color.to_array().map(|c| f32::from(c) * INV_255)
}

/// One edge function `E(r) = (q - p) x (r - p)`, evaluated at pixel centres and stepped
/// incrementally. `E > 0` is inside for the counter-clockwise (in this maths) triangles
/// `draw_triangle` normalises to; `E == 0` (centre exactly on the edge) counts as inside only
/// for top and left edges.
#[derive(Clone, Copy)]
struct Edge {
    /// Value at the first row / first column centre of the bounding box.
    e: i64,
    /// Change per pixel step in x / y.
    step_x: i64,
    step_y: i64,
    /// 0 for top-left edges, -1 for the others (turns `E >= 1` into the inside test).
    bias: i64,
}

impl Edge {
    fn new(p: &Vert, q: &Vert, first_x: i64, first_y: i64) -> Self {
        let (dx, dy) = (q.x - p.x, q.y - p.y);
        let (cx, cy) = (first_x * FIX + HALF, first_y * FIX + HALF);
        Self {
            e: dx * (cy - p.y) - dy * (cx - p.x),
            step_x: -dy * FIX,
            step_y: dx * FIX,
            bias: if dy < 0 || (dy == 0 && dx > 0) { 0 } else { -1 },
        }
    }
}

/// Rasterise one triangle: pixel-centre coverage with the top-left rule, no culling.
fn draw_triangle(
    target: &mut Target,
    clip: PixRect,
    texture: &Texture,
    a: &Vert,
    b: &Vert,
    c: &Vert,
) {
    let mut area2 = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    if area2 == 0 {
        return; // degenerate
    }
    // egui's winding is not consistent: normalise to positive area (swap two vertices).
    let (b, c) = if area2 < 0 {
        area2 = -area2;
        (c, b)
    } else {
        (b, c)
    };

    // Pixels whose centre can be inside, clipped. Pixel x has its centre at x*FIX + HALF.
    let x_lo = ((a.x.min(b.x).min(c.x) - HALF).div_euclid(FIX)).max(clip.x0);
    let x_hi = ((a.x.max(b.x).max(c.x) - HALF).div_euclid(FIX) + 1).min(clip.x1);
    let y_lo = ((a.y.min(b.y).min(c.y) - HALF).div_euclid(FIX)).max(clip.y0);
    let y_hi = ((a.y.max(b.y).max(c.y) - HALF).div_euclid(FIX) + 1).min(clip.y1);
    if x_lo >= x_hi || y_lo >= y_hi {
        return;
    }

    // edges[i] is the edge opposite vertex i, so its value / area is the barycentric weight of i.
    let edges = [
        Edge::new(b, c, x_lo, y_lo),
        Edge::new(c, a, x_lo, y_lo),
        Edge::new(a, b, x_lo, y_lo),
    ];
    let inv_area = 1.0 / area2 as f32;

    // Choose magnification or minification filter by comparing texel and pixel area.
    let texel_area2 = ((b.tc[0] - a.tc[0]) * (c.tc[1] - a.tc[1])
        - (b.tc[1] - a.tc[1]) * (c.tc[0] - a.tc[0]))
        .abs();
    let pixel_area2 = area2 as f32 / (FIX * FIX) as f32;
    let filter = if texel_area2 <= pixel_area2 {
        texture.options.magnification
    } else {
        texture.options.minification
    };
    let nearest = filter == TextureFilter::Nearest;
    let sample = |u: f32, v: f32| {
        if nearest {
            texture.sample_nearest(u, v)
        } else {
            texture.sample_linear(u, v)
        }
    };

    let flat_col = a.col == b.col && a.col == c.col;
    let flat_uv = a.tc == b.tc && a.tc == c.tc;
    // Whole triangle one colour (rect fills, ...): shade once.
    let flat_src = (flat_col && flat_uv).then(|| source(sample(a.tc[0], a.tc[1]), a.col));

    for y in y_lo..y_hi {
        // Edge values at the start of this row, and the x range (as offsets `k` from x_lo)
        // where all three edge tests pass.
        let row_start = [0, 1, 2].map(|i| edges[i].e + (y - y_lo) * edges[i].step_y);
        let (mut k_lo, mut k_hi) = (0, x_hi - x_lo - 1);
        let mut covered = true;
        for (edge, &e) in edges.iter().zip(&row_start) {
            let t = e + edge.bias; // inside while t + step_x * k >= 0
            if edge.step_x > 0 {
                k_lo = k_lo.max(-t.div_euclid(edge.step_x));
            } else if edge.step_x < 0 {
                k_hi = k_hi.min(t.div_euclid(-edge.step_x));
            } else if t < 0 {
                covered = false;
            }
        }
        if !covered || k_lo > k_hi {
            continue;
        }

        let first = (y as usize * target.width + (x_lo + k_lo) as usize) * 4;
        let count = (k_hi - k_lo + 1) as usize;
        let (pixels, _) = target.buf[first..first + count * 4].as_chunks_mut::<4>();

        if let Some((src, inv_a)) = flat_src {
            if inv_a <= 1.0e-4 {
                let rgb = src.map(|s| (s + 0.5) as u8);
                for px in pixels {
                    px[..3].copy_from_slice(&rgb);
                }
            } else if src != [0.0; 3] || inv_a < 1.0 {
                for px in pixels {
                    blend(px, src, inv_a);
                }
            }
            continue;
        }

        for (i, px) in pixels.iter_mut().enumerate() {
            let k = k_lo + i as i64;
            let w0 = (row_start[0] + edges[0].step_x * k) as f32 * inv_area;
            let w1 = (row_start[1] + edges[1].step_x * k) as f32 * inv_area;
            let w2 = (row_start[2] + edges[2].step_x * k) as f32 * inv_area;
            let col = if flat_col {
                a.col
            } else {
                [0, 1, 2, 3].map(|ch| w0 * a.col[ch] + w1 * b.col[ch] + w2 * c.col[ch])
            };
            let (u, v) = if flat_uv {
                (a.tc[0], a.tc[1])
            } else {
                (
                    w0 * a.tc[0] + w1 * b.tc[0] + w2 * c.tc[0],
                    w0 * a.tc[1] + w1 * b.tc[1] + w2 * c.tc[1],
                )
            };
            let (src, inv_a) = source(sample(u, v), col);
            if src != [0.0; 3] || inv_a < 1.0 {
                blend(px, src, inv_a);
            }
        }
    }
}

/// `vertex_colour * texel` -> (premultiplied rgb scaled to 0..255, 1 - alpha).
#[inline]
fn source(texel: [f32; 4], col: [f32; 4]) -> ([f32; 3], f32) {
    (
        [0, 1, 2].map(|ch| texel[ch] * col[ch] * 255.0),
        1.0 - texel[3] * col[3],
    )
}

/// `dst = src + dst * (1 - src.a)` on the rgb of one RGBA8 pixel (alpha stays 255).
#[inline]
fn blend(px: &mut [u8; 4], src: [f32; 3], inv_a: f32) {
    for (dst, s) in px.iter_mut().zip(src) {
        *dst = (s + f32::from(*dst) * inv_a + 0.5) as u8; // saturating cast
    }
}

// ---------------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{ColorImage, Pos2, Rect, Vec2, pos2, vec2};

    const BG: Color32 = Color32::from_rgb(10, 20, 30);

    fn white_texture(raster: &mut Raster, size: usize) {
        let mut delta = TexturesDelta::default();
        delta.push(
            TextureId::default(),
            ImageDelta::full(
                ColorImage::filled([size, size], Color32::WHITE),
                TextureOptions::LINEAR,
            ),
        );
        raster.apply_textures_delta(delta);
    }

    fn mesh_of_rect(rect: Rect, color: Color32) -> ClippedPrimitive {
        let mut mesh = Mesh::default();
        mesh.add_colored_rect(rect, color);
        ClippedPrimitive {
            clip_rect: Rect::EVERYTHING,
            primitive: Primitive::Mesh(mesh),
        }
    }

    fn pixel(rgba: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * width + x) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    }

    #[test]
    fn opaque_red_rect_is_red_inside_and_background_outside() {
        let mut raster = Raster::new();
        white_texture(&mut raster, 4);
        let prims = [mesh_of_rect(
            Rect::from_min_max(pos2(10.0, 5.0), pos2(30.0, 25.0)),
            Color32::RED,
        )];
        let rgba = raster.render(&prims, 1.0, 40, 30, BG);
        assert_eq!(rgba.len(), 40 * 30 * 4);
        for y in 0..30 {
            for x in 0..40 {
                let inside = (10..30).contains(&x) && (5..25).contains(&y);
                let want = if inside {
                    [255, 0, 0, 255]
                } else {
                    [10, 20, 30, 255]
                };
                assert_eq!(pixel(&rgba, 40, x, y), want, "pixel ({x},{y})");
            }
        }
    }

    #[test]
    fn pixels_per_point_scales_geometry() {
        let mut raster = Raster::new();
        white_texture(&mut raster, 4);
        // 10x10 points at ppp 2 = 20x20 pixels, starting at pixel (4, 6).
        let prims = [mesh_of_rect(
            Rect::from_min_size(pos2(2.0, 3.0), vec2(10.0, 10.0)),
            Color32::GREEN,
        )];
        let rgba = raster.render(&prims, 2.0, 40, 40, BG);
        assert_eq!(pixel(&rgba, 40, 3, 10), [10, 20, 30, 255]);
        assert_eq!(pixel(&rgba, 40, 4, 10), [0, 255, 0, 255]);
        assert_eq!(pixel(&rgba, 40, 23, 25), [0, 255, 0, 255]);
        assert_eq!(pixel(&rgba, 40, 24, 25), [10, 20, 30, 255]);
        assert_eq!(pixel(&rgba, 40, 10, 5), [10, 20, 30, 255]);
        assert_eq!(pixel(&rgba, 40, 10, 6), [0, 255, 0, 255]);
        assert_eq!(pixel(&rgba, 40, 10, 26), [10, 20, 30, 255]);
    }

    #[test]
    fn clip_rect_is_respected() {
        let mut raster = Raster::new();
        white_texture(&mut raster, 4);
        let mut prim = mesh_of_rect(
            Rect::from_min_max(pos2(0.0, 0.0), pos2(20.0, 20.0)),
            Color32::WHITE,
        );
        prim.clip_rect = Rect::from_min_max(pos2(5.0, 5.0), pos2(10.0, 8.0));
        let rgba = raster.render(&[prim], 1.0, 20, 20, Color32::BLACK);
        let lit = (0..20)
            .flat_map(|y| (0..20).map(move |x| (x, y)))
            .filter(|&(x, y)| pixel(&rgba, 20, x, y)[0] == 255)
            .count();
        assert_eq!(lit, 5 * 3);
        assert_eq!(pixel(&rgba, 20, 5, 5), [255; 4]);
        assert_eq!(pixel(&rgba, 20, 9, 7), [255; 4]);
        assert_eq!(pixel(&rgba, 20, 10, 7), [0, 0, 0, 255]);
    }

    #[test]
    fn translucent_triangles_sharing_an_edge_are_blended_once() {
        // Both diagonals of this square pass exactly through pixel centres. With the top-left
        // rule each of those pixels belongs to exactly one of the two triangles (whatever the
        // diagonal and the winding), so a translucent quad is uniform: no gap, no double blend.
        let mut raster = Raster::new();
        white_texture(&mut raster, 4);
        let half_white = Color32::from_rgba_premultiplied(128, 128, 128, 128);
        let corners = [
            pos2(1.0, 0.0),
            pos2(17.0, 0.0),
            pos2(17.0, 16.0),
            pos2(1.0, 16.0),
        ];
        let variants: [[u32; 6]; 4] = [
            [0, 1, 2, 0, 2, 3], // diagonal 0-2
            [0, 2, 1, 0, 3, 2], // same, opposite winding
            [0, 1, 3, 1, 2, 3], // diagonal 1-3
            [0, 3, 1, 1, 3, 2], // same, opposite winding
        ];
        for indices in variants {
            let mut mesh = Mesh::default();
            for p in corners {
                mesh.colored_vertex(p, half_white);
            }
            mesh.indices.extend(indices);
            let prim = ClippedPrimitive {
                clip_rect: Rect::EVERYTHING,
                primitive: Primitive::Mesh(mesh),
            };
            let rgba = raster.render(&[prim], 1.0, 20, 16, Color32::BLACK);
            for y in 0..16 {
                for x in 0..20 {
                    let want = if (1..17).contains(&x) {
                        [128, 128, 128, 255]
                    } else {
                        [0, 0, 0, 255]
                    };
                    assert_eq!(
                        pixel(&rgba, 20, x, y),
                        want,
                        "pixel ({x},{y}) with indices {indices:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn full_and_partial_texture_updates() {
        let mut raster = Raster::new();
        let id = TextureId::default();

        let mut delta = TexturesDelta::default();
        delta.push(
            id,
            ImageDelta::full(
                ColorImage::filled([4, 3], Color32::BLUE),
                TextureOptions::NEAREST,
            ),
        );
        raster.apply_textures_delta(delta);
        assert_eq!(raster.textures[&id].size, [4, 3]);
        assert!(
            raster.textures[&id]
                .pixels
                .iter()
                .all(|&p| p == Color32::BLUE)
        );

        // Patch a 2x2 block at (1, 1).
        let mut delta = TexturesDelta::default();
        delta.push(
            id,
            ImageDelta::partial(
                [1, 1],
                ColorImage::filled([2, 2], Color32::RED),
                TextureOptions::NEAREST,
            ),
        );
        raster.apply_textures_delta(delta);
        let tex = &raster.textures[&id];
        assert_eq!(tex.size, [4, 3]);
        for y in 0..3 {
            for x in 0..4 {
                let want = if (1..3).contains(&x) && (1..3).contains(&y) {
                    Color32::RED
                } else {
                    Color32::BLUE
                };
                assert_eq!(tex.pixels[y * 4 + x], want, "texel ({x},{y})");
            }
        }

        // A patch that sticks out of the texture is cropped instead of panicking.
        let mut delta = TexturesDelta::default();
        delta.push(
            id,
            ImageDelta::partial(
                [3, 2],
                ColorImage::filled([5, 5], Color32::GREEN),
                TextureOptions::NEAREST,
            ),
        );
        raster.apply_textures_delta(delta);
        assert_eq!(raster.textures[&id].pixels[2 * 4 + 3], Color32::GREEN);
        assert_eq!(raster.textures[&id].pixels[2 * 4 + 2], Color32::RED);

        // The patched texture is what gets drawn: a 4x3 quad mapped 1:1 onto 4x3 pixels.
        let mut mesh = Mesh::default();
        mesh.add_rect_with_uv(
            Rect::from_min_size(Pos2::ZERO, vec2(4.0, 3.0)),
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        let prim = ClippedPrimitive {
            clip_rect: Rect::EVERYTHING,
            primitive: Primitive::Mesh(mesh),
        };
        let rgba = raster.render(&[prim], 1.0, 4, 3, Color32::BLACK);
        assert_eq!(pixel(&rgba, 4, 0, 0), [0, 0, 255, 255]);
        assert_eq!(pixel(&rgba, 4, 1, 1), [255, 0, 0, 255]);
        assert_eq!(pixel(&rgba, 4, 2, 2), [255, 0, 0, 255]);
        assert_eq!(pixel(&rgba, 4, 3, 2), [0, 255, 0, 255]);

        // A full update replaces size and contents; `free` removes the texture.
        let mut delta = TexturesDelta::default();
        delta.push(
            id,
            ImageDelta::full(
                ColorImage::filled([2, 2], Color32::WHITE),
                TextureOptions::LINEAR,
            ),
        );
        raster.apply_textures_delta(delta);
        assert_eq!(raster.textures[&id].size, [2, 2]);
        let mut delta = TexturesDelta::default();
        delta.free(id);
        raster.apply_textures_delta(delta);
        assert!(raster.textures.is_empty());
    }

    #[test]
    fn bilinear_sampling_blends_neighbours_and_clamps() {
        let mut raster = Raster::new();
        let mut image = ColorImage::filled([2, 1], Color32::BLACK);
        image.pixels[1] = Color32::WHITE;
        let mut delta = TexturesDelta::default();
        delta.push(
            TextureId::default(),
            ImageDelta::full(image, TextureOptions::LINEAR),
        );
        raster.apply_textures_delta(delta);
        // Stretch the 2x1 texture over 8x1 pixels: left clamps to black, right to white, the
        // middle is a ramp.
        let mut mesh = Mesh::default();
        mesh.add_rect_with_uv(
            Rect::from_min_size(Pos2::ZERO, Vec2::new(8.0, 1.0)),
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        let prim = ClippedPrimitive {
            clip_rect: Rect::EVERYTHING,
            primitive: Primitive::Mesh(mesh),
        };
        let rgba = raster.render(&[prim], 1.0, 8, 1, Color32::RED);
        let ramp: Vec<u8> = (0..8).map(|x| pixel(&rgba, 8, x, 0)[0]).collect();
        assert_eq!(ramp[0], 0);
        assert_eq!(ramp[7], 255);
        assert!(ramp.windows(2).all(|w| w[0] <= w[1]), "{ramp:?}");
        assert!(
            ramp[3] > 0 && ramp[3] < 255 && ramp[4] > ramp[3],
            "{ramp:?}"
        );
    }

    #[test]
    fn png_round_trip() {
        let rgba: Vec<u8> = (0..5 * 3)
            .flat_map(|i| [i as u8, 255 - i as u8, 7, 255])
            .collect();
        let mut bytes = Vec::new();
        encode_png(&mut bytes, 5, 3, &rgba).unwrap();
        let mut reader = png::Decoder::new(io::Cursor::new(bytes))
            .read_info()
            .unwrap();
        let mut decoded = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut decoded).unwrap();
        assert_eq!((info.width, info.height), (5, 3));
        assert_eq!(&decoded[..info.buffer_size()], &rgba[..]);
        assert!(encode_png(&mut Vec::new(), 5, 3, &rgba[..10]).is_err());
    }

    fn demo_ui(ui: &mut egui::Ui) {
        egui::Panel::left("side").exact_size(60.0).show(ui, |ui| {
            ui.label("Side");
        });
        egui::CentralPanel::default().show(ui, |ui| {
            ui.colored_label(Color32::RED, "Hello world");
        });
    }

    fn read_png(path: &Path) -> (u32, u32, Vec<u8>) {
        let mut reader = png::Decoder::new(io::BufReader::new(File::open(path).unwrap()))
            .read_info()
            .unwrap();
        let mut rgba = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut rgba).unwrap();
        (info.width, info.height, rgba)
    }

    #[test]
    fn snapshot_rejects_nonsense_sizes() {
        let ctx = egui::Context::default();
        let path = std::env::temp_dir().join("egui-raster-never-written.png");
        for (size, ppp) in [
            (vec2(0.0, 10.0), 1.0),
            (vec2(10.0, f32::NAN), 1.0),
            (vec2(10.0, 10.0), 0.0),
            (vec2(10.0, 10.0), f32::INFINITY),
            (vec2(100_000.0, 10.0), 1.0),
        ] {
            let err = snapshot(&ctx, size, ppp, 2, |_| {}, &path).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "{size:?} at {ppp}");
        }
        assert!(!path.exists());
    }

    /// Real egui pipeline: frames, fonts, panels, text, written to a PNG.
    #[test]
    fn snapshot_renders_text_and_panels() {
        let ctx = egui::Context::default();
        ctx.set_theme(egui::Theme::Dark);
        let path =
            std::env::temp_dir().join(format!("egui-raster-test-{}.png", std::process::id()));
        let size = snapshot(&ctx, vec2(200.0, 100.0), 1.5, 3, demo_ui, &path).unwrap();
        assert_eq!(size, (300, 150));

        let (w, h, rgba) = read_png(&path);
        std::fs::remove_file(&path).ok();
        assert_eq!((w, h), (300, 150));
        assert!(rgba.as_chunks::<4>().0.iter().all(|px| px[3] == 255));

        // Central panel fill (dark theme: gray 27) in the bottom right corner...
        assert_eq!(pixel(&rgba, 300, 290, 140), [27, 27, 27, 255]);
        // ...and red text in the central panel: some pixels clearly red, none of them blue-ish.
        let red = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[0] > 150 && px[1] < 60 && px[2] < 60)
            .count();
        assert!(red > 20, "only {red} red text pixels");
        // The left panel (same fill) holds light text.
        let light = (0..150u32)
            .flat_map(|y| (0..90u32).map(move |x| (x, y)))
            .filter(|&(x, y)| pixel(&rgba, 300, x, y)[0] > 60)
            .count();
        assert!(light > 20, "only {light} light pixels in the side panel");
    }

    /// egui only sends *changes* to the font atlas, so a reused context needs its atlas re-read.
    #[test]
    fn snapshot_on_a_reused_context_gives_the_same_picture() {
        let ctx = egui::Context::default();
        let dir = std::env::temp_dir();
        let (first, second) = (
            dir.join(format!("egui-raster-test-{}-a.png", std::process::id())),
            dir.join(format!("egui-raster-test-{}-b.png", std::process::id())),
        );
        snapshot(&ctx, vec2(200.0, 100.0), 1.25, 2, demo_ui, &first).unwrap();
        snapshot(&ctx, vec2(200.0, 100.0), 1.25, 2, demo_ui, &second).unwrap();
        let (a, b) = (read_png(&first), read_png(&second));
        std::fs::remove_file(&first).ok();
        std::fs::remove_file(&second).ok();
        assert!(
            a.2.as_chunks::<4>()
                .0
                .iter()
                .any(|px| px[0] > 150 && px[1] < 60),
            "no red text in the first picture"
        );
        assert!(
            a == b,
            "second snapshot on the same context differs from the first"
        );
    }

    /// Tiny deterministic generator for the randomised tests.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 33
        }

        /// Uniform in `lo..=hi`.
        fn range(&mut self, lo: i64, hi: i64) -> i64 {
            lo + (self.next() % (hi - lo + 1) as u64) as i64
        }
    }

    fn vert(x: i64, y: i64, color: Color32) -> Vert {
        Vert {
            x,
            y,
            col: color_to_f32(color),
            tc: [0.0, 0.0],
        }
    }

    /// Straightforward per-pixel version of the coverage rule (exact integer edge functions,
    /// top-left tie break), to cross-check the span-based `draw_triangle`.
    fn covered_brute_force(v: [(i64, i64); 3], px: i64, py: i64) -> bool {
        let [a, b, c] = v;
        let area = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
        if area == 0 {
            return false;
        }
        let (b, c) = if area < 0 { (c, b) } else { (b, c) };
        let (cx, cy) = (px * FIX + HALF, py * FIX + HALF);
        [(b, c), (c, a), (a, b)].into_iter().all(|(p, q)| {
            let (dx, dy) = (q.0 - p.0, q.1 - p.1);
            let e = dx * (cy - p.1) - dy * (cx - p.0);
            let top_left = dy < 0 || (dy == 0 && dx > 0);
            e > 0 || (e == 0 && top_left)
        })
    }

    #[test]
    fn triangle_coverage_matches_brute_force() {
        let mut raster = Raster::new();
        white_texture(&mut raster, 4);
        let texture = &raster.textures[&TextureId::default()];
        let mut rng = Lcg(0x5eed);
        let (width, height) = (24_usize, 20_usize);
        for case in 0..4000 {
            // Vertices on a 1/16 pixel grid (so pixel centres often lie exactly on edges),
            // partly outside the target.
            let mut pts = [(0, 0); 3];
            for p in &mut pts {
                *p = (
                    rng.range(-4 * 16, 28 * 16) * 16,
                    rng.range(-4 * 16, 24 * 16) * 16,
                );
            }
            let (x0, y0) = (rng.range(0, 12), rng.range(0, 10));
            let clip = PixRect {
                x0,
                y0,
                x1: rng.range(x0, width as i64),
                y1: rng.range(y0, height as i64),
            };
            // Alternate between the one-colour fast path and interpolated colours.
            let colors = if case % 2 == 0 {
                [Color32::WHITE; 3]
            } else {
                [
                    Color32::WHITE,
                    Color32::from_rgb(255, 128, 0),
                    Color32::from_rgb(0, 128, 255),
                ]
            };
            let mut target = Target {
                buf: [0, 0, 0, 255].repeat(width * height),
                width,
                height,
            };
            let verts: Vec<Vert> = pts
                .iter()
                .zip(colors)
                .map(|(p, c)| vert(p.0, p.1, c))
                .collect();
            draw_triangle(&mut target, clip, texture, &verts[0], &verts[1], &verts[2]);
            for y in 0..height as i64 {
                for x in 0..width as i64 {
                    let in_clip =
                        (clip.x0..clip.x1).contains(&x) && (clip.y0..clip.y1).contains(&y);
                    let want = in_clip && covered_brute_force(pts, x, y);
                    let i = (y as usize * width + x as usize) * 4;
                    let lit = target.buf[i..i + 3] != [0, 0, 0];
                    assert_eq!(
                        lit,
                        want,
                        "case {case}: pixel ({x},{y}) of triangle {pts:?} clip {:?}",
                        (clip.x0, clip.y0, clip.x1, clip.y1)
                    );
                }
            }
        }
    }

    #[test]
    fn random_quads_have_no_seams_whatever_the_diagonal() {
        // A translucent convex quad, split into two triangles along either diagonal and with
        // either winding, must give identical images in which every pixel is either untouched
        // or blended exactly once (a gap would show the background, a double blend a brighter
        // pixel).
        let mut raster = Raster::new();
        white_texture(&mut raster, 4);
        let texture = &raster.textures[&TextureId::default()];
        let half_white = Color32::from_rgba_premultiplied(128, 128, 128, 128);
        let mut rng = Lcg(42);
        let (width, height) = (40_usize, 40_usize);
        let everything = PixRect {
            x0: 0,
            y0: 0,
            x1: width as i64,
            y1: height as i64,
        };
        for case in 0..400 {
            let (x0, y0) = (rng.range(4, 14), rng.range(4, 14));
            let (x1, y1) = (x0 + rng.range(8, 24), y0 + rng.range(8, 24));
            let jitter = |rng: &mut Lcg| rng.range(-2 * 16, 2 * 16) * 16; // up to 2 px, 1/16 px grid
            let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
                .map(|(x, y)| (x * FIX + jitter(&mut rng), y * FIX + jitter(&mut rng)));
            let splits: [[usize; 6]; 4] = [
                [0, 1, 2, 0, 2, 3],
                [0, 2, 1, 0, 3, 2],
                [0, 1, 3, 1, 2, 3],
                [0, 3, 1, 1, 3, 2],
            ];
            let mut images = Vec::new();
            for split in splits {
                let mut target = Target {
                    buf: [0, 0, 0, 255].repeat(width * height),
                    width,
                    height,
                };
                for tri in split.chunks(3) {
                    let [a, b, c] = [tri[0], tri[1], tri[2]]
                        .map(|i| vert(corners[i].0, corners[i].1, half_white));
                    draw_triangle(&mut target, everything, texture, &a, &b, &c);
                }
                images.push(target.buf);
            }
            for px in images[0].as_chunks::<4>().0 {
                assert!(
                    *px == [0, 0, 0, 255] || *px == [128, 128, 128, 255],
                    "case {case}: {px:?}"
                );
            }
            for image in &images[1..] {
                assert!(
                    *image == images[0],
                    "case {case}: the diagonal changes the covered pixels"
                );
            }
        }
    }

    #[test]
    fn one_to_one_linear_sampling_is_exact_even_with_fractional_ppp() {
        let (tw, th) = (16_usize, 12_usize);
        let mut rng = Lcg(7);
        let texels: Vec<Color32> = (0..tw * th)
            .map(|_| Color32::from_rgb(rng.next() as u8, rng.next() as u8, rng.next() as u8))
            .collect();
        let mut raster = Raster::new();
        let mut delta = TexturesDelta::default();
        let image = ColorImage::new([tw, th], texels.clone());
        delta.push(
            TextureId::default(),
            ImageDelta::full(image, TextureOptions::LINEAR),
        );
        raster.apply_textures_delta(delta);

        for ppp in [1.0_f32, 1.25, 1.5, 1.1, 2.0, 2.75] {
            // A quad of exactly tw x th pixels at pixel (5, 3). Its corners are given in points,
            // which are not exact in f32 (n / 1.25 ...).
            let (px, py) = (5.0_f32, 3.0_f32);
            let mut mesh = Mesh::default();
            mesh.add_rect_with_uv(
                Rect::from_min_max(
                    pos2(px / ppp, py / ppp),
                    pos2((px + tw as f32) / ppp, (py + th as f32) / ppp),
                ),
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
            let prim = ClippedPrimitive {
                clip_rect: Rect::EVERYTHING,
                primitive: Primitive::Mesh(mesh),
            };
            let rgba = raster.render(&[prim], ppp, 40, 30, Color32::BLACK);
            for y in 0..th {
                for x in 0..tw {
                    let [r, g, b, _] = texels[y * tw + x].to_array();
                    assert_eq!(
                        pixel(&rgba, 40, 5 + x as u32, 3 + y as u32),
                        [r, g, b, 255],
                        "ppp {ppp}, texel ({x},{y})"
                    );
                }
            }
            assert_eq!(pixel(&rgba, 40, 4, 3), [0, 0, 0, 255]);
            assert_eq!(pixel(&rgba, 40, 5 + tw as u32, 3), [0, 0, 0, 255]);
        }
    }

    #[test]
    fn nearest_filter_and_wrap_modes() {
        assert_eq!(
            (-4..7)
                .map(|i| wrap(i, 3, TextureWrapMode::ClampToEdge))
                .collect::<Vec<_>>(),
            [0, 0, 0, 0, 0, 1, 2, 2, 2, 2, 2]
        );
        assert_eq!(
            (-4..7)
                .map(|i| wrap(i, 3, TextureWrapMode::Repeat))
                .collect::<Vec<_>>(),
            [2, 0, 1, 2, 0, 1, 2, 0, 1, 2, 0]
        );
        assert_eq!(
            (-4..7)
                .map(|i| wrap(i, 3, TextureWrapMode::MirroredRepeat))
                .collect::<Vec<_>>(),
            [2, 2, 1, 0, 0, 1, 2, 2, 1, 0, 0]
        );

        // 2x1 black/white texture with NEAREST, stretched over 8x1 pixels: a hard edge in the middle.
        let mut raster = Raster::new();
        let mut image = ColorImage::filled([2, 1], Color32::BLACK);
        image.pixels[1] = Color32::WHITE;
        let mut delta = TexturesDelta::default();
        delta.push(
            TextureId::default(),
            ImageDelta::full(image, TextureOptions::NEAREST),
        );
        raster.apply_textures_delta(delta);
        let mut mesh = Mesh::default();
        mesh.add_rect_with_uv(
            Rect::from_min_size(Pos2::ZERO, Vec2::new(8.0, 1.0)),
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        let prim = ClippedPrimitive {
            clip_rect: Rect::EVERYTHING,
            primitive: Primitive::Mesh(mesh),
        };
        let rgba = raster.render(&[prim], 1.0, 8, 1, Color32::RED);
        let row: Vec<u8> = (0..8).map(|x| pixel(&rgba, 8, x, 0)[0]).collect();
        assert_eq!(row, [0, 0, 0, 0, 255, 255, 255, 255]);
    }
}
