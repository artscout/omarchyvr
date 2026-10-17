//! P5 acceptance with fake processes: a fake desk session (sleep processes named like the real binaries through
//! symlinks, a fake sysfs, a temp session dir; the real /proc), adopted by the Supervisor; `perf` dies (classified as a
//! SIGABRT crash; the stand-in is killed without a core dump),
//! the reconciler restarts it after the leaf backoff by respawning the argv it captured, through the real executor.
//! Touches nothing outside the temp dir and its own children.
use omarchyvr_autod::executor::{self, Ctx};
use omarchyvr_autod::supervisor::{Captured, Supervisor};
use omarchyvr_components::gpu::Gate;
use omarchyvr_components::model::Mode;
use omarchyvr_components::probe::{probe_all, session_facts, Opts};
use omarchyvr_components::reconcile::Act;
use omarchyvr_components::state::{Switches, World};
use omarchyvr_components::util::{put, Roots, TempDir};
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command};

fn sleep_bin() -> String {
    for p in ["/usr/bin/sleep", "/bin/sleep"] {
        if std::path::Path::new(p).exists() {
            return p.into();
        }
    }
    panic!("no sleep binary")
}

/// The stand-ins never leave a system core dump: on 2026-10-17 every run of this test left a core of `/usr/bin/sleep`
/// named omarchyvr-perf in systemd-coredump (18 that day) and a desktop "Process crashed" notification, which looked
/// like real crashes of our perf tool.
fn no_core_dump(c: &mut Command) -> &mut Command {
    use std::os::unix::process::CommandExt;
    // SAFETY: setrlimit(2) is async-signal-safe; it runs in the child between fork and exec, on a local struct.
    unsafe {
        c.pre_exec(|| {
            let zero = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
            if libc::setrlimit(libc::RLIMIT_CORE, &zero) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        })
    }
}

/// `Max core file size` of a process (from /proc/PID/limits): the soft limit.
fn core_limit(pid: u32) -> Option<String> {
    let l = std::fs::read_to_string(format!("/proc/{pid}/limits")).ok()?;
    l.lines().find(|x| x.starts_with("Max core file size")).and_then(|x| x.split_whitespace().nth(4)).map(str::to_string)
}

struct Kids(Vec<u32>);
impl Drop for Kids {
    fn drop(&mut self) {
        for p in &self.0 {
            // SAFETY: our own test children.
            unsafe { libc::kill(*p as i32, libc::SIGKILL) };
        }
    }
}

#[test]
fn a_crashed_leaf_is_respawned_from_its_captured_argv() {
    let t = TempDir::new("p5-leaf");
    let base = t.path();
    let r = Roots { proc_: "/proc".into(), ..Roots::under(base) };
    for (port, pid) in [("2-4", "0044"), ("2-4.1", "0014"), ("2-4.2", "0043")] {
        put(&r.sys, &format!("bus/usb/devices/{port}/idVendor"), "34a4\n");
        put(&r.sys, &format!("bus/usb/devices/{port}/idProduct"), &format!("{pid}\n"));
    }
    put(&r.sys, "class/video4linux/video0/idVendor", "34a4\n");
    put(&r.sys, "class/video4linux/video0/idProduct", "0014\n");
    std::fs::create_dir_all(r.sys.join("class/video4linux/video0/device")).unwrap();
    put(&r.sys, "class/drm/card1-DP-1/status", "connected\n");
    put(&r.session_dir, "monado.log", "gyro bias done\n");
    put(&r.runtime, "omarchyvr-cam-broker.sock", "");
    put(&r.home, ".config/openxr/1/active_runtime.json", "{\"runtime\":{\"name\":\"Monado\"}}");
    let bin = base.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let mut kids = Kids(vec![]);
    let mut children: Vec<(&str, Child)> = Vec::new();
    for (name, file) in [("monado-service", "monado.pid"), ("pimax-cam", "broker.pid"), ("wayvr", "wayvr.pid"), ("omarchyvr-perf", "perf.pid")] {
        let link = bin.join(name);
        std::os::unix::fs::symlink(sleep_bin(), &link).unwrap();
        let c = no_core_dump(Command::new(&link).arg("300")).spawn().unwrap();
        kids.0.push(c.id());
        put(&r.session_dir, file, &format!("{}\n", c.id()));
        children.push((name, c));
    }
    // the keepalive (`omarchyvr keepalived`) is a hard dependency of monado: the probe reads the real /proc, so the test
    // must run its own stand-in instead of relying on a live keepalive of the machine it runs on
    let ka = bin.join("omarchyvr");
    std::os::unix::fs::symlink("/usr/bin/tail", &ka).unwrap();
    put(base, "keepalived", "");
    let mut ka_child = Command::new(&ka).current_dir(base).args(["keepalived", "-f"]).spawn().unwrap();
    kids.0.push(ka_child.id());
    let mut w = World::new(Default::default(), Default::default(), Switches { enabled: true, ..Default::default() }, Gate::new("t"), Default::default());
    w.desired.sixdof = true;
    let mut sup = Supervisor::new(w);
    let round = |sup: &mut Supervisor, t: f64| {
        let obs = probe_all(&r, &Opts::default(), omarchyvr_components::model::GateLevel::Ok);
        for (id, o) in &obs {
            if let (Some(true), Some(pid)) = (o.up, o.pid) {
                let argv = omarchyvr_components::procs::cmdline(&r.proc_, pid);
                sup.captured.insert(id, Captured { argv, env: omarchyvr_components::procs::environ(&r.proc_, pid), cwd: None });
            }
        }
        sup.w.observe(t, &obs, &session_facts(&r), &|_| false);
        sup.w.now = t;
        sup.decide()
    };
    let d = round(&mut sup, 0.0);
    assert!(d.run.is_none());
    assert!(sup.w.intent.on(), "adopted");
    assert_eq!(sup.w.mode(), Mode::Desk);
    assert!(sup.w.wanted("perf"));
    let old = sup.w.c("perf").pid.unwrap();
    // perf "aborts": the supervisor is told SIGABRT (the exit status below is what it classifies). The stand-in itself
    // is ended with SIGKILL: a real SIGABRT made systemd-coredump journal a crash of `sleep` on every run, and Omarchy's
    // crash watcher showed "Process crashed" for each (2026-10-17: 18 times). No core limit either, as a second guard.
    let (_, mut perf) = children.pop().unwrap();
    assert_eq!(core_limit(perf.id()).as_deref(), Some("0"), "the stand-in must not dump core");
    // SAFETY: our own child.
    unsafe { libc::kill(perf.id() as i32, libc::SIGKILL) };
    let st = perf.wait().unwrap();
    assert_eq!(st.signal(), Some(libc::SIGKILL));
    sup.w.exit_status.insert(old, (None, Some(libc::SIGABRT)));
    let d = round(&mut sup, 1.0);
    assert!(d.run.is_none(), "leaf backoff 2 s");
    assert_eq!(sup.w.c("perf").last_exit.as_ref().unwrap().1, omarchyvr_components::policy::ExitClass::Crash);
    let d = round(&mut sup, 3.0);
    let step = d.run.expect("restart");
    assert_eq!((step.id, &step.act), ("perf", &Act::Start));
    let ctx = Ctx {
        roots: r.clone(),
        pimax_bin: "/nonexistent/pimax".into(),
        mode: Mode::Desk,
        argv: None,
        seen: vec![],
        captured: sup.captured.get("perf").cloned(),
        preset: "none".into(),
        log: |_| {},
    };
    let (tx, _rx) = std::sync::mpsc::channel();
    let o = executor::execute(&step, &ctx, &tx);
    let new = omarchyvr_components::procs::read_pid(&r.session_dir.join("perf.pid")).unwrap();
    kids.0.push(new);
    assert_eq!(o, omarchyvr_components::state::Outcome::Ok);
    sup.finished(&o, 4.0);
    assert_ne!(new, old);
    assert_eq!(omarchyvr_components::procs::info(&r.proc_, new).unwrap().comm, "omarchyvr-perf");
    let _ = round(&mut sup, 6.0);
    assert_eq!(sup.w.c("perf").up, Some(true));
    assert_eq!(sup.w.c("perf").restart.spent.len(), 1, "one restart spent");
    assert_eq!(sup.w.overall(&sup.w.health()).name(), "ok", "{:?}", sup.w.health());
    let _ = ka_child.kill();
    let _ = ka_child.wait();
    for (_, mut c) in children {
        let _ = c.kill();
        let _ = c.wait();
    }
}
