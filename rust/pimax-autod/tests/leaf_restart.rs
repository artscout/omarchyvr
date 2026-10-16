//! P5 acceptance with fake processes: a fake desk session (sleep processes named like the real binaries through
//! symlinks, a fake sysfs, a temp session dir; the real /proc), adopted by the Supervisor; `perf` dies with SIGABRT,
//! the reconciler restarts it after the leaf backoff by respawning the argv it captured, through the real executor.
//! Touches nothing outside the temp dir and its own children.
use pimax_autod::executor::{self, Ctx};
use pimax_autod::supervisor::{Captured, Supervisor};
use pimax_components::gpu::Gate;
use pimax_components::model::Mode;
use pimax_components::probe::{probe_all, session_facts, Opts};
use pimax_components::reconcile::Act;
use pimax_components::state::{Switches, World};
use pimax_components::util::{put, Roots, TempDir};
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
    put(&r.runtime, "pimax-cam-broker.sock", "");
    put(&r.home, ".config/openxr/1/active_runtime.json", "{\"runtime\":{\"name\":\"Monado\"}}");
    let bin = base.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let mut kids = Kids(vec![]);
    let mut children: Vec<(&str, Child)> = Vec::new();
    for (name, file) in [("monado-service", "monado.pid"), ("pimax-cam", "broker.pid"), ("wayvr", "wayvr.pid"), ("pimax-perf", "perf.pid")] {
        let link = bin.join(name);
        std::os::unix::fs::symlink(sleep_bin(), &link).unwrap();
        let c = Command::new(&link).arg("300").spawn().unwrap();
        kids.0.push(c.id());
        put(&r.session_dir, file, &format!("{}\n", c.id()));
        children.push((name, c));
    }
    let mut w = World::new(Default::default(), Default::default(), Switches { enabled: true, ..Default::default() }, Gate::new("t"), Default::default());
    w.desired.sixdof = true;
    let mut sup = Supervisor::new(w);
    let round = |sup: &mut Supervisor, t: f64| {
        let obs = probe_all(&r, &Opts::default(), pimax_components::model::GateLevel::Ok);
        for (id, o) in &obs {
            if let (Some(true), Some(pid)) = (o.up, o.pid) {
                let argv = pimax_components::procs::cmdline(&r.proc_, pid);
                sup.captured.insert(id, Captured { argv, env: pimax_components::procs::environ(&r.proc_, pid), cwd: None });
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
    // perf aborts
    let (_, mut perf) = children.pop().unwrap();
    // SAFETY: our own child.
    unsafe { libc::kill(perf.id() as i32, libc::SIGABRT) };
    let st = perf.wait().unwrap();
    assert_eq!(st.signal(), Some(libc::SIGABRT));
    sup.w.exit_status.insert(old, (None, Some(libc::SIGABRT)));
    let d = round(&mut sup, 1.0);
    assert!(d.run.is_none(), "leaf backoff 2 s");
    assert_eq!(sup.w.c("perf").last_exit.as_ref().unwrap().1, pimax_components::policy::ExitClass::Crash);
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
    let new = pimax_components::procs::read_pid(&r.session_dir.join("perf.pid")).unwrap();
    kids.0.push(new);
    assert_eq!(o, pimax_components::state::Outcome::Ok);
    sup.finished(&o, 4.0);
    assert_ne!(new, old);
    assert_eq!(pimax_components::procs::info(&r.proc_, new).unwrap().comm, "pimax-perf");
    let _ = round(&mut sup, 6.0);
    assert_eq!(sup.w.c("perf").up, Some(true));
    assert_eq!(sup.w.c("perf").restart.spent.len(), 1, "one restart spent");
    assert_eq!(sup.w.overall(&sup.w.health()).name(), "ok", "{:?}", sup.w.health());
    for (_, mut c) in children {
        let _ = c.kill();
        let _ = c.wait();
    }
}
