//! USB and display presence from sysfs (one implementation for the daemon, the CLI and the GUI). Read-only:
//! never a write, a hotplug or a HID byte.
use std::path::Path;

pub const VID: &str = "34a4";
/// Headset HID of the Dream Air (every model: [`headset`]).
pub const PID_HID: &str = "0044";
/// Tracking cameras (USB3, behind the 05e3:0620 hub).
pub const PID_CAM: &str = "0014";
/// Eye tracker / display-bridge MCU of the Dream Air (every model: [`Headset::bridge`]).
pub const PID_EYE: &str = "0043";
/// The headset's USB3 hub.
pub const HUB: (&str, &str) = ("05e3", "0620");

fn rd(p: &Path) -> String {
    std::fs::read_to_string(p).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// A device `vid:pid` is under `<sys>/bus/usb/devices`.
pub fn present(sys: &Path, vid: &str, pid: &str) -> bool {
    let Ok(rdir) = std::fs::read_dir(sys.join("bus/usb/devices")) else { return false };
    rdir.flatten().any(|e| rd(&e.path().join("idVendor")) == vid && rd(&e.path().join("idProduct")) == pid)
}

/// The headset model on the bus, with its USB ids as sysfs spells them (`0044`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Headset {
    /// Name shown to the user.
    pub name: &'static str,
    /// HID chip product id.
    pub hid: String,
    /// Display bridge MCU product id.
    pub bridge: String,
}

/// The first model of `pimax_proto::MODELS` whose HID chip is on the bus.
pub fn headset(sys: &Path) -> Option<Headset> {
    pimax_proto::MODELS.iter().find_map(|m| {
        let hid = format!("{:04x}", m.pid);
        present(sys, VID, &hid).then(|| Headset { name: m.name, hid, bridge: format!("{:04x}", m.pid_bridge) })
    })
}

/// `class/video4linux` has a node whose USB device is the camera (the node's `device` is the interface; its parent
/// carries idVendor/idProduct).
pub fn camera_node(sys: &Path) -> bool {
    let Ok(rdir) = std::fs::read_dir(sys.join("class/video4linux")) else { return false };
    rdir.flatten().any(|e| {
        let dev = e.path().join("device");
        [dev.join(".."), dev].iter().any(|d| rd(&d.join("idVendor")) == VID && rd(&d.join("idProduct")) == PID_CAM)
    })
}

/// The EDID is a Pimax headset's: PNP `PVR` = 0xD242 little-endian (Dream Air product 0x1019; the Crystal Super's
/// product code is not recorded yet, so any product of that vendor counts).
pub fn is_headset_edid(e: &[u8]) -> bool {
    e.len() >= 12 && e[..8] == [0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0] && e[8..10] == [0x42, 0xd2]
}

/// The headset's DRM connector: (`card1-DP-1`, connected). The one whose EDID is the headset's; when none is (the
/// panel is off), the connector named `fallback` (`DP-1`, `$PIMAX_CONNECTOR`).
pub fn headset_connector(sys: &Path, fallback: &str) -> Option<(String, bool)> {
    let Ok(rdir) = std::fs::read_dir(sys.join("class/drm")) else { return None };
    let mut fb = None;
    let mut entries: Vec<_> = rdir.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some((_, conn)) = name.strip_prefix("card").and_then(|r| r.split_once('-')) else { continue };
        let status = rd(&e.path().join("status"));
        if status.is_empty() {
            continue;
        }
        let connected = status == "connected";
        if is_headset_edid(&std::fs::read(e.path().join("edid")).unwrap_or_default()) {
            return Some((name, connected));
        }
        if conn == fallback && fb.is_none() {
            fb = Some((name, connected));
        }
    }
    fb
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::{put, TempDir};

    pub fn fake_usb(sys: &Path, port: &str, vid: &str, pid: &str) {
        put(sys, &format!("bus/usb/devices/{port}/idVendor"), &format!("{vid}\n"));
        put(sys, &format!("bus/usb/devices/{port}/idProduct"), &format!("{pid}\n"));
    }

    #[test]
    fn finds_devices_camera_node_and_connector() {
        let t = TempDir::new("usb");
        let s = t.path();
        assert!(!present(s, VID, PID_HID));
        fake_usb(s, "2-4", VID, PID_HID);
        fake_usb(s, "1-1", "046d", "c52b");
        assert!(present(s, VID, PID_HID));
        assert!(!present(s, VID, PID_CAM));
        assert!(!camera_node(s));
        put(s, "class/video4linux/video0/device/../idVendor", "34a4\n");
        put(s, "class/video4linux/video0/idProduct", "");
        // device/.. resolves inside video0 in this fake tree: the parent dir carries the ids
        put(s, "class/video4linux/video0/idVendor", "34a4\n");
        put(s, "class/video4linux/video0/idProduct", "0014\n");
        assert!(camera_node(s));
        put(s, "class/drm/card1-DP-1/status", "disconnected\n");
        put(s, "class/drm/card1-DP-2/status", "connected\n");
        put(s, "class/drm/card1-HDMI-A-1/status", "connected\n");
        assert_eq!(headset_connector(s, "DP-1"), Some(("card1-DP-1".into(), false)));
        let mut edid = vec![0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0, 0x42, 0xd2, 0x19, 0x10];
        edid.resize(128, 0);
        std::fs::write(s.join("class/drm/card1-DP-2/edid"), &edid).unwrap();
        assert_eq!(headset_connector(s, "DP-1"), Some(("card1-DP-2".into(), true)));
    }
}
