//! Pimax Dream Air (SLAM): the real module. Data in `devices/dream-air/device.toml`; code here only for what the
//! descriptor cannot say.

use crate::descriptor::Descriptor;
use crate::hid::Refused;
use crate::module::{Hand, HeadsetModule};

pub struct DreamAir(pub Descriptor);

/// Controller slot per hand (pimax-proto/src/lib.rs:49-51).
pub fn slot(h: Hand) -> u8 {
    match h {
        Hand::Left => 2,
        Hand::Right => 3,
    }
}

/// One camera auto-exposure setting (same layout as pimax-cam's `AeSetting`, ae.rs:118-129).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ae {
    pub exposure: [u16; 4],
    pub gain: [u16; 4],
    pub led_exposure: u16,
    pub led_gain: u16,
}

impl DreamAir {
    /// The 0x51 camera exposure push, refused outside the Windows ranges.
    pub fn ae_report(&self, s: &Ae) -> Result<Vec<u8>, Refused> {
        let n = ["e0", "e1", "e2", "e3", "g0", "g1", "g2", "g3", "led_e", "led_g"];
        let v = [s.exposure[0], s.exposure[1], s.exposure[2], s.exposure[3], s.gain[0], s.gain[1], s.gain[2], s.gain[3], s.led_exposure, s.led_gain];
        let vals: Vec<(&str, u32)> = n.iter().copied().zip(v.iter().map(|&x| u32::from(x))).collect();
        self.0.allowlist.build("camera_ae", &vals)
    }

    /// Keepalive for a refresh rate (0 = stop announcing).
    pub fn keepalive_report(&self, hz: u8) -> Result<Vec<u8>, Refused> {
        self.0.allowlist.build("keepalive", &[("rate", u32::from(hz))])
    }
}

impl HeadsetModule for DreamAir {
    fn descriptor(&self) -> &Descriptor {
        &self.0
    }

    fn pair_report(&self, hand: Hand) -> Result<Vec<u8>, Refused> {
        self.0.allowlist.build("start_pair", &[("slot", u32::from(slot(hand)))])
    }

    fn quirks(&self) -> &'static [&'static str] {
        &[
            "IF1-IF3 hidraw must stay open and drained or the headset detaches every ~5.4 s (keepalive service)",
            "NVIDIA needs the 2Heads1OR patches, merged_head_sync=1 and the non-desktop EDID override",
            "Only one process may stream the cameras: use pimax-cam broker for several consumers",
            "The eye tracker can be opened by one process only",
            "The microphone sends zeros while the display is off",
            "Camera AE only with Monado builds that have the IMU-queue fix (slam d3138d4)",
        ]
    }
}
