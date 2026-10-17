//! The user's own per-device data in the app: where each piece comes from now (the user's own or the shipped Pimax
//! Dream Air reference), the "Import from the previous install" step (`omarchyvr import-previous`, no password, never
//! overwrites), and its checklist items (docs/INSTALL.md "What a clean install keeps and imports").
//!
//! `./install.sh` runs the import once; the app runs it again by itself when its facts show more to bring back (a
//! backup it has not looked at, an older place on disk), but only while nothing else runs: no job, no VR session, no
//! automatic setup in progress. The checklist shows the result and a Fix button for a manual run.
use crate::app::AppCore;
use crate::checklist::{Fix, Group, Item, UserFix};
use crate::cmd::Cmd;
use crate::facts::Facts;
use crate::job::{JobKind, JobSpec, Step};
use crate::paths::Paths;
use omarchyvr_config::{devicedata, import};
use omarchyvr_doctor::Level;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// The job kind of the import.
pub const KIND: &str = "import-previous";

/// What the facts know about the device data.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceFacts {
    /// Where each piece comes from (None until the first collection).
    pub sources: Option<devicedata::Sources>,
    /// What an import would bring back now, in plain words (empty: nothing).
    pub pending: Vec<String>,
    /// The last import: (unix time, its plain lines).
    pub last: Option<(u64, Vec<String>)>,
}

/// Look at the disk (cheap: no archive is opened).
pub fn collect(home: &Path, env: &dyn Fn(&str) -> Option<String>) -> DeviceFacts {
    // the cache root comes from the machine's environment (the fake one in tests), not from this process
    let places = import::Places { xdg_cache: env("XDG_CACHE_HOME").filter(|v| !v.is_empty()).map(std::path::PathBuf::from), ..import::Places::for_home(home) };
    DeviceFacts {
        sources: Some(devicedata::Sources::read(home, env)),
        pending: import::pending(&places),
        last: import::last_run(home),
    }
}

/// The import command (`omarchyvr import-previous`).
pub fn import_cmd(launcher: &Path) -> Cmd {
    Cmd::with_args(launcher.display().to_string(), vec!["import-previous".into()])
}

/// The checklist items: the import itself and the controller models (the app item asked for on 2026-10-17:
/// "Controller models: found / missing, import"), plus one line naming what uses the shipped reference.
pub fn items(facts: &Facts, p: &Paths) -> Vec<Item> {
    let d = &facts.device;
    let Some(s) = &d.sources else { return vec![] };
    let launcher = p.launcher(&|x| facts.has(x));
    let fix = || {
        Fix::User(UserFix {
            why: "Bring back your own settings and calibration of the previous install on this PC (never overwrites; your cloud AI keys are not touched).".into(),
            steps: vec![import_cmd(&launcher)],
            follow_up: None,
            needs_idle: true,
        })
    };
    let mut v = Vec::new();
    v.push(if d.pending.is_empty() {
        let detail = match &d.last {
            Some((_, lines)) => {
                let brought: Vec<&str> = lines.iter().filter(|l| l.contains(": brought back from ")).map(|l| l.split(':').next().unwrap_or("")).collect();
                if brought.is_empty() { "nothing to bring back".to_string() } else { format!("brought back: {}", brought.join(", ")) }
            }
            None => "nothing to bring back".to_string(),
        };
        Item::new("previous-install", Group::Extras, "Your previous install", Level::Ok, detail, None, Fix::None)
    } else {
        Item::new("previous-install", Group::Extras, "Your previous install", Level::Warn, format!("can bring back {}", d.pending.join("; ")), None, fix())
    });
    let models_pending = d.pending.iter().any(|r| r.contains("controller models"));
    v.push(match &s.controllers {
        Some(dir) => Item::new("controller-models", Group::Extras, "Controller models", Level::Ok, format!("your own ({})", dir.display()), None, Fix::None),
        None if models_pending => Item::new("controller-models", Group::Extras, "Controller models", Level::Warn,
            "using the shipped Dream Air reference; your own are on this PC: Import brings them in", None, fix()),
        None => Item::new("controller-models", Group::Extras, "Controller models", Level::Info,
            "using the shipped Dream Air reference (Pimax's default models; optical tracking works)", None, Fix::None),
    });
    let reference = s.reference_in_use();
    v.push(if reference.is_empty() {
        Item::new("reference-data", Group::Extras, "Headset data", Level::Ok, "all your own (calibration, controller models, lens, gyro)", None, Fix::None)
    } else {
        Item::new("reference-data", Group::Extras, "Headset data", Level::Info,
            format!("using the shipped Pimax Dream Air reference for: {} (your own replaces it once read from the headset or imported)", reference.join(", ")), None, Fix::None)
    });
    v
}

/// Once per app run.
static STARTED: AtomicBool = AtomicBool::new(false);

impl AppCore {
    /// After new facts: run the import by itself when it would bring something back and nothing else runs.
    pub(crate) fn previous_install_after_facts(&mut self) {
        if self.demo || self.facts.device.pending.is_empty() || STARTED.load(Ordering::Relaxed) {
            return;
        }
        let setup_busy = !matches!(
            self.onboarding.setup.phase,
            crate::autosetup::Phase::Idle | crate::autosetup::Phase::Ready | crate::autosetup::Phase::Problems | crate::autosetup::Phase::NeedsRestart
        );
        // nothing else runs: no job, no VR session (Monado or SteamVR's), no setup in progress
        if self.job.is_some() || self.live.service.is_some() || !self.live.monado_pids.is_empty() || self.live.steamvr.is_some() || setup_busy {
            return;
        }
        let spec = JobSpec {
            kind: JobKind::Other(KIND.into()),
            label: "Bringing back your previous install's settings".into(),
            steps: vec![
                Step::Note(format!("Found on this PC: {}. Copying what is missing (nothing is overwritten).", self.facts.device.pending.join("; "))),
                Step::Run(import_cmd(&self.launcher_bin())),
            ],
        };
        if self.spawn(spec) {
            STARTED.store(true, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts_with(d: DeviceFacts) -> Facts {
        Facts { device: d, ..Facts::default() }
    }

    fn sources(own: bool) -> devicedata::Sources {
        let p = |s: &str| own.then(|| std::path::PathBuf::from(s));
        devicedata::Sources { calibration: p("/h/c.xml"), controllers: p("/h/controllers"), lens: p("/h/lens.conf"), gyro_measured: Some(own) }
    }

    #[test]
    fn the_import_and_the_controller_models_show_with_plain_words_and_one_fix() {
        let p = crate::demo::idle_machine().1;
        // nothing collected yet: no items
        assert!(items(&Facts::default(), &p).is_empty());
        // a clean install with the user's own models still in Pimax's Windows data on this PC
        let f = facts_with(DeviceFacts { sources: Some(sources(false)), pending: vec!["your controller models (Pimax's Windows data on this PC)".into()], last: None });
        let v = items(&f, &p);
        let prev = v.iter().find(|i| i.id == "previous-install").unwrap();
        assert_eq!(prev.level, Level::Warn);
        match &prev.fix {
            Fix::User(u) => {
                assert_eq!(u.steps[0].args, ["import-previous"]);
                assert_eq!(crate::safety::check(&u.steps[0]), Ok(()));
            }
            x => panic!("{x:?}"),
        }
        let m = v.iter().find(|i| i.id == "controller-models").unwrap();
        assert_eq!(m.level, Level::Warn);
        assert!(m.detail.contains("Import brings them in"), "{}", m.detail);
        let r = v.iter().find(|i| i.id == "reference-data").unwrap();
        assert!(r.detail.contains("headset calibration, controller models, lens profile, gyro bias"), "{}", r.detail);
        // everything the user's own, the last import listed
        let f = facts_with(DeviceFacts {
            sources: Some(sources(true)),
            pending: vec![],
            last: Some((1, vec!["IPD: brought back from ~/b.tar.gz (.config/pimax/ipd.env)".into(), "lens profile: already there".into()])),
        });
        let v = items(&f, &p);
        assert_eq!(v.iter().find(|i| i.id == "previous-install").unwrap().detail, "brought back: IPD");
        assert!(v.iter().all(|i| i.level == Level::Ok), "{v:?}");
    }
}
