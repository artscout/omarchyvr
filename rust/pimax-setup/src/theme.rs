//! The look of the app, after Pimax Play: near-black glassy surfaces, rounded cards, an indigo accent (#4a5cff),
//! uppercase row titles, outline icons (Phosphor), pills whose colour fades, eased expand/collapse. Visual only.
//!
//! Dark is the default; a light palette exists for people who switch the desktop to light.
use eframe::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, Id, Margin, Stroke, Ui};
use std::sync::Arc;

// Phosphor icons (regular weight), as text glyphs: only the ones the app shows are compiled in (a few KB).
egui_phosphor::subset! {
    pub mod icons {
        use regular::{
            ANCHOR, ARROW_COUNTER_CLOCKWISE, ARROW_SQUARE_OUT, ARROWS_CLOCKWISE, BATTERY_EMPTY, BATTERY_HIGH, BATTERY_LOW,
            BATTERY_MEDIUM, BOOK_OPEN, BRIEFCASE, BUG, CARET_DOWN, CHECK_CIRCLE, CIRCLE, CODE, COPY, CROSSHAIR, CUBE_FOCUS,
            DESKTOP, EYE, EYE_SLASH, FILE_TEXT, FILM_SLATE, FOLDER_OPEN, GAME_CONTROLLER, GEAR, GOGGLES, HAND, HEADSET,
            HOUSE, INFO, LIGHTNING, LINK_SIMPLE, LIST, MAGIC_WAND, MAP_TRIFOLD, MICROPHONE, MONITOR, PLAY, POPCORN, POWER, PULSE,
            QR_CODE, QUESTION, ROBOT, SCAN, SLIDERS, SPARKLE, SQUARES_FOUR, STEAM_LOGO, STOP, SUN, TERMINAL, USERS,
            VIDEO_CAMERA, WARNING, WRENCH, X_CIRCLE,
        };
    }
}

/// The icons, as text glyphs (`icon::GOGGLES`).
pub use icons::regular as icon;

/// Colours of one theme.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    /// Dark theme?
    pub dark: bool,
    /// Window background.
    pub bg: Color32,
    /// Top and bottom bars.
    pub bar: Color32,
    /// Cards and rows.
    pub card: Color32,
    /// A hovered row.
    pub card_hover: Color32,
    /// An expanded row's body and inner boxes.
    pub inset: Color32,
    /// Hairlines around cards.
    pub stroke: Color32,
    /// Stronger lines (focus, separators).
    pub stroke_strong: Color32,
    /// Main text.
    pub text: Color32,
    /// Secondary text (row summaries).
    pub dim: Color32,
    /// Faint text (captions, disabled).
    pub faint: Color32,
    /// The accent (buttons, icons, selection).
    pub accent: Color32,
    /// Accent under the pointer.
    pub accent_hover: Color32,
    /// Good / connected.
    pub ok: Color32,
    /// Needs attention.
    pub warn: Color32,
    /// Failed / blocked / stop.
    pub fail: Color32,
    /// Information.
    pub info: Color32,
}

/// The dark palette (default).
pub const DARK: Palette = Palette {
    dark: true,
    bg: Color32::from_rgb(10, 11, 15),
    bar: Color32::from_rgb(13, 15, 20),
    card: Color32::from_rgb(21, 23, 30),
    card_hover: Color32::from_rgb(28, 31, 40),
    inset: Color32::from_rgb(16, 18, 24),
    stroke: Color32::from_rgb(36, 40, 52),
    stroke_strong: Color32::from_rgb(52, 57, 74),
    text: Color32::from_rgb(233, 235, 242),
    dim: Color32::from_rgb(163, 169, 183),
    faint: Color32::from_rgb(107, 113, 128),
    accent: Color32::from_rgb(74, 92, 255),
    accent_hover: Color32::from_rgb(98, 114, 255),
    ok: Color32::from_rgb(54, 196, 138),
    warn: Color32::from_rgb(240, 180, 76),
    fail: Color32::from_rgb(255, 93, 93),
    info: Color32::from_rgb(92, 157, 255),
};

/// The light palette.
pub const LIGHT: Palette = Palette {
    dark: false,
    bg: Color32::from_rgb(238, 240, 245),
    bar: Color32::from_rgb(246, 247, 250),
    card: Color32::from_rgb(255, 255, 255),
    card_hover: Color32::from_rgb(243, 245, 251),
    inset: Color32::from_rgb(246, 247, 251),
    stroke: Color32::from_rgb(223, 227, 236),
    stroke_strong: Color32::from_rgb(196, 202, 216),
    text: Color32::from_rgb(20, 24, 33),
    dim: Color32::from_rgb(77, 85, 102),
    faint: Color32::from_rgb(138, 145, 161),
    accent: Color32::from_rgb(59, 76, 245),
    accent_hover: Color32::from_rgb(80, 96, 250),
    ok: Color32::from_rgb(26, 154, 98),
    warn: Color32::from_rgb(183, 121, 31),
    fail: Color32::from_rgb(211, 60, 60),
    info: Color32::from_rgb(47, 111, 214),
};

/// Corner radius of cards.
pub const R_CARD: u8 = 14;
/// Corner radius of rows and buttons.
pub const R_ROW: u8 = 12;
/// Corner radius of small controls.
pub const R_SMALL: u8 = 8;
/// Spacing unit.
pub const GAP: f32 = 10.0;
/// Expand / collapse time.
pub const T_EXPAND: f32 = 0.22;
/// Hover fade time.
pub const T_HOVER: f32 = 0.12;
/// Colour fade time of pills.
pub const T_FADE: f32 = 0.35;

/// The palette of this ui's theme.
pub fn pal(ui: &Ui) -> Palette {
    if ui.visuals().dark_mode {
        DARK
    } else {
        LIGHT
    }
}

/// The palette of a context.
pub fn pal_ctx(ctx: &egui::Context) -> Palette {
    if ctx.global_style().visuals.dark_mode {
        DARK
    } else {
        LIGHT
    }
}

fn heading_name() -> FontFamily {
    FontFamily::Name("heading".into())
}

const INSTALLED: &str = "pimax-theme-installed";

/// Has [`install`] taken effect on this context (fonts apply from the pass after `set_fonts`)?
pub fn installed(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<u64>(Id::new(INSTALLED))).is_some_and(|p| ctx.cumulative_pass_nr() > p)
}

/// Font family of headings (Noto Sans Medium when installed); plain text until the theme's fonts are active, so a
/// context without the theme never meets an unknown family.
pub fn heading(ui: &Ui) -> FontFamily {
    if installed(ui.ctx()) {
        heading_name()
    } else {
        FontFamily::Proportional
    }
}

/// Fonts: Noto Sans (when the system has it) for text and headings, Phosphor for icons, egui's own as fallback.
pub fn fonts() -> FontDefinitions {
    let mut f = FontDefinitions::default();
    icons::regular::add_to_fonts(&mut f);
    let load = |name: &str| -> Option<Arc<FontData>> {
        ["/usr/share/fonts/noto", "/usr/share/fonts/TTF", "/usr/share/fonts/truetype/noto"]
            .iter()
            .find_map(|d| std::fs::read(std::path::Path::new(d).join(name)).ok())
            .map(|b| Arc::new(FontData::from_owned(b)))
    };
    let mut heading_keys = f.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    if let Some(regular) = load("NotoSans-Regular.ttf") {
        f.font_data.insert("noto".into(), regular);
        if let Some(p) = f.families.get_mut(&FontFamily::Proportional) {
            p.insert(0, "noto".into());
        }
        heading_keys.insert(0, "noto".into());
    }
    if let Some(medium) = load("NotoSans-Medium.ttf").or_else(|| load("NotoSans-SemiBold.ttf")) {
        f.font_data.insert("noto-medium".into(), medium);
        heading_keys.insert(0, "noto-medium".into());
    }
    f.families.insert(heading_name(), heading_keys);
    f
}

/// egui visuals of a palette.
pub fn visuals(p: &Palette) -> egui::Visuals {
    let mut v = if p.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.panel_fill = p.bg;
    v.window_fill = p.card;
    v.extreme_bg_color = p.inset;
    v.faint_bg_color = p.card;
    v.code_bg_color = p.inset;
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = p.accent.gamma_multiply(0.45);
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.window_corner_radius = CornerRadius::same(R_CARD);
    v.menu_corner_radius = CornerRadius::same(R_ROW);
    v.window_stroke = Stroke::new(1.0, p.stroke);
    v.override_text_color = None;
    let r = CornerRadius::same(R_SMALL);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.card;
    w.noninteractive.weak_bg_fill = p.card;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.stroke);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.dim);
    w.noninteractive.corner_radius = r;
    let control = if p.dark { Color32::from_rgb(30, 33, 43) } else { Color32::from_rgb(234, 237, 244) };
    let control_hover = if p.dark { Color32::from_rgb(40, 44, 57) } else { Color32::from_rgb(224, 228, 238) };
    w.inactive.bg_fill = control;
    w.inactive.weak_bg_fill = control;
    w.inactive.bg_stroke = Stroke::new(1.0, p.stroke);
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.corner_radius = r;
    w.hovered.bg_fill = control_hover;
    w.hovered.weak_bg_fill = control_hover;
    w.hovered.bg_stroke = Stroke::new(1.0, p.accent.gamma_multiply(0.7));
    w.hovered.fg_stroke = Stroke::new(1.5, p.text);
    w.hovered.corner_radius = r;
    w.active.bg_fill = p.accent;
    w.active.weak_bg_fill = p.accent;
    w.active.bg_stroke = Stroke::new(1.0, p.accent_hover);
    w.active.fg_stroke = Stroke::new(1.5, Color32::WHITE);
    w.active.corner_radius = r;
    w.open.weak_bg_fill = control_hover;
    w.open.bg_fill = control_hover;
    w.open.corner_radius = r;
    v
}

/// Install fonts, visuals and spacing on a context (the window, the screenshots and the tests).
pub fn install(ctx: &egui::Context, dark: bool) {
    ctx.set_fonts(fonts());
    for (theme, p) in [(egui::Theme::Dark, DARK), (egui::Theme::Light, LIGHT)] {
        ctx.style_mut_of(theme, |s| {
            s.visuals = visuals(&p);
            s.spacing.item_spacing = egui::vec2(8.0, 7.0);
            s.spacing.button_padding = egui::vec2(12.0, 5.0);
            s.spacing.interact_size.y = 26.0;
            s.spacing.window_margin = Margin::same(16);
            s.spacing.menu_margin = Margin::same(8);
            s.interaction.selectable_labels = false;
            s.animation_time = 0.18;
            use egui::{FontId, TextStyle};
            s.text_styles.insert(TextStyle::Body, FontId::proportional(14.0));
            s.text_styles.insert(TextStyle::Button, FontId::proportional(14.0));
            s.text_styles.insert(TextStyle::Small, FontId::proportional(11.5));
            s.text_styles.insert(TextStyle::Heading, FontId::proportional(22.0));
            s.text_styles.insert(TextStyle::Monospace, FontId::monospace(12.5));
        });
    }
    ctx.set_theme(if dark { egui::Theme::Dark } else { egui::Theme::Light });
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| d.insert_temp(Id::new(INSTALLED), pass));
}

/// Linear blend of two colours.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgba_unmultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

/// Eased 0..1 for an on/off state (cubic in-out, `secs`); asks for frames only while it moves.
pub fn ease(ui: &Ui, id: Id, on: bool, secs: f32) -> f32 {
    ui.ctx().animate_bool_with_time_and_easing(id, on, secs, egui::emath::easing::cubic_in_out)
}

/// A colour that fades towards `target` (pills, status dots) instead of jumping; asks for frames only while it
/// fades.
pub fn fade(ui: &Ui, id: Id, target: Color32) -> Color32 {
    let now = ui.input(|i| i.time);
    let ctx = ui.ctx();
    let (from, to, start) = ctx.data(|d| d.get_temp::<(Color32, Color32, f64)>(id)).unwrap_or((target, target, now - 10.0));
    let k = (((now - start) as f32) / T_FADE).clamp(0.0, 1.0);
    let shown = mix(from, to, egui::emath::easing::cubic_in_out(k));
    if to != target {
        ctx.data_mut(|d| d.insert_temp(id, (shown, target, now)));
        ctx.request_repaint();
    } else if k < 1.0 {
        ctx.request_repaint();
    }
    shown
}
