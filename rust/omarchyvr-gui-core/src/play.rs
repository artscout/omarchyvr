//! One click to VR: the Play and Work buttons, the one-click `cap_sys_nice` fix, "Start VR when I log in" and the
//! readiness line of the Home page.
//!
//! * Three layers: the base Pimax services (Monado + driver, tracking, controllers, hands, recording, keepalive,
//!   omarchyvrd; their own switches), **Play** (the game side, like SteamVR: `omarchyvr vr start` with the base switches, and
//!   auto mode told `set-mode game`; never WayVR or the desk AI; the VR home is still to be decided) and **Work** (the
//!   WayVR desk: `omarchyvr desk start` with the base + desk switches: screens, voice, AI, captions, presets). Both go
//!   through the normal start jobs (same preflight gate, same ordering); while our session runs, the button of the
//!   running side stops it.
//! * Every relink of monado-service drops `cap_sys_nice` (LESSONS row 7). When the build the next start uses lacks it,
//!   Home shows it with a Fix button, and Play/Work restore it first: `/usr/bin/pkexec /usr/bin/setcap cap_sys_nice+ep
//!   <service>`, one graphical password prompt (needs a polkit agent), nothing installed or kept. The path is resolved
//!   and checked first ([`crate::caps::check_service_path`]: a regular file named monado-service under
//!   `~/src/monado*/`). A cancelled prompt does not stop Play: the session starts without the priority.
//! * Start at login (off by default, in gui-state.toml): when the app opens, the session starts only if the headset
//!   is there, the cap is present (never a password prompt at login) and the preflight gate is clear (no reboot
//!   needed, no GPU errors, no consent needed). Otherwise Home says why.
use crate::app::{AppCore, DeskPhase, Pending};
use crate::caps;
use crate::cmd::Cmd;
use crate::job::{JobKind, JobSpec, Step};
use crate::launcher::DeskToggles;
use crate::preflight::Gate;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long start-at-login waits for the headset to show up after the app opened.
pub const AUTOSTART_HEADSET_WAIT: Duration = Duration::from_secs(40);

/// Play, start at login and the hand test (the app's view of them).
#[derive(Debug, Clone, Default)]
pub struct PlayView {
    /// Start at login is still to be decided (set when the app opens with the setting on).
    pub autostart_pending: bool,
    /// When the app opened (start at login waits for the headset a little).
    pub autostart_since: Option<Instant>,
    /// What start at login did or why it did not start.
    pub autostart_note: Option<(bool, String)>,
    /// The last start's message (this run of the app).
    pub last_msg: Option<String>,
    /// The guided hand test.
    pub hand_test: crate::handtest::HandTestView,
}

/// What Play does about the running Steam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteamDecision {
    /// Go on: Steam is fine, absent, or SteamVR is not in use.
    Go,
    /// Ask the user to restart Steam (the Play dialog).
    AskRestart,
}

/// Only a game session with SteamVR cares: Monado games and the desk never meet SteamVR's variables.
pub fn steam_decision(vr: bool, games_steamvr: bool, tainted: Option<&crate::facts::TaintedSteam>) -> SteamDecision {
    if vr && games_steamvr && tainted.is_some() {
        SteamDecision::AskRestart
    } else {
        SteamDecision::Go
    }
}

/// The cap of the monado-service the next start uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapState {
    /// Has `cap_sys_nice`.
    Ok(PathBuf),
    /// Lacks it (the Fix button).
    Missing(PathBuf),
    /// Not known yet, or not built.
    Unknown(Option<PathBuf>),
}

/// One item of the readiness line: ok (None = unknown) and the text.
pub type Ready = (Option<bool>, String);

/// The readiness line of Home.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Readiness {
    /// The headset on USB.
    pub headset: Ready,
    /// The Monado cap.
    pub cap: CapState,
    /// `active_runtime.json` points at the build the next start uses.
    pub runtime: Ready,
    /// The last start.
    pub last: Ready,
}

impl AppCore {
    /// The switches of the last side started (Play: the base switches; Work: base + desk switches).
    pub fn play_toggles(&self) -> DeskToggles {
        if self.state.last_vr {
            self.vr_toggles()
        } else {
            self.toggles
        }
    }

    /// The last side started was Play (a game session), not Work (start at login repeats it).
    pub fn play_is_vr(&self) -> bool {
        self.state.last_vr
    }

    /// The monado-service the next Play start uses (from the launcher's own plan, so `--hands` means the hands
    /// build) and its `getcap` verdict. None while the plan is worked out.
    pub fn active_service(&self) -> Option<(PathBuf, Option<bool>)> {
        let plan = self.plan_for_toggles(&self.play_toggles())?.ok()?;
        Some(match self.facts.build(&plan.monado_build) {
            Some(b) => (b.service.clone(), if b.built() { b.sys_nice } else { None }),
            None => (plan.service, None),
        })
    }

    /// The cap state of the next start's service.
    pub fn cap_state(&self) -> CapState {
        match self.active_service() {
            Some((s, Some(true))) => CapState::Ok(s),
            Some((s, Some(false))) => CapState::Missing(s),
            Some((s, None)) => CapState::Unknown(Some(s)),
            None => CapState::Unknown(None),
        }
    }

    /// The service of the next start when it lacks `cap_sys_nice`.
    pub fn cap_missing(&self) -> Option<PathBuf> {
        match self.cap_state() {
            CapState::Missing(s) => Some(s),
            _ => None,
        }
    }

    /// The checked pkexec command for `service` (needs a polkit agent: the password prompt is graphical).
    fn setcap_cmd(&self, service: &Path) -> Result<Cmd, String> {
        if !self.facts.polkit_agent {
            return Err("no polkit authentication agent runs, so pkexec cannot show a password window".into());
        }
        let resolved = caps::check_service_path(service, &self.paths.home)?;
        Ok(caps::pkexec_setcap(&resolved))
    }

    /// The Fix button: restore `cap_sys_nice` on the next start's service (one password prompt), then check again.
    /// Without a polkit agent: the usual root-fix dialog (copy the line, or pkexec in a terminal).
    pub fn fix_cap(&mut self) {
        let Some(svc) = self.cap_missing() else {
            self.message = "monado-service already has cap_sys_nice".into();
            return;
        };
        if self.demo {
            self.message = format!("demo mode: would run {}", caps::pkexec_setcap(&svc).display());
            return;
        }
        if !self.facts.polkit_agent {
            self.pending = Some(crate::app::Pending::Root {
                title: format!("Restore cap_sys_nice on {}", svc.display()),
                fix: crate::checklist::setcap_fix(&svc),
                catalogue: None,
            });
            return;
        }
        match self.setcap_cmd(&svc) {
            Ok(c) => {
                self.spawn(JobSpec {
                    kind: JobKind::Fix("setcap-active".into()),
                    label: "Restore cap_sys_nice (password prompt)".into(),
                    steps: vec![Step::Run(c)],
                });
            }
            Err(e) => self.message = format!("Not running setcap: {e}"),
        }
    }

    /// The start job Play runs now (without the cap step).
    fn play_start_job(&self) -> Result<JobSpec, String> {
        if self.state.last_vr {
            self.vr_start_job(self.consent_force)
        } else {
            self.desk_start_job(self.consent_force)
        }
    }

    /// Play: the game side, like SteamVR: the base services (`omarchyvr vr start`: Monado, tracking, controllers, hands)
    /// for Steam and OpenXR games, never WayVR or the desk AI; the same button stops a running session. When the
    /// service lacks `cap_sys_nice`, it is restored first (one password prompt; cancelling it still starts).
    pub fn play(&mut self) {
        self.launch(true);
    }

    /// Work: the WayVR desk (screens, voice, AI, captions, presets) on top of the base, `omarchyvr desk start` with the
    /// desk switches; the same button stops a running session.
    pub fn work(&mut self) {
        self.launch(false);
    }

    /// Which side runs now: Some(true) = a game session (Play), Some(false) = the desk (Work), None = nothing of ours.
    pub fn running_side(&self) -> Option<bool> {
        match self.phase() {
            DeskPhase::Running => Some(!self.live.desk_side()),
            _ => None,
        }
    }

    /// Play with SteamVR (Games run with: SteamVR) needs a Steam that did not come from inside SteamVR.
    pub fn steam_decision(&self, vr: bool) -> SteamDecision {
        steam_decision(vr, self.facts.games_steamvr, self.live.steam_tainted.as_ref())
    }

    /// The dialog text for a tainted Steam: (title, body, busy warning).
    pub fn restart_steam_text(steam: &crate::facts::TaintedSteam) -> (String, String, Option<String>) {
        (
            "Steam must restart for VR. Restart Steam now?".into(),
            "Steam was started from inside SteamVR earlier and still carries its settings, so games could freeze. The app closes it the normal way (like Steam menu > Exit), waits for it to quit, then starts VR; Play starts Steam again by itself.".into(),
            steam.busy.as_ref().map(|b| format!("Careful: {b}. Closing Steam ends it.")),
        )
    }

    /// The dialog's Restart Steam button: close the tainted Steam politely, then start (one job; a Steam that does
    /// not quit stops the job with plain words, nothing is forced).
    pub fn confirm_steam_restart(&mut self) {
        let Some(Pending::RestartSteam { vr, .. }) = self.pending.take() else { return };
        self.launch_with(vr, true);
    }

    fn launch(&mut self, vr: bool) {
        match self.phase() {
            DeskPhase::Running => return self.stop(),
            DeskPhase::Foreign => {
                self.message = "Another monado-service is running: stop it first (Stop other Monado).".into();
                return;
            }
            DeskPhase::Idle => {}
            _ => {
                self.message = "Another action is still running; wait for it to finish.".into();
                return;
            }
        }
        if let SteamDecision::AskRestart = self.steam_decision(vr) {
            if let Some(steam) = self.live.steam_tainted.clone() {
                // nothing is closed without the user's yes (the dialog in the app)
                self.pending = Some(Pending::RestartSteam { steam, vr });
                return;
            }
        }
        self.launch_with(vr, false);
    }

    /// Start `vr` (Play) or the desk (Work); `close_steam` first closes the tainted Steam (confirmed by the user).
    fn launch_with(&mut self, vr: bool, close_steam: bool) {
        self.state.last_vr = vr;
        let mut job = match self.play_start_job() {
            Ok(j) => j,
            Err(e) => {
                self.message = e;
                return;
            }
        };
        // SteamVR (Games run with: SteamVR) does not run monado-service: no cap to restore
        if let Some(svc) = self.cap_missing().filter(|_| !(vr && self.facts.games_steamvr)) {
            match self.setcap_cmd(&svc) {
                Ok(c) => {
                    job.steps.insert(0, Step::Note(format!("{} lacks cap_sys_nice: restoring it (password prompt)", svc.display())));
                    job.steps.insert(1, Step::RunAllowFail(c));
                    job.label = format!("Restore cap_sys_nice, then {}", job.label);
                }
                Err(e) => job.steps.insert(0, Step::Note(format!("cap_sys_nice not restored ({e}); starting anyway"))),
            }
        }
        if close_steam {
            let restart = Cmd::with_args(self.launcher_bin().display().to_string(), vec!["vr".into(), "steam-restart".into()]);
            job.steps.insert(0, Step::Note("Closing Steam (it was started from inside SteamVR), then starting VR".into()));
            job.steps.insert(1, Step::Run(restart));
            job.label = format!("Restart Steam, then {}", job.label);
        }
        self.save_state();
        if self.spawn(job) && vr {
            self.tell_auto_game();
        }
    }

    /// The game path of auto mode: when its daemon runs, tell it the session is a game (`set-mode game --runtime
    /// monado|steamvr`), so it does not treat it as the desk. The VR home/launcher of Play is still to be decided.
    fn tell_auto_game(&mut self) {
        if matches!(self.auto.status, Some(Ok(_))) {
            let runtime = if self.facts.games_steamvr { "steamvr" } else { "monado" };
            self.auto_cmd(crate::auto::AutoCmd::SetMode { mode: Some("game".into()), runtime: Some(runtime.into()), preset: None });
        }
    }

    /// One saved setting for the app and the CLI: a start recorded in `start-argv.txt` that is newer than the last
    /// one taken over (from any path: the app, `omarchyvr desk start`, omarchyvrd, a restart) becomes the app's switches and
    /// the kind Play starts. The hand test's own start is not taken over (it is a test, not the user's setup).
    pub(crate) fn adopt_start_argv(&mut self) {
        let Some((text, at)) = self.facts.start_argv.clone() else { return };
        if at <= self.state.argv_at {
            return;
        }
        self.state.argv_at = at;
        let test = self.job.as_ref().is_some_and(|j| j.kind == JobKind::Other(crate::handtest::KIND.into()))
            || self.play.hand_test.log_from.is_some();
        if let (false, Some((t, vr))) = (test, crate::launcher::toggles_from_argv(&text, self.toggles)) {
            if t != self.toggles || vr != self.state.last_vr {
                self.toggles = t;
                self.state.last_vr = vr;
                self.state.toggles = t;
                log::info!("took over the switches of the last start ({})", text.lines().collect::<Vec<_>>().join(" "));
            }
        }
        self.write_state();
    }

    /// Developer mode on or off (the advanced pages and rows).
    pub fn set_dev_mode(&mut self, on: bool) {
        self.state.dev_mode = on;
        self.write_state();
    }

    /// Start at login on or off.
    pub fn set_autostart(&mut self, on: bool) {
        self.state.autostart = on;
        self.write_state();
    }

    /// Called once when the real app opens: decide about start at login after the first check.
    pub fn arm_autostart(&mut self) {
        if self.state.autostart && !self.demo {
            self.play.autostart_pending = true;
            self.play.autostart_since = Some(Instant::now());
        }
    }

    /// Why start at login would not start now (empty = it would).
    pub fn autostart_blockers(&self) -> Vec<String> {
        let mut v = Vec::new();
        if !self.facts.headset {
            v.push("the headset is not connected".to_string());
        }
        if self.live.anything_running() || self.busy() {
            v.push("a session or an action is already running".into());
        }
        if self.steam_decision(self.state.last_vr) == SteamDecision::AskRestart {
            v.push("Steam must be restarted for VR first (press Play in the app and answer the question)".into());
        }
        match self.cap_state() {
            CapState::Ok(_) => {}
            CapState::Missing(s) => v.push(format!("{} lacks cap_sys_nice (press Fix, or Play or Work once; no password prompt at login)", s.display())),
            CapState::Unknown(_) => v.push("the cap of monado-service is not known".into()),
        }
        let pf = if self.state.last_vr { self.vr_preflight() } else { self.preflight() };
        match pf.gate {
            Gate::Ready => {}
            Gate::NeedsConsent(r) => v.push(format!("needs your OK: {}", r.join("; "))),
            Gate::Blocked(r) => v.extend(r),
        }
        v
    }

    /// Start at login, once the first check and the plan are there (from [`AppCore::poll`]); true when it decided.
    pub(crate) fn poll_autostart(&mut self) -> bool {
        if !self.play.autostart_pending {
            return false;
        }
        if self.demo {
            self.play.autostart_pending = false;
            return false;
        }
        if self.facts.at == 0 || self.plan_for_toggles(&self.play_toggles()).is_none() {
            return false;
        }
        let blockers = self.autostart_blockers();
        let waited = self.play.autostart_since.is_none_or(|t| t.elapsed() >= AUTOSTART_HEADSET_WAIT);
        if !waited && blockers.iter().any(|b| b.contains("headset is not connected")) {
            // the headset may still be coming up after the login: the next checks see it
            return false;
        }
        self.play.autostart_pending = false;
        if !blockers.is_empty() {
            self.play.autostart_note = Some((false, format!("Not started at login: {}.", blockers.join("; "))));
            return true;
        }
        match self.play_start_job() {
            Ok(j) => {
                let label = j.label.clone();
                let vr = self.state.last_vr;
                if self.spawn(j) {
                    if vr {
                        self.tell_auto_game();
                    }
                    self.play.autostart_note = Some((true, format!("Started at login: {label}")));
                }
            }
            Err(e) => self.play.autostart_note = Some((false, format!("Not started at login: {e}"))),
        }
        true
    }

    /// The result of a start job (kept in gui-state.toml for the readiness line).
    pub(crate) fn note_start_result(&mut self, label: &str, r: &Result<(), String>) {
        self.state.last_ok = Some(r.is_ok());
        self.state.last_at = self.machine.now_unix();
        self.play.last_msg = Some(match r {
            Ok(()) => format!("{label}: started"),
            Err(e) => format!("{label}: {}", e.lines().next().unwrap_or("failed")),
        });
        self.write_state();
    }

    /// The readiness line: headset, Monado cap, OpenXR runtime, last start.
    pub fn readiness(&self) -> Readiness {
        let headset = if self.facts.at == 0 {
            (None, "Headset: checking".to_string())
        } else if self.facts.headset {
            (Some(true), "Headset connected".to_string())
        } else {
            (Some(false), "Headset not connected".to_string())
        };
        let want = self
            .plan_for_toggles(&self.play_toggles())
            .and_then(|p| p.ok())
            .map(|p| p.monado_build.join("openxr_monado-dev.json"));
        let steam_link = |t: &std::path::Path| {
            let l = t.to_string_lossy().to_lowercase();
            l.contains("steamxr") || l.contains("steamvr")
        };
        let runtime = match (&self.facts.openxr_runtime, want) {
            // Games run with: SteamVR: the link stays at SteamVR, also while Work runs on Monado (its apps get
            // XR_RUNTIME_JSON directly)
            (Some(t), _) if self.facts.games_steamvr && steam_link(t) => (Some(true), "OpenXR runtime: SteamVR (games run with SteamVR)".to_string()),
            (_, Some(_)) if self.facts.games_steamvr => (Some(false), "OpenXR runtime: not SteamVR yet (Play sets it)".to_string()),
            (_, None) => (None, "OpenXR runtime: checking".to_string()),
            (Some(t), Some(w)) if *t == w => (Some(true), "OpenXR runtime: this Monado".to_string()),
            (Some(_), Some(_)) => (Some(false), "OpenXR runtime: another build (the start sets it)".to_string()),
            (None, Some(_)) => (Some(false), "OpenXR runtime not set (the start sets it)".to_string()),
        };
        let ago = |at: u64| {
            let now = self.machine.now_unix();
            if at == 0 || now < at {
                String::new()
            } else {
                format!(", {} ago", crate::procs::fmt_age((now - at) as f64))
            }
        };
        let last = match self.state.last_ok {
            None => (None, "No start from the app yet".to_string()),
            Some(true) => (Some(true), format!("Last start OK{}", ago(self.state.last_at))),
            Some(false) => (Some(false), format!("Last start failed{}", ago(self.state.last_at))),
        };
        Readiness { headset, cap: self.cap_state(), runtime, last }
    }
}

#[cfg(test)]
mod tests {
    use crate::app::AppCore;
    use crate::builds::BuildInfo;
    use omarchyvr_config::Store;

    fn store(tag: &str) -> Store {
        let d = std::env::temp_dir().join(format!("pimax-play-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        Store::new(d.join("config"), d.join("cache"))
    }

    fn drop_cap(a: &mut AppCore) {
        let svc = a.active_service().unwrap().0;
        for b in a.facts.builds.iter_mut().filter(|b: &&mut BuildInfo| b.service == svc) {
            b.sys_nice = Some(false);
        }
    }

    /// A running demo session where only the named helpers are alive.
    fn session_with(tag: &str, started_as: Option<&str>, alive: &[&str]) -> AppCore {
        let mut a = AppCore::demo(true, store(tag));
        for p in a.live.session.iter_mut() {
            p.alive = p.name == "monado" || alive.contains(&p.name);
        }
        a.live.started_as = started_as.map(String::from);
        a
    }

    #[test]
    fn vr_session_with_broker_and_ctrltrack_is_the_play_side() {
        let a = session_with("vrside", Some("vr"), &["broker", "ctrltrack"]);
        assert!(!a.live.desk_clients_alive());
        assert_eq!(a.running_side(), Some(true));
        let crate::job::Step::RunAllowFail(c) = &a.stop_job().steps[0] else { panic!() };
        assert_eq!(c.args, crate::launcher::vr_stop_args(false));
        // no start record (started outside the app): broker and ctrltrack still do not make it a desk
        let a = session_with("vrside2", None, &["broker", "ctrltrack", "perf"]);
        assert_eq!(a.running_side(), Some(true));
    }

    #[test]
    fn desk_session_is_the_work_side() {
        for rec in [Some("desk"), None] {
            let a = session_with("deskside", rec, &["broker", "passthrough", "wayvr"]);
            assert_eq!(a.running_side(), Some(false));
            let crate::job::Step::RunAllowFail(c) = &a.stop_job().steps[0] else { panic!() };
            assert_eq!(c.args, crate::launcher::desk_stop_args());
        }
        // the start record wins over the processes
        let a = session_with("deskrec", Some("desk"), &["broker"]);
        assert_eq!(a.running_side(), Some(false));
    }

    #[test]
    fn readiness_and_the_cap_of_the_next_start() {
        let mut a = AppCore::demo(false, store("ready"));
        let r = a.readiness();
        assert_eq!(r.headset.0, Some(true));
        assert!(matches!(r.cap, super::CapState::Ok(ref s) if s.ends_with("monado-eye/build/src/xrt/targets/service/monado-service")), "{r:?}");
        assert_eq!(r.last.0, None);
        assert_eq!(a.cap_missing(), None);
        drop_cap(&mut a);
        assert!(a.cap_missing().is_some());
        // demo: nothing runs, the command is only shown
        a.fix_cap();
        assert!(a.message.contains("/usr/bin/pkexec /usr/bin/setcap cap_sys_nice+ep /home/u/src/monado-eye/build"), "{}", a.message);
        assert!(a.job.is_none());
    }

    #[test]
    fn readiness_with_steamvr_games_wants_the_steamvr_runtime() {
        let mut a = AppCore::demo(false, store("ready-steam"));
        a.facts.games_steamvr = true;
        a.facts.openxr_runtime = Some(std::path::PathBuf::from("/home/u/.local/share/Steam/steamapps/common/SteamVR/steamxr_linux64.json"));
        let r = a.readiness();
        assert_eq!(r.runtime.0, Some(true), "{r:?}");
        assert!(r.runtime.1.contains("SteamVR"), "{r:?}");
        a.facts.openxr_runtime = Some(std::path::PathBuf::from("/home/u/src/monado-eye/build/openxr_monado-dev.json"));
        let r = a.readiness();
        assert_eq!(r.runtime.0, Some(false), "{r:?}");
    }

    #[test]
    fn play_starts_the_last_kind_of_session_and_stops_a_running_one() {
        let mut a = AppCore::demo(false, store("play"));
        a.consent_force = true; // the demo kernel log has WayVR Xid 31 faults
        a.work();
        assert!(a.message.starts_with("demo mode: would run Start desk mode"), "{}", a.message);
        assert!(!a.play_is_vr());
        a.play();
        assert!(a.message.starts_with("demo mode: would run Start VR"), "{}", a.message);
        assert!(a.play_is_vr(), "start at login repeats the last side");
        let mut r = AppCore::demo(true, store("play-run"));
        assert_eq!(r.running_side(), Some(false), "the demo session is the desk");
        r.work();
        assert!(r.message.starts_with("demo mode: would run Stop"), "{}", r.message);
    }

    fn tainted() -> crate::facts::TaintedSteam {
        crate::facts::TaintedSteam { pid: 77, vars: vec!["SteamAppId".into()], busy: Some("a game seems to be running".into()) }
    }

    #[test]
    fn the_steam_decision_only_asks_for_a_steamvr_game_session() {
        use super::{steam_decision, SteamDecision::*};
        let t = tainted();
        assert_eq!(steam_decision(true, true, Some(&t)), AskRestart);
        assert_eq!(steam_decision(true, true, None), Go, "clean or no Steam");
        assert_eq!(steam_decision(true, false, Some(&t)), Go, "Monado games never meet SteamVR's variables");
        assert_eq!(steam_decision(false, true, Some(&t)), Go, "Work (the desk) does not use SteamVR");
    }

    #[test]
    fn play_asks_before_closing_a_tainted_steam_and_only_a_yes_closes_it() {
        let mut a = AppCore::demo(false, store("steam-ask"));
        a.consent_force = true;
        a.facts.games_steamvr = true;
        a.live.steam_tainted = Some(tainted());
        a.play();
        assert!(matches!(a.pending, Some(crate::app::Pending::RestartSteam { vr: true, .. })), "the dialog, nothing started: {:?}", a.pending);
        assert!(!a.log.iter().any(|l| l.contains("steam-restart")), "nothing is closed without the yes: {:?}", a.log);
        // Cancel: no job, no restart
        a.cancel_pending();
        assert!(a.pending.is_none() && !a.log.iter().any(|l| l.contains("steam-restart")));
        // Yes: one job, the restart first, then the start
        a.play();
        a.confirm_steam_restart();
        assert!(a.message.starts_with("demo mode: would run Restart Steam, then Start VR"), "{}", a.message);
        let restart = a.log.iter().position(|l| l.contains("vr steam-restart")).expect("restart step");
        let start = a.log.iter().position(|l| l.contains("vr start") || l.contains("Start VR") && l.contains("Run(")).unwrap_or(usize::MAX);
        assert!(restart < start, "{:?}", a.log);
        // a clean Steam: no dialog
        let mut b = AppCore::demo(false, store("steam-clean"));
        b.consent_force = true;
        b.facts.games_steamvr = true;
        b.play();
        assert!(b.pending.is_none() && b.message.starts_with("demo mode: would run Start VR"), "{}", b.message);
        let (t, body, busy) = AppCore::restart_steam_text(&tainted());
        assert_eq!(t, "Steam must restart for VR. Restart Steam now?");
        assert!(body.contains("closes it the normal way") && busy.unwrap().contains("a game seems to be running"));
    }

    #[test]
    fn start_at_login_is_off_by_default_and_never_starts_without_the_cap() {
        let mut a = AppCore::demo(false, store("auto"));
        assert!(!a.state.autostart);
        // demo never arms it
        a.state.autostart = true;
        a.arm_autostart();
        assert!(!a.play.autostart_pending);
        // the rules themselves
        drop_cap(&mut a);
        let b = a.autostart_blockers();
        assert!(b.iter().any(|x| x.contains("cap_sys_nice")), "{b:?}");
        a.facts.headset = false;
        assert!(a.autostart_blockers().iter().any(|x| x.contains("headset is not connected")));
    }
}
