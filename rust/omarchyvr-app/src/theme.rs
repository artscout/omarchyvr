//! The look of the app: a dark console in the spirit of Pimax's Windows apps, drawn our own way. Near-black
//! surfaces, flat cards with thin borders and small corners, small-caps labels, one indigo accent (#4a5cff), outline
//! icons (Phosphor), our own line art ([`crate::art`]), pills whose colour fades. Visual only.
//!
//! Dark is the default; a light palette exists for people who switch the desktop to light.
use eframe::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, Id, Margin, RichText, Stroke, Ui};
use omarchyvr_gui_core::colors::{contrast, luminance, OmarchyColors, Rgb};
#[cfg(test)]
use omarchyvr_gui_core::colors::OmarchyPaths;
use std::sync::Arc;

// Phosphor icons (regular weight), as text glyphs: only the ones the app shows are compiled in (a few KB).
egui_phosphor::subset! {
    pub mod icons {
        use regular::{
            ANCHOR, ARROW_COUNTER_CLOCKWISE, ARROW_SQUARE_OUT, ARROWS_CLOCKWISE, BATTERY_EMPTY, BATTERY_HIGH, BATTERY_LOW,
            BATTERY_MEDIUM, BOOK_OPEN, BRAIN, BRIEFCASE, BUG, CARET_DOWN, CARET_RIGHT, CHECK_CIRCLE, CIRCLE, CODE, COPY,
            CROSSHAIR, CUBE_FOCUS, DESKTOP, EYE, EYE_SLASH, FILE_TEXT, FILM_SLATE, FOLDER_OPEN, GAME_CONTROLLER, GEAR,
            GOGGLES, HAND, HEADSET, HOUSE, INFO, LIGHTNING, LINK_SIMPLE, LIST, MAGIC_WAND, MAP_TRIFOLD, MICROPHONE, MONITOR,
            PLAY, POPCORN, POWER, PULSE, QR_CODE, QUESTION, ROBOT, SCAN, SEAL_CHECK, SLIDERS, SPARKLE, SPEAKER_HIGH,
            SQUARES_FOUR, STEAM_LOGO, STOP, SUN, TERMINAL, USERS, VIDEO_CAMERA, WARNING, WRENCH, X_CIRCLE,
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
    /// The sidebar, the action row and the status bar.
    pub bar: Color32,
    /// Cards and rows.
    pub card: Color32,
    /// A hovered row.
    pub card_hover: Color32,
    /// An expanded row's body and inner boxes.
    pub inset: Color32,
    /// Hairlines around cards.
    pub stroke: Color32,
    /// Stronger lines (focus, separators, outlined buttons).
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
    /// Warning as text (the orange of a theme; readable on the cards).
    pub warn_text: Color32,
    /// Links.
    pub link: Color32,
    /// Text and icons on an accent fill (primary buttons, the pressed button).
    pub on_accent: Color32,
    /// Fill of an outlined button.
    pub control: Color32,
    /// Fill of an outlined button under the pointer.
    pub control_hover: Color32,
    /// The track of a switch that is off.
    pub switch_off: Color32,
}

/// The dark palette (default).
pub const DARK: Palette = Palette {
    dark: true,
    bg: Color32::from_rgb(9, 10, 13),
    bar: Color32::from_rgb(13, 14, 19),
    card: Color32::from_rgb(19, 21, 27),
    card_hover: Color32::from_rgb(27, 30, 39),
    inset: Color32::from_rgb(14, 16, 21),
    stroke: Color32::from_rgb(40, 44, 56),
    stroke_strong: Color32::from_rgb(62, 67, 85),
    text: Color32::from_rgb(232, 234, 241),
    dim: Color32::from_rgb(160, 166, 181),
    faint: Color32::from_rgb(104, 110, 126),
    accent: Color32::from_rgb(74, 92, 255),
    accent_hover: Color32::from_rgb(98, 114, 255),
    ok: Color32::from_rgb(54, 196, 138),
    warn: Color32::from_rgb(240, 180, 76),
    fail: Color32::from_rgb(255, 93, 93),
    info: Color32::from_rgb(92, 157, 255),
    warn_text: Color32::from_rgb(240, 180, 76),
    link: Color32::from_rgb(74, 92, 255),
    on_accent: Color32::WHITE,
    control: Color32::from_rgb(28, 31, 41),
    control_hover: Color32::from_rgb(32, 36, 48),
    switch_off: Color32::from_rgb(44, 48, 62),
};

/// The light palette.
pub const LIGHT: Palette = Palette {
    dark: false,
    bg: Color32::from_rgb(238, 240, 245),
    bar: Color32::from_rgb(246, 247, 250),
    card: Color32::from_rgb(255, 255, 255),
    card_hover: Color32::from_rgb(243, 245, 251),
    inset: Color32::from_rgb(246, 247, 251),
    stroke: Color32::from_rgb(220, 224, 233),
    stroke_strong: Color32::from_rgb(190, 196, 211),
    text: Color32::from_rgb(20, 24, 33),
    dim: Color32::from_rgb(77, 85, 102),
    faint: Color32::from_rgb(132, 139, 156),
    accent: Color32::from_rgb(59, 76, 245),
    accent_hover: Color32::from_rgb(80, 96, 250),
    ok: Color32::from_rgb(26, 154, 98),
    warn: Color32::from_rgb(183, 121, 31),
    fail: Color32::from_rgb(211, 60, 60),
    info: Color32::from_rgb(47, 111, 214),
    warn_text: Color32::from_rgb(183, 121, 31),
    link: Color32::from_rgb(59, 76, 245),
    on_accent: Color32::WHITE,
    control: Color32::from_rgb(236, 239, 245),
    control_hover: Color32::from_rgb(230, 234, 243),
    switch_off: Color32::from_rgb(205, 210, 222),
};

/// Both palettes of the running app and which one starts: the omarchyvr colours, or an Omarchy theme in the slot of
/// its mode (the other slot keeps the omarchyvr colours).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Colourway {
    /// The dark palette.
    pub dark: Palette,
    /// The light palette.
    pub light: Palette,
    /// Which of the two is shown.
    pub use_dark: bool,
}

impl Colourway {
    /// The omarchyvr colours (dark or light).
    pub fn default_ways(use_dark: bool) -> Colourway {
        Colourway { dark: DARK, light: LIGHT, use_dark }
    }

    /// An Omarchy theme, mapped onto the app's roles ([`from_omarchy`]).
    pub fn omarchy(c: &OmarchyColors) -> Colourway {
        let p = from_omarchy(c);
        if c.dark {
            Colourway { dark: p, light: LIGHT, use_dark: true }
        } else {
            Colourway { dark: DARK, light: p, use_dark: false }
        }
    }
}

fn c32(c: Rgb) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

fn rgb(c: Color32) -> Rgb {
    [c.r(), c.g(), c.b()]
}

fn mix_rgb(a: Rgb, b: Rgb, t: f32) -> Rgb {
    rgb(mix(c32(a), c32(b), t))
}

/// Does `c` reach `min` contrast on every one of `bgs`?
fn readable(c: Rgb, bgs: &[Rgb], min: f32) -> bool {
    bgs.iter().all(|b| contrast(c, *b) >= min)
}

/// The first candidate that is readable on `bgs`; when none is, the first one pulled towards `text` until it is (the
/// text colour itself is checked first, so this ends).
fn pick(candidates: &[Option<Rgb>], bgs: &[Rgb], min: f32, text: Rgb) -> Rgb {
    let list: Vec<Rgb> = candidates.iter().flatten().copied().collect();
    if let Some(c) = list.iter().find(|c| readable(**c, bgs, min)) {
        return *c;
    }
    let first = list.first().copied().unwrap_or(text);
    (1..=10).map(|i| mix_rgb(first, text, i as f32 / 10.0)).find(|c| readable(*c, bgs, min)).unwrap_or(text)
}

/// Map an Omarchy theme onto the app's roles. Dark: window = `background`, bars and inner boxes = `dark_background`,
/// cards = `lighter_background`. Light: window = `background`, bars = `dark_background`, cards = the background
/// lifted towards white. Text = `bright_foreground`, secondary = `foreground` (dark) or `light_foreground` (light),
/// faint = `dark_foreground`; accent = `accent`; chips = `green` / `yellow` / `red` / `blue`; warning text = `orange`;
/// lines = `muted`; hover = `selection`. A missing key uses the omarchyvr colour of that role; a colour that is hard
/// to read on the surfaces (text 4.5:1, others 3:1) is replaced by the next candidate, the omarchyvr colour of the
/// role, or is pulled towards the text colour.
pub fn from_omarchy(c: &OmarchyColors) -> Palette {
    let base = if c.dark { DARK } else { LIGHT };
    let k = |n: &str| c.get(n);
    let or = |n: &str, d: Color32| k(n).unwrap_or_else(|| rgb(d));
    let white = [255, 255, 255];
    let bg_c = or("background", base.bg);
    let (bg, bar, card, inset) = if c.dark {
        (bg_c, or("dark_background", base.bar), or("lighter_background", base.card), or("dark_background", base.inset))
    } else {
        (bg_c, or("dark_background", base.bar), mix_rgb(bg_c, white, 0.65), bg_c)
    };
    let selection = or("selection", base.card_hover);
    let muted = or("muted", base.stroke_strong);
    let surfaces = [bg, card, inset];
    // text on the window, the cards and the inner boxes
    let text_fallback = if luminance(bg) < 0.4 { [255, 255, 255] } else { [0, 0, 0] };
    let text = pick(&[k("bright_foreground"), k("foreground"), k("light_foreground"), Some(rgb(base.text))], &surfaces, 4.5, text_fallback);
    let dim_c = if c.dark { [k("foreground"), k("light_foreground")] } else { [k("light_foreground"), k("foreground")] };
    let dim = pick(&[dim_c[0], dim_c[1], Some(rgb(base.dim)), Some(mix_rgb(text, bg, 0.3))], &surfaces, 4.5, text);
    let faint = pick(&[k("dark_foreground"), Some(rgb(base.faint)), Some(mix_rgb(text, bg, 0.45))], &surfaces, 3.0, text);
    let accent = pick(&[k("accent"), k("blue"), Some(rgb(base.accent))], &[bg, card], 3.0, text);
    let accent_hover = mix_rgb(accent, if c.dark { white } else { [0, 0, 0] }, 0.18);
    let on_accent = if contrast(accent, [255, 255, 255]) >= contrast(accent, [14, 15, 20]) { white } else { [14, 15, 20] };
    let chip = |cands: &[Option<Rgb>]| pick(cands, &[bg, card], 3.0, text);
    let ok = chip(&[k("green"), k("bright_green"), Some(rgb(base.ok))]);
    let warn = chip(&[k("yellow"), k("orange"), k("bright_yellow"), Some(rgb(base.warn))]);
    let fail = chip(&[k("red"), k("bright_red"), Some(rgb(base.fail))]);
    let info = chip(&[k("blue"), k("cyan"), k("bright_blue"), Some(rgb(base.info))]);
    let warn_text = pick(&[k("orange"), k("yellow"), Some(rgb(base.warn_text))], &surfaces, 4.5, text);
    let link = pick(&[k("blue"), k("accent"), Some(rgb(base.link))], &surfaces, 4.5, text);
    let stroke = mix_rgb(card, muted, 0.5);
    let (control, control_hover, card_hover) = if c.dark {
        (mix_rgb(card, text, 0.07), mix_rgb(card, text, 0.13), mix_rgb(card, selection, 0.8))
    } else {
        (mix_rgb(card, selection, 0.4), mix_rgb(card, selection, 0.7), mix_rgb(card, selection, 0.45))
    };
    Palette {
        dark: c.dark,
        bg: c32(bg),
        bar: c32(bar),
        card: c32(card),
        card_hover: c32(card_hover),
        inset: c32(inset),
        stroke: c32(stroke),
        stroke_strong: c32(muted),
        text: c32(text),
        dim: c32(dim),
        faint: c32(faint),
        accent: c32(accent),
        accent_hover: c32(accent_hover),
        ok: c32(ok),
        warn: c32(warn),
        fail: c32(fail),
        info: c32(info),
        warn_text: c32(warn_text),
        link: c32(link),
        on_accent: c32(on_accent),
        control: c32(control),
        control_hover: c32(control_hover),
        switch_off: c32(mix_rgb(card, muted, 0.8)),
    }
}

/// The lowest contrast of the palette's text roles on its surfaces (tests, and a quick check of a theme).
pub fn weakest_text_contrast(p: &Palette) -> f32 {
    let surfaces = [rgb(p.bg), rgb(p.card), rgb(p.inset)];
    [p.text, p.dim, p.warn_text, p.link].iter().flat_map(|t| surfaces.iter().map(move |s| contrast(rgb(*t), *s))).fold(f32::MAX, f32::min)
}

/// Corner radius of cards.
pub const R_CARD: u8 = 6;
/// Corner radius of rows and big buttons.
pub const R_ROW: u8 = 5;
/// Corner radius of small controls.
pub const R_SMALL: u8 = 4;
/// Spacing unit.
pub const GAP: f32 = 10.0;
/// Expand / collapse time.
pub const T_EXPAND: f32 = 0.22;
/// Hover fade time.
pub const T_HOVER: f32 = 0.12;
/// Colour fade time of pills.
pub const T_FADE: f32 = 0.35;
/// Letter spacing of small-caps labels (points).
pub const CAPS_SPACING: f32 = 0.7;

/// The palette of this ui's theme.
pub fn pal(ui: &Ui) -> Palette {
    palette_of(ui.ctx(), ui.visuals().dark_mode)
}

/// The palette of a context.
pub fn pal_ctx(ctx: &egui::Context) -> Palette {
    palette_of(ctx, ctx.global_style().visuals.dark_mode)
}

const PALETTES: &str = "omarchyvr-theme-palettes";

/// The installed palette of a mode (the omarchyvr colours on a context without the theme).
fn palette_of(ctx: &egui::Context, dark: bool) -> Palette {
    let (d, l) = ctx.data(|x| x.get_temp::<(Palette, Palette)>(Id::new(PALETTES))).unwrap_or((DARK, LIGHT));
    if dark {
        d
    } else {
        l
    }
}

/// Switch the colours of a running context: the palettes, the egui visuals and the dark/light mode. Cheap; call it
/// only when the choice or the Omarchy theme changed.
pub fn set_colourway(ctx: &egui::Context, cw: &Colourway) {
    ctx.data_mut(|d| d.insert_temp(Id::new(PALETTES), (cw.dark, cw.light)));
    for (theme, p) in [(egui::Theme::Dark, cw.dark), (egui::Theme::Light, cw.light)] {
        ctx.style_mut_of(theme, |s| s.visuals = visuals(&p));
    }
    ctx.set_theme(if cw.use_dark { egui::Theme::Dark } else { egui::Theme::Light });
    ctx.request_repaint();
}

/// Small caps: the text in capitals, a little spaced (labels, card titles, buttons of the action row).
pub fn caps(text: &str, size: f32, color: Color32) -> RichText {
    RichText::new(text.to_uppercase()).size(size).color(color).extra_letter_spacing(CAPS_SPACING)
}

fn heading_name() -> FontFamily {
    FontFamily::Name("heading".into())
}

const INSTALLED: &str = "omarchyvr-theme-installed";

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
    v.hyperlink_color = p.link;
    v.selection.bg_fill = p.accent.gamma_multiply(0.45);
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.window_corner_radius = CornerRadius::same(R_CARD + 2);
    v.menu_corner_radius = CornerRadius::same(R_ROW);
    v.window_stroke = Stroke::new(1.0, p.stroke_strong);
    v.override_text_color = None;
    let r = CornerRadius::same(R_SMALL);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.card;
    w.noninteractive.weak_bg_fill = p.card;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.stroke);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.dim);
    w.noninteractive.corner_radius = r;
    // sliders: the part up to the handle in the accent, a round handle
    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Circle;
    // buttons: outlined, a dark fill (the look of the action row)
    let control = p.control;
    let control_hover = p.control_hover;
    w.inactive.bg_fill = control;
    w.inactive.weak_bg_fill = control;
    w.inactive.bg_stroke = Stroke::new(1.0, p.stroke_strong);
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.corner_radius = r;
    w.hovered.bg_fill = control_hover;
    w.hovered.weak_bg_fill = control_hover;
    w.hovered.bg_stroke = Stroke::new(1.0, p.accent.gamma_multiply(0.8));
    w.hovered.fg_stroke = Stroke::new(1.5, p.text);
    w.hovered.corner_radius = r;
    w.active.bg_fill = p.accent;
    w.active.weak_bg_fill = p.accent;
    w.active.bg_stroke = Stroke::new(1.0, p.accent_hover);
    w.active.fg_stroke = Stroke::new(1.5, p.on_accent);
    w.active.corner_radius = r;
    w.open.weak_bg_fill = control_hover;
    w.open.bg_fill = control_hover;
    w.open.bg_stroke = Stroke::new(1.0, p.accent.gamma_multiply(0.8));
    w.open.corner_radius = r;
    v
}

/// Install fonts, visuals and spacing on a context (the window, the screenshots and the tests).
pub fn install(ctx: &egui::Context, dark: bool) {
    install_with(ctx, &Colourway::default_ways(dark));
}

/// [`install`] with chosen colours (the screenshots of an Omarchy theme).
pub fn install_with(ctx: &egui::Context, cw: &Colourway) {
    ctx.set_fonts(fonts());
    ctx.data_mut(|d| d.insert_temp(Id::new(PALETTES), (cw.dark, cw.light)));
    for (theme, p) in [(egui::Theme::Dark, cw.dark), (egui::Theme::Light, cw.light)] {
        ctx.style_mut_of(theme, |s| {
            s.visuals = visuals(&p);
            s.spacing.item_spacing = egui::vec2(8.0, 7.0);
            s.spacing.button_padding = egui::vec2(10.0, 4.0);
            s.spacing.interact_size.y = 24.0;
            // setting rows shorten it when the row is narrow (widgets::at_right)
            s.spacing.slider_width = 160.0;
            s.spacing.window_margin = Margin::same(18);
            s.spacing.menu_margin = Margin::same(6);
            s.interaction.selectable_labels = false;
            s.animation_time = 0.18;
            use egui::{FontId, TextStyle};
            s.text_styles.insert(TextStyle::Body, FontId::proportional(13.5));
            s.text_styles.insert(TextStyle::Button, FontId::proportional(13.0));
            s.text_styles.insert(TextStyle::Small, FontId::proportional(11.5));
            s.text_styles.insert(TextStyle::Heading, FontId::proportional(20.0));
            s.text_styles.insert(TextStyle::Monospace, FontId::monospace(12.5));
        });
    }
    ctx.set_theme(if cw.use_dark { egui::Theme::Dark } else { egui::Theme::Light });
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

#[cfg(test)]
mod tests {
    use super::*;

    const TOKYO: &str = include_str!("../../fixtures/omarchy-themes/tokyo-night/colors.toml");
    const LATTE: &str = include_str!("../../fixtures/omarchy-themes/catppuccin-latte/colors.toml");
    const FLEXOKI: &str = include_str!("../../fixtures/omarchy-themes/flexoki-light/colors.toml");

    fn pal_of(text: &str) -> Palette {
        from_omarchy(&OmarchyColors::parse(text).unwrap())
    }

    #[test]
    fn dark_theme_maps_every_role() {
        let p = pal_of(TOKYO);
        assert!(p.dark);
        assert_eq!(p.bg, Color32::from_rgb(0x1a, 0x1b, 0x26), "window = background");
        assert_eq!(p.bar, Color32::from_rgb(0x13, 0x14, 0x1c), "bars = dark_background");
        assert_eq!(p.card, Color32::from_rgb(0x24, 0x28, 0x3b), "cards = lighter_background");
        assert_eq!(p.text, Color32::from_rgb(0xc0, 0xca, 0xf5), "text = bright_foreground");
        assert_eq!(p.dim, Color32::from_rgb(0xa9, 0xb1, 0xd6), "secondary = foreground");
        assert_eq!(p.accent, Color32::from_rgb(0x7a, 0xa2, 0xf7));
        assert_eq!(p.ok, Color32::from_rgb(0x9e, 0xce, 0x6a), "green");
        assert_eq!(p.warn, Color32::from_rgb(0xe0, 0xaf, 0x68), "yellow chip");
        assert_eq!(p.warn_text, Color32::from_rgb(0xeb, 0x92, 0x7b), "orange warning text");
        assert_eq!(p.fail, Color32::from_rgb(0xf7, 0x76, 0x8e), "red");
        assert_eq!(p.stroke_strong, Color32::from_rgb(0x41, 0x48, 0x68), "muted");
        assert_eq!(p.on_accent, Color32::from_rgb(14, 15, 20), "dark text on a light accent");
        assert!(weakest_text_contrast(&p) >= 4.5);
    }

    #[test]
    fn light_themes_map_to_a_light_palette_and_stay_readable() {
        for text in [LATTE, FLEXOKI] {
            let p = pal_of(text);
            assert!(!p.dark);
            assert!(luminance_of(p.bg) > 0.5 && luminance_of(p.card) > 0.5);
            assert!(luminance_of(p.text) < 0.2, "dark text on light surfaces");
            assert!(weakest_text_contrast(&p) >= 4.5, "text roles are readable: {}", weakest_text_contrast(&p));
        }
        // Colourway puts it in the light slot and keeps the dark one
        let cw = Colourway::omarchy(&OmarchyColors::parse(LATTE).unwrap());
        assert!(!cw.use_dark);
        assert_eq!(cw.dark, DARK);
        assert_ne!(cw.light, LIGHT);
    }

    fn luminance_of(c: Color32) -> f32 {
        luminance(rgb(c))
    }

    #[test]
    fn unreadable_colours_fall_back() {
        // text, secondary text and the orange are nearly the background: each role moves to a readable colour
        let bad = "mode = \"dark\"\nbackground = \"#202020\"\nlighter_background = \"#262626\"\ndark_background = \"#1a1a1a\"\nforeground = \"#222222\"\nbright_foreground = \"#242424\"\nlight_foreground = \"#232323\"\ndark_foreground = \"#212121\"\norange = \"#252525\"\nyellow = \"#222222\"\naccent = \"#202021\"\n";
        let p = pal_of(bad);
        assert!(p.dark);
        assert!(weakest_text_contrast(&p) >= 4.5, "{}", weakest_text_contrast(&p));
        assert_eq!(p.text, DARK.text, "the omarchyvr text colour is the next candidate");
        let card = rgb(p.card);
        assert!(contrast(rgb(p.faint), card) >= 3.0);
        assert!(contrast(rgb(p.accent), card) >= 3.0);
        assert!(contrast(rgb(p.warn), card) >= 3.0);
    }

    #[test]
    fn missing_keys_use_the_omarchyvr_colours() {
        let p = pal_of("mode = \"dark\"\nbackground = \"#101820\"\n");
        assert_eq!(p.bg, Color32::from_rgb(0x10, 0x18, 0x20));
        assert_eq!(p.accent, DARK.accent);
        assert_eq!(p.ok, DARK.ok);
        assert_eq!(p.card, DARK.card);
        let l = pal_of("mode = \"light\"\nforeground = \"#222222\"\n");
        assert!(!l.dark);
        assert_eq!(l.bg, LIGHT.bg);
        assert_eq!(l.accent, LIGHT.accent);
    }

    #[test]
    fn colours_are_stored_per_context() {
        let ctx = egui::Context::default();
        assert_eq!(pal_ctx(&ctx), DARK, "a context without the theme: the omarchyvr colours");
        install_with(&ctx, &Colourway::omarchy(&OmarchyColors::parse(TOKYO).unwrap()));
        assert_eq!(pal_ctx(&ctx).bg, Color32::from_rgb(0x1a, 0x1b, 0x26));
        install(&ctx, true);
        assert_eq!(pal_ctx(&ctx), DARK, "another context or a reinstall is not affected");
    }

    #[test]
    fn every_installed_theme_is_readable() {
        // real themes of this PC when present (not private): none may leave a text role unreadable
        let paths = OmarchyPaths::from_home(std::path::Path::new("/nonexistent"));
        for t in paths.themes() {
            let p = from_omarchy(&t.colors);
            assert!(weakest_text_contrast(&p) >= 4.5, "{}: {}", t.name, weakest_text_contrast(&p));
        }
    }
}
