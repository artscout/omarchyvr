//! "Check VR games": runs `pimax vr game-check` (the library function, in the background) and keeps the result for
//! the Ready check page. See docs/GAMES-CHECK.md.
use crate::app::AppCore;
pub use pimax_tools::vr::game_check::{Check, Status};
use pimax_tools::vr::game_check;

/// The last result of the check.
#[derive(Debug, Clone, Default)]
pub struct GameCheckView {
    /// The report, `None` before the first run.
    pub checks: Option<Vec<Check>>,
}

impl GameCheckView {
    /// (pass, warn, fail) counts.
    pub fn counts(&self) -> (usize, usize, usize) {
        let c = self.checks.as_deref().unwrap_or(&[]);
        let n = |s: Status| c.iter().filter(|x| x.status == s).count();
        (n(Status::Pass), n(Status::Warn), n(Status::Fail))
    }
}

/// Task key of the check.
pub const KEY: &str = "gamecheck";

impl AppCore {
    /// Run the VR games check in the background (the container test takes a few seconds).
    pub fn game_check(&mut self) {
        if self.demo {
            self.gamecheck.checks = Some(vec![game_check::demo_check()]);
            return;
        }
        let home = self.paths.home.clone();
        let line = pimax_tools::vr::config::steam_options_linked();
        self.task(Some(KEY), move || {
            let opts = game_check::Options { steam_root: home.join(".local/share/Steam"), home, appid: None, no_container: false, options_line: line, steamvr: game_check::steamvr_chosen() };
            let r = game_check::run_checks(&opts);
            Box::new(move |c: &mut AppCore| c.gamecheck.checks = Some(r))
        });
    }

    /// Is the check running?
    pub fn game_check_busy(&self) -> bool {
        self.task_busy(KEY)
    }
}
