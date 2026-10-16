//! GPU checks.

use crate::check::{Area, Check, Ctx, Finding, FixRef, Severity};
use std::path::Path;

/// `gpu.merged_head_sync`: the patched nvidia-modeset exposes `merged_head_sync` and it is 1 (ported from
/// omarchyvr-doctor `nvidia` / `merged-head-sync`). Only for modules whose display needs the 2Heads1OR patches.
pub struct MergedHeadSync;

const PARAM: &str = "/sys/module/nvidia_modeset/parameters/merged_head_sync";

impl Check for MergedHeadSync {
    fn id(&self) -> &'static str {
        "gpu.merged_head_sync"
    }
    fn area(&self) -> Area {
        Area::Gpu
    }
    fn run(&self, cx: &Ctx) -> Vec<Finding> {
        let title = "NVIDIA merge mode (merged_head_sync)";
        let needed = !cx.module.is_some_and(|m| !m.descriptor().caps.display_needs.iter().any(|n| n == "nvidia_2heads1or_patches"));
        if !needed {
            return vec![self.finding(Severity::Info, title, "not needed by this headset", None)];
        }
        if !cx.host.exists(Path::new("/sys/module/nvidia_modeset")) {
            return vec![self.finding(Severity::Info, title, "nvidia-modeset not loaded (not an NVIDIA system?)", None)];
        }
        let repo = cx.opts.linux_dir.parent().map(|p| p.display().to_string()).unwrap_or_default();
        let fix = Some(FixRef::new("gpu.nvidia_patches_reapply", &[("repo", &repo)]));
        match cx.host.read_str(Path::new(PARAM)).as_deref() {
            Some("1") => vec![self.finding(Severity::Ok, title, "merged_head_sync = 1", None)],
            Some(v) => vec![self.finding(Severity::Fail, title, format!("merged_head_sync = {v}; the flip-fix patch is loaded but off"), fix)],
            None => vec![self.finding(Severity::Fail, title, "parameter missing: the patched nvidia-modeset is not loaded", fix)],
        }
    }
}
