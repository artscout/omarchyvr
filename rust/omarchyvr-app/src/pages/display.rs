//! Display: refresh rate and render quality, brightness, performance (game mode, compositor budget), the lenses
//! (IPD). Each setting is one row; what it does is behind its "?".
use crate::app::PimaxApp;
use crate::pages::{editable, tool_button, two_columns};
use crate::theme::{self, icon};
use crate::widgets::{self, section, seg_row, switch_row};
use eframe::egui::{self, RichText, Ui};
use omarchyvr_gui_core::quality;
use omarchyvr_gui_core::tools::ToolId;

/// Draw the page.
pub fn show(ui: &mut Ui, app: &mut PimaxApp) {
    two_columns(ui, 380.0, |i, ui| {
        if i == 0 {
            picture(ui, app);
            brightness(ui, app);
        } else {
            performance(ui, app);
            lenses(ui, app);
        }
    });
}

/// Refresh rate, render quality, the custom scale (developer mode).
fn picture(ui: &mut Ui, app: &mut PimaxApp) {
    let ed = editable(app);
    let p = theme::pal(ui);
    section(ui, "Display", None, |ui| {
        let cur = usize::from(app.core.toggles.hz90);
        if let Some(i) = seg_row(ui, "Refresh rate", Some("90 Hz is the default: the headset starts at 90 Hz with exactly the commands Pimax Play sends on Windows. 72 Hz stays available. Applied when VR starts."), &["72 Hz", "90 Hz"], cur, ed) {
            app.core.toggles.hz90 = i == 1;
        }
        let pct = app.core.state.scale_pct;
        let names: Vec<&str> = quality::PRESETS.iter().map(|x| x.0).collect();
        let mut help: Vec<String> = quality::PRESETS.iter().map(|(n, v, what)| format!("{n}: {} - {what}", quality::describe(*v))).collect();
        help.push("Applies at the next Play.".into());
        if app.core.facts.games_steamvr {
            help.push("With SteamVR, SteamVR's own resolution setting multiplies this (on Auto it picks up to 150 % of the pixels on a fast GPU).".into());
        }
        let cur = quality::PRESETS.iter().position(|x| x.1 == pct).unwrap_or(usize::MAX);
        if let Some(i) = seg_row(ui, "Render quality", Some(&help.join("\n")), &names, cur, ed) {
            app.core.set_scale(quality::PRESETS[i].1);
        }
        ui.label(RichText::new(quality::describe(pct)).size(11.5).color(p.faint));
        if app.core.state.dev_mode {
            let mut cp = pct;
            let r = widgets::setting(ui, "Render scale", Some("XRT_COMPOSITOR_SCALE_PERCENTAGE: Monado's recommended size per eye (both sides scale by it); above 100 % is supersampling."), |ui| {
                ui.add_enabled(ed, egui::Slider::new(&mut cp, 50..=200).step_by(5.0).custom_formatter(|v, _| format!("{v:.0} %")))
            });
            if r.changed() {
                app.core.set_scale(cp);
            }
        }
        if !ed {
            ui.label(RichText::new("Changes apply at the next start.").size(11.5).color(p.faint));
        }
    });
}

/// Image brightness (next start) and the panel backlight (now).
fn brightness(ui: &mut Ui, app: &mut PimaxApp) {
    section(ui, "Brightness", None, |ui| {
        let cur = if app.core.facts.image_brightness.is_empty() { "bright".to_string() } else { app.core.facts.image_brightness.clone() };
        let names: Vec<&str> = quality::IMAGES.iter().map(|x| x.1).collect();
        let help = quality::IMAGES.iter().map(|(_, l, what)| format!("{l}: {what}")).collect::<Vec<_>>().join("\n")
            + "\nLifts the mid tones of games; black and white stay as they are. Applies to the next start.";
        let sel = quality::IMAGES.iter().position(|x| x.0 == cur).unwrap_or(usize::MAX);
        if let Some(i) = seg_row(ui, "Image", Some(&help), &names, sel, !app.core.busy()) {
            app.core.image_brightness(quality::IMAGES[i].0);
        }
        let saved = if app.core.facts.backlight == 0 { 100 } else { app.core.facts.backlight };
        let mut b = app.ui.backlight_edit.unwrap_or(saved);
        let r = widgets::setting(ui, "Panel backlight", Some("100 % is the panel's maximum (it is already there on Linux). Changes the headset right away and is sent again at every Play."), |ui| {
            ui.add(egui::Slider::new(&mut b, quality::MIN_BACKLIGHT..=100).suffix(" %"))
        });
        if r.changed() {
            app.ui.backlight_edit = Some(b);
        }
        if (r.drag_stopped() || (r.changed() && !r.dragged())) && b != saved {
            app.core.backlight(b);
            app.ui.backlight_edit = None;
        }
    });
}

/// Game mode, the compositor budget; the wrist readout (developer mode).
fn performance(ui: &mut Ui, app: &mut PimaxApp) {
    let ed = editable(app);
    let busy = app.core.busy();
    section(ui, "Performance", None, |ui| {
        let mut on = app.core.facts.game_mode;
        if switch_row(ui, &mut on, "Game mode", Some("Play pauses the local voice AI (it uses the graphics card) and Stop starts it again. Work is not affected."), !busy).changed() {
            app.core.game_mode(on);
        }
        let names: Vec<&str> = quality::BUDGETS.iter().map(|x| x.1).collect();
        let cur = quality::BUDGETS.iter().position(|x| x.0 == app.core.facts.comp_budget).unwrap_or(usize::MAX);
        if let Some(i) = seg_row(
            ui,
            "Compositor budget",
            Some("How much time the headset compositor keeps for itself before each frame is shown. Auto = Monado's 3 ms. More starts the frame earlier: fewer late frames, a little more delay. To compare: play the same scene for a minute per choice and look at the late % and fps on Home. Applies to the next start."),
            &names,
            cur,
            ed && !busy,
        ) {
            app.core.comp_budget(quality::BUDGETS[i].0);
        }
        if app.core.state.dev_mode {
            let caps = app.core.facts.caps.clone();
            switch_row(ui, &mut app.core.toggles.perf, "Wrist readout", Some("omarchyvr-perf: HMD and app frame rate, missed frames, CPU/GPU time on a small WayVR panel on the right wrist (desk start without --no-perf)."), ed && caps.no_perf);
        }
    });
}

/// The IPD: the lenses now, the value to apply, the move; the eye-tracker measurement and the lens tests (developer
/// mode).
fn lenses(ui: &mut Ui, app: &mut PimaxApp) {
    let dev = app.core.state.dev_mode;
    section(ui, "Lenses", Some("IPD: the distance between your pupils. Apply moves the lenses to this value (58-72 mm, the headset's own motor) and saves it for Monado (~/.config/omarchyvr/ipd.env), which draws with it from the next start."), |ui| {
        let pal = theme::pal(ui);
        app.core.read_lens_ipd();
        let ipd = app.core.facts.ipd_mm;
        let mut v = app.ui.ipd_edit.or(ipd).unwrap_or(63.0);
        let moving = app.core.moving_lenses();
        let blocker = app.core.ipd_motor_blocker();
        let mut apply = false;
        widgets::setting(ui, "IPD", None, |ui| {
            if ui.add(egui::Slider::new(&mut v, omarchyvr_gui_core::ipdmotor::MIN_MM..=omarchyvr_gui_core::ipdmotor::MAX_MM).step_by(0.1).suffix(" mm")).changed() {
                app.ui.ipd_edit = Some(v);
            }
            let changed = app.ui.ipd_edit.is_some_and(|e| Some(e) != ipd);
            if widgets::primary(ui, "Apply", changed && !moving).clicked() {
                apply = true;
            }
        });
        if apply {
            app.core.set_ipd(v);
            app.ui.ipd_edit = None;
        }
        let now = match app.core.lens_ipd {
            Some(l) => format!("{l:.1} mm"),
            None => "not read".to_string(),
        };
        widgets::setting(ui, "Lenses now", Some("Read from the headset. Not read while the headset is off USB or a session runs."), |ui| {
            ui.label(RichText::new(now).size(12.0).color(pal.dim));
        });
        if moving {
            ui.add(egui::ProgressBar::new(0.5).desired_width(ui.available_width()).animate(true).text("Moving the lenses (about 1.8 mm per second)"));
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
        } else if let Some(r) = &app.core.ipd_result {
            widgets::status(ui, &r.text, if r.ok { pal.ok } else { pal.warn });
        }
        if let (Some(b), false) = (&blocker, moving) {
            widgets::status(ui, &format!("Lenses will not move: {b}"), pal.faint);
        }
        if dev {
            ui.horizontal_wrapped(|ui| {
                tool_button(ui, app, ToolId::Ipd, "Measure IPD");
                tool_button(ui, app, ToolId::LensTest, "Lens test");
            });
            let cam = &app.core.live.log.camera;
            let ae = if app.core.live.service.is_some() {
                format!("{} (camera {})", if cam.ae { "on" } else { "off" }, cam.fps.map(|f| format!("{f:.0} fps")).unwrap_or_else(|| "-".into()))
            } else {
                "on with 6DoF".to_string()
            };
            widgets::setting(ui, "Camera exposure", Some("Auto exposure: the approved 0x51 exposure loop of the camera broker, on with 6DoF."), |ui| {
                ui.label(RichText::new(format!("{} {ae}", icon::SUN)).size(12.0).color(pal.dim));
            });
        }
    });
}
