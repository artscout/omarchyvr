//! The `omarchyvr` launcher as the app uses it: what the installed copy supports (`omarchyvr desk help`), the exact command
//! lines (the app never starts Monado itself: LESSONS row 5, every start goes through the launcher's env builder in
//! `rust/omarchyvr-tools/src/vr/config.rs`), the plan of a desk start (which Monado build and which binaries, computed with
//! the launcher's own `parse_desk`/`plan` so the checks look at exactly what will run), and whether the installed copy
//! matches the build (row 8).
use crate::machine::{FileMeta, Machine};
use crate::paths::Paths;
use omarchyvr_config::ctrl_inhand::CtrlMode;
use omarchyvr_tools::args::Args;
use omarchyvr_tools::vr::desk::{parse_desk, plan};
use std::path::{Path, PathBuf};

/// What the installed launcher can do, read from `omarchyvr desk help`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LauncherCaps {
    /// `desk` exists at all.
    pub desk: bool,
    /// `--eye`.
    pub eye: bool,
    /// `--6dof`.
    pub sixdof: bool,
    /// `--ctrl-track` (optical controllers, needs `--6dof`).
    pub ctrl_track: bool,
    /// `--voice`.
    pub voice: bool,
    /// `--no-passthrough`.
    pub no_passthrough: bool,
    /// `--no-perf` (the wrist performance readout is on by default).
    pub no_perf: bool,
    /// `--anchor` (`PIMAX_ANCHOR=1`: the room offset hook).
    pub anchor: bool,
    /// `--preset cinema|work|assistant`.
    pub preset: bool,
    /// `desk captions on|off|status` (the live subtitles).
    pub captions: bool,
    /// `--no-eye`: eye tracking is on by default at a desk start (newer launchers).
    pub no_eye: bool,
    /// `desk restart` / `vr restart` (stop, then start with the recorded arguments) and the session watchdog.
    pub restart: bool,
    /// `--hands` (camera hand tracking from the hands build ~/src/monado-ht/build-ht, needs `--6dof`).
    pub hands: bool,
    /// `--screens N` (VR screens as Hyprland outputs, docs/SCREENS.md).
    pub screens: bool,
    /// `--controllers when-held|always|off` (PIMAX_CTRL_INHAND, docs/CONTROLLERS-INHAND.md).
    pub controllers: bool,
    /// `--ctrl-angle DEG` and the tracking switches `--old-exposure`, `--no-room-glide`, `--no-ctrl-steady`
    /// (tracking plan phase 1, docs/TRACKING-STABILITY.md).
    pub tracking: bool,
}

/// Parse the `omarchyvr desk help` text.
pub fn parse_caps(usage: &str) -> LauncherCaps {
    let has = |f: &str| usage.contains(f);
    LauncherCaps {
        desk: usage.contains("omarchyvr desk"),
        eye: has("--eye"),
        sixdof: has("--6dof"),
        ctrl_track: has("--ctrl-track"),
        voice: has("--voice"),
        no_passthrough: has("--no-passthrough"),
        no_perf: has("--no-perf"),
        anchor: has("--anchor"),
        preset: has("--preset"),
        captions: has("captions on|off"),
        no_eye: has("--no-eye"),
        restart: usage.lines().any(|l| l.starts_with("  restart")),
        hands: has("--hands"),
        screens: has("--screens"),
        controllers: has("--controllers"),
        tracking: has("--ctrl-angle") && has("--no-ctrl-steady"),
    }
}

/// A WayVR preset of `desk start --preset` (conf.d/60-preset.yaml of the launcher).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Preset {
    /// No preset.
    #[default]
    None,
    /// Curved screens, passthrough dimmed.
    Cinema,
    /// Flat, sharper screens.
    Work,
    /// Voice captions (the LLM assistant setup).
    Assistant,
}

impl Preset {
    /// Every preset.
    pub const ALL: [Preset; 4] = [Preset::None, Preset::Cinema, Preset::Work, Preset::Assistant];

    /// The launcher's name.
    pub fn name(self) -> &'static str {
        match self {
            Preset::None => "none",
            Preset::Cinema => "cinema",
            Preset::Work => "work",
            Preset::Assistant => "assistant",
        }
    }

    /// From [`Preset::name`].
    pub fn parse(s: &str) -> Preset {
        Preset::ALL.into_iter().find(|p| p.name() == s.trim()).unwrap_or_default()
    }
}

/// The desk-mode switches of the main page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeskToggles {
    /// Eye tracking (`--eye`).
    pub eye: bool,
    /// 6DoF head tracking (`--6dof`).
    pub sixdof: bool,
    /// Optical controller tracking (`--ctrl-track`, needs 6DoF).
    pub ctrl_track: bool,
    /// Camera hand tracking (`--hands`, needs 6DoF): Monado from the hands build when it is usable.
    pub hands: bool,
    /// The hands take the controller roles (`--hands-roles`, needs `--hands`): pinch = trigger for apps that only
    /// read controllers.
    pub hands_roles: bool,
    /// Camera passthrough (off = `--no-passthrough`).
    pub passthrough: bool,
    /// Voice captions on a WayVR panel under the main screen (`--voice`).
    pub voice: bool,
    /// 90 Hz (the default) instead of 72 Hz.
    pub hz90: bool,
    /// The performance readout on the right wrist (off = `--no-perf`).
    pub perf: bool,
    /// The room offset hook (`--anchor`, `PIMAX_ANCHOR=1`): the AprilTag and the room map can place the world.
    pub anchor: bool,
    /// WayVR preset (`--preset`).
    pub preset: Preset,
    /// The launcher starts eye tracking unless `--no-eye` (newer launchers; from `omarchyvr desk help`, not saved).
    pub eye_default_on: bool,
    /// VR screens (`--screens N`, 1 to 6; 4 = the desktop + left, right and up, 6 adds up-left and up-right; shown on demand).
    pub screens: u8,
    /// Controllers in Work (`desk start --controllers`, default when picked up: docs/CONTROLLERS-INHAND.md).
    pub ctrl_work: CtrlMode,
    /// Controllers in Play (`vr start --controllers`, default always: games expect them).
    pub ctrl_play: CtrlMode,
    /// Controller angle in degrees, -20..70 (`--ctrl-angle`, PIMAX_CTRL_ANGLE; default 30, saved in setup.toml too).
    pub ctrl_angle: i32,
    /// "New camera exposure" (off = `--old-exposure`, PIMAX_CAM_AE_LAW=old).
    pub new_exposure: bool,
    /// "Smooth room corrections" (off = `--no-room-glide`, PIMAX_ANCHOR_GLIDE=0).
    pub room_glide: bool,
    /// "Steadier controllers" (off = `--no-ctrl-steady`, PIMAX_CTRL_TRACK_HYST=0).
    pub ctrl_steady: bool,
    /// The launcher knows `--ctrl-angle` and the tracking switches (from `omarchyvr desk help`, not saved): only then
    /// are they passed.
    pub tracking_flags: bool,
}

impl Default for DeskToggles {
    /// The user's normal setup (camera hands off by default since 2026-10-16: they still need work; the user switches
    /// them on in developer mode, and Play only gets them once the user switched them on): eye tracking, 6DoF, optical controllers, passthrough, voice, the room anchor, the
    /// wrist readout, 90 Hz.
    fn default() -> Self {
        DeskToggles {
            eye: true,
            sixdof: true,
            ctrl_track: true,
            hands: false,
            hands_roles: false,
            passthrough: true,
            voice: true,
            hz90: true,
            perf: true,
            anchor: true,
            preset: Preset::None,
            eye_default_on: false,
            screens: crate::screens::DEFAULT_SCREENS,
            ctrl_work: CtrlMode::WhenHeld,
            ctrl_play: CtrlMode::Always,
            ctrl_angle: omarchyvr_config::ctrl_angle::DEFAULT_DEG,
            new_exposure: true,
            room_glide: true,
            ctrl_steady: true,
            tracking_flags: false,
        }
    }
}

/// The switches of a recorded start (`<session dir>/start-argv.txt`: line 1 `desk` or `vr`, then one argument per
/// line), on top of `base`: a session started anywhere (the app, the CLI, omarchyvrd, the watchdog) becomes the app's
/// setting, so the next Play repeats it. Returns the switches and whether it was a game session (`vr`). Switches a
/// game session does not have (passthrough, voice, eye) keep their value from `base`.
pub fn toggles_from_argv(text: &str, base: DeskToggles) -> Option<(DeskToggles, bool)> {
    let mut lines = text.lines();
    let vr = match lines.next()?.trim() {
        "desk" => false,
        "vr" => true,
        _ => return None,
    };
    let args: Vec<&str> = lines.map(str::trim).collect();
    let has = |f: &str| args.contains(&f);
    let value = |f: &str| args.iter().position(|a| *a == f).and_then(|i| args.get(i + 1)).copied();
    let mut t = base;
    t.hz90 = value("--fps") != Some("72"); // no --fps: the launcher default, 90 Hz
    t.sixdof = has("--6dof");
    t.ctrl_track = t.sixdof && has("--ctrl-track");
    // a game session without --hands says nothing about Work's hands (default on): keep the saved switch
    if !vr || has("--hands") {
        t.hands = t.sixdof && has("--hands");
        t.hands_roles = t.hands && has("--hands-roles");
    }
    if !(vr && has("--home")) {
        t.anchor = has("--anchor");
    }
    // an absent --controllers is the launcher's default for that side (Work: when picked up, Play: always)
    let ctrl = value("--controllers").and_then(CtrlMode::parse);
    if vr {
        t.ctrl_play = ctrl.unwrap_or(CtrlMode::Always);
    } else {
        t.ctrl_work = ctrl.unwrap_or(CtrlMode::WhenHeld);
    }
    if !vr {
        // an absent flag is the launcher's default; `omarchyvr desk screens show|add` raises it in the record
        t.screens = crate::screens::clamp(value("--screens").and_then(|v| v.parse().ok()).unwrap_or(crate::screens::DEFAULT_SCREENS));
    }
    // the tracking switches: an absent flag is on (the launcher's default); an absent angle keeps the saved one
    if let Some(d) = value("--ctrl-angle").and_then(|v| omarchyvr_config::ctrl_angle::parse(v).ok()) {
        t.ctrl_angle = d;
    }
    t.new_exposure = !has("--old-exposure");
    t.room_glide = !has("--no-room-glide");
    t.ctrl_steady = !has("--no-ctrl-steady");
    if vr && has("--home") {
        // the VR home (docs/VR-HOME.md) decides its own readout and room anchor: Play's switches stay as they are
    } else if vr {
        t.perf = has("--perf");
        // a game session's eye flag is Play's own switch ("Eye tracking in games", gamemode.toml): Work's stays
    } else {
        t.passthrough = !has("--no-passthrough");
        t.voice = has("--voice") || value("--preset") == Some("assistant");
        t.perf = !has("--no-perf");
        t.eye = if has("--no-eye") { false } else if has("--eye") { true } else { base.eye_default_on };
        t.preset = match value("--preset") {
            Some(p) if p.starts_with("work") => Preset::Work,
            Some(p) => Preset::parse(p),
            None => Preset::None,
        };
    }
    Some((t, vr))
}

impl DeskToggles {
    /// The default set with eye tracking on (tests and demo data).
    pub fn with_eye() -> Self {
        // the minimal desk set (eye, 6DoF, passthrough, readout): what the demo fixtures' launcher supports
        DeskToggles { eye: true, ctrl_track: false, hands: false, voice: false, anchor: false, ..DeskToggles::default() }
    }

    /// Refresh rate.
    pub fn hz(&self) -> u32 {
        if self.hz90 {
            90
        } else {
            72
        }
    }

    /// Does this mode stream the cameras (LESSONS row 2)?
    pub fn needs_cameras(&self) -> bool {
        self.sixdof || self.ctrl_track || self.passthrough
    }

    /// Does Monado itself open the cameras (needs `PIMAX_CAM=ON`, row 6)?
    pub fn monado_uses_cameras(&self) -> bool {
        self.sixdof || self.ctrl_track
    }

    /// The same toggles with eye tracking off (LESSONS row 3: free the eye chip for the eye tools).
    pub fn without_eye(mut self) -> Self {
        self.eye = false;
        self
    }

    /// Only what the launcher supports (controller tracking also needs 6DoF).
    pub fn supported(mut self, caps: &LauncherCaps) -> Self {
        self.eye &= caps.eye;
        self.sixdof &= caps.sixdof;
        self.ctrl_track &= caps.ctrl_track && self.sixdof;
        self.hands &= caps.hands && self.sixdof;
        self.hands_roles &= self.hands;
        self.voice &= caps.voice;
        self.anchor &= caps.anchor;
        if !caps.preset {
            self.preset = Preset::None;
        }
        // an older launcher has no VR screens: one screen, no flag
        self.screens = if caps.screens { crate::screens::clamp(self.screens) } else { crate::screens::DEFAULT_SCREENS };
        if !caps.no_passthrough {
            self.passthrough = true;
        }
        if !caps.controllers {
            // an older launcher: its own behaviour (always shown), no flag to pass
            self.ctrl_work = CtrlMode::WhenHeld;
            self.ctrl_play = CtrlMode::Always;
        }
        if !caps.no_perf {
            // an older launcher has no readout at all: nothing to switch off, and no --no-perf to pass
            self.perf = true;
        }
        self.tracking_flags &= caps.tracking;
        self
    }
}

/// The flags of a desk start (without `desk start`).
pub fn desk_flags(t: &DeskToggles) -> Vec<String> {
    let mut v = vec!["--fps".to_string(), t.hz().to_string()];
    // a newer launcher starts eye tracking by itself (--no-eye turns it off); an older one needs --eye
    match (t.eye, t.eye_default_on) {
        (true, false) => v.push("--eye".into()),
        (false, true) => v.push("--no-eye".into()),
        _ => {}
    }
    if t.sixdof {
        v.push("--6dof".into());
        if t.ctrl_track {
            v.push("--ctrl-track".into());
        }
        if t.hands {
            v.push("--hands".into());
            if t.hands_roles {
                v.push("--hands-roles".into());
            }
        }
    }
    if !t.passthrough {
        v.push("--no-passthrough".into());
    }
    if t.voice {
        v.push("--voice".into());
    }
    if !t.perf {
        v.push("--no-perf".into());
    }
    if t.anchor {
        v.push("--anchor".into());
    }
    if t.preset != Preset::None {
        v.push("--preset".into());
        v.push(t.preset.name().into());
    }
    if t.ctrl_work != CtrlMode::WhenHeld {
        v.push("--controllers".into());
        v.push(t.ctrl_work.name().into());
    }
    if t.screens != crate::screens::DEFAULT_SCREENS {
        v.push("--screens".into());
        v.push(crate::screens::clamp(t.screens).to_string());
    }
    tracking_flags(t, &mut v);
    v
}

/// `--ctrl-angle DEG` and the flags of the tracking switches that are off (a launcher that knows them; Work and Play).
fn tracking_flags(t: &DeskToggles, v: &mut Vec<String>) {
    if !t.tracking_flags {
        return;
    }
    v.push("--ctrl-angle".into());
    v.push(t.ctrl_angle.clamp(omarchyvr_config::ctrl_angle::MIN_DEG, omarchyvr_config::ctrl_angle::MAX_DEG).to_string());
    for (on, flag) in [(t.new_exposure, "--old-exposure"), (t.room_glide, "--no-room-glide"), (t.ctrl_steady, "--no-ctrl-steady")] {
        if !on {
            v.push(flag.into());
        }
    }
}

/// `omarchyvr vr start` arguments for a game session (Monado alone): refresh rate, 6DoF, controller tracking, the wrist
/// readout (`--perf`, a second minimal WayVR). Eye tracking is a desk switch only. `force` as for the desk.
///
/// `light` is the Light game mode switch (docs/GAME-MODE.md): the launcher gets `--light` (or `--no-light`) explicitly, so the
/// choice is in start-argv and every restart keeps it. `game` is the Steam game the next Play is for (`--game`).
pub fn vr_game_args(t: &DeskToggles, force: bool, light: bool, game: Option<&str>) -> Vec<String> {
    let mut v: Vec<String> = vec!["vr".into(), "start".into(), "--fps".into(), t.hz().to_string()];
    if t.sixdof {
        v.push("--6dof".into());
        if t.ctrl_track {
            v.push("--ctrl-track".into());
        }
        if t.hands {
            v.push("--hands".into());
            if t.hands_roles {
                v.push("--hands-roles".into());
            }
        }
    }
    if t.perf {
        v.push("--perf".into());
    }
    if t.anchor {
        v.push("--anchor".into());
    }
    if t.ctrl_play != CtrlMode::Always {
        v.push("--controllers".into());
        v.push(t.ctrl_play.name().into());
    }
    tracking_flags(t, &mut v);
    // eye tracking in games: its own Play switch (off by default), passed explicitly so start-argv keeps it; a launcher
    // that only knows --no-eye (eye on by default) gets that when the switch is off
    if t.eye {
        v.push("--eye".into());
    } else if t.eye_default_on {
        v.push("--no-eye".into());
    }
    v.push(if light { "--light" } else { "--no-light" }.into());
    if let Some(g) = game.filter(|g| !g.is_empty() && g.bytes().all(|b| b.is_ascii_digit())) {
        v.push("--game".into());
        v.push(g.into());
    }
    if force {
        v.push("--force".into());
    }
    v
}

/// `omarchyvr desk start ...` arguments. `force` only after the user's explicit consent (LESSONS row 9).
pub fn desk_start_args(t: &DeskToggles, force: bool) -> Vec<String> {
    let mut v = vec!["desk".to_string(), "start".to_string()];
    v.extend(desk_flags(t));
    if force {
        v.push("--force".into());
    }
    v
}

/// `omarchyvr desk stop --grace 30` (WayVR, passthrough, ... by pid, then Monado: SIGINT, SIGKILL only after 30 s).
pub fn desk_stop_args() -> Vec<String> {
    ["desk", "stop", "--grace", "30"].map(String::from).to_vec()
}

/// `omarchyvr vr stop --grace 30` (`all`: also monado-service processes the launcher did not start).
pub fn vr_stop_args(all: bool) -> Vec<String> {
    let mut v: Vec<String> = ["vr", "stop", "--grace", "30"].map(String::from).to_vec();
    if all {
        v.push("--all".into());
    }
    v
}

/// `omarchyvr vr start --fps N --monado-build DIR` (tool sessions: lens test, eye calibration).
pub fn vr_start_args(hz: u32, build: &Path) -> Vec<String> {
    vec![
        "vr".into(),
        "start".into(),
        "--fps".into(),
        hz.to_string(),
        "--monado-build".into(),
        build.display().to_string(),
    ]
}

/// One process the desk start launches after Monado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedProc {
    /// `passthrough`, `wayvr`, `ctrltrack`, `voice`, `voicecap`, `voicehud`.
    pub name: String,
    /// Executable.
    pub program: PathBuf,
}

/// What `omarchyvr desk start <flags>` will run, per the launcher's own parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeskPlan {
    /// Monado build directory.
    pub monado_build: PathBuf,
    /// Its service binary.
    pub service: PathBuf,
    /// Clients in start order.
    pub clients: Vec<PlannedProc>,
    /// `--6dof`: `pimax-cam` and `omarchyvr-slam`.
    pub sixdof_bins: Vec<PathBuf>,
    /// Basalt library (`VIT_SYSTEM_LIBRARY_PATH`).
    pub basalt: Option<PathBuf>,
    /// Session directory.
    pub session_dir: PathBuf,
}

/// Compute the plan. `env` is the environment the launcher will see.
pub fn desk_plan(t: &DeskToggles, env: &dyn Fn(&str) -> Option<String>) -> Result<DeskPlan, String> {
    let mut a = Args(desk_flags(t));
    let o = parse_desk(&mut a, env).map_err(|e| e.msg)?;
    if let Some(x) = a.0.first() {
        return Err(format!("the launcher library does not know {x}"));
    }
    let basalt = o
        .vr
        .extra_env
        .iter()
        .find(|(k, _)| k == "VIT_SYSTEM_LIBRARY_PATH")
        .map(|(_, v)| PathBuf::from(v));
    Ok(DeskPlan {
        monado_build: o.vr.monado_build.clone(),
        service: o.vr.service_path(),
        clients: plan(&o)
            .into_iter()
            .map(|s| PlannedProc {
                name: s.name.to_string(),
                program: s.program,
            })
            .collect(),
        sixdof_bins: o
            .sixdof
            .as_ref()
            .map(|s| vec![s.cam_bin.clone(), s.slam_bin.clone()])
            .unwrap_or_default(),
        basalt,
        session_dir: o.vr.session_dir.clone(),
    })
}

/// `OXR_DEBUG_IPD_MM` from `~/.config/omarchyvr/ipd.env` (written by `pimax-eye ipd --apply` / `--watch`; LESSONS row 18:
/// the launcher does not read it, so the app passes it on). Values outside 50-80 mm are ignored.
pub fn parse_ipd_env(text: &str) -> Option<f32> {
    text.lines().find_map(|l| {
        let (k, v) = l.trim().split_once('=')?;
        if k.trim() != "OXR_DEBUG_IPD_MM" {
            return None;
        }
        let mm: f32 = v.trim().trim_matches('"').parse().ok()?;
        (50.0..=80.0).contains(&mm).then_some(mm)
    })
}

/// Variables of the app's own environment that would leak into Monado and change it (the launcher's watch list:
/// stale experiment settings such as `XRT_COMPOSITOR_DUMP_DIR`). The app removes them from the launcher's environment.
pub fn stale_experiment_env(env: &dyn Fn(&str) -> Option<String>) -> Vec<(String, String)> {
    omarchyvr_tools::vr::config::INHERITED_WATCHLIST
        .iter()
        .filter_map(|k| env(k).filter(|v| !v.is_empty()).map(|v| (k.to_string(), v)))
        .collect()
}

/// Installed launcher versus the workspace build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallState {
    /// Neither installed nor built.
    Missing,
    /// Built but not installed.
    NotInstalled,
    /// Installed, no build to compare with.
    InstalledOnly,
    /// Same bytes as the build, and the build is newer than its sources.
    UpToDate,
    /// The installed copy differs from the build.
    Differs,
    /// Same as the build, but sources changed after the build.
    BuildStale,
}

/// Compare an installed binary with its build; `newest_source` = newest mtime of the sources (seconds).
pub fn install_state(m: &dyn Machine, installed: &Path, built: &Path, newest_source: Option<u64>) -> InstallState {
    let i = m.meta(installed);
    let b = m.meta(built);
    match (i, b) {
        (None, None) => InstallState::Missing,
        (None, Some(_)) => InstallState::NotInstalled,
        (Some(_), None) => InstallState::InstalledOnly,
        (Some(im), Some(bm)) => {
            let same = im.len == bm.len && m.read(installed) == m.read(built);
            if !same {
                InstallState::Differs
            } else if newest_source.is_some_and(|s| s > bm.mtime) {
                InstallState::BuildStale
            } else {
                InstallState::UpToDate
            }
        }
    }
}

/// Newest modification time of the `.rs` / `Cargo.toml` files under the given crate directories (two levels of
/// `src/` subdirectories are enough for these crates).
pub fn newest_source(m: &dyn Machine, crates: &[PathBuf]) -> Option<u64> {
    let mut newest: Option<u64> = None;
    let mut visit = |p: &Path| {
        if let Some(FileMeta { mtime, is_dir: false, .. }) = m.meta(p) {
            newest = Some(newest.map_or(mtime, |n: u64| n.max(mtime)));
        }
    };
    for c in crates {
        visit(&c.join("Cargo.toml"));
        let src = c.join("src");
        for e in m.list(&src) {
            let p = src.join(&e);
            if e.ends_with(".rs") {
                visit(&p);
            } else {
                for e2 in m.list(&p) {
                    if e2.ends_with(".rs") {
                        visit(&p.join(e2));
                    }
                }
            }
        }
    }
    newest
}

/// The crates the `omarchyvr` binary is built from.
pub fn launcher_crates(p: &Paths) -> Vec<PathBuf> {
    ["omarchyvr-tools", "pimax", "pimax-proto"].iter().map(|c| p.rust.join(c)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::FakeMachine;

    const USAGE_NEW: &str = include_str!("../tests/fixtures/pimax-desk-help.txt");

    #[test]
    fn capabilities_from_the_help_text() {
        let c = parse_caps(USAGE_NEW);
        assert!(c.desk && c.eye && c.sixdof && c.ctrl_track && c.voice && c.no_passthrough);
        let old = "usage: omarchyvr desk <command> [options]\n  start [--no-passthrough] [--eye] [--6dof] [--voice]\n";
        let c = parse_caps(old);
        assert!(c.sixdof && !c.ctrl_track);
        assert_eq!(parse_caps(""), LauncherCaps::default());
    }

    #[test]
    fn a_recorded_start_becomes_the_setting() {
        let base = DeskToggles { voice: false, ctrl_track: false, anchor: false, eye_default_on: true, ..DeskToggles::default() };
        let (t, vr) = toggles_from_argv("desk\n--6dof\n--ctrl-track\n--voice\n--anchor\n--hands\n--preset\nwork-3\n", base).unwrap();
        assert!(!vr && t.sixdof && t.ctrl_track && t.voice && t.anchor && t.hands && !t.hands_roles && t.eye && t.passthrough);
        assert_eq!(t.preset, Preset::Work);
        // the same flags come back out (Play repeats the session)
        assert_eq!(desk_flags(&t), ["--fps", "90", "--6dof", "--ctrl-track", "--hands", "--voice", "--anchor", "--preset", "work"]);
        let (t, vr) = toggles_from_argv("vr\n--fps\n90\n--6dof\n--perf\n", base).unwrap();
        assert!(vr && t.hz90 && t.sixdof && !t.ctrl_track && t.perf && !t.voice);
        let (t, _) = toggles_from_argv("desk\n--no-eye\n--no-passthrough\n", base).unwrap();
        assert!(!t.eye && !t.passthrough && !t.sixdof);
        let (t, _) = toggles_from_argv("desk\n--screens\n6\n", base).unwrap();
        assert_eq!(t.screens, 6);
        assert_eq!(toggles_from_argv("desk\n--voice\n", base).unwrap().0.screens, 4, "no flag = the default");
        assert!(toggles_from_argv("", base).is_none());
        assert!(toggles_from_argv("other\n", base).is_none());
        let d = DeskToggles::default();
        assert!(d.voice && d.ctrl_track && d.anchor && d.sixdof, "fresh defaults: the user's normal setup");
        assert!(!d.hands && !d.hands_roles, "Work: camera hands off by default (user 2026-10-16), hands-replace-controllers off");
        assert!(!desk_start_args(&d, false).contains(&"--hands".to_string()));
        let on = DeskToggles { hands: true, ..d };
        assert!(desk_start_args(&on, false).contains(&"--hands".to_string()), "switched on: the flag");
        assert!(!desk_start_args(&d.supported(&LauncherCaps::default()), false).contains(&"--hands".to_string()), "no hands build: no flag");
    }

    #[test]
    fn controllers_switches_round_trip() {
        let base = DeskToggles::default();
        assert_eq!((base.ctrl_work, base.ctrl_play), (CtrlMode::WhenHeld, CtrlMode::Always), "Work: when picked up, Play: always");
        assert!(!desk_flags(&base).contains(&"--controllers".to_string()), "the default needs no flag");
        assert!(!vr_game_args(&base, false, false, None).contains(&"--controllers".to_string()));
        let w = DeskToggles { ctrl_work: CtrlMode::Always, ctrl_play: CtrlMode::WhenHeld, ..base };
        let f = desk_flags(&w);
        let i = f.iter().position(|x| x == "--controllers").unwrap();
        assert_eq!(f[i + 1], "always");
        let g = vr_game_args(&w, false, false, None);
        let i = g.iter().position(|x| x == "--controllers").unwrap();
        assert_eq!(g[i + 1], "when-held");
        // the recorded start (start-argv.txt) gives the switch back, per side
        let (t, _) = toggles_from_argv("desk\n--controllers\noff\n", base).unwrap();
        assert_eq!((t.ctrl_work, t.ctrl_play), (CtrlMode::Off, CtrlMode::Always));
        let (t, _) = toggles_from_argv("desk\n--6dof\n", w).unwrap();
        assert_eq!(t.ctrl_work, CtrlMode::WhenHeld, "no flag = the launcher's Work default");
        let (t, _) = toggles_from_argv("vr\n--controllers\nwhen-held\n", base).unwrap();
        assert_eq!((t.ctrl_work, t.ctrl_play), (CtrlMode::WhenHeld, CtrlMode::WhenHeld));
        // an older launcher without the flag: nothing passed
        let old = w.supported(&LauncherCaps { desk: true, ..LauncherCaps::default() });
        assert_eq!((old.ctrl_work, old.ctrl_play), (CtrlMode::WhenHeld, CtrlMode::Always));
        assert!(parse_caps(USAGE_NEW).controllers);
    }

    #[test]
    fn command_lines() {
        let t = DeskToggles::with_eye();
        assert_eq!(desk_start_args(&t, false).join(" "), "desk start --fps 90 --eye --6dof");
        let all = DeskToggles { eye: true, sixdof: true, ctrl_track: true, hands: false, hands_roles: false, passthrough: false, voice: true, hz90: true, perf: false, anchor: false, preset: Preset::None, eye_default_on: false, screens: 4, ctrl_work: CtrlMode::WhenHeld, ctrl_play: CtrlMode::Always, ..DeskToggles::default() };
        // a newer launcher: eye on by default, --no-eye to turn it off
        let newer = DeskToggles { eye_default_on: true, ..all };
        assert!(!desk_flags(&newer).contains(&"--eye".to_string()));
        assert!(desk_flags(&DeskToggles { eye: false, ..newer }).contains(&"--no-eye".to_string()));
        assert!(parse_caps("  start [--eye] [--no-eye]").no_eye);
        assert_eq!(
            desk_start_args(&all, true).join(" "),
            "desk start --fps 90 --eye --6dof --ctrl-track --no-passthrough --voice --no-perf --force"
        );
        // a game session: Monado alone, the wrist readout with --perf; eye tracking only with "Eye tracking in games"
        // (vr_toggles puts that switch in `eye`), passed explicitly
        let t_play = DeskToggles { eye: false, ..t };
        assert_eq!(vr_game_args(&t_play, false, false, None).join(" "), "vr start --fps 90 --6dof --perf --no-light");
        assert_eq!(vr_game_args(&t, false, false, None).join(" "), "vr start --fps 90 --6dof --perf --eye --no-light");
        assert_eq!(vr_game_args(&DeskToggles { eye: false, eye_default_on: true, ..t }, false, false, None).join(" "), "vr start --fps 90 --6dof --perf --no-eye --no-light");
        let all_play = DeskToggles { eye: false, ..all };
        assert_eq!(vr_game_args(&all_play, true, false, None).join(" "), "vr start --fps 90 --6dof --ctrl-track --no-light --force");
        // light game mode: the flag is always passed (start-argv keeps it); the game, when Play is for one
        assert_eq!(vr_game_args(&all_play, false, true, None).join(" "), "vr start --fps 90 --6dof --ctrl-track --light");
        assert_eq!(vr_game_args(&all_play, false, true, Some("546560")).join(" "), "vr start --fps 90 --6dof --ctrl-track --light --game 546560");
        assert!(!vr_game_args(&all_play, false, true, Some("5;rm")).contains(&"--game".to_string()), "digits only");
        // camera hands (the hands build) with 6DoF only, for desk and game sessions
        let h = DeskToggles { hands: true, ..DeskToggles::with_eye() };
        assert_eq!(desk_start_args(&h, false).join(" "), "desk start --fps 90 --eye --6dof --hands");
        assert_eq!(vr_game_args(&DeskToggles { eye: false, ..h }, false, false, None).join(" "), "vr start --fps 90 --6dof --hands --perf --no-light");
        assert!(!desk_flags(&DeskToggles { sixdof: false, ..h }).contains(&"--hands".to_string()));
        assert!(parse_caps("  start [--6dof [--ctrl-track] [--hands [--hands-roles]]]").hands);
        // controller tracking is only passed together with 6DoF
        let ct = DeskToggles { sixdof: false, ctrl_track: true, ..DeskToggles::with_eye() };
        assert!(!desk_flags(&ct).contains(&"--ctrl-track".to_string()));
        assert_eq!(desk_stop_args().join(" "), "desk stop --grace 30");
        assert_eq!(vr_stop_args(false).join(" "), "vr stop --grace 30");
        assert_eq!(vr_stop_args(true).join(" "), "vr stop --grace 30 --all");
        assert_eq!(
            vr_start_args(72, Path::new("/b")).join(" "),
            "vr start --fps 72 --monado-build /b"
        );
        let old = parse_caps("usage: omarchyvr desk\n --eye --6dof --voice --no-passthrough");
        let s = all.supported(&old);
        assert!(!s.ctrl_track && s.sixdof && s.eye);
        let two = DeskToggles { screens: 2, ..DeskToggles::default() };
        assert!(desk_flags(&two).windows(2).any(|w| w == ["--screens", "2"]));
        assert!(!desk_flags(&DeskToggles::default()).iter().any(|f| f == "--screens"), "4 is the launcher default");
        assert_eq!(two.supported(&LauncherCaps::default()).screens, 4, "an older launcher: no flag");
        assert!(parse_caps("omarchyvr desk ... [--screens 1-6]").screens);
        let off = DeskToggles { perf: false, ..DeskToggles::default() };
        assert!(off.supported(&old).perf, "an older launcher has no --no-perf to pass");
        assert!(!off.supported(&parse_caps("omarchyvr desk --no-perf")).perf);
        assert!(!DeskToggles::with_eye().without_eye().eye);
    }

    #[test]
    fn plan_uses_the_launchers_own_build_choice() {
        let env = |k: &str| match k {
            "HOME" => Some("/home/u".to_string()),
            "XDG_RUNTIME_DIR" => Some("/run/user/1000".to_string()),
            _ => None,
        };
        let p = desk_plan(&DeskToggles::with_eye(), &env).unwrap();
        assert_eq!(p.monado_build, PathBuf::from("/home/u/.local/lib/omarchyvr/monado"));
        assert_eq!(p.service, PathBuf::from("/home/u/.local/lib/omarchyvr/monado/src/xrt/targets/service/monado-service"));
        assert_eq!(
            p.clients.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["passthrough", "wayvr", "screens", "perf"],
            "the screens watcher (4 VR screens) and the wrist performance readout run by default"
        );
        assert_eq!(p.sixdof_bins.len(), 2);
        assert_eq!(p.basalt, Some(PathBuf::from("/home/u/.local/lib/libbasalt.so")));
        let plain = DeskToggles { eye: false, sixdof: false, ..DeskToggles::with_eye() };
        let p = desk_plan(&plain, &env).unwrap();
        assert_eq!(p.monado_build, PathBuf::from("/home/u/src/monado/build"), "no eye, no 6DoF: the plain build");
        assert!(p.sixdof_bins.is_empty() && p.basalt.is_none());
        let ct = DeskToggles { ctrl_track: true, voice: true, ..DeskToggles::with_eye() };
        let p = desk_plan(&ct, &env).unwrap();
        // voice may add more clients after these (voice actions)
        assert_eq!(
            p.clients.iter().map(|c| c.name.as_str()).take(7).collect::<Vec<_>>(),
            ["ctrltrack", "passthrough", "wayvr", "screens", "perf", "voice", "voicecap"]
        );
        // PIMAX_MONADO_BUILD wins, like in the launcher
        let env2 = |k: &str| match k {
            "HOME" => Some("/home/u".to_string()),
            "PIMAX_MONADO_BUILD" => Some("/b".to_string()),
            _ => None,
        };
        assert_eq!(desk_plan(&DeskToggles::with_eye(), &env2).unwrap().monado_build, PathBuf::from("/b"));
    }

    #[test]
    fn ipd_env_and_stale_variables() {
        assert_eq!(parse_ipd_env("OXR_DEBUG_IPD_MM=65.9\n"), Some(65.9));
        assert_eq!(parse_ipd_env("# x\nOXR_DEBUG_IPD_MM = \"64\"\n"), Some(64.0));
        assert_eq!(parse_ipd_env("OXR_DEBUG_IPD_MM=650\n"), None);
        assert_eq!(parse_ipd_env("OTHER=65\n"), None);
        let env = |k: &str| match k {
            "XRT_COMPOSITOR_DUMP_DIR" => Some("/tmp/old".to_string()),
            "PIMAX_LENS_CONF" => Some(String::new()),
            _ => None,
        };
        assert_eq!(stale_experiment_env(&env), vec![("XRT_COMPOSITOR_DUMP_DIR".to_string(), "/tmp/old".to_string())]);
    }

    #[test]
    fn install_freshness() {
        let mut m = FakeMachine::new();
        let (i, b) = (Path::new("/home/u/.local/bin/omarchyvr"), Path::new("/repo/rust/target/release/omarchyvr"));
        assert_eq!(install_state(&m, i, b, None), InstallState::Missing);
        m.file("/repo/rust/target/release/omarchyvr", "v2");
        m.set_meta("/repo/rust/target/release/omarchyvr", FileMeta { len: 2, mtime: 100, ..Default::default() });
        assert_eq!(install_state(&m, i, b, None), InstallState::NotInstalled);
        m.file("/home/u/.local/bin/omarchyvr", "v1");
        assert_eq!(install_state(&m, i, b, Some(50)), InstallState::Differs);
        m.file("/home/u/.local/bin/omarchyvr", "v2");
        assert_eq!(install_state(&m, i, b, Some(50)), InstallState::UpToDate);
        assert_eq!(install_state(&m, i, b, Some(150)), InstallState::BuildStale);
        m.set_meta("/repo/rust/omarchyvr-tools/src/vr/desk.rs", FileMeta { mtime: 170, ..Default::default() });
        m.set_meta("/repo/rust/omarchyvr-tools/src/main.rs", FileMeta { mtime: 120, ..Default::default() });
        m.host.dirs.insert("/repo/rust/omarchyvr-tools/src/vr".into());
        assert_eq!(newest_source(&m, &[PathBuf::from("/repo/rust/omarchyvr-tools")]), Some(170));
    }
}
