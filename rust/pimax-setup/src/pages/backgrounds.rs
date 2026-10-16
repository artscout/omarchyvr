//! Backgrounds: the VR environment around you (the WayVR skybox; not the desktop wallpaper). A thumbnail grid by
//! theme, tap one to show it, a resolution, "Off / passthrough", and "Download more" (the downloader of
//! `pimax-backgrounds`, resumable). The choice is saved in `~/.config/pimax/background.toml`, shared with
//! "Max, set background ..." and applied at every Work start. Self-contained: its state lives here.
//!
//! The UI thread never touches a file (tests/e2e.rs checks it): a worker thread reads the library, the choice and
//! decodes the thumbnails; saving and applying a choice run on a thread too.
use crate::app::PimaxApp;
use crate::theme;
use crate::widgets::{self, caption, card, chip, note};
use eframe::egui::{self, Color32, CornerRadius, RichText, Sense, Stroke, StrokeKind, TextureHandle, Ui, Vec2};
use pimax_backgrounds::{fetch, Dirs, Entry, Res, Setting};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const TILE_W: f32 = 184.0;
const TILE_H: f32 = 92.0;

/// Progress of the downloader thread.
#[derive(Default, Clone)]
struct Dl {
    running: bool,
    done: usize,
    total: usize,
    failed: usize,
    last: String,
}

/// What the worker thread read.
#[derive(Default)]
struct Shared {
    lib: Vec<Entry>,
    setting: Setting,
    bytes: u64,
    /// Decoded thumbnails not yet uploaded as textures.
    images: Vec<(String, egui::ColorImage)>,
    /// Thumbnails already decoded (or tried).
    decoded: Vec<String>,
    loading: bool,
    loaded: bool,
}

struct State {
    dirs: Dirs,
    shared: Arc<Mutex<Shared>>,
    lib: Vec<Entry>,
    setting: Setting,
    /// A choice made here: the worker's copy of the setting is stale until then.
    pending_until: Option<Instant>,
    asked: Option<Instant>,
    tex: HashMap<String, TextureHandle>,
    dl: Arc<Mutex<Dl>>,
    msg: String,
    bytes: u64,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(State {
            dirs: Dirs::from_env(),
            shared: Arc::new(Mutex::new(Shared::default())),
            lib: Vec::new(),
            setting: Setting::default(),
            pending_until: None,
            asked: None,
            tex: HashMap::new(),
            dl: Arc::new(Mutex::new(Dl::default())),
            msg: String::new(),
            bytes: 0,
        })
    })
}

/// "city" -> "City night".
pub fn theme_title(t: &str) -> String {
    match t {
        "city" => "City night".into(),
        "night" => "Night sky".into(),
        t => {
            let mut c = t.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }
    }
}

fn load_png(path: &std::path::Path) -> Option<egui::ColorImage> {
    let f = std::fs::File::open(path).ok()?;
    let mut dec = png::Decoder::new(std::io::BufReader::new(f));
    dec.set_transformations(png::Transformations::EXPAND);
    let mut r = dec.read_info().ok()?;
    let mut buf = vec![0u8; r.output_buffer_size()?];
    let info = r.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width as usize, info.height as usize);
    let data = &buf[..info.buffer_size()];
    match info.color_type {
        png::ColorType::Rgb => Some(egui::ColorImage::from_rgb([w, h], data)),
        png::ColorType::Rgba => Some(egui::ColorImage::from_rgba_unmultiplied([w, h], data)),
        png::ColorType::Grayscale => Some(egui::ColorImage::from_gray([w, h], data)),
        _ => None,
    }
}

/// Ask the worker thread to read the library, the choice and the new thumbnails (one at a time).
fn reload(s: &mut State) {
    s.asked = Some(Instant::now());
    {
        let mut sh = s.shared.lock().unwrap_or_else(|e| e.into_inner());
        if sh.loading {
            return;
        }
        sh.loading = true;
    }
    let (dirs, shared) = (s.dirs.clone(), s.shared.clone());
    std::thread::spawn(move || read_all(&dirs, &shared));
}

/// The worker's job: library, choice, size, and the thumbnails not decoded yet.
fn read_all(dirs: &Dirs, shared: &Mutex<Shared>) {
    let lib = pimax_backgrounds::library(dirs);
    let setting = Setting::load(dirs);
    let bytes = fetch::library_bytes(&dirs.library);
    let have: Vec<String> = shared.lock().map(|sh| sh.decoded.clone()).unwrap_or_default();
    let mut new = Vec::new();
    for e in lib.iter().filter(|e| !have.contains(&e.id)) {
        if let Some(img) = pimax_backgrounds::thumb_of(dirs, e).and_then(|p| load_png(&p)) {
            new.push((e.id.clone(), img));
        }
    }
    if let Ok(mut sh) = shared.lock() {
        sh.decoded.extend(new.iter().map(|(id, _)| id.clone()));
        sh.images.extend(new);
        sh.lib = lib;
        sh.setting = setting;
        sh.bytes = bytes;
        sh.loading = false;
        sh.loaded = true;
    }
}

/// Read everything now, before the first frame (the screenshots, which draw a few frames only).
pub fn preload() {
    let (dirs, shared) = {
        let s = state().lock().unwrap_or_else(|e| e.into_inner());
        (s.dirs.clone(), s.shared.clone())
    };
    read_all(&dirs, &shared);
}

/// Save the choice and show it in the running session (on a thread; a failure lands in the status line).
fn choose(app: &PimaxApp, s: &mut State, f: impl FnOnce(&mut Setting)) {
    f(&mut s.setting);
    s.pending_until = Some(Instant::now() + Duration::from_secs(4));
    if app.core.demo {
        return;
    }
    let (dirs, setting) = (s.dirs.clone(), s.setting.clone());
    let live = app.core.live.alive("wayvr").then(|| app.core.wayvr_paths.ctl.clone());
    std::thread::spawn(move || {
        let r = setting.save(&dirs).map_err(|e| format!("cannot save: {e}")).and_then(|_| match live {
            Some(ctl) => pimax_backgrounds::apply(&dirs, &ctl).map(|_| ()),
            None => Ok(()),
        });
        if let Ok(mut st) = state().lock() {
            st.msg = r.err().unwrap_or_default();
            st.asked = None;
        }
    });
}

fn start_download(s: &mut State) {
    let dl = s.dl.clone();
    if dl.lock().map(|d| d.running).unwrap_or(true) {
        return;
    }
    if let Ok(mut d) = dl.lock() {
        *d = Dl { running: true, ..Dl::default() };
    }
    let dirs = s.dirs.clone();
    std::thread::spawn(move || {
        let r = fetch::run(&dirs, &fetch::Options::default(), &mut |p| {
            if let Ok(mut d) = dl.lock() {
                d.done = p.done;
                d.total = p.total;
                if p.status.starts_with("failed") {
                    d.failed += 1;
                }
                d.last = format!("{} {}", p.id, p.status);
            }
        });
        if let Ok(mut d) = dl.lock() {
            d.running = false;
            if let Err(e) = r {
                d.last = e;
            }
        }
    });
}

pub fn show(ui: &mut Ui, app: &mut PimaxApp) {
    let mut guard = state().lock().unwrap_or_else(|e| e.into_inner());
    let s = &mut *guard;
    let dl = s.dl.lock().map(|d| d.clone()).unwrap_or_default();
    // the worker reads again: first time, every 2 s while downloading, every 3 s otherwise (voice may change the choice)
    let every = if dl.running { 2 } else { 3 };
    if s.asked.is_none_or(|t| t.elapsed() > Duration::from_secs(every)) {
        reload(s);
    }
    ui.ctx().request_repaint_after(Duration::from_millis(if s.tex.len() < s.lib.len() || s.lib.is_empty() { 300 } else { every * 1000 }));
    {
        let mut sh = s.shared.lock().unwrap_or_else(|e| e.into_inner());
        if sh.loaded {
            s.lib = sh.lib.clone();
            s.bytes = sh.bytes;
            if s.pending_until.is_none_or(|t| Instant::now() > t) {
                s.setting = sh.setting.clone();
            }
        }
        let ctx = ui.ctx().clone();
        for (id, img) in sh.images.drain(..) {
            let tx = ctx.load_texture(format!("bg-{id}"), img, egui::TextureOptions::LINEAR);
            s.tex.insert(id, tx);
        }
    }
    let p = theme::pal(ui);
    card(ui, |ui| {
        ui.horizontal(|ui| {
            widgets::title(ui, "Backgrounds");
            if app.core.demo {
                chip(ui, "demo", widgets::pal(ui).info);
            }
            let cur = if s.setting.enabled { s.lib.iter().find(|e| e.id == s.setting.id).map(|e| e.title.clone()) } else { None };
            chip(ui, cur.as_deref().unwrap_or("Off"), if cur.is_some() { widgets::pal(ui).ok } else { p.faint });
        });
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Resolution").color(p.dim));
            for r in Res::ALL {
                let on = s.setting.res == r;
                let resp = ui.selectable_label(on, r.label());
                resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, on, format!("Resolution {}", r.label())));
                if resp.clicked() && !on {
                    choose(app, s, |st| st.res = r);
                }
            }
            ui.add_space(14.0);
            let can = !dl.running && !app.core.demo;
            if widgets::ibutton(ui, theme::icon::ARROWS_CLOCKWISE, "Download more", can).clicked() {
                start_download(s);
            }
            if dl.running {
                widgets::spinner(ui);
                ui.label(RichText::new(format!("{}/{}", dl.done, dl.total.max(1))).color(p.dim));
            } else if dl.total > 0 {
                let t = if dl.failed > 0 { format!("{} failed", dl.failed) } else { "Done".into() };
                chip(ui, &t, if dl.failed > 0 { widgets::pal(ui).warn } else { widgets::pal(ui).ok });
            }
        });
        if dl.running {
            ui.add(egui::ProgressBar::new(dl.done as f32 / dl.total.max(1) as f32).desired_height(6.0));
        }
        if !dl.last.is_empty() && (dl.running || dl.failed > 0) {
            ui.label(RichText::new(&dl.last).size(11.5).color(p.faint));
        }
        if !s.msg.is_empty() {
            ui.label(RichText::new(&s.msg).size(12.0).color(widgets::pal(ui).warn));
        }
    });
    ui.add_space(8.0);
    card(ui, |ui| {
        caption(ui, "Room");
        ui.add_space(4.0);
        let on = !s.setting.enabled;
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(TILE_W, TILE_H), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, "Off / passthrough"));
        ui.painter().rect_filled(rect, CornerRadius::same(6), p.inset);
        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "Off / passthrough", egui::FontId::proportional(14.0), p.text);
        outline(ui, rect, on, resp.hovered(), p.accent);
        if resp.clicked() && !on {
            choose(app, s, |st| st.enabled = false);
        }
    });
    if s.lib.is_empty() {
        ui.add_space(8.0);
        card(ui, |ui| {
            note(ui, "No pictures yet.");
        });
        return;
    }
    let cur_id = if s.setting.enabled { s.setting.id.clone() } else { String::new() };
    let mut themes: Vec<String> = Vec::new();
    for e in &s.lib {
        if !themes.contains(&e.theme) {
            themes.push(e.theme.clone());
        }
    }
    let cols = (((ui.available_width() - 32.0) / (TILE_W + 10.0)).floor() as usize).max(1);
    let mut pick: Option<String> = None;
    for t in &themes {
        ui.add_space(8.0);
        card(ui, |ui| {
            caption(ui, &theme_title(t));
            ui.add_space(4.0);
            let items: Vec<&Entry> = s.lib.iter().filter(|e| &e.theme == t).collect();
            for row in items.chunks(cols) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    for e in row {
                        ui.vertical(|ui| {
                            let (rect, resp) = ui.allocate_exact_size(Vec2::new(TILE_W, TILE_H), Sense::click());
                            let on = e.id == cur_id;
                            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, e.title.clone()));
                            match s.tex.get(&e.id) {
                                Some(tx) => {
                                    egui::Image::new(tx).corner_radius(CornerRadius::same(6)).paint_at(ui, rect);
                                }
                                None => {
                                    ui.painter().rect_filled(rect, CornerRadius::same(6), p.inset);
                                }
                            }
                            outline(ui, rect, on, resp.hovered(), p.accent);
                            let res: Vec<&str> = e.files.iter().map(|f| f.res.as_str()).collect();
                            let resp = resp.on_hover_text(format!("{}\n{} · {}\n{}", e.title, e.author, e.licence, res.join(" ").to_uppercase()));
                            ui.add(egui::Label::new(RichText::new(&e.title).size(11.5).color(if on { p.text } else { p.dim })).truncate());
                            if resp.clicked() {
                                pick = Some(e.id.clone());
                            }
                        });
                    }
                });
                ui.add_space(4.0);
            }
        });
    }
    if let Some(id) = pick {
        choose(app, s, |st| {
            st.enabled = true;
            st.id = id;
        });
    }
    ui.add_space(8.0);
    ui.label(RichText::new(format!("{} pictures · {:.1} GB", s.lib.len(), s.bytes as f64 / 1e9)).size(11.5).color(p.faint));
}

fn outline(ui: &Ui, rect: egui::Rect, on: bool, hover: bool, accent: Color32) {
    if on {
        ui.painter().rect_stroke(rect, CornerRadius::same(6), Stroke::new(2.5, accent), StrokeKind::Outside);
    } else if hover {
        ui.painter().rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.5, accent.gamma_multiply(0.6)), StrokeKind::Outside);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_names() {
        assert_eq!(theme_title("city"), "City night");
        assert_eq!(theme_title("night"), "Night sky");
        assert_eq!(theme_title("flowers"), "Flowers");
    }
}
