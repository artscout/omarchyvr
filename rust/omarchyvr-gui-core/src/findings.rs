//! The system checks of `pimax-device`: its own checks plus every omarchyvr-doctor check through `DoctorBridge`, grouped
//! by area for the Setup page and the onboarding, and how each catalogue fix runs here:
//!
//! * user fixes run in the job runner, as the user (the safety rules check every command again);
//! * root fixes become the confirmed pkexec dialog of the Setup page ([`RootFix`]): never silently, and copy only when
//!   the safety rules do not allow the lines through pkexec;
//! * reboots are only shown (the app never reboots for you).
//!
//! The checks run in the background with the doctor ([`crate::facts::collect_facts`]), never on the UI thread.
use crate::checklist::RootFix;
use crate::cmd::Cmd;
use pimax_device::check::{self, Area, Ctx, Finding, FixRef, Planned, Severity};
use omarchyvr_doctor::{CmdOut, Host, Opts};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

/// A [`Host`] that starts each read-only command once per round: the doctor runs twice in a round (its own report
/// and `DoctorBridge`), the second run reads the first one's outputs.
pub struct Memo<'a> {
    inner: &'a dyn Host,
    runs: Mutex<HashMap<String, Option<CmdOut>>>,
}

impl<'a> Memo<'a> {
    /// Wrap a host for one round.
    pub fn new(inner: &'a dyn Host) -> Memo<'a> {
        Memo { inner, runs: Mutex::default() }
    }
}

impl Host for Memo<'_> {
    fn read(&self, p: &Path) -> Option<Vec<u8>> {
        self.inner.read(p)
    }
    fn list(&self, p: &Path) -> Vec<String> {
        self.inner.list(p)
    }
    fn exists(&self, p: &Path) -> bool {
        self.inner.exists(p)
    }
    fn can_rw(&self, p: &Path) -> bool {
        self.inner.can_rw(p)
    }
    fn run(&self, prog: &str, args: &[&str]) -> Option<CmdOut> {
        let key = std::iter::once(prog).chain(args.iter().copied()).collect::<Vec<_>>().join("\0");
        if let Some(r) = self.runs.lock().ok().and_then(|m| m.get(&key).cloned()) {
            return r;
        }
        let r = self.inner.run(prog, args);
        if let Ok(mut m) = self.runs.lock() {
            m.insert(key, r.clone());
        }
        r
    }
    fn env(&self, key: &str) -> Option<String> {
        self.inner.env(key)
    }
}

/// Doctor checks that `pimax-device` has ported (the ported check's finding replaces the bridged one).
pub const PORTED: [(&str, &str); 2] = [("doctor.setcap", "cpu.cap_sys_nice"), ("doctor.merged-head-sync", "gpu.merged_head_sync")];

/// Run the default checks and the doctor bridge. The module is the connected headset's, if a module claims it.
pub fn run(host: &dyn Host, opts: &Opts, usb: &[(u16, u16)]) -> Vec<Finding> {
    let module = match crate::headsets::detect_usb(usb) {
        pimax_device::Detected::Module(m) => Some(m),
        _ => None,
    };
    let mut checks = pimax_device::checks::default_checks();
    checks.push(&pimax_device::checks::doctor::DoctorBridge);
    let cx = Ctx { host, opts, module };
    let mut v = match crate::crash::catching(|| check::run_all(&checks, &cx)) {
        Ok(v) => v,
        Err(e) => {
            let msg = crate::worker::panic_text(e.as_ref());
            log::error!("the system checks failed: {msg}");
            vec![Finding {
                check: "app.checks".into(),
                area: Area::Runtime,
                severity: Severity::Warn,
                title: "The system checks failed".into(),
                detail: msg,
                fix: None,
            }]
        }
    };
    let have: Vec<String> = v.iter().map(|f| f.check.clone()).collect();
    v.retain(|f| !PORTED.iter().any(|(old, new)| f.check == *old && have.iter().any(|h| h == new)));
    v
}

/// What the app knows better than a bridged doctor check: WayVR Xid 31 faults alone are recoverable per-app faults
/// (LESSONS row 9; the desk start allows "Start anyway"), not a reason to reboot.
pub fn adjust(v: &mut [Finding], kernel: &crate::kernel::Verdict) {
    if let crate::kernel::Verdict::WayvrOnly(ev) = kernel {
        for f in v.iter_mut().filter(|f| f.check == "doctor.kernel-log" && f.severity > Severity::Warn) {
            f.severity = Severity::Warn;
            f.detail = format!(
                "{} WayVR Xid 31 MMU fault(s) this boot: recoverable per-app faults; desk mode can start anyway. A reboot clears the list.",
                ev.len()
            );
        }
    }
}

/// Areas in page order.
pub const AREAS: [Area; 7] = [Area::Device, Area::Usb, Area::Drivers, Area::Gpu, Area::Cpu, Area::Audio, Area::Runtime];

/// Section title of an area.
pub fn area_title(a: Area) -> &'static str {
    match a {
        Area::Device => "Headset",
        Area::Usb => "USB and access",
        Area::Drivers => "Drivers",
        Area::Gpu => "GPU and display",
        Area::Cpu => "CPU",
        Area::Audio => "Audio",
        Area::Runtime => "Runtime",
    }
}

/// Findings per area, in page order, worst first inside an area (empty areas left out).
pub fn grouped(f: &[Finding]) -> Vec<(Area, Vec<&Finding>)> {
    AREAS
        .iter()
        .filter_map(|a| {
            let mut v: Vec<&Finding> = f.iter().filter(|x| x.area == *a).collect();
            v.sort_by_key(|x| std::cmp::Reverse(x.severity));
            (!v.is_empty()).then_some((*a, v))
        })
        .collect()
}

/// (ok + info, warnings, failing).
pub fn counts(f: &[Finding]) -> (usize, usize, usize) {
    let n = |s: Severity| f.iter().filter(|x| x.severity == s).count();
    (n(Severity::Ok) + n(Severity::Info), n(Severity::Warn), n(Severity::Fail))
}

/// Needs attention and has a catalogue fix.
pub fn fixable(f: &Finding) -> bool {
    f.severity >= Severity::Warn && f.fix.is_some()
}

/// A catalogue fix (or its undo) on its way to the job runner or the pkexec dialog: logged to `fixes.log` when it ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogueRun {
    /// The rendered fix.
    pub planned: Planned,
    /// This runs its undo lines.
    pub undo: bool,
}

impl CatalogueRun {
    /// The `fixes.log` line for an outcome.
    pub fn log_line(&self, unix_s: u64, outcome: &str) -> String {
        let p = if self.undo { flipped(&self.planned) } else { self.planned.clone() };
        let outcome = if self.undo { format!("undo {outcome}") } else { outcome.to_string() };
        pimax_device::autofix::log_line(unix_s, &p, &outcome)
    }
}

/// The same fix with commands and undo swapped (what an undo runs, and what undoes it again).
pub fn flipped(p: &Planned) -> Planned {
    Planned { fix: p.fix, commands: p.undo.clone(), undo: p.commands.clone() }
}

/// How a catalogue fix runs here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixAction {
    /// As the user, in the job runner.
    User(Vec<Cmd>),
    /// Root: the pkexec dialog (or copy only).
    Root(RootFix),
    /// Only shown: lines to copy and why (a reboot, or nothing to undo).
    Manual(String, Vec<String>),
}

/// One template line as a command: `omarchyvr` is the launcher the app uses; the arguments passed `safe_arg` (plain
/// words and paths), so splitting at spaces is exact.
fn to_cmd(line: &str, launcher: &Path) -> Cmd {
    let w: Vec<&str> = line.split_whitespace().collect();
    let prog = match w.first() {
        Some(&"omarchyvr") => launcher.display().to_string(),
        Some(p) => p.to_string(),
        None => String::new(),
    };
    Cmd::new(prog, w.get(1..).unwrap_or(&[]))
}

/// What running a planned fix (or its undo) means here.
pub fn action(r: &CatalogueRun, launcher: &Path) -> FixAction {
    let p = if r.undo { flipped(&r.planned) } else { r.planned.clone() };
    let f = p.fix;
    if p.commands.is_empty() {
        return FixAction::Manual("Nothing to run: this fix changes no state that could be undone.".into(), Vec::new());
    }
    if p.commands.iter().any(|c| c.split_whitespace().any(|w| w == "reboot")) {
        return FixAction::Manual("The app never reboots for you: reboot when it suits you.".into(), p.commands);
    }
    if f.root || p.commands.iter().any(|c| c.starts_with("sudo ")) {
        return FixAction::Root(RootFix::new(f.why, p.commands, p.undo, f.reboot && !r.undo));
    }
    FixAction::User(p.commands.iter().map(|c| to_cmd(c, launcher)).collect())
}

/// Plan a finding's fix.
pub fn plan(f: &Finding) -> Option<Result<Planned, String>> {
    f.fix.as_ref().map(FixRef::plan)
}

/// The `udev.install_rules` fix for a repository checkout (the onboarding's udev step).
pub fn udev_fix(repo: &Path) -> Result<Planned, String> {
    FixRef::new("udev.install_rules", &[("repo", &repo.display().to_string())]).plan()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Counting(omarchyvr_doctor::fake::FakeHost, AtomicUsize);
    impl Host for Counting {
        fn read(&self, p: &Path) -> Option<Vec<u8>> {
            self.0.read(p)
        }
        fn list(&self, p: &Path) -> Vec<String> {
            self.0.list(p)
        }
        fn exists(&self, p: &Path) -> bool {
            self.0.exists(p)
        }
        fn can_rw(&self, p: &Path) -> bool {
            self.0.can_rw(p)
        }
        fn run(&self, prog: &str, args: &[&str]) -> Option<CmdOut> {
            self.1.fetch_add(1, Ordering::SeqCst);
            self.0.run(prog, args)
        }
        fn env(&self, key: &str) -> Option<String> {
            self.0.env(key)
        }
    }

    #[test]
    fn the_doctor_commands_run_once_per_round() {
        let (m, p) = crate::demo::idle_machine();
        let opts = crate::demo::opts(&p);
        let h = Counting(m.host.clone(), AtomicUsize::new(0));
        let _ = omarchyvr_doctor::run_checks(&h, &opts);
        let once = h.1.swap(0, Ordering::SeqCst);
        assert!(once > 0);
        let memo = Memo::new(&h);
        let _ = omarchyvr_doctor::run_checks(&memo, &opts);
        let f = run(&memo, &opts, &[(0x34a4, 0x0044)]);
        assert!(h.1.load(Ordering::SeqCst) <= once + 2, "{} commands for {once}", h.1.load(Ordering::SeqCst));
        assert!(f.iter().any(|x| x.check.starts_with("doctor.")), "{f:?}");
        assert!(f.iter().any(|x| x.check == "cpu.cap_sys_nice"));
        assert!(!f.iter().any(|x| x.check == "doctor.setcap"), "ported checks replace the bridged ones");
        let g = grouped(&f);
        assert!(g.windows(2).all(|w| AREAS.iter().position(|a| *a == w[0].0) < AREAS.iter().position(|a| *a == w[1].0)));
        let (ok, warn, fail) = counts(&f);
        assert_eq!(ok + warn + fail, f.len());
    }

    #[test]
    fn wayvr_faults_alone_are_a_warning() {
        let (m, p) = crate::demo::idle_machine();
        let opts = crate::demo::opts(&p);
        let mut f = run(&m.host, &opts, &[(0x34a4, 0x0044)]);
        let ev = crate::kernel::scan(crate::demo::KERNEL_LOG);
        let before = f.iter().find(|x| x.check == "doctor.kernel-log").map(|x| x.severity);
        adjust(&mut f, &crate::kernel::verdict(&ev));
        let k = f.iter().find(|x| x.check == "doctor.kernel-log").expect("kernel-log finding");
        assert_eq!(before, Some(Severity::Fail), "the doctor alone says reboot");
        assert_eq!(k.severity, Severity::Warn);
        assert!(k.detail.starts_with("2 WayVR Xid 31"), "{}", k.detail);
    }

    #[test]
    fn catalogue_fixes_map_to_the_job_runner_the_pkexec_dialog_or_copy() {
        let launcher = Path::new("/home/u/.local/bin/omarchyvr");
        let run = |id: &'static str, args: &[(&str, &str)], undo: bool| {
            let planned = FixRef::new(id, args).plan().unwrap();
            action(&CatalogueRun { planned, undo }, launcher)
        };
        // a user fix: the launcher path, every word kept
        match run("runtime.stop_stale_monado", &[], false) {
            FixAction::User(c) => {
                assert_eq!(c[0].program, "/home/u/.local/bin/omarchyvr");
                assert_eq!(c[0].args, ["vr", "stop", "--grace", "30"]);
            }
            o => panic!("{o:?}"),
        }
        // root: the pkexec dialog with the documented lines, never a user command
        match run("udev.install_rules", &[("repo", "/home/u/omarchyvr")], false) {
            FixAction::Root(r) => {
                assert!(r.lines[0].starts_with("sudo install -Dm644 /home/u/omarchyvr/linux/70-pimax.rules"));
                assert!(r.command().is_some_and(|c| c.root && c.program == crate::safety::PKEXEC), "{:?}", r.pkexec);
            }
            o => panic!("{o:?}"),
        }
        let svc = "/home/u/.local/lib/omarchyvr/monado/src/xrt/targets/service/monado-service";
        match run("cpu.setcap_monado", &[("path", svc)], true) {
            FixAction::Root(r) => assert_eq!(r.lines, vec![format!("sudo setcap -r {svc}")]),
            o => panic!("{o:?}"),
        }
        // a reboot is only shown
        assert!(matches!(run("system.reboot", &[], false), FixAction::Manual(_, l) if l == ["systemctl reboot"]));
        // nothing to undo
        assert!(matches!(run("runtime.stop_stale_monado", &[], true), FixAction::Manual(_, l) if l.is_empty()));
        // a log line names the undo
        let planned = FixRef::new("user.keepalive_enable", &[]).plan().unwrap();
        let l = CatalogueRun { planned, undo: true }.log_line(5, "done");
        assert!(l.starts_with("5\tuser.keepalive_enable\tundo done\tsystemctl --user disable --now omarchyvr-keepalived\t"), "{l}");
    }
}
