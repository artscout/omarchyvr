//! The Recording page: the video (eyes, frame rate, quality, Start / Stop, the live "REC" indicator and the last
//! file) and the tracking data takes. The logic is `omarchyvr_gui_core::record`; this only draws.
use crate::app::PimaxApp;
use crate::theme::{self, icon};
use crate::pages::two_columns;
use crate::widgets::{self, caption, kv, Kind};
use eframe::egui::{self, RichText, Ui};
use omarchyvr_gui_core::record::{self, Indicator, Opt};

/// The Recording page.
pub fn show(ui: &mut Ui, app: &mut PimaxApp) {
    two_columns(ui, 380.0, |i, ui| {
        if i == 0 {
            card_ui(ui, app);
        } else {
            widgets::section(ui, "Tracking data", Some("Cameras, controller logs and the session log of one take, in a folder of its own, to compare with Windows. Works during Play, Work or SteamVR and never disturbs them (docs/RECORD.md)."), |ui| tracking_ui(ui, app));
        }
    });
}

/// The video card.
pub fn card_ui(ui: &mut Ui, app: &mut PimaxApp) {
    app.core.want_record();
    let p = theme::pal(ui);
    let s = app.core.record.current();
    let ind = app.core.record.indicator();
    let active = record::active(&ind);
    let op = app.core.record.op;
    let pending = app.core.record_busy();
    let session = app.core.live.service.is_some();
    widgets::section(ui, "Video", Some("What the headset shows, as a video in ~/Videos/pimax: 30 fps, high quality, with sound. Needs a running session. You can also say: Max, start recording / stop recording."), |ui| {
        let can_edit = !active && !pending;
        if let Some(i) = widgets::selector(ui, icon::EYE, "Eyes", &record::EYES, record::eyes_index(&s), can_edit) {
            app.core.record_set(Opt::Eyes, i);
        }
        if let Some(i) = widgets::selector(ui, icon::FILM_SLATE, "FPS", &record::FPS, record::fps_index(&s), can_edit) {
            app.core.record_set(Opt::Fps, i);
        }
        if let Some(i) = widgets::selector(ui, icon::SLIDERS, "Quality", &record::QUALITY, record::quality_index(&s), can_edit) {
            app.core.record_set(Opt::Quality, i);
        }
        let mut sound = s.audio;
        if widgets::switch_row(ui, &mut sound, "Sound", Some("What you hear in the headset is recorded into the video."), can_edit).changed() {
            app.core.record_set(Opt::Audio, sound as usize);
        }
        let mut mic = s.mic;
        if widgets::switch_row(ui, &mut mic, "Microphone", Some("Also records your voice, mixed with the sound. Needs Sound on."), can_edit && s.audio).changed() {
            app.core.record_set(Opt::Mic, mic as usize);
        }
        ui.add_space(6.0);
        let w = ui.available_width();
        if active {
            if widgets::action(ui, icon::STOP, "Stop recording", Kind::Danger, !pending, w).on_hover_text("omarchyvr record stop: the file is finished (a few seconds)").clicked() {
                app.core.record_stop();
            }
        } else {
            let hint = if session { "omarchyvr record start with the options above" } else { "Start a VR or desk session first: the recorder reads what Monado shows" };
            if widgets::action(ui, icon::PLAY, "Start recording", Kind::Primary, session && !pending, w).on_hover_text(hint).clicked() {
                app.core.record_start();
            }
        }
        ui.add_space(4.0);
        if pending {
            ui.horizontal(|ui| {
                widgets::spinner(ui);
                ui.label(RichText::new(op).color(p.dim));
            });
        }
        match &ind {
            Indicator::Recording { clock, file } => {
                ui.horizontal(|ui| {
                    widgets::dot(ui, egui::Color32::from_rgb(230, 40, 40), 5.0);
                    ui.label(RichText::new(format!("REC {clock}")).strong().color(egui::Color32::from_rgb(230, 40, 40)));
                    ui.add(egui::Label::new(RichText::new(file).color(p.dim)).truncate());
                });
            }
            Indicator::Starting => {
                ui.horizontal(|ui| {
                    widgets::spinner(ui);
                    ui.label(RichText::new("Waiting for the first frame...").color(p.dim));
                });
            }
            Indicator::Finishing { file } => {
                ui.horizontal(|ui| {
                    widgets::spinner(ui);
                    ui.label(RichText::new(format!("Finishing... {file}")).color(p.warn));
                });
            }
            Indicator::Error { note } => {
                ui.label(RichText::new(note).color(p.fail));
            }
            Indicator::Idle { last } => {
                if let Some(f) = last {
                    egui::Grid::new("record-last").num_columns(2).show(ui, |ui| kv(ui, "Last file", RichText::new(f).monospace()));
                    if widgets::ghost(ui, icon::FOLDER_OPEN, "Open folder", true).clicked() {
                        app.core.record_open_folder();
                    }
                }
            }
        }
        match &app.core.record.result {
            Some(Ok(line)) => caption(ui, line),
            Some(Err(e)) => {
                ui.label(RichText::new(e).color(p.fail).size(12.0));
            }
            None => {}
        }
        if let Some(e) = &app.core.record.settings_error {
            ui.label(RichText::new(e).color(p.warn).size(12.0));
        }
    });
}

/// "Record tracking": cameras and logs of the session into one folder per take, for comparing with Windows.
fn tracking_ui(ui: &mut Ui, app: &mut PimaxApp) {
    use record::tracking::{self as tr, View};
    app.core.want_track();
    let p = theme::pal(ui);
    let view = app.core.record.track.clone().unwrap_or(View::Idle { last: None });
    let busy = app.core.task_busy("track-op");
    let rec = matches!(view, View::Recording { .. });
    let scen: Vec<&str> = tr::SCENARIOS.iter().map(|s| s.1).collect();
    let durs: Vec<&str> = tr::DURATIONS.iter().map(|s| s.1).collect();
    widgets::setting(ui, "Scenario", None, |ui| {
        ui.add_enabled_ui(!rec && !busy, |ui| {
            egui::ComboBox::from_id_salt("track-scenario").selected_text(scen[app.core.record.track_scenario.min(scen.len() - 1)]).show_ui(ui, |ui| {
                for (i, l) in scen.iter().enumerate() {
                    ui.selectable_value(&mut app.core.record.track_scenario, i, *l);
                }
            });
        });
    });
    if let Some(i) = widgets::selector(ui, icon::SLIDERS, "Length", &durs, app.core.record.track_duration, !rec && !busy) {
        app.core.record.track_duration = i;
    }
    ui.add_space(4.0);
    let w = ui.available_width();
    if rec {
        if widgets::action(ui, icon::STOP, "Stop tracking record", Kind::Danger, !busy, w).on_hover_text("Finishes the take and copies the logs").clicked() {
            app.core.track_stop();
        }
    } else if widgets::action(ui, icon::PLAY, "Record tracking", Kind::Primary, !busy, w).on_hover_text("Works while a session runs and never disturbs it").clicked() {
        app.core.track_start();
    }
    ui.add_space(4.0);
    if busy {
        widgets::spinner(ui);
    }
    match &view {
        View::Recording { dir, elapsed, seconds, bytes } => {
            ui.horizontal(|ui| {
                widgets::dot(ui, egui::Color32::from_rgb(230, 40, 40), 5.0);
                let of = if *seconds > 0 { format!(" / {}", record::clock(*seconds as f64)) } else { String::new() };
                ui.label(RichText::new(format!("REC {}{of}  {}", record::clock(*elapsed as f64), tr::human(*bytes))).strong().color(egui::Color32::from_rgb(230, 40, 40)));
            });
            ui.add(egui::Label::new(RichText::new(dir.display().to_string()).color(p.dim)).truncate());
        }
        View::Idle { last: Some((dir, bytes)) } => {
            ui.label(RichText::new(format!("Saved {} ({})", tr::human(*bytes), dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())).color(p.dim));
            ui.add(egui::Label::new(RichText::new(dir.display().to_string()).monospace().color(p.dim)).truncate());
        }
        View::Idle { last: None } => {}
    }
    match &app.core.record.track_result {
        Some(Ok(line)) => caption(ui, line),
        Some(Err(e)) => {
            ui.label(RichText::new(e).color(p.fail).size(12.0));
        }
        None => {}
    }
    if widgets::ghost(ui, icon::FOLDER_OPEN, "Open folder", true).clicked() {
        app.core.track_open_folder();
    }
}
