//! The Appearance card (on the Display page): the colours of the app. Follow Omarchy (default), the omarchyvr colours,
//! or one installed Omarchy theme from a list with a colour swatch row each. See `crate::appearance` and docs/GUI.md.
use crate::app::PimaxApp;
use crate::theme;
use crate::widgets::{section, seg_row};
use eframe::egui::{self, Color32, CornerRadius, RichText, Sense, Stroke, Ui};
use omarchyvr_gui_core::colors::{Colours, OmarchyColors, ThemeName};

/// The card.
pub fn show(ui: &mut Ui, app: &mut PimaxApp) {
    let p = theme::pal(ui);
    // the list is read in the background while the card is on screen, so it is there when "Pick a theme" is clicked
    let ctx = ui.ctx().clone();
    let _ = app.appearance.themes(&ctx);
    section(
        ui,
        "Appearance",
        Some("Follow Omarchy: the app uses the colours of your Omarchy theme and changes with it within a couple of seconds. omarchyvr: the app's own dark colours. Pick a theme: one of the installed Omarchy themes, whatever Omarchy itself uses. Text that would be hard to read in a theme keeps a readable colour."),
        |ui| {
            let choice = app.core.state.colours;
            let cur = match choice {
                Colours::Follow => 0,
                Colours::Default => 1,
                Colours::Theme(_) => 2,
            };
            if let Some(i) = seg_row(ui, "Colours", None, &["Follow Omarchy", "omarchyvr", "Pick a theme"], cur, true) {
                let next = match i {
                    0 => Colours::Follow,
                    1 => Colours::Default,
                    _ => {
                        // start from the theme Omarchy uses now, else the first one installed
                        let now = app.appearance.omarchy_current();
                        let themes = app.appearance.themes(&ctx);
                        let name = now.filter(|n| themes.iter().any(|t| &t.name == n)).or_else(|| themes.first().map(|t| t.name.clone()));
                        name.and_then(|n| ThemeName::new(&n)).map(Colours::Theme).unwrap_or(Colours::Follow)
                    }
                };
                app.core.set_colours(next);
            }
            let note = if app.appearance.enabled { app.appearance.note.clone() } else { demo_note(choice) };
            if !note.is_empty() {
                ui.add(egui::Label::new(RichText::new(note).size(11.5).color(p.faint)).wrap());
            }
            if let Colours::Theme(sel) = choice {
                let themes = app.appearance.themes(&ctx).to_vec();
                if themes.is_empty() {
                    ui.label(RichText::new("No Omarchy themes were found on this PC.").size(12.0).color(p.warn_text));
                }
                let mut pick = None;
                let h = (themes.len() as f32 * 31.0).clamp(31.0, 250.0);
                egui::ScrollArea::vertical().id_salt("appearance-themes").min_scrolled_height(h).max_height(h).auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    for t in &themes {
                        if theme_row(ui, &t.name, &t.colors, t.name == sel.as_str()) {
                            pick = ThemeName::new(&t.name);
                        }
                    }
                });
                if let Some(n) = pick {
                    app.core.set_colours(Colours::Theme(n));
                }
            }
        },
    );
}

/// What the note says when the real Omarchy files are not looked at (demo data, pictures).
fn demo_note(c: Colours) -> String {
    match c {
        Colours::Follow => "Following the Omarchy theme; it changes here when you change it in Omarchy.".into(),
        Colours::Default => "The omarchyvr colours.".into(),
        Colours::Theme(n) => format!("The Omarchy theme \"{}\".", n.as_str()),
    }
}

/// One theme in the list: its name and a row of its colours. True when clicked.
fn theme_row(ui: &mut Ui, name: &str, c: &OmarchyColors, selected: bool) -> bool {
    let p = theme::pal(ui);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 28.0), Sense::click());
    let hover = theme::ease(ui, resp.id.with("hover"), resp.hovered(), theme::T_HOVER);
    if ui.is_rect_visible(rect) {
        let fill = if selected { p.accent.gamma_multiply(0.18) } else { theme::mix(Color32::TRANSPARENT, p.card_hover, hover) };
        let stroke = if selected { p.accent } else { p.stroke };
        ui.painter().rect(rect, CornerRadius::same(theme::R_SMALL), fill, Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
        let col = if selected { p.text } else { p.dim };
        ui.painter().text(rect.left_center() + egui::vec2(10.0, 0.0), egui::Align2::LEFT_CENTER, name, egui::FontId::proportional(12.5), col);
        // the swatches: window, card, text, accent, green, yellow, red, blue
        let roles = ["background", "lighter_background", "bright_foreground", "accent", "green", "yellow", "red", "blue"];
        let (sw, gap) = (14.0, 3.0);
        let total = roles.len() as f32 * sw + (roles.len() as f32 - 1.0) * gap;
        let mut x = rect.right() - 10.0 - total;
        for r in roles {
            if let Some(v) = c.get(r) {
                let sq = egui::Rect::from_min_size(egui::pos2(x, rect.center().y - sw / 2.0), egui::vec2(sw, sw));
                ui.painter().rect(sq, CornerRadius::same(3), Color32::from_rgb(v[0], v[1], v[2]), Stroke::new(1.0, p.stroke_strong), egui::StrokeKind::Inside);
            }
            x += sw + gap;
        }
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, selected, name));
    resp.clicked() && !selected
}
