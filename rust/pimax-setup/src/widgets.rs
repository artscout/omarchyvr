//! Drawing helpers shared by the pages, in the Pimax Play style of [`crate::theme`]: rounded cards, pills whose
//! colour fades, rows that expand with an eased height, device cards, big action buttons, a throttled spinner.
use crate::theme::{self, icon, Palette, R_CARD, R_ROW, R_SMALL, T_EXPAND, T_HOVER};
use eframe::egui::{self, Align, Color32, CornerRadius, Frame, Id, Layout, Margin, Rect, RichText, Sense, Stroke, Ui, UiBuilder, WidgetInfo, WidgetType};
use pimax_doctor::Level;
use pimax_gui_core::devices::{Battery, Dev, DeviceView};
use pimax_gui_core::preflight::Sev;

/// State colours for the current theme (the pages' short names for [`theme::Palette`]).
#[derive(Debug, Clone, Copy)]
pub struct Pal {
    /// Good.
    pub ok: Color32,
    /// Information.
    pub info: Color32,
    /// Warning / needs attention.
    pub warn: Color32,
    /// Failure / blocked.
    pub fail: Color32,
    /// Secondary text.
    pub muted: Color32,
    /// Accent (primary buttons).
    pub accent: Color32,
}

/// The palette for this ui's theme.
pub fn pal(ui: &Ui) -> Pal {
    let p = theme::pal(ui);
    Pal {
        ok: p.ok,
        info: p.info,
        warn: p.warn,
        fail: p.fail,
        muted: p.dim,
        accent: p.accent,
    }
}

/// Colour of a doctor/checklist level.
pub fn level_color(ui: &Ui, l: Level) -> Color32 {
    let p = pal(ui);
    match l {
        Level::Ok => p.ok,
        Level::Info => p.info,
        Level::Warn => p.warn,
        Level::Fail => p.fail,
    }
}

/// Colour of a preflight severity.
pub fn sev_color(ui: &Ui, s: Sev) -> Color32 {
    let p = pal(ui);
    match s {
        Sev::Ok => p.ok,
        Sev::Info => p.info,
        Sev::Warn | Sev::Consent => p.warn,
        Sev::Block => p.fail,
    }
}

/// Colour of a device state.
pub fn dev_color(p: &Palette, d: Dev) -> Color32 {
    match d {
        Dev::Off => p.faint,
        Dev::Connected => p.accent,
        Dev::Active => p.ok,
        Dev::Searching => p.info,
        Dev::Warn => p.warn,
    }
}

/// A small rounded label with a tinted background.
pub fn chip(ui: &mut Ui, text: &str, color: Color32) {
    Frame::new()
        .fill(color.gamma_multiply(0.14))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.45)))
        .corner_radius(CornerRadius::same(255))
        .inner_margin(Margin::symmetric(9, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text).color(color).size(11.5).strong());
        });
}

/// A status pill: a dot and a label in a dark capsule; the colour fades when the state changes.
pub fn pill(ui: &mut Ui, id: Id, text: &str, color: Color32) -> egui::Response {
    let c = theme::fade(ui, id, color);
    let p = theme::pal(ui);
    Frame::new()
        .fill(if p.dark { Color32::from_rgba_unmultiplied(255, 255, 255, 10) } else { Color32::from_rgba_unmultiplied(0, 0, 0, 10) })
        .stroke(Stroke::new(1.0, c.gamma_multiply(0.35)))
        .corner_radius(CornerRadius::same(255))
        .inner_margin(Margin::symmetric(10, 3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                dot(ui, c, 3.5);
                ui.label(RichText::new(text).size(12.0).color(p.text));
            });
        })
        .response
}

/// A filled dot.
pub fn dot(ui: &mut Ui, color: Color32, r: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(r * 2.0 + 2.0, r * 2.0 + 2.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), r, color);
}

/// Level chip (`OK`, `WARN`, ...).
pub fn level_chip(ui: &mut Ui, l: Level) {
    let t = match l {
        Level::Ok => "OK",
        Level::Info => "INFO",
        Level::Warn => "WARN",
        Level::Fail => "FAIL",
    };
    chip(ui, t, level_color(ui, l));
}

/// Severity chip.
pub fn sev_chip(ui: &mut Ui, s: Sev) {
    let t = match s {
        Sev::Ok => "OK",
        Sev::Info => "INFO",
        Sev::Warn => "WARN",
        Sev::Consent => "YOUR OK",
        Sev::Block => "BLOCKED",
    };
    chip(ui, t, sev_color(ui, s));
}

/// `LESSONS #n` tag.
pub fn row_tag(ui: &mut Ui, row: Option<u8>) {
    if let Some(r) = row {
        ui.label(RichText::new(format!("LESSONS #{r}")).size(10.5).color(theme::pal(ui).faint))
            .on_hover_text("The rule in docs/LESSONS.md this check implements");
    }
}

/// A card: rounded, a hairline, the card colour.
pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let p = theme::pal(ui);
    Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.stroke))
        .corner_radius(CornerRadius::same(R_CARD))
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// An inset box inside a card (an expanded row's groups).
pub fn inset<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let p = theme::pal(ui);
    Frame::new()
        .fill(p.inset)
        .corner_radius(CornerRadius::same(R_SMALL + 2))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// Section title inside a card.
pub fn title(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(15.0).family(theme::heading(ui)).color(theme::pal(ui).text));
}

/// A small uppercase label over a group.
pub fn caption(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text.to_uppercase()).size(10.5).color(theme::pal(ui).faint).strong());
}

/// Monospace lines in a box with a Copy button. Returns true when copied.
pub fn code_box(ui: &mut Ui, lines: &[String]) -> bool {
    let mut copied = false;
    let p = theme::pal(ui);
    Frame::new()
        .fill(p.inset)
        .stroke(Stroke::new(1.0, p.stroke))
        .corner_radius(CornerRadius::same(R_SMALL))
        .inner_margin(Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            // the button first, from the right: the lines wrap in what is left (a fixed reserve for the button made
            // the box a few pixels wider than its card, and the cards grew from frame to frame)
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                if ismall(ui, icon::COPY, "Copy").on_hover_text("Copy to the clipboard").clicked() {
                    ui.ctx().copy_text(lines.join("\n"));
                    copied = true;
                }
                ui.with_layout(Layout::top_down(Align::Min), |ui| {
                    for l in lines {
                        ui.add(egui::Label::new(RichText::new(l).monospace().color(p.dim)).wrap());
                    }
                });
            });
        });
    copied
}

/// How a big button looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Accent fill.
    Primary,
    /// Card fill with a hairline.
    Secondary,
    /// Red fill.
    Danger,
}

/// A big rounded button with an icon (Start Desk, Start VR, Stop). Hover lightens it smoothly.
pub fn action(ui: &mut Ui, ic: &str, text: &str, kind: Kind, enabled: bool, width: f32) -> egui::Response {
    let p = theme::pal(ui);
    let h = 50.0;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, h), if enabled { Sense::click() } else { Sense::hover() });
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, text));
    let hover = theme::ease(ui, resp.id.with("hover"), enabled && resp.hovered(), T_HOVER);
    let (fill, fg, stroke) = match (kind, enabled) {
        (_, false) => (p.card, p.faint, p.stroke),
        (Kind::Primary, true) => (theme::mix(p.accent, p.accent_hover, hover), Color32::WHITE, p.accent_hover),
        (Kind::Danger, true) => (theme::mix(p.fail, p.fail.gamma_multiply(1.15), hover), Color32::WHITE, p.fail),
        (Kind::Secondary, true) => (theme::mix(p.card, p.card_hover, hover), p.text, theme::mix(p.stroke, p.accent, hover * 0.6)),
    };
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect(rect, CornerRadius::same(R_ROW), fill, Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
        let font = egui::FontId::new(16.0, theme::heading(ui));
        let label = format!("{ic}  {text}");
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, label, font, fg);
    }
    resp
}

/// A big coloured button (older pages).
pub fn big_button(ui: &mut Ui, text: &str, fill: Color32, enabled: bool) -> egui::Response {
    let p = theme::pal(ui);
    let kind = if fill == p.fail { Kind::Danger } else { Kind::Primary };
    action(ui, if kind == Kind::Danger { icon::STOP } else { icon::PLAY }, text, kind, enabled, 230.0)
}

/// A primary (accent) button.
pub fn primary(ui: &mut Ui, text: &str, enabled: bool) -> egui::Response {
    let p = theme::pal(ui);
    let (fill, fg) = if enabled { (p.accent, Color32::WHITE) } else { (p.card, p.faint) };
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(text).color(fg).strong())
            .fill(fill)
            .corner_radius(CornerRadius::same(R_SMALL)),
    )
}

/// A quiet button with an icon (ROOM SETUP).
pub fn ghost(ui: &mut Ui, ic: &str, text: &str, enabled: bool) -> egui::Response {
    let p = theme::pal(ui);
    let r = ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(format!("{ic}  {}", text.to_uppercase())).size(13.0).color(p.dim))
            .fill(p.card)
            .stroke(Stroke::new(1.0, p.stroke))
            .corner_radius(CornerRadius::same(R_ROW))
            .min_size(egui::vec2(170.0, 38.0)),
    );
    r.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, text));
    r
}

/// A button with an icon; its accessible name is the text alone (screen readers, the UI tests).
pub fn ibutton(ui: &mut Ui, ic: &str, text: &str, enabled: bool) -> egui::Response {
    let r = ui.add_enabled(enabled, egui::Button::new(format!("{ic} {text}")));
    r.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, text));
    r
}

/// A small button with an icon (accessible name: the text).
pub fn ismall(ui: &mut Ui, ic: &str, text: &str) -> egui::Response {
    let r = ui.small_button(format!("{ic} {text}"));
    r.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, text));
    r
}

/// Key / value row in a grid.
pub fn kv(ui: &mut Ui, key: &str, value: impl Into<egui::WidgetText>) {
    ui.label(RichText::new(key).color(theme::pal(ui).dim));
    ui.add(egui::Label::new(value).wrap());
    ui.end_row();
}

/// Muted wrapped text.
pub fn note(ui: &mut Ui, text: &str) {
    let c = theme::pal(ui).dim;
    ui.add(egui::Label::new(RichText::new(text).color(c)).wrap());
}

/// A "coming soon" tag.
pub fn soon(ui: &mut Ui) {
    chip(ui, "coming soon", theme::pal(ui).faint);
}

/// A small spinner that animates at 10 frames per second (egui's own spinner asks for a new frame right away,
/// which without vsync means as many frames as the CPU can draw).
pub fn spinner(ui: &mut Ui) -> egui::Response {
    let size = ui.text_style_height(&egui::TextStyle::Body).max(12.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), Sense::hover());
    if ui.is_rect_visible(rect) {
        let t = ui.input(|i| i.time) as f32;
        let start = t * std::f32::consts::TAU / 1.2;
        let r = rect.height() / 2.6;
        let n = 16;
        let points: Vec<egui::Pos2> = (0..=n)
            .map(|i| {
                let a = start + i as f32 / n as f32 * std::f32::consts::TAU * 0.75;
                rect.center() + r * egui::vec2(a.cos(), a.sin())
            })
            .collect();
        ui.painter().add(egui::Shape::line(points, Stroke::new(2.0, theme::pal(ui).accent)));
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
    }
    resp
}

/// Open or close a row from elsewhere (the ROOM SETUP button opens the Room row).
pub fn set_row_open(ctx: &egui::Context, id: Id, open: bool) {
    ctx.data_mut(|d| d.insert_temp(id.with("open"), open));
}

/// Is a row open?
pub fn row_open(ctx: &egui::Context, id: Id) -> bool {
    ctx.data(|d| d.get_temp::<bool>(id.with("open"))).unwrap_or(false)
}

/// A settings row: icon, uppercase title, the current values on the right, a chevron; a click expands the body in
/// place with an eased height. Returns true while it is open.
pub fn row(ui: &mut Ui, id: Id, ic: &str, title: &str, summary: &str, body: impl FnOnce(&mut Ui)) -> bool {
    let p = theme::pal(ui);
    let mut open = row_open(ui.ctx(), id);
    // the frame's hairline adds a pixel on each side: the content is that much narrower, so rows do not creep
    let width = ui.available_width() - 2.0;
    let t = theme::ease(ui, id.with("t"), open, T_EXPAND);
    let header_h = 56.0;
    let frame = Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, theme::mix(p.stroke, p.stroke_strong, t)))
        .corner_radius(CornerRadius::same(R_ROW));
    frame.show(ui, |ui| {
        ui.set_width(width);
        ui.spacing_mut().item_spacing.y = 0.0;
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, header_h), Sense::click());
        resp.widget_info(|| WidgetInfo::labeled(WidgetType::CollapsingHeader, true, title));
        if resp.clicked() {
            open = !open;
            set_row_open(ui.ctx(), id, open);
        }
        let hover = theme::ease(ui, id.with("hover"), resp.hovered(), T_HOVER);
        if ui.is_rect_visible(rect) {
            let painter = ui.painter();
            if hover > 0.0 {
                let r = CornerRadius { nw: R_ROW, ne: R_ROW, sw: if t > 0.0 { 0 } else { R_ROW }, se: if t > 0.0 { 0 } else { R_ROW } };
                painter.rect_filled(rect.shrink(1.0), r, theme::mix(p.card, p.card_hover, hover));
            }
            let icon_c = theme::mix(p.dim, p.accent, t.max(hover * 0.6));
            painter.text(rect.left_center() + egui::vec2(22.0, 0.0), egui::Align2::CENTER_CENTER, ic, egui::FontId::proportional(20.0), icon_c);
            let title_font = egui::FontId::new(13.5, theme::heading(ui));
            let title_galley = painter.layout_no_wrap(title.to_uppercase(), title_font, p.text);
            let tx = rect.left() + 48.0;
            painter.galley(egui::pos2(tx, rect.center().y - title_galley.size().y / 2.0), title_galley.clone(), p.text);
            // the summary, right-aligned before the chevron, elided to the space left
            let sx0 = tx + title_galley.size().x + 18.0;
            let sx1 = rect.right() - 40.0;
            if sx1 - sx0 > 30.0 {
                let mut job = egui::text::LayoutJob::simple_singleline(summary.to_uppercase(), egui::FontId::proportional(11.5), p.dim);
                job.wrap = egui::text::TextWrapping::truncate_at_width(sx1 - sx0);
                let g = painter.layout_job(job);
                painter.galley(egui::pos2(sx1 - g.size().x, rect.center().y - g.size().y / 2.0), g, p.dim);
            }
            // chevron, turned by the animation
            let c = rect.right_center() - egui::vec2(20.0, 0.0);
            let a = t * std::f32::consts::PI;
            let pts: Vec<egui::Pos2> = [(-4.5f32, -2.0f32), (0.0, 2.5), (4.5, -2.0)]
                .iter()
                .map(|(x, y)| {
                    let (s, co) = a.sin_cos();
                    c + egui::vec2(x * co - y * s, x * s + y * co)
                })
                .collect();
            painter.add(egui::Shape::line(pts, Stroke::new(1.6, theme::mix(p.dim, p.text, hover))));
        }
        if t > 0.0 {
            let full: f32 = ui.ctx().data(|d| d.get_temp(id.with("h"))).unwrap_or(0.0);
            let shown = if t >= 1.0 { full } else { (full * t).round() };
            let (body_rect, _) = ui.allocate_exact_size(egui::vec2(width, shown), Sense::hover());
            let inner = Rect::from_min_size(body_rect.min + egui::vec2(16.0, 4.0), egui::vec2(width - 32.0, 10_000.0));
            let mut child = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::Min)).id_salt(id.with("body")));
            child.set_clip_rect(body_rect.intersect(ui.clip_rect()));
            if t < 1.0 {
                child.multiply_opacity(t);
            }
            child.spacing_mut().item_spacing.y = 7.0;
            child.painter().hline(inner.x_range(), body_rect.top() + 0.5, Stroke::new(1.0, p.stroke));
            child.add_space(10.0);
            body(&mut child);
            child.add_space(12.0);
            let used = child.min_rect().height() + 4.0;
            if (used - full).abs() > 0.5 {
                ui.ctx().data_mut(|d| d.insert_temp(id.with("h"), used));
                ui.ctx().request_repaint();
            }
        }
    });
    open
}

/// The runtime selector: an icon, a caption and dot-radio options. Returns the clicked option.
pub fn selector(ui: &mut Ui, ic: &str, caption_text: &str, options: &[&str], selected: usize, enabled: bool) -> Option<usize> {
    let p = theme::pal(ui);
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new(ic).size(16.0).color(p.dim));
        ui.label(RichText::new(caption_text.to_uppercase()).size(12.0).color(p.dim).strong());
        ui.add_space(8.0);
        for (i, o) in options.iter().enumerate() {
            let on = i == selected;
            let t = theme::ease(ui, Id::new(("selector", caption_text, i)), on, 0.18);
            let galley = ui.fonts_mut(|f| f.layout_no_wrap(o.to_string(), egui::FontId::proportional(13.0), p.text));
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(galley.size().x + 22.0, 24.0), if enabled { Sense::click() } else { Sense::hover() });
            resp.widget_info(|| WidgetInfo::selected(WidgetType::RadioButton, enabled, on, *o));
            if ui.is_rect_visible(rect) {
                let painter = ui.painter();
                let c = rect.left_center() + egui::vec2(6.0, 0.0);
                painter.circle_stroke(c, 5.0, Stroke::new(1.2, theme::mix(p.faint, p.accent, t)));
                if t > 0.0 {
                    painter.circle_filled(c, 3.0 * t, p.accent);
                }
                let col = if enabled { theme::mix(p.dim, p.text, t) } else { p.faint };
                painter.galley(egui::pos2(c.x + 10.0, rect.center().y - galley.size().y / 2.0), galley, col);
            }
            if resp.clicked() && !on {
                clicked = Some(i);
            }
            ui.add_space(6.0);
        }
    });
    clicked
}

/// A device card: a big outline icon, the name, a pill; controllers show their battery.
pub fn device_card(ui: &mut Ui, id: Id, ic: &str, v: &DeviceView, big: bool, extra: impl FnOnce(&mut Ui)) {
    let p = theme::pal(ui);
    Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.stroke))
        .corner_radius(CornerRadius::same(R_CARD + 2))
        .inner_margin(Margin::symmetric(18, if big { 22 } else { 14 }))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if let Some(b) = v.battery {
                let (bi, text) = match b {
                    Battery::Unknown => (icon::BATTERY_EMPTY, "battery unknown".to_string()),
                    Battery::Mock(x) => (if x > 70 { icon::BATTERY_HIGH } else { icon::BATTERY_MEDIUM }, format!("{x}% (mock)")),
                    Battery::Level(x) => (if x > 70 { icon::BATTERY_HIGH } else if x > 30 { icon::BATTERY_MEDIUM } else { icon::BATTERY_LOW }, format!("{x}%")),
                };
                ui.label(RichText::new(format!("{bi} {text}")).size(11.0).color(p.faint));
            }
            ui.horizontal(|ui| {
                let size = if big { 64.0 } else { 34.0 };
                let col = theme::fade(ui, id.with("icon"), if v.state == Dev::Off { p.faint } else { p.accent });
                ui.label(RichText::new(ic).size(size).color(col));
                ui.add_space(if big { 18.0 } else { 6.0 });
                ui.vertical(|ui| {
                    ui.add_space(if big { 6.0 } else { 0.0 });
                    ui.label(RichText::new(&v.name).size(if big { 20.0 } else { 15.0 }).family(theme::heading(ui)).color(p.text));
                    pill(ui, id.with("pill"), &v.label, dev_color(&p, v.state));
                    if big && !v.detail.is_empty() {
                        ui.label(RichText::new(&v.detail).size(11.5).color(p.faint));
                    }
                    extra(ui);
                });
            });
        });
}

/// An on/off switch with a sliding knob and a label (the settings rows).
pub fn toggle(ui: &mut Ui, on: &mut bool, label: &str, enabled: bool) -> egui::Response {
    let p = theme::pal(ui);
    let h: f32 = 20.0;
    let w = 36.0;
    let text_w = ui.fonts_mut(|f| f.layout_no_wrap(label.to_string(), egui::FontId::proportional(14.0), p.text).size().x);
    let (rect, mut resp) = ui.allocate_exact_size(egui::vec2(w + 10.0 + text_w, h.max(22.0)), if enabled { Sense::click() } else { Sense::hover() });
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    resp.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, enabled, *on, label));
    let t = theme::ease(ui, resp.id.with("knob"), *on, 0.16);
    if ui.is_rect_visible(rect) {
        let track = Rect::from_min_size(egui::pos2(rect.left(), rect.center().y - h / 2.0), egui::vec2(w, h));
        let off_c = if p.dark { Color32::from_rgb(48, 52, 66) } else { Color32::from_rgb(205, 210, 222) };
        let mut fill = theme::mix(off_c, p.accent, t);
        let mut knob_c = Color32::WHITE;
        let mut text_c = p.text;
        if !enabled {
            fill = fill.gamma_multiply(0.45);
            knob_c = knob_c.gamma_multiply(0.6);
            text_c = p.faint;
        }
        let painter = ui.painter();
        painter.rect_filled(track, CornerRadius::same(255), fill);
        let x = egui::lerp((track.left() + h / 2.0)..=(track.right() - h / 2.0), t);
        painter.circle_filled(egui::pos2(x, track.center().y), h / 2.0 - 3.0, knob_c);
        painter.text(egui::pos2(track.right() + 10.0, rect.center().y), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(14.0), text_c);
    }
    resp
}
