//! The app's line art: the SVG files in `assets/art/` (drawn for this project, CC-BY-4.0), compiled in and drawn as
//! textures. No new dependency: the small SVG subset these files use is read here, the path data goes through
//! kurbo's SVG path parser and egui's own CPU rasteriser (vello_cpu, which egui already uses for its glyphs), and the
//! result is cached per size and colours.
//!
//! The subset: `<svg viewBox>`, `<g>` (inherits its style and `transform`), `<path d>`, `<circle>`, `<ellipse>`,
//! `<rect>` (with `rx`), `<line>`, `<polyline>`, `<polygon>`; the attributes `stroke`, `stroke-width`,
//! `stroke-opacity`, `stroke-linecap`, `stroke-linejoin`, `fill`, `fill-opacity`, `opacity` and `transform`
//! (`matrix`, `translate`, `scale`, `rotate`). Colours: `currentColor` is the accent, [`NEUTRAL`] is the theme's
//! neutral line colour, other `#rgb` / `#rrggbb` colours are kept as they are.
use eframe::egui::{self, Color32, ColorImage, Rect, TextureHandle, TextureOptions, Ui};
use std::collections::HashMap;
use std::sync::OnceLock;
use vello_cpu::color::{AlphaColor, Srgb};
use vello_cpu::kurbo::{self, Affine, BezPath, Shape};

/// The colour the SVG files use for neutral lines (replaced by the theme's).
pub const NEUTRAL: (u8, u8, u8) = (0x9a, 0xa1, 0xb5);

/// One drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Art {
    /// The headset, front view from a little above.
    Headset,
    /// The left controller.
    ControllerLeft,
    /// The right controller.
    ControllerRight,
    /// The app's mark (a small goggle).
    Mark,
}

impl Art {
    /// Every drawing.
    pub const ALL: [Art; 4] = [Art::Headset, Art::ControllerLeft, Art::ControllerRight, Art::Mark];

    /// The SVG source.
    pub fn svg(self) -> &'static str {
        match self {
            Art::Headset => include_str!("../assets/art/headset.svg"),
            Art::ControllerLeft => include_str!("../assets/art/controller-left.svg"),
            Art::ControllerRight => include_str!("../assets/art/controller-right.svg"),
            Art::Mark => include_str!("../assets/art/mark.svg"),
        }
    }

    fn index(self) -> usize {
        Art::ALL.iter().position(|a| *a == self).unwrap_or(0)
    }

    /// Width / height of the drawing.
    pub fn aspect(self) -> f32 {
        let vb = scene(self).view_box;
        if vb.height() > 0.0 {
            (vb.width() / vb.height()) as f32
        } else {
            1.0
        }
    }
}

/// A paint of the SVG.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Paint {
    None,
    Current,
    Rgb(u8, u8, u8),
}

/// The presentation attributes in effect.
#[derive(Debug, Clone, Copy)]
struct Style {
    stroke: Paint,
    stroke_width: f64,
    stroke_opacity: f64,
    fill: Paint,
    fill_opacity: f64,
    opacity: f64,
    cap: kurbo::Cap,
    join: kurbo::Join,
    transform: Affine,
}

impl Default for Style {
    fn default() -> Self {
        // the SVG defaults: black fill, no stroke
        Style {
            stroke: Paint::None,
            stroke_width: 1.0,
            stroke_opacity: 1.0,
            fill: Paint::Rgb(0, 0, 0),
            fill_opacity: 1.0,
            opacity: 1.0,
            cap: kurbo::Cap::Butt,
            join: kurbo::Join::Miter,
            transform: Affine::IDENTITY,
        }
    }
}

/// One shape, ready to draw.
#[derive(Debug, Clone)]
struct Item {
    path: BezPath,
    style: Style,
}

/// A parsed drawing.
#[derive(Debug, Clone)]
pub struct Scene {
    /// The `viewBox`.
    pub view_box: kurbo::Rect,
    items: Vec<Item>,
}

impl Scene {
    /// Number of shapes.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// No shapes?
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

fn numbers(s: &str) -> Vec<f64> {
    s.split(|c: char| c.is_whitespace() || c == ',').filter(|t| !t.is_empty()).filter_map(|t| t.parse().ok()).collect()
}

fn parse_paint(v: &str) -> Result<Paint, String> {
    let v = v.trim();
    if v == "none" {
        return Ok(Paint::None);
    }
    if v == "currentColor" {
        return Ok(Paint::Current);
    }
    let hex = v.strip_prefix('#').ok_or_else(|| format!("unsupported colour {v:?}"))?;
    let d = |i: usize, n: usize| u8::from_str_radix(&hex[i..i + n], 16).map_err(|e| format!("colour {v:?}: {e}"));
    match hex.len() {
        3 => Ok(Paint::Rgb(d(0, 1)? * 17, d(1, 1)? * 17, d(2, 1)? * 17)),
        6 => Ok(Paint::Rgb(d(0, 2)?, d(2, 2)?, d(4, 2)?)),
        _ => Err(format!("unsupported colour {v:?}")),
    }
}

fn transform(v: &str) -> Result<Affine, String> {
    let mut t = Affine::IDENTITY;
    let mut rest = v.trim();
    while !rest.is_empty() {
        let open = rest.find('(').ok_or_else(|| format!("bad transform {v:?}"))?;
        let close = rest.find(')').ok_or_else(|| format!("bad transform {v:?}"))?;
        let name = rest[..open].trim().trim_start_matches(',').trim();
        let a = numbers(&rest[open + 1..close]);
        let get = |i: usize| a.get(i).copied();
        let one = match (name, a.len()) {
            ("matrix", 6) => Affine::new([a[0], a[1], a[2], a[3], a[4], a[5]]),
            ("translate", 1 | 2) => Affine::translate((a[0], get(1).unwrap_or(0.0))),
            ("scale", 1 | 2) => Affine::scale_non_uniform(a[0], get(1).unwrap_or(a[0])),
            ("rotate", 1) => Affine::rotate(a[0].to_radians()),
            ("rotate", 3) => Affine::translate((a[1], a[2])) * Affine::rotate(a[0].to_radians()) * Affine::translate((-a[1], -a[2])),
            _ => return Err(format!("unsupported transform {name}({})", &rest[open + 1..close])),
        };
        t *= one;
        rest = rest[close + 1..].trim();
    }
    Ok(t)
}

/// The attributes of one tag (`name="value"` or `name='value'`).
fn attributes(s: &str) -> Result<Vec<(&str, &str)>, String> {
    let mut out = Vec::new();
    let mut rest = s.trim();
    while !rest.is_empty() {
        let eq = rest.find('=').ok_or_else(|| format!("attribute without a value in {s:?}"))?;
        let name = rest[..eq].trim();
        let after = rest[eq + 1..].trim_start();
        let quote = after.chars().next().filter(|c| *c == '"' || *c == '\'').ok_or_else(|| format!("unquoted value of {name}"))?;
        let end = after[1..].find(quote).ok_or_else(|| format!("unterminated value of {name}"))?;
        out.push((name, &after[1..1 + end]));
        rest = after[end + 2..].trim_start();
    }
    Ok(out)
}

fn style_of(parent: &Style, attrs: &[(&str, &str)]) -> Result<Style, String> {
    let mut s = *parent;
    let num = |v: &str| v.trim().parse::<f64>().map_err(|e| format!("number {v:?}: {e}"));
    for (k, v) in attrs {
        match *k {
            "stroke" => s.stroke = parse_paint(v)?,
            "stroke-width" => s.stroke_width = num(v)?,
            "stroke-opacity" => s.stroke_opacity = num(v)?,
            "fill" => s.fill = parse_paint(v)?,
            "fill-opacity" => s.fill_opacity = num(v)?,
            "opacity" => s.opacity *= num(v)?,
            "stroke-linecap" => {
                s.cap = match *v {
                    "round" => kurbo::Cap::Round,
                    "square" => kurbo::Cap::Square,
                    _ => kurbo::Cap::Butt,
                }
            }
            "stroke-linejoin" => {
                s.join = match *v {
                    "round" => kurbo::Join::Round,
                    "bevel" => kurbo::Join::Bevel,
                    _ => kurbo::Join::Miter,
                }
            }
            "transform" => s.transform = parent.transform * transform(v)?,
            _ => {}
        }
    }
    Ok(s)
}

fn shape(name: &str, attrs: &[(&str, &str)]) -> Result<Option<BezPath>, String> {
    let get = |k: &str| attrs.iter().find(|(n, _)| *n == k).map(|(_, v)| *v);
    let f = |k: &str| -> Result<f64, String> {
        match get(k) {
            Some(v) => v.trim().parse::<f64>().map_err(|e| format!("{name} {k}={v:?}: {e}")),
            None => Ok(0.0),
        }
    };
    const TOL: f64 = 0.05;
    let p = match name {
        "path" => BezPath::from_svg(get("d").unwrap_or("")).map_err(|e| format!("path data: {e}"))?,
        "circle" => kurbo::Circle::new((f("cx")?, f("cy")?), f("r")?).to_path(TOL),
        "ellipse" => kurbo::Ellipse::new((f("cx")?, f("cy")?), (f("rx")?, f("ry")?), 0.0).to_path(TOL),
        "rect" => {
            let r = kurbo::Rect::new(f("x")?, f("y")?, f("x")? + f("width")?, f("y")? + f("height")?);
            let rx = if get("rx").is_some() { f("rx")? } else { f("ry")? };
            kurbo::RoundedRect::from_rect(r, rx).to_path(TOL)
        }
        "line" => {
            let mut p = BezPath::new();
            p.move_to((f("x1")?, f("y1")?));
            p.line_to((f("x2")?, f("y2")?));
            p
        }
        "polyline" | "polygon" => {
            let n = numbers(get("points").unwrap_or(""));
            let mut p = BezPath::new();
            for (i, xy) in n.as_chunks::<2>().0.iter().enumerate() {
                if i == 0 {
                    p.move_to((xy[0], xy[1]));
                } else {
                    p.line_to((xy[0], xy[1]));
                }
            }
            if name == "polygon" {
                p.close_path();
            }
            p
        }
        _ => return Ok(None),
    };
    Ok(Some(p))
}

/// Read a drawing.
pub fn parse(svg: &str) -> Result<Scene, String> {
    let mut stack: Vec<Style> = vec![Style::default()];
    let mut view_box = None;
    let mut items = Vec::new();
    let mut rest = svg;
    while let Some(lt) = rest.find('<') {
        rest = &rest[lt..];
        if let Some(r) = rest.strip_prefix("<!--") {
            let end = r.find("-->").ok_or("unterminated comment")?;
            rest = &r[end + 3..];
            continue;
        }
        let gt = rest.find('>').ok_or("unterminated tag")?;
        let tag = &rest[1..gt];
        rest = &rest[gt + 1..];
        if tag.starts_with('?') || tag.starts_with('!') {
            continue;
        }
        if let Some(name) = tag.strip_prefix('/') {
            if matches!(name.trim(), "g" | "svg") && stack.len() > 1 {
                stack.pop();
            }
            continue;
        }
        let self_closing = tag.ends_with('/');
        let tag = tag.trim_end_matches('/');
        let (name, attrs) = match tag.find(char::is_whitespace) {
            Some(i) => (&tag[..i], attributes(&tag[i..])?),
            None => (tag, Vec::new()),
        };
        let parent = *stack.last().unwrap_or(&Style::default());
        let style = style_of(&parent, &attrs)?;
        match name {
            "svg" | "g" => {
                if name == "svg" {
                    if let Some((_, v)) = attrs.iter().find(|(k, _)| *k == "viewBox") {
                        let n = numbers(v);
                        if n.len() == 4 {
                            view_box = Some(kurbo::Rect::new(n[0], n[1], n[0] + n[2], n[1] + n[3]));
                        }
                    }
                }
                if !self_closing {
                    stack.push(style);
                }
            }
            _ => {
                if let Some(path) = shape(name, &attrs)? {
                    items.push(Item { path, style });
                }
            }
        }
    }
    let view_box = view_box.ok_or("no viewBox")?;
    Ok(Scene { view_box, items })
}

/// The parsed drawing (parsed once; a file that does not parse draws nothing and is logged).
pub fn scene(a: Art) -> &'static Scene {
    static SCENES: OnceLock<Vec<Scene>> = OnceLock::new();
    let all = SCENES.get_or_init(|| {
        Art::ALL
            .iter()
            .map(|a| {
                parse(a.svg()).unwrap_or_else(|e| {
                    log::warn!("art {a:?}: {e}");
                    Scene { view_box: kurbo::Rect::new(0.0, 0.0, 1.0, 1.0), items: Vec::new() }
                })
            })
            .collect()
    });
    &all[a.index()]
}

fn color(p: Paint, alpha: f64, accent: Color32, neutral: Color32) -> Option<AlphaColor<Srgb>> {
    let c = match p {
        Paint::None => return None,
        Paint::Current => accent,
        Paint::Rgb(r, g, b) if (r, g, b) == NEUTRAL => neutral,
        Paint::Rgb(r, g, b) => Color32::from_rgb(r, g, b),
    };
    let a = (f64::from(c.a()) * alpha.clamp(0.0, 1.0)).round() as u8;
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    Some(AlphaColor::from_rgba8(r, g, b, a))
}

/// Rasterise a drawing into `w` x `h` pixels (fitted and centred), premultiplied.
pub fn rasterize(a: Art, w: u16, h: u16, accent: Color32, neutral: Color32) -> ColorImage {
    let s = scene(a);
    let (w, h) = (w.max(1), h.max(1));
    let vb = s.view_box;
    let k = (f64::from(w) / vb.width()).min(f64::from(h) / vb.height());
    let base = Affine::translate(((f64::from(w) - vb.width() * k) / 2.0, (f64::from(h) - vb.height() * k) / 2.0)) * Affine::scale(k) * Affine::translate((-vb.x0, -vb.y0));
    let mut ctx = vello_cpu::RenderContext::new(w, h);
    for it in &s.items {
        let st = &it.style;
        ctx.set_transform(base * st.transform);
        if let Some(c) = color(st.fill, st.fill_opacity * st.opacity, accent, neutral) {
            ctx.set_paint(c);
            ctx.fill_path(&it.path);
        }
        if let Some(c) = color(st.stroke, st.stroke_opacity * st.opacity, accent, neutral) {
            ctx.set_stroke(kurbo::Stroke::new(st.stroke_width).with_caps(st.cap).with_join(st.join));
            ctx.set_paint(c);
            ctx.stroke_path(&it.path);
        }
    }
    let mut pm = vello_cpu::Pixmap::new(w, h);
    ctx.render(&mut pm, &mut vello_cpu::Resources::new());
    ColorImage::from_rgba_premultiplied([usize::from(w), usize::from(h)], pm.data_as_u8_slice())
}

type Key = (Art, u32, u32, [u8; 4], [u8; 4]);

/// Textures already made (a few sizes per drawing: the window was resized, or the theme changed).
#[derive(Clone, Default)]
struct Cache(HashMap<Key, TextureHandle>);

/// Draw `a` into `rect` (fitted, centred, aspect kept): `accent` for `currentColor`, `neutral` for [`NEUTRAL`].
pub fn paint(ui: &Ui, a: Art, rect: Rect, accent: Color32, neutral: Color32) {
    if !ui.is_rect_visible(rect) || rect.width() < 1.0 || rect.height() < 1.0 {
        return;
    }
    let ppp = ui.ctx().pixels_per_point();
    // the drawing's own box inside rect, whole pixels
    let asp = a.aspect();
    let (mut w, mut h) = (rect.width(), rect.width() / asp);
    if h > rect.height() {
        h = rect.height();
        w = h * asp;
    }
    let wp = (w * ppp).round().clamp(1.0, 2048.0) as u32;
    let hp = (h * ppp).round().clamp(1.0, 2048.0) as u32;
    let size = egui::vec2(wp as f32, hp as f32) / ppp;
    let min = egui::pos2(((rect.center().x - size.x / 2.0) * ppp).round() / ppp, ((rect.center().y - size.y / 2.0) * ppp).round() / ppp);
    let key: Key = (a, wp, hp, accent.to_array(), neutral.to_array());
    let id = egui::Id::new("pimax-art-cache");
    let ctx = ui.ctx().clone();
    // look up and insert in two steps: the context is locked inside data_mut, and load_texture locks it too
    let cached = ctx.data_mut(|d| d.get_temp_mut_or_default::<Cache>(id).0.get(&key).cloned());
    let tex = match cached {
        Some(t) => t,
        None => {
            let img = rasterize(a, wp as u16, hp as u16, accent, neutral);
            let t = ctx.load_texture(format!("art-{a:?}-{wp}x{hp}"), img, TextureOptions::LINEAR);
            ctx.data_mut(|d| {
                let cache = d.get_temp_mut_or_default::<Cache>(id);
                if cache.0.len() > 24 {
                    cache.0.clear();
                }
                cache.0.insert(key, t.clone());
            });
            t
        }
    };
    ui.painter().image(tex.id(), Rect::from_min_size(min, size), Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
}

/// Allocate `size` and draw `a` there.
pub fn show(ui: &mut Ui, a: Art, size: egui::Vec2, accent: Color32, neutral: Color32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    paint(ui, a, rect, accent, neutral);
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_drawing_parses_and_draws_lines() {
        for a in Art::ALL {
            let s = parse(a.svg()).unwrap_or_else(|e| panic!("{a:?}: {e}"));
            assert!(s.len() >= 2, "{a:?} has {} shapes", s.len());
            assert!(s.view_box.width() > 0.0 && s.view_box.height() > 0.0);
            let img = rasterize(a, 120, 80, Color32::from_rgb(74, 92, 255), Color32::from_rgb(160, 160, 170));
            assert_eq!(img.size, [120, 80]);
            // line art: some pixels drawn, most of the box left empty
            let drawn = img.pixels.iter().filter(|p| p.a() > 40).count();
            assert!(drawn > 30, "{a:?}: only {drawn} pixels drawn");
            assert!(drawn < 120 * 80 / 2, "{a:?}: {drawn} pixels drawn: not line art");
        }
    }

    #[test]
    fn the_right_controller_mirrors_the_left_one() {
        let l = rasterize(Art::ControllerLeft, 50, 80, Color32::WHITE, Color32::WHITE);
        let r = rasterize(Art::ControllerRight, 50, 80, Color32::WHITE, Color32::WHITE);
        let mut diff = 0u64;
        for y in 0..80 {
            for x in 0..50 {
                let a = l.pixels[y * 50 + x].a();
                let b = r.pixels[y * 50 + (49 - x)].a();
                diff += u64::from(a.abs_diff(b));
            }
        }
        let ink: u64 = l.pixels.iter().map(|p| u64::from(p.a())).sum();
        assert!(diff * 10 < ink, "mirror differs by {diff} of {ink}");
    }

    #[test]
    fn painting_makes_one_texture_per_size() {
        // a frame draws the art twice at the same size (one texture) and once larger (a second one); no deadlock on
        // the context's lock while the texture is made
        let ctx = egui::Context::default();
        for _ in 0..2 {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                show(ui, Art::Headset, egui::vec2(120.0, 66.0), Color32::WHITE, Color32::GRAY);
                show(ui, Art::Headset, egui::vec2(120.0, 66.0), Color32::WHITE, Color32::GRAY);
                show(ui, Art::Headset, egui::vec2(240.0, 132.0), Color32::WHITE, Color32::GRAY);
            });
            // no renderer here: the new textures are not uploaded anywhere
            out.textures_delta.clear();
        }
        let n = ctx.data_mut(|d| d.get_temp_mut_or_default::<Cache>(egui::Id::new("pimax-art-cache")).0.len());
        assert_eq!(n, 2);
    }

    #[test]
    fn colours_and_transforms() {
        assert_eq!(parse_paint("#9aa1b5").unwrap(), Paint::Rgb(0x9a, 0xa1, 0xb5));
        assert_eq!(parse_paint("#fff").unwrap(), Paint::Rgb(255, 255, 255));
        assert_eq!(parse_paint("currentColor").unwrap(), Paint::Current);
        assert!(parse_paint("red").is_err());
        let t = transform("matrix(-1 0 0 1 100 0)").unwrap();
        assert_eq!(t * kurbo::Point::new(10.0, 5.0), kurbo::Point::new(90.0, 5.0));
        let t = transform("translate(10, 20) scale(2)").unwrap();
        assert_eq!(t * kurbo::Point::new(1.0, 1.0), kurbo::Point::new(12.0, 22.0));
        assert!(transform("skewX(10)").is_err());
        assert!(parse("<svg><path d=\"M 0 0 L 1 1\"/></svg>").is_err(), "no viewBox");
    }
}
