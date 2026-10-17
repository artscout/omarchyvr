//! The NVIDIA display patch on drivers that were not tested (docs/NVIDIA-UNTESTED.md).
//!
//! * The app's switch "Allow the patch on untested NVIDIA drivers" (off by default) is saved in `gui-state.toml`
//!   (`nvidia_allow_untested`) and passed to the root job as `OVR_NVIDIA_ALLOW_UNTESTED` in `root-plan.env`; the root
//!   job writes it to `/etc/omarchyvr/nvidia.conf` (`allow_untested=1|0`) so the pacman hook honours it on later
//!   driver updates.
//! * `nvidia-patch-reapply.sh` writes `/var/lib/omarchyvr/nvidia-patch-status` (`driver=`, `result=`, `fuzz=`, `time=`)
//!   after every run that patches or refuses; this module reads both files and says it in plain words.
//!
//! Everything here is parsing and wording; the files are read by [`read_info`] only.
use crate::install::Install;
use omarchyvr_doctor::Level;
use std::path::{Path, PathBuf};

/// The root job's copy of the choice (world-readable).
pub const CONF_FILE: &str = "/etc/omarchyvr/nvidia.conf";
/// What the patch script did last (world-readable).
pub const STATUS_FILE: &str = "/var/lib/omarchyvr/nvidia-patch-status";
/// The key of the choice in `root-plan.env`.
pub const PLAN_KEY: &str = "OVR_NVIDIA_ALLOW_UNTESTED";
/// The switch's label (the same words in the app, the script's banner and the docs).
pub const SWITCH_LABEL: &str = "Allow the patch on untested NVIDIA drivers";
/// The plain-words warning shown next to the switch.
pub const WARNING: &str = "Only the NVIDIA driver versions we tested are patched by default. On any other version the patch is untested driver \
code: the headset picture may break, or the patched driver build may fail. The desktop stays working either way: if the patched \
build fails, the normal NVIDIA module is built instead (the headset picture then stays broken until a patch for your version \
exists), and a restore snapshot is taken first.";

/// What the patch script reported for the newest driver source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchResult {
    /// A tested driver, patched (or already patched).
    Tested,
    /// An untested driver, patched because the user allowed it.
    UntestedAllowed,
    /// An untested driver, not patched (the switch is off).
    UntestedRefused,
    /// The patches did not apply or the patched build failed (the stock module was built instead).
    Failed,
    /// A result word this version does not know.
    Other(String),
}

/// The status file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchStatus {
    /// The driver version the result is about.
    pub driver: String,
    /// The result.
    pub result: PatchResult,
    /// The patch fuzz that was needed (`Some(2)` = some context lines differed); `None` = nothing was applied.
    pub fuzz: Option<u8>,
    /// When (UTC, as written).
    pub time: String,
}

impl PatchStatus {
    /// Parse `key=value` lines; `None` without a driver.
    pub fn parse(text: &str) -> Option<PatchStatus> {
        let (mut driver, mut result, mut fuzz, mut time) = (String::new(), String::new(), None, String::new());
        for l in text.lines() {
            let Some((k, v)) = l.split_once('=') else { continue };
            let v = v.trim();
            match k.trim() {
                "driver" => driver = v.to_string(),
                "result" => result = v.to_string(),
                "fuzz" => fuzz = v.parse::<u8>().ok(),
                "time" => time = v.to_string(),
                _ => {}
            }
        }
        if driver.is_empty() {
            return None;
        }
        let result = match result.as_str() {
            "tested" => PatchResult::Tested,
            "untested-allowed" => PatchResult::UntestedAllowed,
            "untested-refused" => PatchResult::UntestedRefused,
            "failed" => PatchResult::Failed,
            o => PatchResult::Other(o.to_string()),
        };
        Some(PatchStatus { driver, result, fuzz, time })
    }
}

/// Does the conf file allow untested drivers? Same grammar as the script: `allow_untested=1|yes|true|on`
/// (optionally quoted), nothing else counts, nothing is executed.
pub fn conf_allows(text: &str) -> bool {
    text.lines().any(|l| {
        let Some((k, v)) = l.split_once('=') else { return false };
        k.trim() == "allow_untested" && matches!(v.trim().trim_matches('"').to_ascii_lowercase().as_str(), "1" | "yes" | "true" | "on")
    })
}

/// The versions of `tested-versions.txt` (one per line, `#` comments).
pub fn parse_tested(text: &str) -> Vec<String> {
    text.lines().map(|l| l.split('#').next().unwrap_or("").split_whitespace().collect::<String>()).filter(|l| !l.is_empty()).collect()
}

/// The value of `KEY=value` in a plan file.
pub fn plan_value(plan: &str, key: &str) -> Option<String> {
    plan.lines().find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix('=')).map(|v| v.trim().trim_matches('"').to_string()))
}

/// `root-plan.env` with the choice set (the key is replaced, or added at the end).
pub fn set_plan_allow(plan: &str, allow: bool) -> String {
    let line = format!("{PLAN_KEY}={}", if allow { "yes" } else { "no" });
    let mut out = Vec::new();
    let mut done = false;
    for l in plan.lines() {
        if l.strip_prefix(PLAN_KEY).is_some_and(|r| r.starts_with('=')) {
            if !done {
                out.push(line.clone());
                done = true;
            }
        } else {
            out.push(l.to_string());
        }
    }
    if !done {
        out.push(line);
    }
    out.join("\n") + "\n"
}

/// What the app knows about the NVIDIA patch on this machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NvidiaInfo {
    /// The installed driver version (the status file, else the setup plan, else the loaded module).
    pub driver: Option<String>,
    /// Is it in `tested-versions.txt`? `None` = the list is unknown.
    pub tested: Option<bool>,
    /// The tested versions.
    pub tested_list: Vec<String>,
    /// What the patch script reported last.
    pub status: Option<PatchStatus>,
    /// The root job's copy of the choice: `None` = no conf file yet.
    pub conf_allow: Option<bool>,
}

/// The files [`read_with`] reads (paths are overridable for tests).
#[derive(Debug, Clone)]
pub struct NvidiaFiles {
    /// [`STATUS_FILE`].
    pub status: PathBuf,
    /// [`CONF_FILE`].
    pub conf: PathBuf,
    /// `root-plan.env`.
    pub plan_env: PathBuf,
    /// `tested-versions.txt` of the installed data.
    pub tested: PathBuf,
    /// `/sys/module/nvidia/version`.
    pub loaded: PathBuf,
}

impl NvidiaFiles {
    /// The real files of this install.
    pub fn of(i: &Install) -> NvidiaFiles {
        NvidiaFiles {
            status: PathBuf::from(STATUS_FILE),
            conf: PathBuf::from(CONF_FILE),
            plan_env: i.plan_env(),
            tested: i.data().join("patches/nvidia/tested-versions.txt"),
            loaded: PathBuf::from("/sys/module/nvidia/version"),
        }
    }
}

/// Read the NVIDIA patch state.
pub fn read_with(f: &NvidiaFiles) -> NvidiaInfo {
    let rd = |p: &Path| std::fs::read_to_string(p).ok();
    let status = rd(&f.status).and_then(|t| PatchStatus::parse(&t));
    let plan = rd(&f.plan_env);
    let tested_list = match rd(&f.tested) {
        Some(t) => parse_tested(&t),
        None => plan.as_deref().and_then(|p| plan_value(p, "OVR_NVIDIA_TESTED")).map(|v| v.split_whitespace().map(str::to_string).collect()).unwrap_or_default(),
    };
    let driver = status
        .as_ref()
        .map(|s| s.driver.clone())
        .or_else(|| plan.as_deref().and_then(|p| plan_value(p, "OVR_NVIDIA_VERSION")).filter(|v| !v.is_empty()))
        .or_else(|| rd(&f.loaded).map(|v| v.trim().to_string()).filter(|v| !v.is_empty()));
    let tested = driver.as_ref().filter(|_| !tested_list.is_empty()).map(|d| tested_list.iter().any(|t| t == d));
    NvidiaInfo { driver, tested, tested_list, status, conf_allow: rd(&f.conf).map(|t| conf_allows(&t)) }
}

/// Read the real files.
pub fn read_info(i: &Install) -> NvidiaInfo {
    read_with(&NvidiaFiles::of(i))
}

impl NvidiaInfo {
    /// Show the card at all (an NVIDIA driver is known).
    pub fn present(&self) -> bool {
        self.driver.is_some()
    }

    /// The system's copy of the choice differs from the switch: the root job has to run again (Apply).
    pub fn needs_apply(&self, switch: bool, root_done: bool) -> bool {
        root_done && self.present() && self.conf_allow.unwrap_or(false) != switch
    }

    /// Plain words for the card: (level, text).
    pub fn lines(&self, switch: bool, root_done: bool) -> Vec<(Level, String)> {
        let mut out = Vec::new();
        let Some(d) = &self.driver else {
            out.push((Level::Info, "No NVIDIA driver found on this PC.".into()));
            return out;
        };
        match self.tested {
            Some(true) => out.push((Level::Ok, format!("Installed driver: {d}. It is a tested version."))),
            Some(false) => out.push((
                Level::Warn,
                format!(
                    "Installed driver: {d}. It is NOT a tested version (tested: {}).",
                    if self.tested_list.is_empty() { "none listed".to_string() } else { self.tested_list.join(", ") }
                ),
            )),
            None => out.push((Level::Info, format!("Installed driver: {d}. Whether it is tested is not known yet."))),
        }
        match &self.status {
            Some(s) if &s.driver == d || self.driver.is_none() => out.push(status_line(s)),
            Some(s) => out.push((Level::Info, format!("The last patch run was for driver {}, not {d}; it runs again when the driver updates.", s.driver))),
            None => out.push((Level::Info, "Patch state: not recorded yet (it is written when the system setup runs or the driver updates).".into())),
        }
        if self.tested == Some(false) && !switch && !matches!(self.status.as_ref().map(|s| &s.result), Some(PatchResult::UntestedAllowed)) {
            out.push((Level::Warn, "The switch is off, so this driver stays unpatched: the desktop works, the headset picture does not.".into()));
        }
        if self.needs_apply(switch, root_done) {
            out.push((
                Level::Warn,
                format!("The switch is {} but this PC still has it {}: press Apply (one password) so driver updates follow it.", on_off(switch), on_off(!switch)),
            ));
        }
        out
    }
}

fn on_off(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}

fn status_line(s: &PatchStatus) -> (Level, String) {
    match &s.result {
        PatchResult::Tested => (Level::Ok, "Patch: applied (tested driver).".into()),
        PatchResult::UntestedAllowed => (
            Level::Warn,
            if s.fuzz == Some(2) {
                "Patch: applied to this untested driver, but only with relaxed matching (some lines of the driver source differ from the tested one). Check the headset picture.".into()
            } else {
                "Patch: applied to this untested driver because you allowed it. Check the headset picture.".into()
            },
        ),
        PatchResult::UntestedRefused => (
            Level::Warn,
            "Patch: NOT applied. This driver is not tested and the switch is off. The desktop works with the normal NVIDIA module; the headset picture will not.".into(),
        ),
        PatchResult::Failed => (
            Level::Fail,
            "Patch: failed. It did not fit this driver, or the patched build failed. The normal NVIDIA module is used instead, so the desktop works; the headset picture does not. Help > Report a problem sends the details.".into(),
        ),
        PatchResult::Other(o) => (Level::Info, format!("Patch state: {o}.")),
    }
}

/// The demo machine (screenshots): a tested driver, patched, the switch off.
pub fn demo_info() -> NvidiaInfo {
    NvidiaInfo {
        driver: Some("610.57.04".into()),
        tested: Some(true),
        tested_list: vec!["610.57.04".into()],
        status: PatchStatus::parse("driver=610.57.04\nresult=tested\nfuzz=0\ntime=2026-10-17T08:00:00Z\n"),
        conf_allow: Some(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_file_is_parsed() {
        let s = PatchStatus::parse("driver=999.1\nresult=untested-allowed\nfuzz=2\ntime=2026-10-17T01:02:03Z\n").unwrap();
        assert_eq!(s.driver, "999.1");
        assert_eq!(s.result, PatchResult::UntestedAllowed);
        assert_eq!(s.fuzz, Some(2));
        let s = PatchStatus::parse("driver=999.1\nresult=untested-refused\nfuzz=-\n").unwrap();
        assert_eq!((s.result, s.fuzz), (PatchResult::UntestedRefused, None));
        assert_eq!(PatchStatus::parse("driver=1\nresult=failed\n").unwrap().result, PatchResult::Failed);
        assert_eq!(PatchStatus::parse("driver=1\nresult=tested\nfuzz=0\n").unwrap().result, PatchResult::Tested);
        assert_eq!(PatchStatus::parse("driver=1\nresult=weird\n").unwrap().result, PatchResult::Other("weird".into()));
        assert!(PatchStatus::parse("").is_none());
        assert!(PatchStatus::parse("result=tested\n").is_none(), "no driver, no status");
    }

    #[test]
    fn the_conf_file_grammar_matches_the_script() {
        assert!(conf_allows("# x\nallow_untested=1\n"));
        assert!(conf_allows("allow_untested = yes"));
        assert!(conf_allows("allow_untested=\"true\""));
        assert!(conf_allows("allow_untested=ON"));
        assert!(!conf_allows("allow_untested=0\n"));
        assert!(!conf_allows(""));
        assert!(!conf_allows("allow_untested=1; touch /x"), "anything after the value makes it not match");
        assert!(!conf_allows("# allow_untested=1"));
    }

    #[test]
    fn the_tested_list_ignores_comments() {
        assert_eq!(parse_tested("# a\n610.57.04\n 999.5  # added\n\n"), vec!["610.57.04".to_string(), "999.5".to_string()]);
    }

    #[test]
    fn the_plan_choice_is_replaced_or_added() {
        let p = "OVR_USER=u\nOVR_NVIDIA=patch\n";
        let a = set_plan_allow(p, true);
        assert_eq!(plan_value(&a, PLAN_KEY).as_deref(), Some("yes"));
        assert!(a.contains("OVR_USER=u\n") && a.ends_with('\n'));
        let b = set_plan_allow(&a, false);
        assert_eq!(plan_value(&b, PLAN_KEY).as_deref(), Some("no"));
        assert_eq!(b.matches(PLAN_KEY).count(), 1, "replaced, not appended twice");
        assert_eq!(set_plan_allow(&set_plan_allow(p, true), true).matches(PLAN_KEY).count(), 1);
        assert_eq!(plan_value("OVR_NVIDIA_VERSION=\"610.57.04\"\n", "OVR_NVIDIA_VERSION").as_deref(), Some("610.57.04"));
        assert_eq!(plan_value("OVR_NVIDIA_VERSIONX=1\n", "OVR_NVIDIA_VERSION"), None);
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("omarchyvr-nvpatch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_state_is_read_from_the_files_and_worded() {
        let d = scratch("read");
        let f = NvidiaFiles { status: d.join("status"), conf: d.join("nvidia.conf"), plan_env: d.join("plan.env"), tested: d.join("tested.txt"), loaded: d.join("loaded") };
        // nothing at all: no card
        assert!(!read_with(&f).present());
        // the setup plan names the driver; the list says it is not tested
        std::fs::write(&f.plan_env, "OVR_NVIDIA_VERSION=999.1\nOVR_NVIDIA_TESTED=610.57.04\n").unwrap();
        let i = read_with(&f);
        assert_eq!((i.driver.as_deref(), i.tested), (Some("999.1"), Some(false)));
        assert_eq!(i.tested_list, vec!["610.57.04".to_string()]);
        let l = i.lines(false, false);
        assert!(l[0].1.contains("NOT a tested version") && l[0].0 == Level::Warn);
        assert!(l.iter().any(|x| x.1.contains("not recorded yet")));
        assert!(l.iter().any(|x| x.1.contains("switch is off")), "{l:?}");
        // the installed list wins over the plan; the loaded module is the last resort
        std::fs::write(&f.tested, "610.57.04\n999.1\n").unwrap();
        assert_eq!(read_with(&f).tested, Some(true));
        std::fs::remove_file(&f.plan_env).unwrap();
        std::fs::write(&f.loaded, "610.57.04\n").unwrap();
        assert_eq!(read_with(&f).driver.as_deref(), Some("610.57.04"));
        // the status file and the conf file
        std::fs::write(&f.status, "driver=610.57.04\nresult=tested\nfuzz=0\ntime=t\n").unwrap();
        std::fs::write(&f.conf, "allow_untested=0\n").unwrap();
        let i = read_with(&f);
        assert_eq!(i.conf_allow, Some(false));
        assert!(i.lines(false, true).iter().any(|x| x.0 == Level::Ok && x.1.contains("applied (tested")));
        assert!(!i.needs_apply(false, true));
        assert!(i.needs_apply(true, true), "the switch was turned on after the setup ran");
        assert!(!i.needs_apply(true, false), "before the setup ran the setup button does it");
        assert!(i.lines(true, true).iter().any(|x| x.1.contains("press Apply")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn every_result_has_plain_words() {
        let mk = |r: &str, fuzz: &str| NvidiaInfo {
            driver: Some("999.1".into()),
            tested: Some(false),
            tested_list: vec!["610.57.04".into()],
            status: PatchStatus::parse(&format!("driver=999.1\nresult={r}\nfuzz={fuzz}\n")),
            conf_allow: Some(r == "untested-allowed"),
        };
        let words = |i: &NvidiaInfo, sw| i.lines(sw, true).into_iter().map(|x| x.1).collect::<Vec<_>>().join(" | ");
        assert!(words(&mk("untested-refused", "-"), false).contains("NOT applied"));
        assert!(words(&mk("untested-allowed", "0"), true).contains("because you allowed it"));
        assert!(words(&mk("untested-allowed", "2"), true).contains("relaxed matching"));
        let failed = mk("failed", "0");
        assert!(words(&failed, true).contains("desktop works"));
        assert_eq!(failed.lines(true, true)[1].0, Level::Fail);
        // a status about another driver is not shown as this one's
        let mut other = mk("tested", "0");
        other.status.as_mut().unwrap().driver = "610.57.04".into();
        assert!(words(&other, false).contains("last patch run was for driver 610.57.04"));
        assert!(WARNING.contains("restore snapshot") && WARNING.contains("desktop stays working"));
    }
}
