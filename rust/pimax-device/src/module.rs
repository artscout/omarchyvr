//! The `HeadsetModule` trait and the compiled-in registry.
//!
//! A module is its descriptor (data) plus a few methods for what data cannot express (slot mapping, value
//! conversions, quirks). Modules are compiled in, one directory each under `devices/`, no dynamic loading.

use crate::descriptor::{Descriptor, Status};
use crate::hid::Refused;
use std::sync::OnceLock;

/// Controller hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hand {
    Left,
    Right,
}

/// What every headset module offers. Everything that sends bytes returns the report only; sending goes through
/// [`crate::hid::Gate`] with the module's allowlist, so the allowlist is checked twice (build and send).
pub trait HeadsetModule: Send + Sync {
    fn descriptor(&self) -> &Descriptor;

    fn id(&self) -> &str {
        &self.descriptor().id
    }

    fn is_mock(&self) -> bool {
        self.descriptor().status == Status::Mock
    }

    /// Lens IPD motor report for `mm`, clamped to the descriptor's range. Refused when the model has no motor.
    fn ipd_report(&self, mm: f32) -> Result<Vec<u8>, Refused> {
        let d = self.descriptor();
        let (lo, hi) = d.caps.ipd_range_mm.ok_or_else(|| Refused::UnknownCommand("ipd_motor".into()))?;
        if !mm.is_finite() {
            return Err(Refused::OutOfRange { cmd: "ipd_motor".into(), param: "ipd".into(), value: 0 });
        }
        let v = (mm.clamp(lo, hi) * 100.0).round() as u32;
        d.allowlist.build("ipd_motor", &[("ipd", v)])
    }

    /// Controller pairing report for one hand. Default: not supported.
    fn pair_report(&self, _hand: Hand) -> Result<Vec<u8>, Refused> {
        Err(Refused::UnknownCommand("start_pair".into()))
    }

    /// Known quirks, shown in the GUI and the diagnostic bundle.
    fn quirks(&self) -> &'static [&'static str] {
        &[]
    }
}

/// A module that is only its descriptor (every mock).
pub struct DataModule(pub Descriptor);

impl HeadsetModule for DataModule {
    fn descriptor(&self) -> &Descriptor {
        &self.0
    }
}

/// Descriptor sources, compiled in. Add a line here when adding `devices/<id>/`.
pub const SOURCES: [(&str, &str); 5] = [
    ("dream-air", include_str!("../devices/dream-air/device.toml")),
    ("dream-air-se", include_str!("../devices/dream-air-se/device.toml")),
    ("crystal-light", include_str!("../devices/crystal-light/device.toml")),
    ("crystal-super", include_str!("../devices/crystal-super/device.toml")),
    ("8kx", include_str!("../devices/8kx/device.toml")),
];

/// Every module, real ones first. Descriptors are validated by the tests, so a parse failure here is a build bug.
pub fn all() -> &'static [Box<dyn HeadsetModule>] {
    static ALL: OnceLock<Vec<Box<dyn HeadsetModule>>> = OnceLock::new();
    ALL.get_or_init(|| {
        let mut v: Vec<Box<dyn HeadsetModule>> = Vec::new();
        for (id, src) in SOURCES {
            let d = Descriptor::parse(src).unwrap_or_else(|e| panic!("devices/{id}/device.toml: {e}"));
            assert_eq!(d.id, id, "devices/{id}: id mismatch");
            v.push(match id {
                "dream-air" => Box::new(crate::devices::dream_air::DreamAir(d)),
                _ => Box::new(DataModule(d)),
            });
        }
        v.sort_by_key(|m| m.is_mock());
        v
    })
}

pub fn by_id(id: &str) -> Option<&'static dyn HeadsetModule> {
    all().iter().find(|m| m.id() == id).map(|b| b.as_ref())
}

/// Result of matching a USB device.
pub enum Detected {
    /// A module claims these ids.
    Module(&'static dyn HeadsetModule),
    /// Only an unverified candidate id of a mock: show "possibly <name>, not supported yet".
    Possibly(&'static dyn HeadsetModule),
    None,
}

/// Match one USB vid:pid against the modules.
pub fn detect(vid: u16, pid: u16) -> Detected {
    if let Some(m) = all().iter().find(|m| m.descriptor().matches_usb(vid, pid)) {
        return Detected::Module(m.as_ref());
    }
    match all().iter().find(|m| {
        let d = m.descriptor();
        d.usb_vid == vid && d.usb_pid_candidates.contains(&pid)
    }) {
        Some(m) => Detected::Possibly(m.as_ref()),
        None => Detected::None,
    }
}
