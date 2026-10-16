//! Bridge: run all of pimax-doctor's checks and map them to findings. Doctor fixes become catalogue `FixRef`s where
//! a catalogue entry exists; other doctor fixes stay as text in `detail` until they are migrated (ARCHITECTURE.md
//! "Migration").

use crate::check::{Area, Check, Ctx, Finding, FixRef, Severity};

pub struct DoctorBridge;

/// Area for each doctor check id.
pub fn area_of(id: &str) -> Area {
    match id {
        "headset" | "tobii" | "lens-conf" => Area::Device,
        "udev" | "keepalive" => Area::Usb,
        "gpu" | "patches" | "merged-head-sync" | "connector" | "kernel-log" => Area::Gpu,
        "lease" | "stale-service" => Area::Runtime,
        "setcap" => Area::Cpu,
        _ => Area::Drivers,
    }
}

/// Catalogue fix for a doctor check, when one exists.
fn fix_of(id: &str, cx: &Ctx) -> Option<FixRef> {
    let repo = cx.opts.linux_dir.parent().map(|p| p.display().to_string()).unwrap_or_default();
    Some(match id {
        "setcap" => {
            let s = crate::checks::cpu::monado_service(&cx.opts.monado_build).display().to_string();
            FixRef::new("cpu.setcap_monado", &[("path", &s)])
        }
        "udev" => FixRef::new("udev.install_rules", &[("repo", &repo)]),
        "keepalive" => FixRef::new("user.keepalive_enable", &[]),
        "patches" | "merged-head-sync" => FixRef::new("gpu.nvidia_patches_reapply", &[("repo", &repo)]),
        "kernel-log" => FixRef::new("system.reboot", &[]),
        "stale-service" => FixRef::new("runtime.stop_stale_monado", &[]),
        _ => return None,
    })
}

impl Check for DoctorBridge {
    fn id(&self) -> &'static str {
        "doctor"
    }
    fn area(&self) -> Area {
        Area::Drivers
    }
    fn run(&self, cx: &Ctx) -> Vec<Finding> {
        let r = pimax_doctor::run_checks(cx.host, cx.opts);
        r.checks
            .iter()
            .map(|c| {
                let severity = Severity::from(c.level);
                let mut detail = c.detail.clone();
                let fix = match (&c.fix, severity >= Severity::Warn) {
                    (Some(f), true) => {
                        let mapped = fix_of(c.id, cx);
                        if mapped.is_none() {
                            detail = format!("{detail}\ndoctor suggests: {}", f.why);
                        }
                        mapped
                    }
                    _ => None,
                };
                Finding { check: format!("doctor.{}", c.id), area: area_of(c.id), severity, title: c.title.into(), detail, fix }
            })
            .collect()
    }
}
