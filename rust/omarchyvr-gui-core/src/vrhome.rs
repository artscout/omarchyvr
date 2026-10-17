//! The VR home from the app's side (docs/VR-HOME.md; the engine is `omarchyvr home`, omarchyvr-tools `vr::home`).
//!
//! * **Start in VR home** (default on, `~/.config/omarchyvr/home.toml` `start_in_home`, shared with `omarchyvr home
//!   set`): Play starts the VR home: `omarchyvr vr start <Play's switches> --home --runtime monado`. From the home panel
//!   in the headset the user picks Work, Games with SteamVR, a game, Room setup (Quick / Full) or Exit VR. Off: Play
//!   starts the game session directly, as before. Work and Room setup on Home are unchanged.
//! * **Menu button opens the VR home in games** (default on, `pi_menu`): every Monado game session also runs the home
//!   overlay (a minimal WayVR), hidden until either controller's menu button is pressed.
//! * **Behind the panel**: passthrough (default) or WayVR's sky (`background`).
//! * At every Play and Work start the app writes what its buttons would start (`home-launch.json`), so a transition
//!   from the headset starts Work or a game session with exactly the app's switches.
use crate::app::AppCore;
use crate::cmd::Cmd;
use crate::job::{JobKind, JobSpec, Step};
use crate::launcher;
use crate::preflight::Gate;
use omarchyvr_tools::vr::home as h;
pub use omarchyvr_tools::vr::home::{Background, Game, Settings, Status};
use std::time::Duration;

/// A finished transition stays on Home this long.
pub const STATUS_SHOWN_SECS: u64 = 600;

/// The headset panel's entries in plain words, in order (the app's preview of the VR home): (entry, what it does).
pub fn preview(games: &[h::Game]) -> Vec<(String, String)> {
    let mut v = vec![
        ("Work".to_string(), "the desk: screens, voice, the AI (the home closes first)".to_string()),
        ("Games with SteamVR".into(), "SteamVR with the Pimax driver and its own dashboard".into()),
    ];
    for g in games {
        let how = match g.runtime {
            omarchyvr_tools::vr::steamvr::Games::Monado => "starts right there with Monado (xrizer)",
            omarchyvr_tools::vr::steamvr::Games::SteamVr => "starts SteamVR for it",
        };
        v.push((format!("{} ({})", g.name, g.runtime_words()), how.into()));
    }
    v.push(("Room setup: Quick (30 s) / Full (3 min)".into(), "guided in the headset, without leaving VR".into()));
    v.push(("Recenter, Fix floor, Close".into(), "a few settings".into()));
    v.push(("Exit VR".into(), "stops everything, like Stop here (press it twice)".into()));
    v
}

impl AppCore {
    /// Play lands in the VR home.
    pub fn home_lands(&self) -> bool {
        self.facts.home.start_in_home
    }

    /// The VR home runs now (a game session started with `--home`).
    pub fn home_running(&self) -> bool {
        self.running_side() == Some(true) && self.facts.start_argv.as_ref().is_some_and(|(t, _)| t.lines().next() == Some("vr") && t.lines().any(|l| l.trim() == h::HOME_FLAG))
    }

    /// Change the VR home's switches (saved at once in home.toml, shared with the CLI; the next Play uses them).
    pub fn set_home(&mut self, s: h::Settings) {
        self.facts.home = s;
        if self.demo {
            return;
        }
        if let Err(e) = h::save(&self.paths.home, &s) {
            self.message = format!("Could not save the VR home settings: {e}");
        }
    }

    /// What the headset's transitions start: the app's own Work and Play arguments (never `--force`).
    pub fn home_launch_args(&self) -> h::LaunchArgs {
        h::LaunchArgs {
            work: launcher::desk_start_args(&self.effective_toggles(), false),
            play: launcher::vr_game_args(&self.vr_toggles(), false, self.light_game(), None),
        }
    }

    /// Write `home-launch.json` (at every Play and Work start).
    pub(crate) fn save_home_launch(&self) {
        if self.demo {
            return;
        }
        if let Err(e) = h::save_launch(&self.paths.home, &self.home_launch_args()) {
            log::warn!("VR home: cannot write {}: {e}", h::launch_path(&self.paths.home).display());
        }
    }

    /// `omarchyvr vr start <Play's switches> --home --runtime monado [--force]`.
    pub fn home_start_args(&self, force: bool) -> Vec<String> {
        let mut v = h::home_start(&self.home_launch_args(), None);
        if force {
            v.push("--force".into());
        }
        v
    }

    /// The start job of the VR home: the game session's gate and steps, with the home's arguments.
    pub fn home_start_job(&self, force: bool) -> Result<JobSpec, String> {
        let pf = self.vr_preflight();
        match (&pf.gate, force) {
            (Gate::Blocked(r), _) => return Err(format!("not starting: {}", r.join("; "))),
            (Gate::NeedsConsent(r), false) => return Err(format!("needs your OK first: {}", r.join("; "))),
            _ => {}
        }
        let force = force && matches!(pf.gate, Gate::NeedsConsent(_));
        let t = self.vr_toggles();
        let mut steps = vec![
            Step::WaitNoMonado(Duration::from_secs(45)),
            Step::Guard("camera still free", crate::guards::devices_free(t.needs_cameras(), false)),
        ];
        if force {
            steps.push(Step::Guard("still only WayVR Xid 31", crate::guards::force_still_justified()));
        }
        steps.push(Step::Run(self.own_scope(self.home_start_cmd(force), "vr")));
        Ok(JobSpec { kind: JobKind::DeskStart, label: "Start the VR home".into(), steps })
    }

    /// The launcher command of the home's start (the IPD and the stale variables as for Play).
    pub(crate) fn home_start_cmd(&self, force: bool) -> Cmd {
        let mut c = Cmd::with_args(self.launcher_bin().display().to_string(), self.home_start_args(force));
        for (k, v) in self.launcher_env() {
            c = c.env(&k, v);
        }
        c.env_remove = self.facts.stale_env.iter().map(|(k, _)| k.clone()).collect();
        c
    }

    /// The last transition from the headset panel, while it is recent: (ok, words).
    pub fn home_transition_line(&self, now_unix: u64) -> Option<(bool, String)> {
        let s = self.facts.home_status.as_ref()?;
        if now_unix.saturating_sub(s.when) > STATUS_SHOWN_SECS && s.state != "running" {
            return None;
        }
        let ok = s.state != "failed";
        let words = match s.state.as_str() {
            "running" => format!("In the headset: {}", s.text),
            "failed" => format!("In the headset (VR home > {}): {}", s.target, s.text),
            _ => format!("In the headset: {}", s.text),
        };
        Some((ok, words))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn core() -> AppCore {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!("pimax-vrhome-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&d);
        AppCore::demo(false, omarchyvr_config::Store::new(d.join("config"), d.join("cache")))
    }

    #[test]
    fn play_lands_in_the_home_by_default_with_plays_switches_and_monado() {
        let a = core();
        assert!(a.home_lands(), "Start in VR home is on by default");
        let args = a.home_start_args(false);
        assert_eq!(&args[..2], ["vr", "start"]);
        assert!(args.windows(2).any(|w| w == ["--runtime", "monado"]), "the home is always Monado: {args:?}");
        assert!(args.contains(&"--home".to_string()));
        assert!(!args.iter().any(|x| x == "--perf" || x == "--light" || x == "--no-light" || x == "--force" || x == "--game"), "{args:?}");
        assert!(a.home_start_args(true).ends_with(&["--force".to_string()]));
        // the headset's transitions start what the app's buttons would
        let la = a.home_launch_args();
        assert_eq!(&la.work[..2], ["desk", "start"]);
        assert_eq!(la.work, launcher::desk_start_args(&a.effective_toggles(), false));
        assert_eq!(&la.play[..2], ["vr", "start"]);
    }

    #[test]
    fn the_home_job_waits_for_monado_and_runs_the_launcher_with_the_home() {
        let a = core();
        let j = a.home_start_job(true).expect("ready in the demo (consent over its WayVR Xid 31 faults)");
        assert_eq!(j.label, "Start the VR home");
        assert!(matches!(j.steps[0], Step::WaitNoMonado(_)));
        let Step::Run(c) = j.steps.last().unwrap() else { panic!("{:?}", j.steps) };
        assert!(c.display().contains("vr start") && c.display().contains("--home --runtime monado"), "{}", c.display());
    }

    #[test]
    fn switches_are_saved_and_the_off_switch_keeps_the_old_play() {
        let mut a = core();
        a.set_home(h::Settings { start_in_home: false, ..h::Settings::default() });
        assert!(!a.home_lands());
    }

    #[test]
    fn the_preview_lists_every_entry_and_the_games_with_their_runtime() {
        let a = core();
        assert_eq!(a.facts.home_games.len(), 3, "{:?}", a.facts.home_games);
        let p = preview(&a.facts.home_games);
        let names: Vec<&str> = p.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names[0], "Work");
        assert_eq!(names[1], "Games with SteamVR");
        assert!(names.contains(&"Half-Life: Alyx (SteamVR)"), "{names:?}");
        assert!(names.contains(&"Dungeons of Eternity (xrizer)"), "{names:?}");
        assert!(names.last().unwrap().starts_with("Exit VR"));
    }

    #[test]
    fn a_recent_transition_shows_on_home() {
        let mut a = core();
        assert_eq!(a.home_transition_line(1000), None);
        a.facts.home_status = Some(h::Status { state: "failed".into(), target: "steamvr".into(), text: "SteamVR did not start: Steam must restart".into(), when: 1000 });
        let (ok, l) = a.home_transition_line(1100).unwrap();
        assert!(!ok && l.contains("(VR home > steamvr): SteamVR did not start"), "{l}");
        assert_eq!(a.home_transition_line(1000 + STATUS_SHOWN_SECS + 1), None, "an old one is gone");
    }
}
