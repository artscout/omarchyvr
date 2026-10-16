//! Checks that run inside a job, right before the launcher is called: the facts on screen can be a minute old, and a
//! start first waits up to 45 s for a previous Monado. LESSONS rows 2 and 3 (camera and eye chip still free) and, for
//! a start with `--force`, row 9 (still only WayVR Xid 31 faults) plus the launcher checks that `--force` skips
//! (headset on USB, keepalive, display connector) and row 10 (merged_head_sync).
use crate::facts::{camera_nodes, kernel_events};
use crate::job::GuardFn;
use crate::kernel::{self, Verdict};
use crate::machine::Machine;
use crate::procs;
use omarchyvr_doctor::checks::{connectors, usb_devices};
use std::path::Path;
use std::sync::Arc;

/// Cameras (when `cameras`) and the eye chip (when `eye`) have no holder.
pub fn devices_free(cameras: bool, eye: bool) -> GuardFn {
    Arc::new(move |m: &dyn Machine, log: &mut dyn FnMut(String)| {
        if cameras {
            let nodes = camera_nodes(m);
            let h = procs::holders(m, &nodes);
            if !h.is_empty() {
                return Err(format!(
                    "camera busy (LESSONS row 2): {}",
                    h.iter().map(|x| x.describe()).collect::<Vec<_>>().join("; ")
                ));
            }
            log(format!("camera free ({} nodes)", nodes.len()));
        }
        if eye {
            let devs = usb_devices(m.host());
            if let Some(node) = devs
                .iter()
                .find(|d| d.vid == omarchyvr_doctor::TOBII_VID && d.pid == omarchyvr_doctor::TOBII_PID)
                .and_then(|d| d.node.clone())
            {
                let h = procs::holders(m, &[node]);
                if !h.is_empty() {
                    return Err(format!(
                        "eye chip busy (LESSONS row 3): {}",
                        h.iter().map(|x| x.describe()).collect::<Vec<_>>().join("; ")
                    ));
                }
                log("eye chip free".into());
            }
        }
        Ok(())
    })
}

/// What `--force` may skip, re-read now: abort unless the kernel log still has only WayVR Xid 31 faults and the
/// headset, the keepalive, the connector and merged_head_sync are fine.
pub fn force_still_justified() -> GuardFn {
    Arc::new(|m: &dyn Machine, log: &mut dyn FnMut(String)| {
        let ev = kernel_events(m).map_err(|e| format!("not starting with --force: {e}"))?;
        if let Verdict::Reboot(v) = kernel::verdict(&ev) {
            return Err(format!(
                "not starting with --force: the kernel log now has {} (LESSONS row 9: reboot first)",
                v.first().map(|e| e.summary()).unwrap_or_default()
            ));
        }
        let devs = usb_devices(m.host());
        if !devs.iter().any(|d| d.vid == omarchyvr_doctor::PIMAX_VID && d.pid == omarchyvr_doctor::DREAM_AIR_PID) {
            return Err("not starting with --force: the headset (34a4:0044) is not on USB".into());
        }
        let ka = m
            .run("systemctl", &["--user", "is-active", "omarchyvr-keepalived"])
            .map(|o| o.stdout.trim().to_string())
            .unwrap_or_default();
        if ka != "active" {
            return Err(format!("not starting with --force: omarchyvr-keepalived is {}", if ka.is_empty() { "unknown" } else { &ka }));
        }
        if !connectors(m.host()).iter().any(|c| c.is_pimax() && c.status == "connected") {
            return Err("not starting with --force: the headset's display connector is not connected".into());
        }
        let mhs = Path::new("/sys/module/nvidia_modeset/parameters/merged_head_sync");
        if m.exists(Path::new("/sys/module/nvidia_modeset")) && !matches!(m.read_str(mhs).as_deref(), Some("1") | Some("Y")) {
            return Err("not starting with --force: merged_head_sync is not 1 (LESSONS row 10)".into());
        }
        log("re-checked for --force: only WayVR Xid 31 in the kernel log; headset, keepalive, connector, merged_head_sync ok".into());
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo;

    #[test]
    fn force_is_only_justified_while_nothing_else_is_wrong() {
        let g = force_still_justified();
        let (m, _) = demo::idle_machine();
        let mut lines = Vec::new();
        assert_eq!(g(&m, &mut |l| lines.push(l)), Ok(()));
        // a new display engine error since the user agreed
        let (mut m, _) = demo::idle_machine();
        m.cmd(
            "journalctl -k -b --no-pager -q",
            0,
            &format!("{}Oct 14 18:00:00 mau kernel: nvidia-modeset: ERROR: GPU:0: Idling display engine timed out: 0x1\n", demo::KERNEL_LOG),
        );
        assert!(g(&m, &mut |_| {}).unwrap_err().contains("row 9"));
        // headset unplugged: --force would have skipped that launcher check
        let (mut m, _) = demo::idle_machine();
        m.host.files.retain(|p, _| !p.starts_with("/sys/bus/usb/devices/1-5.2"));
        assert!(g(&m, &mut |_| {}).unwrap_err().contains("34a4:0044"));
        let (mut m, _) = demo::idle_machine();
        m.cmd("systemctl --user is-active omarchyvr-keepalived", 3, "inactive\n");
        assert!(g(&m, &mut |_| {}).unwrap_err().contains("inactive"));
        let (mut m, _) = demo::idle_machine();
        m.file("/sys/class/drm/card1-DP-1/status", "disconnected\n");
        assert!(g(&m, &mut |_| {}).unwrap_err().contains("connector"));
        let (mut m, _) = demo::idle_machine();
        m.file("/sys/module/nvidia_modeset/parameters/merged_head_sync", "0\n");
        assert!(g(&m, &mut |_| {}).unwrap_err().contains("merged_head_sync"));
    }

    #[test]
    fn devices_free_checks_camera_and_eye() {
        let g = devices_free(true, true);
        let (m, _) = demo::idle_machine();
        assert_eq!(g(&m, &mut |_| {}), Ok(()));
        let (mut m, _) = demo::idle_machine();
        m.process(4242, "pimax-cam", "pimax-cam broker", &[], 1, &["/dev/video1"]);
        assert!(g(&m, &mut |_| {}).unwrap_err().contains("camera busy"));
        let (mut m, _) = demo::idle_machine();
        m.process(4243, "pimax-eye", "pimax-eye watch", &[], 1, &["/dev/bus/usb/001/009"]);
        assert!(g(&m, &mut |_| {}).unwrap_err().contains("eye chip busy"));
        assert_eq!(devices_free(false, false)(&m, &mut |_| {}), Ok(()));
    }
}
