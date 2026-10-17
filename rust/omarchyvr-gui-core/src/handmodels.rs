//! Download of the Mercury hand models from the Hands switch's area (docs/HANDS.md "Models and licence").
//!
//! The models have no licence and are never shipped. The page shows [`CONSENT`] first; only the user's click on
//! "Download" runs `omarchyvr vr hand-models download --yes` (as the user, no root) as a job. The job's lines are the
//! progress (the page shows the last one); its end re-reads the facts, so the Camera hands switch becomes available
//! without a restart. Failures (no git-lfs, no network) are shown in plain words.
use crate::app::AppCore;
use crate::cmd::Cmd;
use crate::job::{JobKind, JobSpec, Step};

/// The job kind.
pub const KIND: &str = "hand models";

/// What the user agrees to (same text as the CLI prints).
pub const CONSENT: &str = omarchyvr_tools::vr::hand_models::CONSENT;

/// The page's state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HandModelsView {
    /// The consent text is on screen, waiting for the click.
    pub asking: bool,
    /// Where the running download's output starts in the log.
    pub from: Option<usize>,
    /// The last download's outcome (plain words): Ok = done, Err = what went wrong.
    pub result: Option<Result<String, String>>,
}

/// The plain-words outcome from a download's output lines (the CLI prints its sentences; git's own noise is skipped).
pub fn outcome(lines: &[String], ok: bool) -> Result<String, String> {
    let known = |l: &&String| {
        let l = l.trim_start_matches("omarchyvr: ");
        l.starts_with("Done:")
            || l.starts_with("The hand models are already")
            || l.contains("git-lfs is not installed")
            || l.contains("git is not installed")
            || l.starts_with("Could not")
            || l.starts_with("The model files")
            || l.starts_with("The download")
            || l.contains("left alone")
            || l.starts_with("Cannot ")
    };
    let last = lines.iter().rev().find(known).map(|l| l.trim_start_matches("omarchyvr: ").to_string());
    match (ok, last) {
        (true, Some(l)) => Ok(l),
        (true, None) => Ok("The hand models are installed.".into()),
        (false, Some(l)) => Err(l),
        (false, None) => Err("The download did not finish (see the output below). Check the internet connection and try again.".into()),
    }
}

impl AppCore {
    /// The "Download hand models" button: show the consent text first.
    pub fn hand_models_ask(&mut self) {
        self.hand_models.asking = true;
        self.hand_models.result = None;
    }

    /// "Cancel" on the consent text.
    pub fn hand_models_cancel(&mut self) {
        self.hand_models.asking = false;
    }

    /// "I agree, download": runs the download as a job. Only reachable through the consent text.
    pub fn hand_models_download(&mut self) {
        if !self.hand_models.asking {
            return;
        }
        let pl = self.paths.launcher(&|x| self.facts.has(x));
        let c = Cmd::with_args(pl.display().to_string(), vec!["vr".into(), "hand-models".into(), "download".into(), "--yes".into()]);
        let spec = JobSpec { kind: JobKind::Other(KIND.into()), label: "Download the hand models".into(), steps: vec![Step::Run(c)] };
        self.hand_models.asking = false;
        if self.spawn(spec) {
            self.hand_models.from = Some(self.log.len());
            self.hand_models.result = None;
        }
    }

    /// True while the download runs.
    pub fn hand_models_running(&self) -> bool {
        self.job.as_ref().is_some_and(|j| j.kind == JobKind::Other(KIND.into()))
    }

    /// The newest step line of the running download (the progress).
    pub fn hand_models_progress(&self) -> Option<String> {
        let from = self.hand_models.from?.min(self.log.len());
        self.log[from..].iter().rev().find(|l| l.starts_with("Step ")).cloned()
    }

    pub(crate) fn hand_models_done(&mut self, ok: bool) {
        let from = self.hand_models.from.take().unwrap_or(0).min(self.log.len());
        self.hand_models.result = Some(outcome(&self.log[from..], ok));
        self.refresh(false); // re-read the facts: the Camera hands switch
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omarchyvr_config::Store;

    fn app() -> AppCore {
        let d = std::env::temp_dir().join(format!("omarchyvr-hmgui-{}", std::process::id()));
        AppCore::demo(true, Store::new(d.join("c"), d.join("k")))
    }

    #[test]
    fn nothing_runs_without_the_consent_click() {
        let mut a = app();
        a.hand_models_download();
        assert!(a.hand_models.from.is_none() && !a.hand_models.asking && a.log.is_empty());
        a.hand_models_ask();
        assert!(a.hand_models.asking);
        a.hand_models_cancel();
        assert!(!a.hand_models.asking);
        assert!(CONSENT.contains("no licence") && CONSENT.contains("personal use") && CONSENT.contains("never shipped"));
    }

    #[test]
    fn consent_then_download_builds_the_user_command() {
        let mut a = app();
        a.hand_models_ask();
        a.hand_models_download(); // demo mode: logged, not run
        assert!(!a.hand_models.asking, "the notice closes on the click");
        assert!(a.log.iter().any(|l| l.contains("hand-models") && l.contains("download") && l.contains("--yes")), "{:?}", a.log);
        // the command is allowed by the safety rules and never root
        let c = Cmd::with_args("omarchyvr", vec!["vr".into(), "hand-models".into(), "download".into(), "--yes".into()]);
        assert!(!c.root);
        assert_eq!(crate::safety::check(&c), Ok(()));
    }

    #[test]
    fn outcomes_are_plain_words() {
        let l = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let done = "Done: the hand models are installed. The Camera hands switch is available now.";
        assert_eq!(outcome(&l(&["Step 1 of 3: connecting", done]), true), Ok(done.into()));
        let e = outcome(&l(&["Cloning into x", "omarchyvr: git-lfs is not installed on this PC, and the hand models need it. Install the package \"git-lfs\", then press Download again. Nothing was changed."]), false).unwrap_err();
        assert!(e.starts_with("git-lfs is not installed"), "{e}");
        assert!(outcome(&l(&["fatal: x"]), false).unwrap_err().contains("did not finish"));
    }

    #[test]
    fn progress_is_the_newest_step_line() {
        let mut a = app();
        a.log = vec!["old".into(), "Step 1 of 3: connecting to Monado's server.".into(), "Receiving objects: 5%".into(), "Step 3 of 3: downloading the model files (this can take a few minutes).".into()];
        a.hand_models.from = Some(1);
        assert!(a.hand_models_progress().unwrap().starts_with("Step 3 of 3"));
    }
}
