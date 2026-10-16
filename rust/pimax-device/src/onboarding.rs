//! Onboarding steps and the progress file (`~/.local/state/pimax/onboarding.state`, plain `key = value` lines).
//!
//! Every step is resumable and skippable: the file records `done`, `skipped` or `failed` per step, and the next step
//! is the first one of the module's list that is neither done nor skipped. Nothing here touches a device.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// The steps, in the default order. A module lists the subset it supports (`[onboarding] steps`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Step {
    Detect,
    Checks,
    Fixes,
    Udev,
    Ipd,
    Pair,
    EyeCal,
    Room,
    Done,
}

pub const ALL: [Step; 9] = [Step::Detect, Step::Checks, Step::Fixes, Step::Udev, Step::Ipd, Step::Pair, Step::EyeCal, Step::Room, Step::Done];

impl Step {
    pub fn id(self) -> &'static str {
        match self {
            Step::Detect => "detect",
            Step::Checks => "checks",
            Step::Fixes => "fixes",
            Step::Udev => "udev",
            Step::Ipd => "ipd",
            Step::Pair => "pair",
            Step::EyeCal => "eye_cal",
            Step::Room => "room",
            Step::Done => "done",
        }
    }

    pub fn from_id(s: &str) -> Option<Step> {
        ALL.into_iter().find(|x| x.id() == s)
    }

    /// For people.
    pub fn title(self) -> &'static str {
        match self {
            Step::Detect => "Find the headset",
            Step::Checks => "Check this computer",
            Step::Fixes => "Fix what the checks found",
            Step::Udev => "Headset access (udev rule)",
            Step::Ipd => "Set your IPD",
            Step::Pair => "Pair the controllers",
            Step::EyeCal => "Calibrate eye tracking",
            Step::Room => "Map your room",
            Step::Done => "Done",
        }
    }
}

/// State of one step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Pending,
    Done,
    Skipped,
    Failed,
}

impl State {
    fn id(self) -> &'static str {
        match self {
            State::Pending => "pending",
            State::Done => "done",
            State::Skipped => "skipped",
            State::Failed => "failed",
        }
    }
    fn parse(s: &str) -> Option<State> {
        [State::Pending, State::Done, State::Skipped, State::Failed].into_iter().find(|x| x.id() == s)
    }
}

/// The progress file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Progress {
    /// Module id chosen at the detect step.
    pub module: Option<String>,
    pub steps: Vec<(Step, State)>,
}

impl Progress {
    /// Default location: `$XDG_STATE_HOME/pimax/onboarding.state` or `~/.local/state/pimax/onboarding.state`.
    pub fn path(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
        let base = env("XDG_STATE_HOME").map(PathBuf::from).or_else(|| env("HOME").map(|h| Path::new(&h).join(".local/state")))?;
        Some(base.join("pimax/onboarding.state"))
    }

    pub fn state(&self, s: Step) -> State {
        self.steps.iter().find(|(x, _)| *x == s).map_or(State::Pending, |(_, st)| *st)
    }

    pub fn set(&mut self, s: Step, st: State) {
        match self.steps.iter_mut().find(|(x, _)| *x == s) {
            Some(e) => e.1 = st,
            None => self.steps.push((s, st)),
        }
    }

    /// The next step to show for a module's step list (failed steps are shown again).
    pub fn next(&self, module_steps: &[Step]) -> Option<Step> {
        module_steps.iter().copied().find(|s| !matches!(self.state(*s), State::Done | State::Skipped))
    }

    /// Parse; unknown keys and bad lines are ignored (a newer app may have written them).
    pub fn parse(src: &str) -> Progress {
        let mut p = Progress::default();
        for l in src.lines() {
            let l = l.trim();
            if l.starts_with('#') {
                continue;
            }
            let Some((k, v)) = l.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            if k == "module" {
                p.module = Some(v.to_string()).filter(|v| !v.is_empty());
            } else if let (Some(s), Some(st)) = (Step::from_id(k), State::parse(v)) {
                p.set(s, st);
            }
        }
        p
    }

    pub fn render(&self) -> String {
        let mut o = String::from("# Pimax onboarding progress (plain text; delete this file to start over)\n");
        if let Some(m) = &self.module {
            let _ = writeln!(o, "module = {m}");
        }
        for s in ALL {
            let st = self.state(s);
            if st != State::Pending {
                let _ = writeln!(o, "{} = {}", s.id(), st.id());
            }
        }
        o
    }

    /// Write atomically (temp file + rename).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = path.with_extension("state.tmp");
        std::fs::write(&tmp, self.render())?;
        std::fs::rename(tmp, path)
    }

    pub fn load(path: &Path) -> Progress {
        std::fs::read_to_string(path).map(|s| Progress::parse(&s)).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resumes_and_skips() {
        let steps = [Step::Detect, Step::Checks, Step::Ipd, Step::Pair, Step::Done];
        let mut p = Progress::default();
        assert_eq!(p.next(&steps), Some(Step::Detect));
        p.module = Some("dream-air".into());
        p.set(Step::Detect, State::Done);
        p.set(Step::Checks, State::Done);
        p.set(Step::Ipd, State::Skipped);
        p.set(Step::Pair, State::Failed);
        let back = Progress::parse(&p.render());
        assert_eq!(back, Progress { module: p.module.clone(), steps: back.steps.clone() });
        assert_eq!(back.next(&steps), Some(Step::Pair));
        assert_eq!(back.state(Step::Ipd), State::Skipped);
        assert_eq!(Progress::parse("junk\nfuture_step = done\nipd = maybe\n"), Progress::default());
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = std::env::temp_dir().join(format!("pimax-device-onb-{}", std::process::id()));
        let path = dir.join("pimax/onboarding.state");
        let mut p = Progress::default();
        p.set(Step::Udev, State::Done);
        p.save(&path).unwrap();
        assert_eq!(Progress::load(&path).state(Step::Udev), State::Done);
        let _ = std::fs::remove_dir_all(dir);
    }
}
