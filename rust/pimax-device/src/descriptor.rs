//! The device descriptor: what a headset is and can do, read from `devices/<id>/device.toml`.

use crate::hid::{Allowlist, CommandSpec, Denied, SpecError};
use crate::toml_lite::{self, Section};

/// How far a capability works with this stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    Works,
    Experimental,
    /// Known not to exist on this model.
    None,
    /// Not known yet (every mock capability).
    Unknown,
}

impl Support {
    fn parse(s: Option<&str>) -> Result<Support, SpecError> {
        Ok(match s.unwrap_or("unknown") {
            "works" => Support::Works,
            "experimental" => Support::Experimental,
            "none" => Support::None,
            "unknown" => Support::Unknown,
            o => return Err(SpecError(format!("support {o:?} (works|experimental|none|unknown)"))),
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Support::Works => "works",
            Support::Experimental => "experimental",
            Support::None => "not on this model",
            Support::Unknown => "unknown",
        }
    }
}

/// Real (driven by the stack) or mock (placeholder).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Real,
    Mock,
}

/// Capabilities, one per area the GUI and onboarding care about.
#[derive(Debug, Clone, PartialEq)]
pub struct Capabilities {
    pub display: Support,
    /// Combined mode (w, h); empty for mocks.
    pub mode: Option<(u32, u32)>,
    pub refresh_hz: Vec<u32>,
    pub edid_vendor: Option<String>,
    pub edid_product: Option<u16>,
    /// Host requirements the system checks must cover (e.g. "drm_lease").
    pub display_needs: Vec<String>,
    pub tracking: Support,
    /// "slam", "lighthouse", "inside_out", "imu3dof", ...
    pub head_tracking: String,
    pub controllers: Support,
    pub controller_slots: Vec<u8>,
    /// "start_pair", "unknown", ...
    pub pairing: String,
    pub eye_tracking: Support,
    pub eye_usb: Option<(u16, u16)>,
    pub ipd: Support,
    pub ipd_motor: bool,
    pub ipd_range_mm: Option<(f32, f32)>,
    pub cameras: Support,
    pub camera_count: u32,
    pub audio: Support,
    pub audio_sink_match: Vec<String>,
    pub audio_source_match: Vec<String>,
}

/// A parsed descriptor.
#[derive(Debug, Clone, PartialEq)]
pub struct Descriptor {
    pub id: String,
    pub name: String,
    /// Vendor id string ("pimax"). Not assumed anywhere else, so another vendor's module is just another directory.
    pub vendor: String,
    pub status: Status,
    pub usb_vid: u16,
    /// PIDs that identify this model. Empty for mocks: a mock is never auto-matched.
    pub usb_pids: Vec<u16>,
    /// Unverified PIDs (mocks), shown as "possibly".
    pub usb_pid_candidates: Vec<u16>,
    pub command_interface: u8,
    pub caps: Capabilities,
    pub calibration_paths: Vec<String>,
    pub onboarding: Vec<String>,
    pub allowlist: Allowlist,
}

fn table<'a>(secs: &'a [Section], name: &str) -> Result<&'a Section, SpecError> {
    secs.iter()
        .find(|s| !s.array && s.name == name)
        .ok_or_else(|| SpecError(format!("missing [{name}]")))
}

fn u16_of(v: i64, what: &str) -> Result<u16, SpecError> {
    u16::try_from(v).map_err(|_| SpecError(format!("{what} {v} out of u16")))
}

impl Descriptor {
    /// Parse a `device.toml`.
    pub fn parse(src: &str) -> Result<Descriptor, SpecError> {
        let secs = toml_lite::parse(src).map_err(|e| SpecError(e.to_string()))?;
        let dev = table(&secs, "device")?;
        let need = |s: &Section, k: &str| s.str(k).map(String::from).ok_or_else(|| SpecError(format!("[{}] lacks {k}", s.name)));
        let status = match need(dev, "status")?.as_str() {
            "real" => Status::Real,
            "mock" => Status::Mock,
            o => return Err(SpecError(format!("status {o:?}"))),
        };
        let pids = |k: &str| dev.ints(k).into_iter().map(|p| u16_of(p, k)).collect::<Result<Vec<_>, _>>();
        let disp = table(&secs, "display")?;
        let trk = table(&secs, "tracking")?;
        let ctl = table(&secs, "controllers")?;
        let eye = table(&secs, "eye_tracking")?;
        let ipd = table(&secs, "ipd")?;
        let cam = table(&secs, "cameras")?;
        let aud = table(&secs, "audio")?;
        let mode = disp.ints("mode");
        let caps = Capabilities {
            display: Support::parse(disp.str("status"))?,
            mode: (mode.len() == 2).then(|| (mode[0] as u32, mode[1] as u32)),
            refresh_hz: disp.ints("refresh_hz").into_iter().map(|v| v as u32).collect(),
            edid_vendor: disp.str("edid_vendor").map(String::from),
            edid_product: disp.int("edid_product").map(|v| u16_of(v, "edid_product")).transpose()?,
            display_needs: disp.strs("needs"),
            tracking: Support::parse(trk.str("status"))?,
            head_tracking: trk.str("head").unwrap_or("unknown").into(),
            controllers: Support::parse(ctl.str("status"))?,
            controller_slots: ctl.ints("slots").into_iter().map(|v| v as u8).collect(),
            pairing: ctl.str("pairing").unwrap_or("unknown").into(),
            eye_tracking: Support::parse(eye.str("status"))?,
            eye_usb: match (eye.int("usb_vid"), eye.int("usb_pid")) {
                (Some(v), Some(p)) => Some((u16_of(v, "usb_vid")?, u16_of(p, "usb_pid")?)),
                _ => None,
            },
            ipd: Support::parse(ipd.str("status"))?,
            ipd_motor: ipd.bool("motor").unwrap_or(false),
            ipd_range_mm: match (ipd.float("min_mm"), ipd.float("max_mm")) {
                (Some(a), Some(b)) if a < b => Some((a as f32, b as f32)),
                (None, None) => None,
                _ => return Err(SpecError("[ipd] min_mm/max_mm".into())),
            },
            cameras: Support::parse(cam.str("status"))?,
            camera_count: cam.int("count").unwrap_or(0) as u32,
            audio: Support::parse(aud.str("status"))?,
            audio_sink_match: aud.strs("sink_match"),
            audio_source_match: aud.strs("source_match"),
        };
        let mut allowlist = Allowlist::default();
        for s in secs.iter().filter(|s| s.array) {
            match s.name.as_str() {
                "hid" => {
                    let c = CommandSpec::from_section(s)?;
                    if allowlist.get(&c.id).is_some() {
                        return Err(SpecError(format!("hid id {} twice", c.id)));
                    }
                    allowlist.commands.push(c);
                }
                "hid_denied" => allowlist.denied.push(Denied::parse(&need(s, "prefix")?, &need(s, "why")?)?),
                o => return Err(SpecError(format!("unknown [[{o}]]"))),
            }
        }
        let d = Descriptor {
            id: need(dev, "id")?,
            name: need(dev, "name")?,
            vendor: need(dev, "vendor")?,
            status,
            usb_vid: u16_of(dev.int("usb_vid").ok_or_else(|| SpecError("[device] lacks usb_vid".into()))?, "usb_vid")?,
            usb_pids: pids("usb_pids")?,
            usb_pid_candidates: pids("usb_pid_candidates")?,
            command_interface: dev.int("command_interface").unwrap_or(0) as u8,
            caps,
            calibration_paths: table(&secs, "calibration")?.strs("paths"),
            onboarding: table(&secs, "onboarding")?.strs("steps"),
            allowlist,
        };
        d.validate()?;
        Ok(d)
    }

    /// Rules every descriptor must follow (enforced in tests for every module in `devices/`).
    fn validate(&self) -> Result<(), SpecError> {
        let e = |m: String| Err(SpecError(format!("{}: {m}", self.id)));
        if self.status == Status::Mock {
            if !self.allowlist.commands.is_empty() {
                return e("a mock may not declare HID commands".into());
            }
            if !self.usb_pids.is_empty() {
                return e("a mock may not claim USB ids (use usb_pid_candidates)".into());
            }
            let c = &self.caps;
            for (n, s) in [("display", c.display), ("tracking", c.tracking), ("controllers", c.controllers), ("ipd", c.ipd), ("cameras", c.cameras), ("audio", c.audio), ("eye", c.eye_tracking)] {
                if matches!(s, Support::Works | Support::Experimental) {
                    return e(format!("a mock may not promise {n}"));
                }
            }
        } else if self.usb_pids.is_empty() {
            return e("a real module needs usb_pids".into());
        }
        for s in &self.onboarding {
            if crate::onboarding::Step::from_id(s).is_none() {
                return e(format!("unknown onboarding step {s}"));
            }
        }
        if self.caps.ipd_motor && self.allowlist.get("ipd_motor").is_none() {
            return e("ipd.motor = true needs an ipd_motor command".into());
        }
        if self.caps.pairing == "start_pair" && self.allowlist.get("start_pair").is_none() {
            return e("pairing = start_pair needs a start_pair command".into());
        }
        Ok(())
    }

    /// Does a USB device belong to this model?
    pub fn matches_usb(&self, vid: u16, pid: u16) -> bool {
        vid == self.usb_vid && self.usb_pids.contains(&pid)
    }
}
