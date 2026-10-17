//! Light game mode and the per-game runtime, as the app sees them (docs/GAME-MODE.md).
//!
//! * The Light game mode switch (`light_game` in gamemode.toml, default on) is read into `facts.light_game`; Play passes
//!   `--light` / `--no-light` to `omarchyvr vr start`, so start-argv keeps it for every restart.
//! * A game that ends is looked at once: its xrizer log is kept, and an xrizer abort puts the game on the "Runs with
//!   SteamVR" list (omarchyvr_tools::vr::gameruntime). The Play after that starts with SteamVR for this game.
use crate::app::AppCore;
use omarchyvr_tools::vr::gameruntime as gr;
pub use omarchyvr_tools::vr::steamvr::Games;

/// Steam app ids that are not games the user plays (SteamVR itself, Steamworks redistributables).
const NOT_GAMES: [&str; 2] = ["250820", "228980"];

/// `AppId=NNN` of a `reaper SteamLaunch AppId=NNN -- ...` command line.
pub fn appid_of_cmdline(cmd: &[String]) -> Option<String> {
    let id = cmd.iter().find_map(|w| w.strip_prefix("AppId="))?;
    (!id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) && !NOT_GAMES.contains(&id)).then(|| id.to_string())
}

/// The Steam game that runs now (Steam wraps each game in `reaper SteamLaunch AppId=N`).
pub fn running_game(m: &dyn crate::machine::Machine) -> Option<String> {
    crate::procs::by_comm(m, "reaper").into_iter().find_map(|pid| appid_of_cmdline(&crate::procs::cmdline(m, pid)))
}

/// A game seen running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameRun {
    pub appid: String,
    pub since: u64,
}

/// What the app knows about games and runtimes.
#[derive(Debug, Clone, Default)]
pub struct GameWatch {
    /// The game running now.
    pub current: Option<GameRun>,
    /// The plain-words message after the last crash (until dismissed).
    pub crash_note: Option<String>,
}

/// One round of the watch: the game now (`seen`) against the one before. Returns the run that just ended
/// (a different game starting also ends the old one).
pub fn step(current: &mut Option<GameRun>, seen: Option<String>, now: u64) -> Option<GameRun> {
    match (current.as_ref(), seen) {
        (Some(c), Some(s)) if c.appid == s => None,
        (_, Some(s)) => current.replace(GameRun { appid: s, since: now }),
        (Some(_), None) => current.take(),
        (None, None) => None,
    }
}

impl AppCore {
    /// Light game mode (default on until the facts are read).
    pub fn light_game(&self) -> bool {
        self.facts.at == 0 || self.facts.light_game
    }

    /// The Light game mode switch (saved in gamemode.toml, shared with the CLI; applies to the next Play).
    pub fn set_light_game(&mut self, on: bool) {
        let j = crate::tools::gamemode_job(&self.paths, &self.facts, &["light", if on { "on" } else { "off" }]);
        self.spawn(j);
    }

    /// Every game with a known runtime: the built-in seed, overridden by the saved entries.
    pub fn game_runtime_list(&self) -> Vec<gr::Entry> {
        let st = gr::Store { saved: self.facts.game_runtimes.iter().map(|e| (e.appid.clone(), e.clone())).collect(), next: self.facts.next_game.clone() };
        st.list()
    }

    /// Set a game's runtime (`steamvr`, `xrizer`, `default`); in the background, then the facts are read again.
    pub fn set_game_runtime(&mut self, appid: &str, runtime: &str, name: &str) {
        if !matches!(runtime, "steamvr" | "xrizer" | "default") || appid.is_empty() || !appid.bytes().all(|b| b.is_ascii_digit()) {
            return;
        }
        if self.demo {
            self.message = format!("demo mode: would set game {appid} to {runtime}");
            return;
        }
        let home = self.paths.home.clone();
        let (appid, runtime, name) = (appid.to_string(), runtime.to_string(), name.to_string());
        self.task(None, move || {
            let mut w = vec!["set".to_string(), appid, runtime];
            if !name.is_empty() {
                w.push(name);
            }
            let r = gr::cli(&w, &home);
            Box::new(move |c: &mut AppCore| {
                if let Err(e) = r {
                    c.message = format!("Could not save the game's runtime: {e}");
                }
                c.refresh(false);
            })
        });
    }

    /// The game the next Play is for (`None` = any game: the global "Games run with" choice).
    pub fn set_next_game(&mut self, appid: Option<&str>) {
        if self.demo {
            return;
        }
        let home = self.paths.home.clone();
        let w = vec!["next".to_string(), appid.unwrap_or("none").to_string()];
        self.task(None, move || {
            let r = gr::cli(&w, &home);
            Box::new(move |c: &mut AppCore| {
                if let Err(e) = r {
                    c.message = format!("Could not save the next game: {e}");
                }
                c.refresh(false);
            })
        });
    }

    /// Called with every live update: remember the running game; when one ended, keep its xrizer log and look for an
    /// xrizer abort (background; the coredump lookup may take a moment).
    pub(crate) fn watch_game(&mut self) {
        if self.demo {
            return;
        }
        let now = self.machine.now_unix();
        let Some(ended) = step(&mut self.game_watch.current, self.live.game.clone(), now) else { return };
        let home = self.paths.home.clone();
        self.task(None, move || {
            // give the kernel a moment to write the core dump of a game that just aborted
            std::thread::sleep(std::time::Duration::from_secs(4));
            let v = gr::after_game(&home, &ended.appid, ended.since, gr::now_unix(), &gr::coredump_text(ended.since.saturating_sub(5)));
            Box::new(move |c: &mut AppCore| {
                if let Some(m) = gr::crash_message(&v) {
                    c.game_watch.crash_note = Some(m);
                    c.refresh(false);
                }
            })
        });
    }

    /// The Steam game the next `vr start` is for: the one the user picked ("Next Play is for this game"), else the
    /// game that already runs when Play is pressed (Steam started it first), when it is on the SteamVR list. The list
    /// is only read by `vr start --game`, so without this a listed game launched before/after Play ran under xrizer
    /// (Half-Life: Alyx, 2026-10-17).
    pub fn start_game(&self) -> Option<String> {
        if let Some(n) = &self.facts.next_game {
            return Some(n.clone());
        }
        let g = self.live.game.as_ref()?;
        self.game_runtime_list().into_iter().find(|e| e.appid == *g && e.runtime == Games::SteamVr).map(|e| e.appid)
    }

    /// The offer on Home when a listed game runs under Monado + xrizer: restart Play for this game with SteamVR
    /// (stops the session by pid, waits for Monado to go, `vr start --game APPID`). The game itself is not touched:
    /// it ends by itself (xrizer aborts it) or the user closes it, then starts it again from Steam.
    pub fn switch_to_steamvr_for_game(&mut self, appid: &str) {
        if self.demo || appid.is_empty() || !appid.bytes().all(|b| b.is_ascii_digit()) {
            return;
        }
        match self.switch_job_for(true, Some(appid.to_string())) {
            Ok(j) => {
                self.state.last_vr = true;
                self.save_state();
                self.spawn(j);
            }
            Err(e) => self.message = e,
        }
    }

    /// A game that runs now and is on the SteamVR list while this session runs Monado + xrizer: it will crash.
    pub fn game_needs_steamvr_now(&self) -> Option<gr::Entry> {
        let g = self.game_watch.current.as_ref()?;
        if self.facts.games_steamvr {
            return None;
        }
        self.game_runtime_list().into_iter().find(|e| e.appid == g.appid && e.runtime == Games::SteamVr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn the_game_is_the_appid_of_the_steam_wrapper() {
        assert_eq!(appid_of_cmdline(&cmd("reaper SteamLaunch AppId=546560 -- /x/proton")).as_deref(), Some("546560"));
        assert_eq!(appid_of_cmdline(&cmd("reaper SteamLaunch AppId=250820 -- vrmonitor")), None, "SteamVR is not a game");
        assert_eq!(appid_of_cmdline(&cmd("reaper SteamLaunch AppId=x1 --")), None);
        assert_eq!(appid_of_cmdline(&cmd("bash -c nothing")), None);
    }

    #[test]
    fn a_game_that_goes_away_is_reported_once() {
        let mut cur = None;
        assert_eq!(step(&mut cur, None, 1), None);
        assert_eq!(step(&mut cur, Some("7".into()), 10), None);
        assert_eq!(step(&mut cur, Some("7".into()), 11), None, "still running");
        assert_eq!(step(&mut cur, None, 20), Some(GameRun { appid: "7".into(), since: 10 }));
        assert_eq!(step(&mut cur, None, 21), None);
        // another game replacing the first ends the first
        step(&mut cur, Some("1".into()), 30);
        assert_eq!(step(&mut cur, Some("2".into()), 31), Some(GameRun { appid: "1".into(), since: 30 }));
        assert_eq!(cur.unwrap().appid, "2");
    }

    #[test]
    fn play_is_for_the_running_listed_game_without_the_next_pick() {
        let d = std::env::temp_dir().join(format!("gr-start-game-{}", std::process::id()));
        let mut a = AppCore::demo(false, omarchyvr_config::Store::new(d.join("c"), d.join("k")));
        a.facts.next_game = None;
        a.facts.game_runtimes.clear();
        a.live.game = Some(gr::ALYX.to_string());
        assert_eq!(a.start_game().as_deref(), Some(gr::ALYX), "Alyx is in the built-in SteamVR list");
        a.live.game = Some("730".to_string());
        assert_eq!(a.start_game(), None, "a game not on the list does not change the start");
        a.facts.next_game = Some("123".to_string());
        assert_eq!(a.start_game().as_deref(), Some("123"), "the user's pick wins");
        a.facts.next_game = None;
        a.live.game = None;
        assert_eq!(a.start_game(), None);
        let _ = std::fs::remove_dir_all(&d);
    }
}
