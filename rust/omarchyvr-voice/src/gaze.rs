//! Gaze target resolver: "what is the user looking at", for voice commands such as "Max, close this" or
//! "Max, move that to workspace 2".
//!
//! WayVR (branch `eye-gaze`, silent eye tracking: `gaze_target` / `gaze_laser`) answers `wayvrctl gaze` with the
//! fixation in progress, the last stable fixation (>= 200 ms, ended in the last ~2 s) and ~10 s of history, each
//! with the overlay name, UV, the desktop pixel (screen overlays) and Unix-ms times ([`GazeReport`]). This module
//! picks the fixation that belongs to a command ([`pick_fixation`]: the one at the utterance time if given, else
//! the last stable one), turns it into a desktop point (WayVR's `px`, or the overlay UV on the Hyprland monitor of
//! the same name), and finds the Hyprland window under it ([`window_at`]: visible windows only; fullscreen, then
//! floating, then tiled; the most recently focused first), plus a best-effort tab guess ([`tab_guess`]).
//!
//! Everything goes through [`Runner`] (`wayvrctl gaze`, `hyprctl -j monitors`, `hyprctl -j clients`), so tests use
//! fake JSON. Read-only: nothing here focuses, moves or closes anything; the caller acts on [`GazeTarget::window`].
//!
//! Limits (see docs/EYE.md): the calibration error is ~3 deg (about 50-80 px on a 2 m wide screen at 1.2 m), so
//! small targets near an edge are ambiguous (`edge_px` says how close the point is to the window border); browser
//! tabs are not known to Hyprland, so a tab is only a region guess (Hyprland groups are exact).

use crate::act::Runner;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `last_stable` may be at most this old when no utterance time is given (WayVR keeps it ~2 s anyway).
pub const STABLE_MAX_AGE_MS: u64 = 2500;
/// With an utterance time, a fixation that ended at most this long before it still counts.
pub const BEFORE_AT_MS: u64 = 2000;
/// Measured per-eye calibration error (eye-calibration.toml v1), degrees: used for the pixel radius hint.
pub const ACCURACY_DEG: f32 = 3.08;
/// Chromium / Firefox tab strip height at scale 1 (title bar off, Omarchy default), px.
pub const TAB_STRIP_PX: f32 = 46.0;
/// Widest tab (Chromium 256 px, Firefox ~225 px at scale 1): with few tabs they all have this width.
pub const TAB_MAX_PX: f32 = 240.0;
/// Hyprland groupbar height (default `group:groupbar:height` 14 + indicator and gaps), px above the window.
pub const GROUPBAR_PX: f32 = 24.0;

/// One fixation as WayVR reports it (`WlxGazeFixation`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fixation {
    pub overlay: String,
    pub uv: [f32; 2],
    pub px: Option<[f32; 2]>,
    pub start_ms: u64,
    pub end_ms: u64,
    pub duration_ms: u32,
    pub confidence: f32,
    pub dispersion_deg: f32,
    pub dist_m: f32,
    pub samples: u32,
}

/// `wayvrctl gaze` (`WlxGaze`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GazeReport {
    pub ts_ms: u64,
    pub tracking: bool,
    pub laser: bool,
    pub valid: bool,
    pub min_fixation_ms: u32,
    pub current: Option<Fixation>,
    pub last_stable: Option<Fixation>,
    pub history: Vec<Fixation>,
}

pub fn parse_report(json: &str) -> Result<GazeReport, String> {
    serde_json::from_str(json.trim()).map_err(|e| format!("wayvrctl gaze: {e}"))
}

/// A Hyprland monitor (the fields used).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Monitor {
    pub id: i64,
    pub name: String,
    pub x: f32,
    pub y: f32,
    /// Logical size (mode / scale, swapped for 90/270 deg transforms).
    pub width: f32,
    pub height: f32,
    pub active_ws: i64,
    pub special_ws: i64,
}

pub fn parse_monitors(json: &str) -> Result<Vec<Monitor>, String> {
    let v: Value = serde_json::from_str(json.trim()).map_err(|e| format!("hyprctl monitors: {e}"))?;
    let a = v.as_array().ok_or("hyprctl monitors: not a list")?;
    Ok(a.iter()
        .filter_map(|m| {
            let f = |k: &str| m.get(k).and_then(Value::as_f64).map(|x| x as f32);
            let scale = f("scale").filter(|s| *s > 0.0).unwrap_or(1.0);
            let (mut w, mut h) = (f("width")? / scale, f("height")? / scale);
            if m.get("transform").and_then(Value::as_i64).is_some_and(|t| t % 2 == 1) {
                std::mem::swap(&mut w, &mut h);
            }
            Some(Monitor {
                id: m.get("id").and_then(Value::as_i64).unwrap_or(-1),
                name: m.get("name")?.as_str()?.to_string(),
                x: f("x").unwrap_or(0.0),
                y: f("y").unwrap_or(0.0),
                width: w,
                height: h,
                active_ws: m.pointer("/activeWorkspace/id").and_then(Value::as_i64).unwrap_or(0),
                special_ws: m.pointer("/specialWorkspace/id").and_then(Value::as_i64).unwrap_or(0),
            })
        })
        .collect())
}

/// A Hyprland window with its geometry (`hyprctl -j clients`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Window {
    pub address: String,
    pub class: String,
    pub title: String,
    pub workspace: i64,
    pub monitor: i64,
    pub at: [f32; 2],
    pub size: [f32; 2],
    pub floating: bool,
    pub fullscreen: bool,
    pub pinned: bool,
    /// Hyprland's own `visible` when it reports one (0.5x); None = decide from the workspaces.
    #[serde(skip)]
    pub visible: Option<bool>,
    /// Group ("tab") members including this window; empty = not grouped.
    pub grouped: Vec<String>,
    /// 0 = focused now.
    pub focus: i64,
}

pub fn parse_windows(json: &str) -> Result<Vec<Window>, String> {
    let v: Value = serde_json::from_str(json.trim()).map_err(|e| format!("hyprctl clients: {e}"))?;
    let a = v.as_array().ok_or("hyprctl clients: not a list")?;
    Ok(a.iter()
        .filter(|c| c.get("mapped").and_then(Value::as_bool).unwrap_or(true))
        .filter(|c| !c.get("hidden").and_then(Value::as_bool).unwrap_or(false))
        .filter_map(|c| {
            let pair = |k: &str| -> Option<[f32; 2]> {
                let a = c.get(k)?.as_array()?;
                Some([a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32])
            };
            let s = |k: &str| c.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            let address = s("address");
            if !crate::windows::address_ok(&address) {
                return None;
            }
            Some(Window {
                address,
                class: s("class"),
                title: s("title"),
                workspace: c.pointer("/workspace/id").and_then(Value::as_i64).unwrap_or(0),
                monitor: c.get("monitor").and_then(Value::as_i64).unwrap_or(-1),
                at: pair("at")?,
                size: pair("size")?,
                floating: c.get("floating").and_then(Value::as_bool).unwrap_or(false),
                fullscreen: c.get("fullscreen").and_then(Value::as_i64).unwrap_or(0) > 0,
                pinned: c.get("pinned").and_then(Value::as_bool).unwrap_or(false),
                visible: c.get("visible").and_then(Value::as_bool),
                grouped: c
                    .get("grouped")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                    .unwrap_or_default(),
                focus: c.get("focusHistoryID").and_then(Value::as_i64).unwrap_or(i64::MAX),
            })
        })
        .collect())
}

/// Which fixation answers the question, and why.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Picked {
    pub fixation: Fixation,
    /// "current" | "last_stable" | "history"
    pub source: &'static str,
    /// Milliseconds between the end of the fixation and the reference time (now, or the utterance time).
    pub age_ms: u64,
}

/// The fixation for a command. `at_ms` (Unix ms, e.g. when the utterance started): the long-enough fixation
/// that covers it, else the newest one that ended at most [`BEFORE_AT_MS`] before it, else the current one if it
/// started before `at_ms`. Without `at_ms`: WayVR's `last_stable` if at most [`STABLE_MAX_AGE_MS`] old.
pub fn pick_fixation(r: &GazeReport, at_ms: Option<u64>) -> Option<Picked> {
    let min = r.min_fixation_ms;
    let Some(at) = at_ms else {
        let f = r.last_stable.clone()?;
        let age = r.ts_ms.saturating_sub(f.end_ms);
        if age > STABLE_MAX_AGE_MS {
            return None;
        }
        let source = if r.current.as_ref().is_some_and(|c| c.start_ms == f.start_ms && c.overlay == f.overlay) {
            "current"
        } else {
            "last_stable"
        };
        return Some(Picked { fixation: f, source, age_ms: age });
    };
    let long = |f: &&Fixation| f.duration_ms >= min;
    let mut all: Vec<(&Fixation, &'static str)> = r.history.iter().filter(long).map(|f| (f, "history")).collect();
    if let Some(c) = r.current.as_ref().filter(long) {
        all.push((c, "current"));
    }
    if let Some((f, s)) = all.iter().find(|(f, _)| f.start_ms <= at && at <= f.end_ms) {
        return Some(Picked { fixation: (*f).clone(), source: s, age_ms: 0 });
    }
    if let Some((f, s)) = all
        .iter()
        .filter(|(f, _)| f.end_ms <= at && at - f.end_ms <= BEFORE_AT_MS)
        .max_by_key(|(f, _)| f.end_ms)
    {
        return Some(Picked { fixation: (*f).clone(), source: s, age_ms: at - f.end_ms });
    }
    // looked at it only after the utterance started: still the best answer while it lasts
    r.current
        .as_ref()
        .filter(|c| c.duration_ms >= min)
        .map(|c| Picked { fixation: c.clone(), source: "current", age_ms: 0 })
}

/// The desktop point of a fixation: WayVR's `px`, else the UV on the monitor named like the overlay.
pub fn desktop_point(f: &Fixation, monitors: &[Monitor]) -> Option<([f32; 2], Option<String>)> {
    let on = |p: [f32; 2]| {
        monitors
            .iter()
            .find(|m| p[0] >= m.x && p[0] < m.x + m.width && p[1] >= m.y && p[1] < m.y + m.height)
            .map(|m| m.name.clone())
    };
    if let Some(p) = f.px {
        return Some((p, on(p)));
    }
    let m = monitors.iter().find(|m| m.name == f.overlay)?;
    let p = [m.x + f.uv[0] * m.width, m.y + f.uv[1] * m.height];
    Some((p, Some(m.name.clone())))
}

fn is_visible(w: &Window, monitors: &[Monitor]) -> bool {
    if let Some(v) = w.visible {
        return v;
    }
    w.pinned || monitors.iter().any(|m| w.workspace == m.active_ws || (m.special_ws != 0 && w.workspace == m.special_ws))
}

fn contains(w: &Window, p: [f32; 2], pad_top: f32) -> bool {
    p[0] >= w.at[0] && p[0] < w.at[0] + w.size[0] && p[1] >= w.at[1] - pad_top && p[1] < w.at[1] + w.size[1]
}

/// The visible window under a desktop point. Stacking: a window on an open special workspace, then fullscreen,
/// then floating, then tiled; inside a tier the most recently focused wins. A grouped window also owns its
/// groupbar ([`GROUPBAR_PX`] above it).
pub fn window_at<'a>(windows: &'a [Window], monitors: &[Monitor], p: [f32; 2]) -> Option<&'a Window> {
    let special = |w: &Window| monitors.iter().any(|m| m.special_ws != 0 && w.workspace == m.special_ws);
    let tier = |w: &Window| {
        if special(w) {
            0
        } else if w.fullscreen {
            1
        } else if w.floating || w.pinned {
            2
        } else {
            3
        }
    };
    windows
        .iter()
        .filter(|w| is_visible(w, monitors))
        .filter(|w| contains(w, p, if w.grouped.len() > 1 { GROUPBAR_PX } else { 0.0 }))
        .min_by_key(|w| (tier(w), w.focus))
}

/// Distance (px) from the point to the nearest window border: small = the gaze error may mean the neighbour.
pub fn edge_px(w: &Window, p: [f32; 2]) -> f32 {
    let dx = (p[0] - w.at[0]).min(w.at[0] + w.size[0] - p[0]);
    let dy = (p[1] - w.at[1]).min(w.at[1] + w.size[1] - p[1]);
    dx.min(dy).max(0.0)
}

/// Best-effort tab under the gaze.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TabGuess {
    /// "group" (a Hyprland group: exact member) | "browser" (a region guess)
    pub kind: &'static str,
    /// Group: index of the member under the point; browser: index if the tabs have their maximum width
    /// (few tabs), else None.
    pub index: Option<usize>,
    /// Group: the member's address.
    pub address: Option<String>,
    /// Horizontal position in the strip, 0..1 (with a known tab count N: index = floor(x_frac * N)).
    pub x_frac: f32,
}

/// The tab under `p` in `w`: a Hyprland group's bar (exact), or a browser's tab strip (top [`TAB_STRIP_PX`] of
/// the window, a guess). None when the point is not on a strip.
pub fn tab_guess(w: &Window, p: [f32; 2]) -> Option<TabGuess> {
    let x_frac = ((p[0] - w.at[0]) / w.size[0].max(1.0)).clamp(0.0, 0.9999);
    if w.grouped.len() > 1 && p[1] < w.at[1] + 2.0 {
        let i = (x_frac * w.grouped.len() as f32) as usize;
        return Some(TabGuess { kind: "group", index: Some(i), address: w.grouped.get(i).cloned(), x_frac });
    }
    if crate::windows::is_browser(&w.class) && p[1] >= w.at[1] && p[1] < w.at[1] + TAB_STRIP_PX {
        let i = ((p[0] - w.at[0] - 8.0).max(0.0) / TAB_MAX_PX) as usize;
        return Some(TabGuess { kind: "browser", index: Some(i), address: None, x_frac });
    }
    None
}

/// What the user looks at, resolved.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GazeTarget {
    pub picked: Picked,
    /// Desktop (Hyprland logical) pixel.
    pub px: Option<[f32; 2]>,
    pub monitor: Option<String>,
    pub window: Option<Window>,
    pub edge_px: Option<f32>,
    pub tab: Option<TabGuess>,
    /// The calibration error at this distance, in metres on the overlay (multiply by px per metre for pixels).
    pub error_m: f32,
}

/// Resolve offline from the three JSON texts (tests, `omarchyvr-voice gaze --input`).
pub fn resolve_from(report: &GazeReport, monitors: &[Monitor], windows: &[Window], at_ms: Option<u64>) -> Option<GazeTarget> {
    let picked = pick_fixation(report, at_ms)?;
    let error_m = picked.fixation.dist_m * ACCURACY_DEG.to_radians().tan();
    let (px, monitor) = match desktop_point(&picked.fixation, monitors) {
        Some((p, m)) => (Some(p), m),
        None => (None, None),
    };
    let window = px.and_then(|p| window_at(windows, monitors, p)).cloned();
    let edge = window.as_ref().zip(px).map(|(w, p)| edge_px(w, p));
    let tab = window.as_ref().zip(px).and_then(|(w, p)| tab_guess(w, p));
    Some(GazeTarget { picked, px, monitor, window, edge_px: edge, tab, error_m })
}

/// `wayvrctl gaze` through the runner.
pub fn read_report(r: &mut dyn Runner, wayvrctl: &str) -> Result<GazeReport, String> {
    let out = r.run(&[wayvrctl.to_string(), "gaze".into()], true)?;
    parse_report(&out)
}

/// Resolve live: `wayvrctl gaze` + `hyprctl -j monitors` + `hyprctl -j clients`. Ok(None) = no usable fixation
/// (tracking off, eyes closed, nothing looked at long enough). `at_ms`: when the command was spoken (Unix ms).
pub fn resolve(r: &mut dyn Runner, wayvrctl: &str, at_ms: Option<u64>) -> Result<Option<GazeTarget>, String> {
    let report = read_report(r, wayvrctl)?;
    if !report.tracking {
        return Err("gaze tracking is off in WayVR (desk start without eye tracking, or gaze_target off)".into());
    }
    let monitors = parse_monitors(&r.run(&["hyprctl".into(), "-j".into(), "monitors".into()], true)?)?;
    let windows = parse_windows(&r.run(&["hyprctl".into(), "-j".into(), "clients".into()], true)?)?;
    Ok(resolve_from(&report, &monitors, &windows, at_ms))
}

/// One line for people (the CLI and toasts).
pub fn describe(t: &GazeTarget) -> String {
    let f = &t.picked.fixation;
    let mut s = format!(
        "{} {} ms (conf {:.2}, {}, {} ms ago) on {}",
        t.picked.source, f.duration_ms, f.confidence, if f.px.is_some() { "screen" } else { "overlay" }, t.picked.age_ms, f.overlay
    );
    if let Some(p) = t.px {
        s += &format!(" at {:.0},{:.0}", p[0], p[1]);
    }
    match &t.window {
        Some(w) => s += &format!(" -> {} {} \"{}\" ws {} (edge {:.0} px)", w.address, w.class, w.title, w.workspace, t.edge_px.unwrap_or(0.0)),
        None => s += " -> no window",
    }
    if let Some(tab) = &t.tab {
        s += &format!(", {} tab {:?}", tab.kind, tab.index);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONITORS: &str = r#"[{"id":2,"name":"SUNSHINE","width":1920,"height":1080,"x":0,"y":0,"scale":1,"transform":0,
        "activeWorkspace":{"id":2,"name":"2"},"specialWorkspace":{"id":0,"name":""}}]"#;

    fn clients() -> String {
        serde_json::json!([
            {"address":"0xa1","mapped":true,"hidden":false,"at":[0,28],"size":[960,1052],"workspace":{"id":2},"floating":false,
             "monitor":2,"class":"chromium","title":"Docs - Chromium","pinned":false,"fullscreen":0,"grouped":[],"focusHistoryID":1},
            {"address":"0xa2","mapped":true,"hidden":false,"at":[960,52],"size":[960,1028],"workspace":{"id":2},"floating":false,
             "monitor":2,"class":"Alacritty","title":"zsh","pinned":false,"fullscreen":0,"grouped":["0xa2","0xa3"],"focusHistoryID":2},
            {"address":"0xa3","mapped":true,"hidden":true,"at":[960,52],"size":[960,1028],"workspace":{"id":2},"floating":false,
             "monitor":2,"class":"Alacritty","title":"htop","pinned":false,"fullscreen":0,"grouped":["0xa2","0xa3"],"focusHistoryID":5},
            {"address":"0xb1","mapped":true,"hidden":false,"at":[400,300],"size":[600,400],"workspace":{"id":2},"floating":true,
             "monitor":2,"class":"pimax","title":"Pimax","pinned":false,"fullscreen":0,"grouped":[],"focusHistoryID":0},
            {"address":"0xc1","mapped":true,"hidden":false,"at":[0,28],"size":[1920,1052],"workspace":{"id":3},"floating":false,
             "monitor":2,"class":"steam","title":"Steam","pinned":false,"fullscreen":0,"grouped":[],"focusHistoryID":3}
        ])
        .to_string()
    }

    fn fix(overlay: &str, uv: [f32; 2], px: Option<[f32; 2]>, start: u64, end: u64) -> Fixation {
        Fixation {
            overlay: overlay.into(),
            uv,
            px,
            start_ms: start,
            end_ms: end,
            duration_ms: (end - start) as u32,
            confidence: 0.9,
            dispersion_deg: 0.5,
            dist_m: 1.2,
            samples: ((end - start) / 11) as u32,
        }
    }

    fn report(current: Option<Fixation>, last: Option<Fixation>, history: Vec<Fixation>, ts: u64) -> GazeReport {
        GazeReport { ts_ms: ts, tracking: true, laser: true, valid: true, min_fixation_ms: 200, current, last_stable: last, history }
    }

    #[test]
    fn report_json_round_trip_matches_wayvr() {
        // the exact shape `wayvrctl gaze` prints (serde of wayvr_ipc::packet_server::WlxGaze)
        let j = r#"{"ts_ms":1760000000500,"tracking":true,"laser":true,"valid":true,"min_fixation_ms":200,
          "current":{"overlay":"SUNSHINE","uv":[0.25,0.5],"px":[480.0,540.0],"start_ms":1760000000000,"end_ms":1760000000490,
            "duration_ms":490,"confidence":0.93,"dispersion_deg":0.4,"dist_m":1.3,"samples":44},
          "last_stable":null,"history":[]}"#;
        let r = parse_report(j).unwrap();
        assert_eq!(r.current.as_ref().unwrap().px, Some([480.0, 540.0]));
        assert!(parse_report("{}").is_err());
    }

    #[test]
    fn uv_to_pixel_to_window() {
        let m = parse_monitors(MONITORS).unwrap();
        assert_eq!((m[0].width, m[0].height, m[0].active_ws), (1920.0, 1080.0, 2));
        let w = parse_windows(&clients()).unwrap();
        assert_eq!(w.len(), 4, "the hidden group member is dropped");
        // no px from WayVR: the UV on the monitor named like the overlay
        let f = fix("SUNSHINE", [0.25, 0.5], None, 0, 400);
        let (p, mon) = desktop_point(&f, &m).unwrap();
        assert_eq!((p, mon.as_deref()), ([480.0, 540.0], Some("SUNSHINE")));
        assert_eq!(window_at(&w, &m, p).unwrap().address, "0xb1", "the floating window is on top");
        assert_eq!(window_at(&w, &m, [100.0, 900.0]).unwrap().address, "0xa1");
        assert_eq!(window_at(&w, &m, [1500.0, 900.0]).unwrap().address, "0xa2");
        assert!(window_at(&w, &m, [100.0, 10.0]).is_none(), "the bar: no window (Steam is on workspace 3)");
        // a non-screen overlay has no desktop point
        assert!(desktop_point(&fix("watch", [0.5, 0.5], None, 0, 400), &m).is_none());
    }

    #[test]
    fn scaled_and_rotated_monitors() {
        let j = r#"[{"id":0,"name":"DP-1","width":3840,"height":2160,"x":1920,"y":0,"scale":2,"transform":1,
            "activeWorkspace":{"id":1},"specialWorkspace":{"id":-98}}]"#;
        let m = parse_monitors(j).unwrap();
        assert_eq!((m[0].x, m[0].width, m[0].height, m[0].special_ws), (1920.0, 1080.0, 1920.0, -98));
        let (p, _) = desktop_point(&fix("DP-1", [0.5, 0.5], None, 0, 300), &m).unwrap();
        assert_eq!(p, [1920.0 + 540.0, 960.0]);
    }

    #[test]
    fn special_workspace_and_fullscreen_stack_on_top() {
        let m = parse_monitors(&MONITORS.replace(r#""specialWorkspace":{"id":0,"name":""}"#, r#""specialWorkspace":{"id":-98,"name":"special:scratch"}"#)).unwrap();
        let mut w = parse_windows(&clients()).unwrap();
        w.push(Window { address: "0xd1".into(), workspace: -98, at: [100.0, 100.0], size: [500.0, 500.0], focus: 9, ..w[0].clone() });
        assert_eq!(window_at(&w, &m, [450.0, 350.0]).unwrap().address, "0xd1");
        w[0].fullscreen = true;
        w[0].size = [1920.0, 1052.0];
        assert_eq!(window_at(&w, &m, [1500.0, 900.0]).unwrap().address, "0xa1", "fullscreen above tiled");
    }

    #[test]
    fn hyprland_visible_flag_wins() {
        let m = parse_monitors(MONITORS).unwrap();
        let mut v: Value = serde_json::from_str(&clients()).unwrap();
        v[4]["visible"] = Value::Bool(true); // Steam, on workspace 3
        v[0]["visible"] = Value::Bool(false);
        let w = parse_windows(&v.to_string()).unwrap();
        assert_eq!(window_at(&w, &m, [100.0, 900.0]).unwrap().address, "0xc1", "visible says so");
    }

    #[test]
    fn tabs_group_exact_browser_guess() {
        let w = parse_windows(&clients()).unwrap();
        let term = w.iter().find(|w| w.address == "0xa2").unwrap();
        let g = tab_guess(term, [1500.0, 40.0]).unwrap();
        assert_eq!((g.kind, g.index, g.address.as_deref()), ("group", Some(1), Some("0xa3")));
        assert!(tab_guess(term, [1500.0, 400.0]).is_none());
        let m = parse_monitors(MONITORS).unwrap();
        assert_eq!(window_at(&w, &m, [1500.0, 40.0]).unwrap().address, "0xa2", "the groupbar belongs to the group");
        let web = &w[0];
        let t = tab_guess(web, [300.0, 50.0]).unwrap();
        assert_eq!((t.kind, t.index), ("browser", Some(1)));
        assert!(tab_guess(web, [300.0, 200.0]).is_none());
        assert!((edge_px(web, [300.0, 50.0]) - 22.0).abs() < 1e-3);
    }

    #[test]
    fn pick_last_stable_or_by_utterance_time() {
        let a = fix("SUNSHINE", [0.2, 0.5], Some([384.0, 540.0]), 1000, 1600);
        let b = fix("SUNSHINE", [0.8, 0.5], Some([1536.0, 540.0]), 1700, 2100);
        let c = fix("SUNSHINE", [0.5, 0.1], Some([960.0, 108.0]), 2150, 2250); // short, in progress
        let r = report(Some(c.clone()), Some(b.clone()), vec![a.clone(), b.clone()], 2260);
        let p = pick_fixation(&r, None).unwrap();
        assert_eq!((p.source, p.fixation.start_ms, p.age_ms), ("last_stable", 1700, 160));
        // spoken at 1200: the fixation on the left
        let p = pick_fixation(&r, Some(1200)).unwrap();
        assert_eq!((p.source, p.fixation.start_ms, p.age_ms), ("history", 1000, 0));
        // spoken at 1650 (a saccade in between): the one that just ended
        let p = pick_fixation(&r, Some(1650)).unwrap();
        assert_eq!((p.fixation.start_ms, p.age_ms), (1000, 50));
        // too old
        let old = report(None, Some(a.clone()), vec![a.clone()], 1600 + STABLE_MAX_AGE_MS + 1);
        assert!(pick_fixation(&old, None).is_none());
        assert!(pick_fixation(&old, Some(1600 + BEFORE_AT_MS + 1)).is_none());
        // the current long fixation is reported as "current"
        let d = fix("SUNSHINE", [0.5, 0.5], None, 3000, 3500);
        let r = report(Some(d.clone()), Some(d), vec![], 3510);
        assert_eq!(pick_fixation(&r, None).unwrap().source, "current");
        assert_eq!(pick_fixation(&r, Some(2900)).unwrap().source, "current", "looked after speaking started");
    }

    struct Fake {
        gaze: String,
    }
    impl Runner for Fake {
        fn run(&mut self, argv: &[String], _c: bool) -> Result<String, String> {
            match (argv[0].as_str(), argv.last().map(String::as_str)) {
                (_, Some("gaze")) => Ok(self.gaze.clone()),
                ("hyprctl", Some("monitors")) => Ok(MONITORS.into()),
                ("hyprctl", Some("clients")) => Ok(clients()),
                _ => Err("unexpected".into()),
            }
        }
        fn passthrough(&mut self, _l: &str) -> Result<String, String> {
            Err("no".into())
        }
        fn toast(&mut self, _t: &str) {}
        fn captions(&mut self, _on: bool) -> Result<(), String> {
            Ok(())
        }
        fn skyboxes(&mut self) -> Vec<String> {
            Vec::new()
        }
    }

    #[test]
    fn resolve_end_to_end_with_fake_tools() {
        let f = fix("SUNSHINE", [0.1, 0.5], Some([192.0, 540.0]), 1000, 1500);
        let r = report(Some(f.clone()), Some(f), vec![], 1510);
        let mut fake = Fake { gaze: serde_json::to_string(&r).unwrap() };
        let t = resolve(&mut fake, "wayvrctl", None).unwrap().unwrap();
        let w = t.window.as_ref().unwrap();
        assert_eq!((w.address.as_str(), w.class.as_str()), ("0xa1", "chromium"));
        assert_eq!(t.monitor.as_deref(), Some("SUNSHINE"));
        assert!(t.error_m > 0.06 && t.error_m < 0.07);
        assert!(describe(&t).contains("0xa1 chromium"));
        let off = GazeReport { tracking: false, ..r };
        fake.gaze = serde_json::to_string(&off).unwrap();
        assert!(resolve(&mut fake, "wayvrctl", None).is_err());
        fake.gaze = "garbage".into();
        assert!(resolve(&mut fake, "wayvrctl", None).is_err());
    }
}
