//! Drawing helpers shared by the pages, in the look of [`crate::theme`]: flat cards with a small-caps title, setting
//! rows (label on the left, the control on the right, details behind a "?"), switches, segmented choices, pills
//! whose colour fades, big action buttons, a throttled spinner.
use crate::theme::{self, caps, icon, Palette, R_CARD, R_ROW, R_SMALL, T_HOVER};
use eframe::egui::{self, Align, Color32, CornerRadius, Frame, Id, Layout, Margin, Rect, RichText, Sense, Stroke, Ui, WidgetInfo, WidgetType};
use omarchyvr_doctor::Level;
use omarchyvr_gui_core::devices::Dev;
use omarchyvr_gui_core::preflight::Sev;

/// State colours for the current theme (the pages' short names for [`theme::Palette`]).
#[derive(Debug, Clone, Copy)]
pub struct Pal {
    /// Good.
    pub ok: Color32,
    /// Information.
    pub info: Color32,
    /// Warning / needs attention.
    pub warn: Color32,
    /// Warning as text.
    pub warn_text: Color32,
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
        warn_text: p.warn_text,
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

/// A small tag: capitals in a thin frame of its colour.
pub fn chip(ui: &mut Ui, text: &str, color: Color32) {
    Frame::new()
        .fill(color.gamma_multiply(0.10))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.55)))
        .corner_radius(CornerRadius::same(3))
        .inner_margin(Margin::symmetric(6, 1))
        .show(ui, |ui| {
            ui.label(caps(text, 10.0, color).strong());
        });
}

/// A status pill: a dot and the state in capitals in a thin frame; the colour fades when the state changes.
pub fn pill(ui: &mut Ui, id: Id, text: &str, color: Color32) -> egui::Response {
    let c = theme::fade(ui, id, color);
    let p = theme::pal(ui);
    Frame::new()
        .fill(c.gamma_multiply(0.08))
        .stroke(Stroke::new(1.0, c.gamma_multiply(0.6)))
        .corner_radius(CornerRadius::same(3))
        .inner_margin(Margin::symmetric(7, 2))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                dot(ui, c, 3.0);
                ui.label(caps(text, 10.5, p.text));
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

/// The frame of a card.
pub fn card_frame(p: &Palette) -> Frame {
    Frame::new().fill(p.card).stroke(Stroke::new(1.0, p.stroke)).corner_radius(CornerRadius::same(R_CARD)).inner_margin(Margin::same(16))
}

/// A card: flat, a hairline, the card colour.
pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let p = theme::pal(ui);
    card_frame(&p)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// A card with a small-caps title (and a "?" when `help` is given).
pub fn section<R>(ui: &mut Ui, title_text: &str, help_text: Option<&str>, add: impl FnOnce(&mut Ui) -> R) -> R {
    section_with(ui, title_text, help_text, |_| {}, add)
}

/// A card with a title, a "?" and something on the right of the title (a chip, a button).
pub fn section_with<R>(ui: &mut Ui, title_text: &str, help_text: Option<&str>, right: impl FnOnce(&mut Ui), add: impl FnOnce(&mut Ui) -> R) -> R {
    card(ui, |ui| {
        title_row(ui, title_text, help_text, right);
        ui.add_space(6.0);
        add(ui)
    })
}

/// The title line of a card: the title, a "?", and `right` aligned to the right.
pub fn title_row(ui: &mut Ui, title_text: &str, help_text: Option<&str>, right: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.set_min_height(22.0);
        title(ui, title_text);
        if let Some(h) = help_text {
            help(ui, h);
        }
        at_right(ui, &format!("title {title_text}"), right);
    });
}

/// In a horizontal row: `add` placed so that it ends at the row's right edge, its widgets still left to right (a
/// right-to-left layout would turn pills, segmented choices and sliders around). The width comes from the previous
/// frame; the first frame draws it right after what is on the left. `key` tells the rows of one card apart (sibling
/// rows share their `Ui::id`): their label.
pub fn at_right<R>(ui: &mut Ui, key: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    // sibling Uis share their id (egui): the same label twice on a page is told apart by its order in this pass
    let base = ui.id().with(("at-right", key));
    let pass = ui.ctx().cumulative_pass_nr();
    let nth = ui.ctx().data_mut(|d| {
        let seen = d.get_temp_mut_or_default::<(u64, std::collections::HashMap<Id, u32>)>(Id::new("at-right-seen"));
        if seen.0 != pass {
            *seen = (pass, Default::default());
        }
        let n = seen.1.entry(base).or_insert(0);
        *n += 1;
        *n
    });
    let id = base.with(nth);
    let avail = ui.available_width();
    let prev: f32 = ui.ctx().data(|d| d.get_temp(id)).unwrap_or(avail);
    // a point of slack: a row that is exactly as wide as the room left would wrap its last item
    let gap = (avail - prev - 1.0).max(0.0);
    if gap > 0.0 {
        ui.add_space(gap);
    }
    let r = ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        // a slider and its value box fit in what the label leaves
        let sw = ui.spacing().slider_width.min((avail - 140.0).max(48.0));
        ui.spacing_mut().slider_width = sw;
        ui.horizontal(add).inner
    });
    let used = r.response.rect.width();
    if (used - prev).abs() > 0.5 {
        ui.ctx().data_mut(|d| d.insert_temp(id, used));
        ui.ctx().request_repaint();
    }
    r.inner
}

/// An inset box inside a card.
pub fn inset<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let p = theme::pal(ui);
    Frame::new()
        .fill(p.inset)
        .stroke(Stroke::new(1.0, p.stroke))
        .corner_radius(CornerRadius::same(R_SMALL))
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// The title of a card, in small caps.
pub fn title(ui: &mut Ui, text: &str) {
    ui.label(caps(text, 14.0, theme::pal(ui).text).family(theme::heading(ui)));
}

/// A small-caps label over a group.
pub fn caption(ui: &mut Ui, text: &str) {
    ui.label(caps(text, 10.5, theme::pal(ui).faint).strong());
}

/// Open the next "?" that is drawn (for the screenshot of an open popup).
pub fn open_next_help(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("open-next-help"), true));
}

/// The "?" next to a label, the one help widget of the app: the explanation shows on hover and, for the headset
/// (WayVR: hover is unreliable there), in a small popup that a click opens and closes; a click elsewhere or Esc
/// closes it.
pub fn help(ui: &mut Ui, text: &str) -> egui::Response {
    let p = theme::pal(ui);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(15.0, 15.0), Sense::click());
    // screenshots: `open_next_help` opens the next "?" drawn
    if ui.ctx().data_mut(|d| d.remove_temp::<bool>(egui::Id::new("open-next-help"))).unwrap_or(false) {
        egui::Popup::open_id(ui.ctx(), egui::Popup::default_response_id(&resp));
    }
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
    let c = if resp.hovered() || open { p.text } else { p.faint };
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, icon::QUESTION, egui::FontId::proportional(13.0), c);
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "Help"));
    let shown = egui::Popup::from_toggle_button_response(&resp)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_max_width(360.0);
            ui.label(text);
        })
        .is_some();
    if shown {
        resp
    } else {
        resp.on_hover_ui(|ui| {
            ui.set_max_width(360.0);
            ui.label(text);
        })
    }
}

/// A setting row: the label (small caps, a "?" when `help` is given) on the left, `control` on the right (its
/// widgets left to right).
pub fn setting<R>(ui: &mut Ui, label: &str, help_text: Option<&str>, control: impl FnOnce(&mut Ui) -> R) -> R {
    let p = theme::pal(ui);
    ui.horizontal(|ui| {
        ui.set_min_height(24.0);
        ui.label(caps(label, 11.0, p.text));
        if let Some(h) = help_text {
            help(ui, h);
        }
        at_right(ui, &format!("setting {label}"), control)
    })
    .inner
}

/// A switch alone (accessible name: `label`).
pub fn switch(ui: &mut Ui, on: &mut bool, label: &str, enabled: bool) -> egui::Response {
    let p = theme::pal(ui);
    let (w, h): (f32, f32) = (30.0, 16.0);
    let (rect, mut resp) = ui.allocate_exact_size(egui::vec2(w, h.max(20.0)), if enabled { Sense::click() } else { Sense::hover() });
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    resp.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, enabled, *on, label));
    let t = theme::ease(ui, resp.id.with("knob"), *on, 0.16);
    if ui.is_rect_visible(rect) {
        let track = Rect::from_center_size(rect.center(), egui::vec2(w, h));
        let off_c = p.switch_off;
        let mut fill = theme::mix(off_c, p.accent, t);
        let mut knob = theme::mix(if p.dark { Color32::from_rgb(150, 156, 172) } else { Color32::WHITE }, p.on_accent, t);
        if !enabled {
            fill = fill.gamma_multiply(0.45);
            knob = knob.gamma_multiply(0.55);
        }
        let painter = ui.painter();
        painter.rect_filled(track, CornerRadius::same(255), fill);
        let x = egui::lerp((track.left() + h / 2.0)..=(track.right() - h / 2.0), t);
        painter.circle_filled(egui::pos2(x, track.center().y), h / 2.0 - 3.0, knob);
    }
    resp
}

/// A setting row with a switch on the right.
pub fn switch_row(ui: &mut Ui, on: &mut bool, label: &str, help_text: Option<&str>, enabled: bool) -> egui::Response {
    setting(ui, label, help_text, |ui| switch(ui, on, label, enabled))
}

/// An on/off switch. In a column it is a setting row ([`switch_row`]: the label left, the switch right); in a line
/// of other controls the switch comes first and its label after it.
pub fn toggle(ui: &mut Ui, on: &mut bool, label: &str, enabled: bool) -> egui::Response {
    if ui.layout().is_vertical() && !label.is_empty() {
        return switch_row(ui, on, label, None, enabled);
    }
    let p = theme::pal(ui);
    let font = egui::FontId::proportional(13.5);
    let text_w = if label.is_empty() { 0.0 } else { ui.fonts_mut(|f| f.layout_no_wrap(label.to_string(), font.clone(), p.text).size().x) + 9.0 };
    let (w, h) = (30.0, 16.0);
    let (rect, mut resp) = ui.allocate_exact_size(egui::vec2(w + text_w, 22.0), if enabled { Sense::click() } else { Sense::hover() });
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    resp.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, enabled, *on, label));
    let t = theme::ease(ui, resp.id.with("knob"), *on, 0.16);
    if ui.is_rect_visible(rect) {
        let track = Rect::from_min_size(egui::pos2(rect.left(), rect.center().y - h / 2.0), egui::vec2(w, h));
        let off_c = p.switch_off;
        let mut fill = theme::mix(off_c, p.accent, t);
        let mut knob = theme::mix(if p.dark { Color32::from_rgb(150, 156, 172) } else { Color32::WHITE }, p.on_accent, t);
        let mut text_c = p.text;
        if !enabled {
            fill = fill.gamma_multiply(0.45);
            knob = knob.gamma_multiply(0.55);
            text_c = p.faint;
        }
        let painter = ui.painter();
        painter.rect_filled(track, CornerRadius::same(255), fill);
        let x = egui::lerp((track.left() + h / 2.0)..=(track.right() - h / 2.0), t);
        painter.circle_filled(egui::pos2(x, track.center().y), h / 2.0 - 3.0, knob);
        if !label.is_empty() {
            painter.text(egui::pos2(track.right() + 9.0, rect.center().y), egui::Align2::LEFT_CENTER, label, font, text_c);
        }
    }
    resp
}

/// Width of a segmented choice.
fn seg_width(ui: &Ui, options: &[&str]) -> f32 {
    let font = egui::FontId::proportional(11.0);
    options
        .iter()
        .map(|o| ui.fonts_mut(|f| f.layout_no_wrap(o.to_uppercase(), font.clone(), Color32::WHITE).size().x) + o.chars().count() as f32 * theme::CAPS_SPACING + 18.0)
        .sum()
}

/// A segmented choice: the options side by side, the chosen one filled. Returns the clicked option.
pub fn seg(ui: &mut Ui, id: &str, options: &[&str], selected: usize, enabled: bool) -> Option<usize> {
    seg_inner(ui, id, options, selected, enabled, true)
}

/// [`seg`], wrapping to more lines (`wrap`) or always on one line (next to a label that left room for it).
fn seg_inner(ui: &mut Ui, id: &str, options: &[&str], selected: usize, enabled: bool, wrap: bool) -> Option<usize> {
    let p = theme::pal(ui);
    let mut clicked = None;
    let id = Id::new(("seg", id));
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let layout = if wrap { Layout::left_to_right(Align::Center).with_main_wrap(true) } else { Layout::left_to_right(Align::Center) };
        // like ui.horizontal: one row tall to start with, so Align::Center centres in the row
        ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), 24.0), layout, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 4.0);
            let n = options.len();
            for (i, o) in options.iter().enumerate() {
                let on = i == selected;
                let text = caps(o, 11.0, p.text);
                let galley = egui::WidgetText::from(text).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Button);
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(galley.size().x + 18.0, 24.0), if enabled { Sense::click() } else { Sense::hover() });
                resp.widget_info(|| WidgetInfo::selected(WidgetType::RadioButton, enabled, on, *o));
                let t = theme::ease(ui, id.with(i), on, 0.16);
                let hover = theme::ease(ui, id.with(("h", i)), enabled && resp.hovered() && !on, T_HOVER);
                if ui.is_rect_visible(rect) {
                    let r = CornerRadius {
                        nw: if i == 0 { R_SMALL } else { 0 },
                        sw: if i == 0 { R_SMALL } else { 0 },
                        ne: if i + 1 == n { R_SMALL } else { 0 },
                        se: if i + 1 == n { R_SMALL } else { 0 },
                    };
                    let fill = theme::mix(theme::mix(Color32::TRANSPARENT, p.card_hover, hover), p.accent.gamma_multiply(0.55), t);
                    let stroke = theme::mix(p.stroke_strong, p.accent, t);
                    ui.painter().rect(rect, r, fill, Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
                    let col = if !enabled {
                        p.faint
                    } else {
                        theme::mix(p.dim, p.text, t.max(hover))
                    };
                    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, col);
                }
                if resp.clicked() && !on {
                    clicked = Some(i);
                }
            }
        });
    });
    clicked
}

/// A setting row with a segmented choice; the choice goes under the label when the row is too narrow.
pub fn seg_row(ui: &mut Ui, label: &str, help_text: Option<&str>, options: &[&str], selected: usize, enabled: bool) -> Option<usize> {
    let label_w = ui.fonts_mut(|f| f.layout_no_wrap(label.to_uppercase(), egui::FontId::proportional(11.0), Color32::WHITE).size().x) + 40.0;
    if label_w + seg_width(ui, options) + 12.0 <= ui.available_width() {
        setting(ui, label, help_text, |ui| seg_inner(ui, label, options, selected, enabled, false))
    } else {
        let p = theme::pal(ui);
        ui.horizontal(|ui| {
            ui.label(caps(label, 11.0, p.text));
            if let Some(h) = help_text {
                help(ui, h);
            }
        });
        seg(ui, label, options, selected, enabled)
    }
}

/// The old name of [`seg_row`] (the icon is no longer drawn).
pub fn selector(ui: &mut Ui, _ic: &str, caption_text: &str, options: &[&str], selected: usize, enabled: bool) -> Option<usize> {
    seg_row(ui, caption_text, None, options, selected, enabled)
}

/// A key and its value side by side (the headset card): the key in small caps, the value in plain text.
pub fn kv_row(ui: &mut Ui, key: &str, key_w: f32, add: impl FnOnce(&mut Ui)) {
    let p = theme::pal(ui);
    ui.horizontal(|ui| {
        ui.set_min_height(22.0);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(key_w, 18.0), Sense::hover());
        ui.painter().text(rect.left_center(), egui::Align2::LEFT_CENTER, key.to_uppercase(), egui::FontId::proportional(10.5), p.faint);
        add(ui);
    });
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
    /// Outlined.
    Secondary,
    /// Red fill.
    Danger,
}

/// A big button with an icon and a small-caps label (Play, Work, Stop). Hover lightens it smoothly.
pub fn action(ui: &mut Ui, ic: &str, text: &str, kind: Kind, enabled: bool, width: f32) -> egui::Response {
    let p = theme::pal(ui);
    let h = 40.0;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, h), if enabled { Sense::click() } else { Sense::hover() });
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, text));
    let hover = theme::ease(ui, resp.id.with("hover"), enabled && resp.hovered(), T_HOVER);
    let (fill, fg, stroke) = match (kind, enabled) {
        (_, false) => (Color32::TRANSPARENT, p.faint, p.stroke),
        (Kind::Primary, true) => (theme::mix(p.accent, p.accent_hover, hover), p.on_accent, p.accent_hover),
        (Kind::Danger, true) => (theme::mix(p.fail, p.fail.gamma_multiply(1.15), hover), Color32::WHITE, p.fail),
        (Kind::Secondary, true) => (theme::mix(Color32::TRANSPARENT, p.card_hover, hover), p.text, theme::mix(p.stroke_strong, p.accent, hover * 0.8)),
    };
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect(rect, CornerRadius::same(R_ROW), fill, Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
        let job = egui::text::LayoutJob::single_section(
            format!("{ic}  {}", text.to_uppercase()),
            egui::TextFormat { font_id: egui::FontId::new(13.0, theme::heading(ui)), color: fg, extra_letter_spacing: 1.0, ..Default::default() },
        );
        let g = painter.layout_job(job);
        painter.galley(rect.center() - g.size() / 2.0, g, fg);
    }
    resp
}

/// A primary (accent) button.
pub fn primary(ui: &mut Ui, text: &str, enabled: bool) -> egui::Response {
    let p = theme::pal(ui);
    let (fill, fg) = if enabled { (p.accent, p.on_accent) } else { (Color32::TRANSPARENT, p.faint) };
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(text).color(fg).strong())
            .fill(fill)
            .stroke(Stroke::new(1.0, if enabled { p.accent_hover } else { p.stroke }))
            .corner_radius(CornerRadius::same(R_SMALL)),
    )
}

/// A quiet outlined button with an icon and a small-caps label (Room setup, Restart service, Test sound). With
/// `compact` only the icon shows (the label is the tooltip and the accessible name).
pub fn ghost_btn(ui: &mut Ui, ic: &str, text: &str, enabled: bool, compact: bool) -> egui::Response {
    let p = theme::pal(ui);
    let label = if compact { RichText::new(ic).size(14.0).color(p.dim) } else { caps(&format!("{ic} {text}"), 10.5, p.dim) };
    let r = outlined(ui, label, text, enabled);
    if compact {
        r.on_hover_text(text)
    } else {
        r
    }
}

/// [`ghost`] with a shorter visible label than its accessible name ("Pair" for "Pair left").
pub fn ghost_short(ui: &mut Ui, ic: &str, short: &str, name: &str, enabled: bool) -> egui::Response {
    let p = theme::pal(ui);
    outlined(ui, caps(&format!("{ic} {short}"), 10.5, p.dim), name, enabled)
}

fn outlined(ui: &mut Ui, label: RichText, name: &str, enabled: bool) -> egui::Response {
    let p = theme::pal(ui);
    let r = ui.add_enabled(enabled, egui::Button::new(label).fill(Color32::TRANSPARENT).stroke(Stroke::new(1.0, p.stroke_strong)).corner_radius(CornerRadius::same(R_SMALL)).min_size(egui::vec2(0.0, 26.0)));
    r.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, name));
    r
}

/// A quiet outlined button with an icon and a small-caps label.
pub fn ghost(ui: &mut Ui, ic: &str, text: &str, enabled: bool) -> egui::Response {
    ghost_btn(ui, ic, text, enabled, false)
}

/// A button with an icon and a small-caps label, outlined; its accessible name is the text alone (screen readers,
/// the UI tests).
pub fn ibutton(ui: &mut Ui, ic: &str, text: &str, enabled: bool) -> egui::Response {
    ghost_btn(ui, ic, text, enabled, false)
}

/// A small-caps outlined button without an icon.
pub fn tbutton(ui: &mut Ui, text: &str, enabled: bool) -> egui::Response {
    let p = theme::pal(ui);
    let r = ui.add_enabled(enabled, egui::Button::new(caps(text, 10.5, p.dim)).fill(Color32::TRANSPARENT).stroke(Stroke::new(1.0, p.stroke_strong)).corner_radius(CornerRadius::same(R_SMALL)).min_size(egui::vec2(0.0, 26.0)));
    r.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, text));
    r
}

/// A small button with an icon (accessible name: the text).
pub fn ismall(ui: &mut Ui, ic: &str, text: &str) -> egui::Response {
    let p = theme::pal(ui);
    let r = ui.add(egui::Button::new(caps(&format!("{ic} {text}"), 9.5, p.dim)).fill(Color32::TRANSPARENT).stroke(Stroke::new(1.0, p.stroke_strong)).corner_radius(CornerRadius::same(3)));
    r.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, text));
    r
}

/// Key / value row in a grid.
pub fn kv(ui: &mut Ui, key: &str, value: impl Into<egui::WidgetText>) {
    ui.label(RichText::new(key).color(theme::pal(ui).dim));
    ui.add(egui::Label::new(value).wrap());
    ui.end_row();
}

/// Muted wrapped text (a status line, a short note).
pub fn note(ui: &mut Ui, text: &str) {
    let c = theme::pal(ui).dim;
    ui.add(egui::Label::new(RichText::new(text).size(12.5).color(c)).wrap());
}

/// A small status line in a colour.
pub fn status(ui: &mut Ui, text: &str, color: Color32) {
    ui.add(egui::Label::new(RichText::new(text).size(12.0).color(color)).wrap());
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

/// Cards side by side (`weights`: their share of the width) with the same height: each card is as tall as the
/// tallest one's content (measured in the previous frame). One column when the row is narrower than `min_w` per
/// card.
pub fn card_row(ui: &mut Ui, id: &str, weights: &[f32], min_w: f32, mut add: impl FnMut(usize, &mut Ui)) {
    let id = Id::new(("card-row", id));
    let gap = 12.0;
    let avail = ui.available_width();
    let n = weights.len().max(1);
    let total: f32 = weights.iter().sum::<f32>().max(0.001);
    let inner_w = avail - gap * (n - 1) as f32;
    let narrow = weights.iter().any(|w| inner_w * w / total < min_w);
    let p = theme::pal(ui);
    if narrow {
        for i in 0..n {
            card(ui, |ui| add(i, ui));
            if i + 1 < n {
                ui.add_space(gap);
            }
        }
        return;
    }
    let target: f32 = ui.ctx().data(|d| d.get_temp(id)).unwrap_or(0.0);
    let mut tallest: f32 = 0.0;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for (i, w) in weights.iter().enumerate() {
            let cw = (inner_w * w / total).floor();
            ui.allocate_ui_with_layout(egui::vec2(cw, 0.0), Layout::top_down(Align::Min), |ui| {
                card_frame(&p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let top = ui.min_rect().top();
                    add(i, ui);
                    let natural = ui.min_rect().bottom() - top;
                    tallest = tallest.max(natural);
                    // set_min_height would reserve that much more below the cursor: reach down to the target instead
                    if target > natural + 0.5 {
                        ui.expand_to_include_y(top + target);
                    }
                });
            });
        }
    });
    if (tallest - target).abs() > 0.5 {
        ui.ctx().data_mut(|d| d.insert_temp(id, tallest));
        ui.ctx().request_repaint();
    }
}

/// A navigation entry of the sidebar: icon, small-caps name, an optional count; the open page is filled.
pub fn nav_item(ui: &mut Ui, selected: bool, ic: &str, label: &str, badge: Option<(String, Color32)>) -> egui::Response {
    let p = theme::pal(ui);
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 30.0), Sense::click());
    resp.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, selected, label));
    let t = theme::ease(ui, resp.id.with("sel"), selected, 0.18);
    let hover = theme::ease(ui, resp.id.with("hover"), resp.hovered() && !selected, T_HOVER);
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let fill = theme::mix(theme::mix(Color32::TRANSPARENT, p.card_hover, hover), p.accent.gamma_multiply(0.16), t);
        let stroke = theme::mix(Color32::TRANSPARENT, p.accent.gamma_multiply(0.85), t);
        painter.rect(rect, CornerRadius::same(R_SMALL), fill, Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
        let fg = theme::mix(p.dim, p.text, t.max(hover));
        painter.text(rect.left_center() + egui::vec2(18.0, 0.0), egui::Align2::CENTER_CENTER, ic, egui::FontId::proportional(14.5), theme::mix(fg, p.accent_hover, t * 0.5));
        let job = egui::text::LayoutJob::single_section(
            label.to_uppercase(),
            egui::TextFormat { font_id: egui::FontId::proportional(11.0), color: fg, extra_letter_spacing: 0.9, ..Default::default() },
        );
        let g = painter.layout_job(job);
        painter.galley(egui::pos2(rect.left() + 34.0, rect.center().y - g.size().y / 2.0), g, fg);
        if let Some((text, c)) = badge {
            let g = painter.layout_no_wrap(text, egui::FontId::proportional(10.5), c);
            let b = Rect::from_center_size(egui::pos2(rect.right() - 10.0 - g.size().x / 2.0, rect.center().y), g.size() + egui::vec2(10.0, 2.0));
            painter.rect(b, CornerRadius::same(3), c.gamma_multiply(0.12), Stroke::new(1.0, c.gamma_multiply(0.6)), egui::StrokeKind::Inside);
            painter.galley(b.center() - g.size() / 2.0, g, c);
        }
    }
    resp
}
