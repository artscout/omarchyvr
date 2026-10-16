//! The Room row, a guided room setup: the anchor hook of the next start, (1) map the room once, (2) recognise it at
//! every start, (3) the floor (found in the mapping walk, refined with a controller on the floor, or typed in), the
//! floor grid in VR, a top-down preview of the map with the floor and where you are, the saved rooms, and the
//! printed AprilTag. Only one of the room map and the tag places the world per Monado session.
//!
//! On top (normal mode): Room setup (docs/ROOM-SETUP.md): Fast setup / Full setup guided in the headset, the room's
//! status line, and the switches "Put my room in place at every Play and Work" and "Tell SteamVR my play area". The
//! by-hand steps (map, recognise, floor, AprilTag) are shown in developer mode.
use crate::app::PimaxApp;
use crate::theme::{self, icon};
use crate::widgets::{self, caption, chip, inset, note, toggle};
use eframe::egui::{self, Pos2, Rect, RichText, Sense, Stroke, Ui};
use pimax_gui_core::app::{DeskPhase, RoomAction};
use pimax_gui_core::room::{self, FloorAction, Preview, SetupInfo};

/// The row summary.
pub fn summary(app: &PimaxApp) -> String {
    let st = &app.core.room.state;
    let map = if st.maps.is_empty() { "no map".to_string() } else { format!("map {}", st.maps.join(", ")) };
    let floor = match st.floors.iter().find(|(n, _)| *n == app.core.room.map_name) {
        Some(_) => "floor set",
        None if st.maps.is_empty() => "",
        None => "floor -",
    };
    let area = st
        .setups
        .iter()
        .find(|(n, _)| *n == app.core.room.map_name)
        .map(|(_, s)| format!("play area {:.1} x {:.1} m", s.play_cm[0] as f64 / 100.0, s.play_cm[1] as f64 / 100.0))
        .unwrap_or_default();
    let every = if st.prefs.auto { "at every start" } else { "" };
    if !app.core.state.dev_mode {
        return [map.as_str(), area.as_str(), floor, every].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" | ");
    }
    let tag = if st.tag_calibrated { "tag calibrated" } else { "tag -" };
    let hook = if app.core.toggles.anchor { "hook on" } else { "hook off" };
    [map.as_str(), area.as_str(), floor, tag, hook].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" | ")
}

fn action(ui: &mut Ui, app: &mut PimaxApp, a: RoomAction, ic: &str, label: &str) {
    let b = app.core.room_blockers(a);
    let can = b.is_empty() && !app.core.busy();
    let r = widgets::ibutton(ui, ic, label, can);
    let r = if can { r } else { r.on_disabled_hover_text(if b.is_empty() { "Another action is running".to_string() } else { b.join(" ") }) };
    if r.clicked() {
        app.core.room_action(a);
    }
}

fn floor_button(ui: &mut Ui, app: &mut PimaxApp, a: FloorAction, ic: &str, label: &str, hover: &str) {
    let b = room::floor_blockers(&app.core.live, &app.core.room.state, &app.core.room.map_name, &a);
    let can = b.is_empty() && !app.core.busy();
    let r = widgets::ibutton(ui, ic, label, can);
    let r = if can { r.on_hover_text(hover) } else { r.on_disabled_hover_text(if b.is_empty() { "Another action is running".to_string() } else { b.join(" ") }) };
    if r.clicked() {
        app.core.floor_action(a);
    }
}

/// Small per-page values kept in egui memory (the core holds only what the tools need).
#[derive(Clone, Copy)]
struct Inputs {
    floor_y: f64,
    eye_height: f64,
    left: bool,
}

impl Default for Inputs {
    fn default() -> Self {
        Inputs { floor_y: -1.5, eye_height: 1.65, left: false }
    }
}

/// A synthetic room setup for demo mode and screenshots (matches [`room::demo_preview`]).
fn demo_setup() -> SetupInfo {
    SetupInfo {
        label: "home".into(),
        mode: "fast".into(),
        play_cm: [260, 200],
        centre_cm: [10, 0],
        yaw_decideg: 0,
        boundary_cm: vec![[-120, -100], [140, -100], [140, 100], [-120, 100]],
        when: 1_791_968_000,
    }
}

fn setup_button(ui: &mut Ui, app: &mut PimaxApp, full: bool, ic: &str, label: &str, hover: &str) {
    let b = app.core.room_setup_blockers(full);
    let can = b.is_empty() && !app.core.busy();
    let r = widgets::ibutton(ui, ic, label, can);
    let r = if can { r.on_hover_text(hover) } else { r.on_disabled_hover_text(if b.is_empty() { "Another action is running".to_string() } else { b.join(" ") }) };
    if r.clicked() {
        app.core.start_room_setup(full);
    }
}

/// Room setup: status, Fast / Full setup, the switches.
fn setup_section(ui: &mut Ui, app: &mut PimaxApp, setup: Option<&SetupInfo>) {
    let p = theme::pal(ui);
    caption(ui, "Room setup");
    inset(ui, |ui| {
        let line = pimax_gui_core::roomsetup::status_line(&app.core);
        let ok = setup.is_some();
        ui.horizontal_wrapped(|ui| {
            chip(ui, if ok { "set up" } else { "not set up" }, if ok { p.ok } else { p.warn });
            ui.label(RichText::new(line).color(p.dim));
        });
        if let Some(s) = setup {
            note(ui, &format!("Saved: {}.", s.describe()));
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new("Room name").color(p.dim));
            ui.add(egui::TextEdit::singleline(&mut app.core.room.map_name).desired_width(120.0));
        });
        ui.horizontal_wrapped(|ui| {
            setup_button(
                ui,
                app,
                false,
                icon::LIGHTNING,
                "Fast setup (30 s)",
                "Stand in the middle of your play space and look around. Floor and play area are found by themselves; look forward and pull a trigger (or hold still) to set forward.",
            );
            setup_button(
                ui,
                app,
                true,
                icon::MAP_TRIFOLD,
                "Full setup (about 3 min)",
                "Walk the room, lay a controller on the floor, trace your play area holding a trigger, then set the centre and forward.",
            );
        });
        note(ui, "The steps are shown in the headset. Work starts by itself if it is not running. Run it again any time, e.g. after moving furniture. Turn the room lights on: at night the cameras see too little, and the headset says so (points in view, brightness).");
        let pr = app.core.room.state.prefs.clone();
        let mut auto = pr.auto;
        if toggle(ui, &mut auto, "Put my room in place at every Play and Work", !app.core.busy()).changed() {
            app.core.room_pref(&[if auto { "on" } else { "off" }]);
            if auto && pr.room != app.core.room.map_name && room::valid_name(&app.core.room.map_name) {
                let n = app.core.room.map_name.clone();
                app.core.room_pref(&["name", &n]);
            }
        }
        note(ui, &format!("The room \"{}\" is recognised a few seconds after each start: the floor, the centre and forward are the same every session.", pr.room));
        let mut steam = pr.steamvr;
        if toggle(ui, &mut steam, "Tell SteamVR my play area and floor", !app.core.busy() && pr.auto).changed() {
            app.core.room_pref(&["steamvr", if steam { "on" } else { "off" }]);
        }
        note(ui, "Before SteamVR starts, its room setup gets this play area and boundary (SteamVR's own setup is kept as a backup).");
        if pr.room != app.core.room.map_name && room::valid_name(&app.core.room.map_name) && app.core.room.state.maps.contains(&app.core.room.map_name) {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format!("Every start uses the room \"{}\".", pr.room)).color(p.dim));
                if widgets::ibutton(ui, icon::CHECK_CIRCLE, &format!("Use \"{}\" instead", app.core.room.map_name), !app.core.busy()).clicked() {
                    let n = app.core.room.map_name.clone();
                    app.core.room_pref(&["name", &n]);
                }
            });
        }
    });
}

/// The row body.
pub fn body(ui: &mut Ui, app: &mut PimaxApp) {
    app.core.want_room();
    let p = theme::pal(ui);
    let dev = app.core.state.dev_mode;
    let setup: Option<SetupInfo> = app
        .core
        .room
        .state
        .setups
        .iter()
        .find(|(n, _)| *n == app.core.room.map_name)
        .map(|(_, s)| s.clone())
        .or_else(|| app.core.demo.then(demo_setup));
    setup_section(ui, app, setup.as_ref());
    if !dev {
        preview_section(ui, app, setup.as_ref());
        saved_rooms(ui, app);
        return;
    }
    let editable = matches!(app.core.phase(), DeskPhase::Idle | DeskPhase::Foreign) && !app.core.busy();
    let caps_anchor = app.core.facts.caps.anchor;
    let on = app.core.toggles.anchor;
    let mut t = on;
    let r = toggle(ui, &mut t, "Room anchor hook at the next start (--anchor, PIMAX_ANCHOR=1)", editable && (caps_anchor || on));
    if !caps_anchor {
        r.on_hover_text("The installed launcher has no --anchor: install the current build (Setup)");
    }
    if t != on {
        app.core.toggles.anchor = t;
        app.core.save_state();
        app.ui.pf = None;
    }
    note(ui, "Monado then keeps its head poses for the room tools and applies the world offset they write: the windows and the floor come back to the same real place.");
    if let Some(s) = &app.core.live.service {
        ui.horizontal_wrapped(|ui| {
            chip(ui, if s.anchor { "hook on in this session" } else { "hook off in this session" }, if s.anchor { p.ok } else { p.faint });
            let st = &app.core.room.state;
            match (st.offset_pid, st.owner) {
                (Some(op), Some((w, wp))) if op == s.pid && wp == s.pid => chip(ui, &format!("offset from {}", w.title()), p.accent),
                (Some(op), _) if op == s.pid => chip(ui, "offset from outside the app", p.warn),
                _ => chip(ui, "no offset", p.faint),
            }
        });
    }

    let name = app.core.room.map_name.clone();
    // a clone of a few thousand small points per frame while the row is open: microseconds
    let preview: Option<Preview> = match app.core.room.state.previews.iter().find(|(n, _)| *n == name) {
        Some((_, pv)) => Some(pv.clone()),
        None if app.core.demo => Some(room::demo_preview()),
        None => None,
    };
    let keyframes = preview.as_ref().map(|pv| pv.heads.len()).unwrap_or(0);
    let floor = app.core.room.state.floors.iter().find(|(n, _)| *n == name).map(|(_, f)| f.clone()).or_else(|| {
        // demo mode: the synthetic room's floor
        (app.core.demo && preview.is_some()).then(|| room::FloorInfo { y_mm: -1420, source: "points".into(), confidence: 93, sigma_tenth_mm: 9, head_above_mm: 1430 })
    });

    caption(ui, "1  Map the room");
    inset(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Name").color(p.dim));
            ui.add(egui::TextEdit::singleline(&mut app.core.room.map_name).desired_width(120.0));
            ui.label(RichText::new("Walk").color(p.dim));
            ui.add(egui::DragValue::new(&mut app.core.room.seconds).range(30..=300).suffix(" s"));
        });
        note(ui, "Walk slowly through the room, look at walls, shelves, the desk and the floor around you, turn around and come back past where you started (that closes the loop). Keep the usual lights on; a second map helps for day and night.");
        ui.horizontal_wrapped(|ui| {
            action(ui, app, RoomAction::MapRoom, icon::MAP_TRIFOLD, "Map my room");
            if preview.is_some() {
                let (q, c) = match keyframes {
                    0..=39 => ("weak map: walk more of the room", p.warn),
                    40..=99 => ("usable map", p.accent),
                    _ => ("good map", p.ok),
                };
                chip(ui, &format!("{keyframes} keyframes, {q}"), c);
            }
            action(ui, app, RoomAction::ViewMap, icon::EYE, "View room map");
        });
    });

    caption(ui, "2  Recognise the room");
    inset(ui, |ui| {
        note(ui, "At every start, once 6DoF tracks: look around for a few seconds. The world origin, the windows and the floor come back to where they were.");
        ui.horizontal_wrapped(|ui| {
            action(ui, app, RoomAction::Recognise, icon::SCAN, "Recognise room");
            let fixed = app.core.live.service.as_ref().is_some_and(|s| app.core.room.state.fixes.iter().any(|(n, pid)| *n == name && *pid == s.pid));
            if fixed {
                chip(ui, "recognised in this session", p.ok);
            }
        });
    });

    caption(ui, "3  Floor");
    inset(ui, |ui| {
        match &floor {
            Some(f) => {
                let c = if f.confidence >= 80 { p.ok } else if f.confidence >= 50 { p.accent } else { p.warn };
                ui.horizontal_wrapped(|ui| {
                    chip(ui, &format!("{} % sure", f.confidence), c);
                    ui.label(RichText::new(f.describe()).color(p.dim));
                });
            }
            None => note(ui, "No floor for this map yet. Find it in the map, or lay a controller on the floor."),
        }
        note(ui, "Recognise room puts this floor on Monado's floor (STAGE / LOCAL_FLOOR), so you stand on the real floor instead of under it.");
        let id = ui.id().with("room-floor-inputs");
        let mut inp: Inputs = ui.data_mut(|d| *d.get_temp_mut_or_default::<Inputs>(id));
        ui.horizontal_wrapped(|ui| {
            floor_button(ui, app, FloorAction::Detect, icon::CROSSHAIR, "Find floor in the map", "From the map's points: the lowest large level surface below you (tables and desks are skipped).");
            let hand = if inp.left { "left" } else { "right" };
            floor_button(ui, app, FloorAction::Touch(hand), icon::GAME_CONTROLLER, "Touch floor with controller", "Lay the controller flat on the floor where the headset sees it and let go: one second still is enough. Corrects the found floor.");
            if ui.selectable_label(inp.left, "left").clicked() {
                inp.left = true;
            }
            if ui.selectable_label(!inp.left, "right").clicked() {
                inp.left = false;
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Standing eye height").color(p.dim));
            ui.add(egui::DragValue::new(&mut inp.eye_height).range(1.0..=2.3).speed(0.005).suffix(" m").fixed_decimals(2));
            floor_button(ui, app, FloorAction::Standing(inp.eye_height), icon::SLIDERS, "Set from my height", "Stand upright, then press: the floor goes this far below your eyes.");
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Floor in the map").color(p.dim));
            ui.add(egui::DragValue::new(&mut inp.floor_y).range(-3.0..=1.0).speed(0.005).suffix(" m").fixed_decimals(3));
            floor_button(ui, app, FloorAction::Set(inp.floor_y), icon::SLIDERS, "Set", "Type the floor height in the map's frame (pimax-roommap floor show prints it).");
        });
        ui.data_mut(|d| d.insert_temp(id, inp));
        let mut grid = app.core.room.state.grid_on;
        if toggle(ui, &mut grid, "Floor grid in VR (pimax-passthrough)", !app.core.busy()).changed() {
            app.core.set_floor_grid(grid);
        }
        note(ui, "A faint grid on the floor around you, fading with distance; the passthrough window's o key toggles it too.");
    });

    preview_section(ui, app, setup.as_ref());
    saved_rooms(ui, app);

    caption(ui, "AprilTag (printed, mm-accurate)");
    inset(ui, |ui| {
        note(ui, "Print tag36h11-id0-150mm.pdf at 100 % (the black square measures 150 mm), glue it flat on the desk 40-70 cm in front of you. Calibrate once with the windows where you like them; apply in every later session after tracking runs.");
        ui.horizontal_wrapped(|ui| {
            action(ui, app, RoomAction::TagCalibrate, icon::QR_CODE, "Calibrate");
            action(ui, app, RoomAction::TagApply, icon::ANCHOR, "Apply");
            action(ui, app, RoomAction::Clear, icon::X_CIRCLE, "Clear offset");
            if widgets::ibutton(ui, icon::FILE_TEXT, "Printable tag", true).clicked() {
                let f = app.core.room_paths.tag_pdf.clone();
                app.core.open_folder(&f);
            }
        });
        if app.core.room.state.tag_calibrated {
            ui.label(RichText::new(format!("{} calibrated (~/.config/pimax/anchor.toml)", icon::CHECK_CIRCLE)).size(12.5).color(p.ok));
        }
    });
    note(ui, "Only one of the room map and the AprilTag places the world per session: the other one is refused until Clear offset.");
}

/// The map preview with the play area (boundary, centre, forward) when the room is set up.
fn preview_section(ui: &mut Ui, app: &mut PimaxApp, setup: Option<&SetupInfo>) {
    let name = app.core.room.map_name.clone();
    let preview: Option<Preview> = match app.core.room.state.previews.iter().find(|(n, _)| *n == name) {
        Some((_, pv)) => Some(pv.clone()),
        None if app.core.demo => Some(room::demo_preview()),
        None => None,
    };
    let floor_cm = app.core.room.state.floors.iter().find(|(n, _)| *n == name).map(|(_, f)| f.y_mm / 10).or(app.core.demo.then_some(-142));
    caption(ui, "Map preview");
    inset(ui, |ui| match &preview {
        Some(pv) => {
            let head = app.core.room.state.head.as_ref().filter(|(n, _)| *n == name).map(|(_, h)| *h).or(app.core.demo.then_some([40, -30, 2]));
            draw_preview(ui, pv, floor_cm.or(pv.floor_cm), head, setup);
            let area = if setup.is_some() { " The green frame is the play area, the arrow points forward." } else { "" };
            note(ui, &format!(
                "{}{area}",
                if head.is_some() {
                    "Top: from above (floor points tinted, red = below the floor). Bottom: from the side. The dot is you."
                } else {
                    "Top: from above (floor points tinted, red = below the floor). Bottom: from the side. The ring marks the end of the mapping walk."
                }
            ));
        }
        None => note(ui, "No map of this room yet: Fast setup makes one."),
    });
}

/// The saved rooms (pick one by name).
fn saved_rooms(ui: &mut Ui, app: &mut PimaxApp) {
    let p = theme::pal(ui);
    caption(ui, "Saved rooms");
    inset(ui, |ui| {
        let st = &app.core.room.state;
        type Row = (String, Option<usize>, Option<String>, Option<String>);
        let rows: Vec<Row> = st
            .maps
            .iter()
            .map(|m| {
                let kf = st.previews.iter().find(|(n, _)| n == m).map(|(_, pv)| pv.heads.len());
                let fl = st.floors.iter().find(|(n, _)| n == m).map(|(_, f)| format!("floor {:+.2} m ({})", f.y_mm as f64 / 1000.0, f.source));
                let pa = st.setups.iter().find(|(n, _)| n == m).map(|(_, s)| format!("play area {:.1} x {:.1} m", s.play_cm[0] as f64 / 100.0, s.play_cm[1] as f64 / 100.0));
                (m.clone(), kf, fl, pa)
            })
            .collect();
        if rows.is_empty() {
            note(ui, "No rooms yet (~/.local/share/pimax/roommap).");
        }
        for (m, kf, fl, pa) in rows {
            ui.horizontal_wrapped(|ui| {
                if ui.selectable_label(m == app.core.room.map_name, m.as_str()).clicked() {
                    app.core.room.map_name = m.clone();
                }
                if let Some(k) = kf {
                    ui.label(RichText::new(format!("{k} keyframes")).color(p.dim));
                }
                match fl {
                    Some(f) => ui.label(RichText::new(f).color(p.dim)),
                    None => ui.label(RichText::new("no floor").color(p.faint)),
                };
                match pa {
                    Some(a) => ui.label(RichText::new(a).color(p.dim)),
                    None => ui.label(RichText::new("no play area").color(p.faint)),
                };
            });
        }
    });
}

/// Top-down view and side view of a map: points (floor level tinted, below the floor red), the mapping walk, and
/// the user. A few thousand small rects at most, drawn only when the row is open (egui repaints on input).
fn draw_preview(ui: &mut Ui, pv: &Preview, floor_cm: Option<i32>, head: Option<[i32; 3]>, setup: Option<&SetupInfo>) {
    let p = theme::pal(ui);
    let Some((x0, x1, z0, z1)) = pv.bounds() else { return };
    let w = ui.available_width().clamp(200.0, 560.0);
    let span_x = (x1 - x0).max(50) as f32;
    let span_z = (z1 - z0).max(50) as f32;
    let top_h = (w * span_z / span_x).clamp(120.0, 320.0);
    let side_h = 90.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, top_h + 8.0 + side_h), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter_at(rect);
    let top = Rect::from_min_size(rect.min, egui::vec2(w, top_h));
    let side = Rect::from_min_size(Pos2::new(rect.min.x, rect.min.y + top_h + 8.0), egui::vec2(w, side_h));
    painter.rect_filled(top, 6.0, p.bg);
    painter.rect_filled(side, 6.0, p.bg);
    let s = ((w - 16.0) / span_x).min((top_h - 16.0) / span_z);
    let cx = (x0 + x1) as f32 / 2.0;
    let cz = (z0 + z1) as f32 / 2.0;
    let to_top = |x: i32, z: i32| top.center() + egui::vec2((x as f32 - cx) * s, (z as f32 - cz) * s);
    let (y_lo, y_hi) = pv.points.iter().chain(pv.heads.iter()).fold((i32::MAX, i32::MIN), |b, q| (b.0.min(q[2]), b.1.max(q[2])));
    let ys = (side_h - 12.0) / (y_hi - y_lo).max(50) as f32;
    let to_side = |x: i32, y: i32| Pos2::new(side.center().x + (x as f32 - cx) * s, side.bottom() - 6.0 - (y - y_lo) as f32 * ys);
    let floor_c = p.accent.gamma_multiply(0.85);
    let below_c = p.fail;
    let other_c = p.faint.gamma_multiply(0.55);
    let step = (pv.points.len() / 3000).max(1);
    for q in pv.points.iter().step_by(step) {
        let c = match floor_cm {
            Some(f) if (q[2] - f).abs() <= 5 => floor_c,
            Some(f) if q[2] < f - 5 => below_c,
            _ => other_c,
        };
        painter.rect_filled(Rect::from_center_size(to_top(q[0], q[1]), egui::vec2(1.6, 1.6)), 0.0, c);
        painter.rect_filled(Rect::from_center_size(to_side(q[0], q[2]), egui::vec2(1.4, 1.4)), 0.0, c);
    }
    if let Some(f) = floor_cm {
        let y = to_side(0, f).y;
        painter.line_segment([Pos2::new(side.left() + 4.0, y), Pos2::new(side.right() - 4.0, y)], Stroke::new(1.5, p.accent));
        painter.text(Pos2::new(side.left() + 8.0, y - 2.0), egui::Align2::LEFT_BOTTOM, "floor", egui::FontId::proportional(11.0), p.accent);
    }
    if let Some(st) = setup {
        // the play area: boundary, centre, forward arrow (map frame, cm)
        let b: Vec<Pos2> = st.boundary_cm.iter().map(|q| to_top(q[0], q[1])).collect();
        if b.len() >= 3 {
            painter.add(egui::Shape::closed_line(b, Stroke::new(2.0, p.ok)));
        }
        let c = to_top(st.centre_cm[0], st.centre_cm[1]);
        let yaw = (st.yaw_decideg as f32 / 10.0).to_radians();
        // forward = R_y(yaw) (0, 0, -1) = (-sin, -cos) in x z
        let f = egui::vec2(-yaw.sin(), -yaw.cos()) * (60.0 * s);
        painter.arrow(c, f, Stroke::new(2.0, p.ok));
        painter.circle_stroke(c, 4.0, Stroke::new(1.5, p.ok));
    }
    let walk: Vec<Pos2> = pv.heads.iter().map(|h| to_top(h[0], h[1])).collect();
    if walk.len() >= 2 {
        painter.add(egui::Shape::line(walk.clone(), Stroke::new(1.2, p.ok.gamma_multiply(0.7))));
    }
    match head {
        Some(h) => {
            painter.circle_filled(to_top(h[0], h[1]), 5.0, p.ok);
            painter.circle_filled(to_side(h[0], h[2]), 4.0, p.ok);
            if let Some(f) = floor_cm {
                let a = to_side(h[0], h[2]);
                painter.line_segment([a, Pos2::new(a.x, to_side(0, f).y)], Stroke::new(1.0, p.ok.gamma_multiply(0.6)));
                painter.text(a + egui::vec2(6.0, 0.0), egui::Align2::LEFT_CENTER, format!("{:.2} m", (h[2] - f) as f64 / 100.0), egui::FontId::proportional(11.0), p.ok);
            }
        }
        None => {
            if let Some(&last) = walk.last() {
                painter.circle_stroke(last, 5.0, Stroke::new(1.5, p.ok));
            }
        }
    }
    // scale bar: 1 m
    let bar = Pos2::new(top.right() - 12.0 - 100.0 * s, top.bottom() - 10.0);
    painter.line_segment([bar, bar + egui::vec2(100.0 * s, 0.0)], Stroke::new(2.0, p.dim));
    painter.text(bar + egui::vec2(50.0 * s, -3.0), egui::Align2::CENTER_BOTTOM, "1 m", egui::FontId::proportional(11.0), p.dim);
}

#[cfg(test)]
mod tests {
    /// `PIMAX_ROOM_SHOT=DIR cargo test -p pimax-setup room_page_picture -- --ignored`: the Room row alone, demo data.
    #[test]
    #[ignore]
    fn room_page_picture() {
        let dir = std::path::PathBuf::from(std::env::var("PIMAX_ROOM_SHOT").unwrap_or_else(|_| "/tmp".into()));
        let tmp = std::env::temp_dir().join(format!("pimax-room-shot-{}", std::process::id()));
        let store = pimax_config::Store::with_clock(tmp.join("cfg"), tmp.join("cache"), || 1_791_968_155);
        let core = pimax_gui_core::app::AppCore::demo(true, store);
        let mut app = crate::app::PimaxApp::new(core);
        for dark in [true, false] {
            let ctx = eframe::egui::Context::default();
            crate::theme::install(&ctx, dark);
            let path = dir.join(if dark { "room-page.png" } else { "room-page-light.png" });
            crate::screenshot::snapshot(&ctx, eframe::egui::vec2(620.0, 1900.0), 1.0, 3, |ui| super::body(ui, &mut app), &path).unwrap();
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
