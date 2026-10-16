//! Headset models the app knows: the modules of `pimax-device` (one TOML descriptor per model). Only the Pimax Dream
//! Air (the SLAM version this stack drives) is real today; the other Pimax headsets are mocks that claim no USB id and
//! promise no capability, so the layout already has room for them. Nothing here talks to a device.
use crate::facts::Facts;
use pimax_device::Capabilities;
pub use pimax_device::{Descriptor, Detected, HeadsetModule, Status, Support};

/// Every module, the real ones first.
pub fn all() -> &'static [Box<dyn HeadsetModule>] {
    pimax_device::all()
}

/// A module by id.
pub fn by_id(id: &str) -> Option<&'static dyn HeadsetModule> {
    pimax_device::by_id(id)
}

/// The model the app drives when nothing else is known (the first real module).
pub fn default_module() -> &'static dyn HeadsetModule {
    all().iter().find(|m| !m.is_mock()).or_else(|| all().first()).map(|b| b.as_ref()).expect("pimax-device has modules")
}

/// Match a USB device list: the first device a module claims wins; else the first unverified candidate id of a mock
/// ("possibly <name>, not supported yet").
pub fn detect_usb(usb: &[(u16, u16)]) -> Detected {
    let mut possibly = None;
    for &(vid, pid) in usb {
        match pimax_device::detect(vid, pid) {
            Detected::Module(m) => return Detected::Module(m),
            Detected::Possibly(m) => {
                possibly.get_or_insert(m);
            }
            Detected::None => {}
        }
    }
    possibly.map_or(Detected::None, Detected::Possibly)
}

/// What is plugged in, from the facts' USB list.
pub fn detect(f: &Facts) -> Detected {
    detect_usb(&f.usb)
}

/// The connected model, when a module claims it.
pub fn connected(f: &Facts) -> Option<&'static dyn HeadsetModule> {
    match detect(f) {
        Detected::Module(m) => Some(m),
        _ => None,
    }
}

/// One line of a model's card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cap {
    /// What it is ("Display 7104x3840, 72 / 90 Hz").
    pub name: String,
    /// How far it works with this stack.
    pub support: Support,
}

fn head_tracking(kind: &str) -> &str {
    match kind {
        "slam" => "inside-out (SLAM)",
        "inside_out" => "inside-out",
        "lighthouse" => "Lighthouse",
        "imu3dof" => "3DoF (IMU)",
        _ => "",
    }
}

/// The capability lines of a descriptor (labels come from [`Support::label`]).
pub fn caps(d: &Descriptor) -> Vec<Cap> {
    let c: &Capabilities = &d.caps;
    let mut display = "Display".to_string();
    if let Some((w, h)) = c.mode {
        display += &format!(" {w}x{h}");
    }
    if !c.refresh_hz.is_empty() {
        display += &format!(", {} Hz", c.refresh_hz.iter().map(|h| h.to_string()).collect::<Vec<_>>().join(" / "));
    }
    let ht = head_tracking(&c.head_tracking);
    let ipd = match (c.ipd_motor, c.ipd_range_mm) {
        (true, Some((lo, hi))) => format!("IPD lens motor {lo:.0}-{hi:.0} mm"),
        _ => "IPD".to_string(),
    };
    vec![
        Cap { name: display, support: c.display },
        Cap { name: if ht.is_empty() { "Head tracking".into() } else { format!("Head tracking, {ht}") }, support: c.tracking },
        Cap { name: if c.pairing == "start_pair" { "Controllers (pairing in the app)".into() } else { "Controllers".into() }, support: c.controllers },
        Cap { name: "Eye tracking".into(), support: c.eye_tracking },
        Cap { name: ipd, support: c.ipd },
        Cap { name: if c.camera_count > 0 { format!("Cameras ({}) and passthrough", c.camera_count) } else { "Cameras and passthrough".into() }, support: c.cameras },
        Cap { name: "Audio".into(), support: c.audio },
    ]
}

/// The label of a capability on a card: a mock's unknown capabilities read "not supported yet".
pub fn support_label(s: Support, mock: bool) -> &'static str {
    match s {
        Support::Unknown if mock => "not supported yet",
        s => s.label(),
    }
}

/// The USB ids of a model for people ("34a4:0044"), empty for mocks.
pub fn usb_ids(d: &Descriptor) -> String {
    d.usb_pids.iter().map(|p| format!("{:04x}:{p:04x}", d.usb_vid)).collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_real_model_and_mocks_that_promise_nothing() {
        assert_eq!(all().iter().filter(|m| !m.is_mock()).count(), 1);
        let d = default_module().descriptor();
        assert_eq!(d.id, "dream-air");
        assert_eq!(usb_ids(d), "34a4:0044");
        assert!(caps(d).iter().any(|c| c.name.contains("72 / 90 Hz") && c.support == Support::Works));
        for m in all().iter().filter(|m| m.is_mock()) {
            let d = m.descriptor();
            assert!(caps(d).iter().all(|c| matches!(c.support, Support::Unknown | Support::None)), "{} promises a capability", d.name);
            assert!(d.usb_pids.is_empty() && usb_ids(d).is_empty());
            assert_eq!(support_label(Support::Unknown, true), "not supported yet");
        }
    }

    #[test]
    fn detection_from_the_usb_list() {
        let mut f = Facts::default();
        assert!(matches!(detect(&f), Detected::None));
        // a hub, then the Dream Air
        f.usb = vec![(0x1d6b, 0x0003), (0x34a4, 0x0044)];
        assert_eq!(connected(&f).map(|m| m.id().to_string()).as_deref(), Some("dream-air"));
        // only an unverified candidate id of a mock: "possibly"
        let mock = all().iter().find(|m| m.is_mock() && !m.descriptor().usb_pid_candidates.is_empty()).expect("a mock with candidates");
        let d = mock.descriptor();
        f.usb = vec![(d.usb_vid, d.usb_pid_candidates[0])];
        assert!(matches!(detect(&f), Detected::Possibly(_)));
        assert!(connected(&f).is_none());
        // the real one wins over a candidate seen first
        f.usb.push((0x34a4, 0x0044));
        assert!(matches!(detect(&f), Detected::Module(m) if m.id() == "dream-air"));
    }
}
