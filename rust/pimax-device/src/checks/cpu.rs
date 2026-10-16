//! CPU checks.

use crate::check::{Area, Check, Ctx, Finding, FixRef, Severity};
use std::path::Path;

/// `cpu.cap_sys_nice`: monado-service carries cap_sys_nice (ported from omarchyvr-doctor `setcap`, checks.rs:716).
pub struct CapSysNice;

/// Path of monado-service inside a build dir.
pub fn monado_service(build: &Path) -> std::path::PathBuf {
    build.join("src/xrt/targets/service/monado-service")
}

impl Check for CapSysNice {
    fn id(&self) -> &'static str {
        "cpu.cap_sys_nice"
    }
    fn area(&self) -> Area {
        Area::Cpu
    }
    fn run(&self, cx: &Ctx) -> Vec<Finding> {
        let svc = monado_service(&cx.opts.monado_build);
        let s = svc.display().to_string();
        let title = "monado-service has cap_sys_nice";
        if !cx.host.exists(&svc) {
            return vec![self.finding(Severity::Fail, title, format!("{s} not found (PIMAX_MONADO_BUILD / setup.toml monado_build)"), None)];
        }
        match cx.host.run("getcap", &[&s]) {
            Some(c) if c.stdout.contains("cap_sys_nice") => vec![self.finding(Severity::Ok, title, format!("{s} has cap_sys_nice"), None)],
            Some(_) => vec![self.finding(
                Severity::Fail,
                title,
                format!("{s} lacks cap_sys_nice (realtime compositor thread; lost on every relink)"),
                Some(FixRef::new("cpu.setcap_monado", &[("path", &s)])),
            )],
            None => vec![self.finding(Severity::Warn, title, "getcap is not installed (libcap)", None)],
        }
    }
}

/// `cpu.governor`: the scaling governor of every CPU (new; informational).
pub struct Governor;

impl Check for Governor {
    fn id(&self) -> &'static str {
        "cpu.governor"
    }
    fn area(&self) -> Area {
        Area::Cpu
    }
    fn run(&self, cx: &Ctx) -> Vec<Finding> {
        let title = "CPU frequency governor";
        let root = Path::new("/sys/devices/system/cpu");
        let mut govs: Vec<String> = cx
            .host
            .list(root)
            .into_iter()
            .filter(|n| n.strip_prefix("cpu").is_some_and(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_digit())))
            .filter_map(|n| cx.host.read_str(&root.join(&n).join("cpufreq/scaling_governor")))
            .collect();
        let cores = govs.len();
        if cores == 0 {
            return vec![self.finding(Severity::Info, title, "no cpufreq data (VM or no driver)", None)];
        }
        govs.sort();
        govs.dedup();
        let detail = format!("{cores} CPUs: {}", govs.join(", "));
        if govs.iter().all(|g| g == "performance") {
            vec![self.finding(Severity::Ok, title, detail, None)]
        } else if govs.iter().any(|g| g == "powersave") {
            // Under intel_pstate / amd-pstate EPP "powersave" still boosts; a hint, never a failure.
            let prev = govs.iter().find(|g| *g != "performance").cloned().unwrap_or_default();
            vec![self.finding(
                Severity::Info,
                title,
                format!("{detail} (fine with pstate EPP drivers; switch only if you see CPU frame-time spikes)"),
                Some(FixRef::new("cpu.governor_performance", &[("previous", &prev)])),
            )]
        } else {
            vec![self.finding(Severity::Ok, title, detail, None)]
        }
    }
}
