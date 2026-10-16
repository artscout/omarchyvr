//! The example checks against omarchyvr-doctor's fake host (no real machine access).

use pimax_device::check::{run_all, Ctx, Severity};
use pimax_device::checks::{cpu, default_checks, doctor::DoctorBridge, gpu};
use pimax_device::{by_id, Check};
use omarchyvr_doctor::fake::FakeHost;
use omarchyvr_doctor::Opts;
use std::path::PathBuf;

const SVC: &str = "/home/u/src/monado/build/src/xrt/targets/service/monado-service";

fn opts() -> Opts {
    Opts {
        monado_build: "/home/u/src/monado/build".into(),
        linux_dir: "/repo/linux".into(),
        config_root: PathBuf::from("/nonexistent-pimax-config"),
        cache_root: PathBuf::from("/nonexistent-pimax-cache"),
        nvidia_src_root: "/usr/src".into(),
        connector: None,
    }
}

fn one(c: &dyn Check, h: &FakeHost) -> pimax_device::Finding {
    let o = opts();
    let cx = Ctx { host: h, opts: &o, module: by_id("dream-air") };
    let mut f = c.run(&cx);
    assert_eq!(f.len(), 1);
    f.remove(0)
}

#[test]
fn cap_sys_nice_ok_then_missing_with_root_fix() {
    let mut h = FakeHost::healthy();
    assert_eq!(one(&cpu::CapSysNice, &h).severity, Severity::Ok);
    h.cmd(&format!("getcap {SVC}"), 0, "");
    let f = one(&cpu::CapSysNice, &h);
    assert_eq!(f.severity, Severity::Fail);
    let p = f.fix.unwrap().plan().unwrap();
    assert!(p.fix.root);
    assert_eq!(p.commands, vec![format!("sudo setcap cap_sys_nice+ep {SVC}")]);
    assert_eq!(p.undo, vec![format!("sudo setcap -r {SVC}")]);
}

#[test]
fn merged_head_sync() {
    let mut h = FakeHost::healthy();
    assert_eq!(one(&gpu::MergedHeadSync, &h).severity, Severity::Ok);
    h.file("/sys/module/nvidia_modeset/parameters/merged_head_sync", "0\n");
    let f = one(&gpu::MergedHeadSync, &h);
    assert_eq!(f.severity, Severity::Fail);
    let p = f.fix.unwrap().plan().unwrap();
    assert!(p.fix.reboot && p.commands[0].contains("/repo/linux/nvidia-patch-reapply.sh"));
    // a headset that does not need the patches (mock: no display needs) gets Info
    let o = opts();
    let cx = Ctx { host: &h, opts: &o, module: by_id("crystal-light") };
    assert_eq!(gpu::MergedHeadSync.run(&cx)[0].severity, Severity::Info);
}

#[test]
fn governor() {
    let mut h = FakeHost::default();
    assert_eq!(one(&cpu::Governor, &h).severity, Severity::Info);
    for i in 0..4 {
        h.file(&format!("/sys/devices/system/cpu/cpu{i}/cpufreq/scaling_governor"), "performance\n");
    }
    h.file("/sys/devices/system/cpu/cpufreq/policy0/x", "");
    let f = one(&cpu::Governor, &h);
    assert_eq!((f.severity, f.detail.as_str()), (Severity::Ok, "4 CPUs: performance"));
    h.file("/sys/devices/system/cpu/cpu3/cpufreq/scaling_governor", "powersave\n");
    let f = one(&cpu::Governor, &h);
    assert_eq!(f.severity, Severity::Info);
    let p = f.fix.unwrap().plan().unwrap();
    assert_eq!(p.undo, vec!["sudo cpupower frequency-set -g powersave"]);
}

#[test]
fn doctor_bridge_maps_every_doctor_check() {
    let o = opts();
    let mut h = FakeHost::healthy();
    let cx = Ctx { host: &h, opts: &o, module: by_id("dream-air") };
    let f = DoctorBridge.run(&cx);
    let doc = omarchyvr_doctor::run_checks(&h, &o);
    assert_eq!(f.len(), doc.checks.len());
    assert!(f.iter().all(|x| x.check.starts_with("doctor.") && x.fix.is_none()));
    // break setcap: the doctor's free-text fix becomes the catalogue fix
    h.cmd(&format!("getcap {SVC}"), 0, "");
    let cx = Ctx { host: &h, opts: &o, module: by_id("dream-air") };
    let f = DoctorBridge.run(&cx);
    let s = f.iter().find(|x| x.check == "doctor.setcap").unwrap();
    assert_eq!(s.severity, Severity::Fail);
    assert_eq!(s.fix.as_ref().unwrap().id, "cpu.setcap_monado");
    assert!(s.fix.as_ref().unwrap().plan().is_ok());
}

#[test]
fn default_checks_run_on_a_healthy_machine() {
    let o = opts();
    let h = FakeHost::healthy();
    let cx = Ctx { host: &h, opts: &o, module: by_id("dream-air") };
    let checks = default_checks();
    let f = run_all(&checks, &cx);
    assert!(f.iter().all(|x| x.severity <= Severity::Ok), "{f:#?}");
}
