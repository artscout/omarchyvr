//! What the device cards and the status bar show (headset, left and right controller, tracking mode), derived
//! from the live state and the facts. Controllers only report through a running Monado; their battery is not in
//! Monado's log, so it is shown as unknown (demo data marks its values as mock).
use crate::facts::{Facts, Live};
use crate::launcher::DeskToggles;
use crate::monlog::TrackState;

/// The state of a device (the colour of its pill).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dev {
    /// Not there / not running.
    Off,
    /// Connected, idle.
    Connected,
    /// Streaming / tracked.
    Active,
    /// Starting, searching.
    Searching,
    /// Something is wrong.
    Warn,
}

/// A battery reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Battery {
    /// Not reported.
    Unknown,
    /// Demo value (marked as mock).
    Mock(u8),
    /// Reported percent.
    Level(u8),
}

/// One device card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceView {
    /// Name.
    pub name: String,
    /// State.
    pub state: Dev,
    /// Pill text.
    pub label: String,
    /// One line under it.
    pub detail: String,
    /// Battery (controllers).
    pub battery: Option<Battery>,
}

/// Everything the right column and the status bar show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Devices {
    /// Headset.
    pub headset: DeviceView,
    /// Left controller.
    pub left: DeviceView,
    /// Right controller.
    pub right: DeviceView,
    /// Tracking mode badge.
    pub tracking: (String, Dev),
}

/// Controller slots (`pimax_proto::SLOT_LEFT` / `SLOT_RIGHT`).
pub const SLOT_LEFT: u32 = 2;
/// Right controller slot.
pub const SLOT_RIGHT: u32 = 3;

fn controller(name: &str, slot: u32, hand: usize, live: &Live, demo: bool) -> DeviceView {
    let c = &live.log.ctrl;
    let running = live.service.is_some();
    let (state, label, detail) = if !running {
        (Dev::Off, "Not active".to_string(), "Controllers connect through a running desk or VR session".to_string())
    } else if c.slots_seen.contains(&slot) {
        let hands = match c.hands[hand] {
            Some(true) => ", hand tracking on",
            _ => "",
        };
        if c.optical && c.tracking {
            (Dev::Active, "Tracked".into(), format!("Optical 6DoF tracking{hands}"))
        } else if c.optical {
            (Dev::Searching, "Connected".into(), format!("Optical tracking searching{hands}"))
        } else {
            (Dev::Connected, "Connected".into(), format!("3DoF{hands}"))
        }
    } else {
        (Dev::Searching, "Not seen yet".into(), "Wake it with a button press".into())
    };
    let battery = if demo {
        Battery::Mock(if hand == 0 { 80 } else { 65 })
    } else {
        Battery::Unknown
    };
    DeviceView {
        name: name.into(),
        state,
        label,
        detail,
        battery: Some(battery),
    }
}

/// The device views.
pub fn devices(f: &Facts, live: &Live, t: &DeskToggles, demo: bool) -> Devices {
    let module = crate::headsets::connected(f).unwrap_or_else(crate::headsets::default_module);
    let model = module.descriptor().name.as_str();
    let headset = match (&live.service, f.headset) {
        (Some(s), _) => {
            let hz = live.log.video_hz.or(s.hz).map(|h| format!(" {h} Hz")).unwrap_or_default();
            DeviceView {
                name: model.into(),
                state: Dev::Active,
                label: format!("Streaming{hz}"),
                detail: if live.desk_clients_alive() { "Desk mode".into() } else { "VR mode".into() },
                battery: None,
            }
        }
        (None, true) if !f.usb.contains(&(pimax_doctor::PIMAX_VID, crate::reset::CAMERA_PID)) => DeviceView {
            name: model.into(),
            state: Dev::Warn,
            label: "Camera missing".into(),
            detail: "The headset's camera did not start: Restart headset (Home) brings it back".into(),
            battery: None,
        },
        (None, true) => DeviceView {
            name: model.into(),
            state: Dev::Connected,
            label: "Connected".into(),
            detail: if f.tobii { "Eye tracker found".into() } else { "Eye tracker not found".into() },
            battery: None,
        },
        (None, false) if f.at == 0 => DeviceView {
            name: model.into(),
            state: Dev::Searching,
            label: "Checking".into(),
            detail: String::new(),
            battery: None,
        },
        (None, false) => DeviceView {
            name: model.into(),
            state: Dev::Off,
            label: "Not connected".into(),
            detail: format!("USB {} not found", crate::headsets::usb_ids(module.descriptor())),
            battery: None,
        },
    };
    let tracking = match &live.service {
        Some(s) if s.sixdof => match live.log.sixdof.state {
            TrackState::Tracking => ("INSIDE-OUT 6DoF".to_string(), Dev::Active),
            TrackState::Lost => ("6DoF LOST".to_string(), Dev::Warn),
            TrackState::Waiting => ("6DoF STARTING".to_string(), Dev::Searching),
            TrackState::Off => ("3DoF".to_string(), Dev::Connected),
        },
        Some(_) => ("3DoF".to_string(), Dev::Connected),
        None if t.sixdof => ("INSIDE-OUT".to_string(), Dev::Off),
        None => ("3DoF".to_string(), Dev::Off),
    };
    Devices {
        headset,
        left: controller("Left Controller", SLOT_LEFT, 0, live, demo),
        right: controller("Right Controller", SLOT_RIGHT, 1, live, demo),
        tracking,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::ServiceInfo;

    #[test]
    fn idle_running_and_tracked() {
        let mut f = Facts { at: 1, ..Default::default() };
        let mut live = Live::default();
        let t = DeskToggles::default();
        let d = devices(&f, &live, &t, false);
        assert_eq!((d.headset.state, d.headset.label.as_str()), (Dev::Off, "Not connected"));
        assert_eq!(d.left.state, Dev::Off);
        assert_eq!(d.left.battery, Some(Battery::Unknown));
        assert_eq!(d.tracking, ("INSIDE-OUT".to_string(), Dev::Off));
        f.headset = true;
        f.usb = vec![(0x34a4, 0x0044)];
        let h = devices(&f, &live, &t, false).headset;
        assert_eq!((h.state, h.label.as_str()), (Dev::Warn, "Camera missing"));
        assert!(!h.detail.to_lowercase().contains("unplug"));
        f.usb.push((0x34a4, 0x0014));
        assert_eq!(devices(&f, &live, &t, false).headset.state, Dev::Connected);
        live.service = Some(ServiceInfo { pid: 1, age: None, build: None, hz: Some(72), eye: false, sixdof: true, ctrl_track: true, broker: true, ipd: None, anchor: false });
        live.log.sixdof.state = TrackState::Tracking;
        live.log.ctrl.slots_seen.insert(SLOT_RIGHT);
        live.log.ctrl.optical = true;
        live.log.ctrl.tracking = true;
        let d = devices(&f, &live, &t, true);
        assert_eq!(d.headset.label, "Streaming 72 Hz");
        assert_eq!(d.tracking.0, "INSIDE-OUT 6DoF");
        assert_eq!(d.right.label, "Tracked");
        assert_eq!(d.left.label, "Not seen yet");
        assert_eq!(d.left.battery, Some(Battery::Mock(80)), "demo values are marked");
    }
}
