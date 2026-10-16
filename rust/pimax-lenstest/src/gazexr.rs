//! OpenXR side of the `gaze` pattern: the `XR_EXT_eye_gaze_interaction` action, its space, and locating it in VIEW
//! space (= the head frame, the frame the accuracy test works in). Also the mapping of keys to test commands.

use crate::gaze::{Cmd, Reading, V3};
use crate::state::Event;
use openxr as xr;
use openxr::sys;
use std::ffi::c_void;
use std::path::PathBuf;

/// The gaze action and (after the session exists) its space.
pub struct GazeInput {
    set: xr::ActionSet,
    action: xr::Action<xr::Posef>,
    space: Option<xr::Space>,
}

/// Check that the extension was enabled and the system supports eye gaze interaction; `Err` is the text for the
/// wearer.
pub fn probe(
    instance: &xr::Instance,
    system: xr::SystemId,
    extension_enabled: bool,
) -> Result<(), String> {
    if !extension_enabled {
        return Err(
            "the runtime does not offer XR_EXT_eye_gaze_interaction (service without PIMAX_EYE=1, or an old build)"
                .to_string(),
        );
    }
    let mut gaze = sys::SystemEyeGazeInteractionPropertiesEXT {
        ty: sys::SystemEyeGazeInteractionPropertiesEXT::TYPE,
        next: std::ptr::null_mut(),
        supports_eye_gaze_interaction: sys::FALSE,
    };
    // SAFETY: plain C structs; `gaze` outlives the call and `props.next` points at it.
    let r = unsafe {
        let mut props: sys::SystemProperties = std::mem::zeroed();
        props.ty = sys::SystemProperties::TYPE;
        props.next =
            (&mut gaze as *mut sys::SystemEyeGazeInteractionPropertiesEXT).cast::<c_void>();
        (instance.fp().get_system_properties)(instance.as_raw(), system, &mut props)
    };
    if r != sys::Result::SUCCESS {
        return Err(format!("xrGetSystemProperties: {r:?}"));
    }
    if gaze.supports_eye_gaze_interaction == sys::FALSE {
        return Err(
            "the system reports supportsEyeGazeInteraction = false (service started without PIMAX_EYE=1?)"
                .to_string(),
        );
    }
    Ok(())
}

impl GazeInput {
    /// Create the action set and suggest the binding. Must run before the action sets are attached.
    pub fn new(instance: &xr::Instance) -> Result<GazeInput, String> {
        let e = |what: &str, r: sys::Result| format!("{what}: {r:?}");
        let set = instance
            .create_action_set("gaze", "gaze", 0)
            .map_err(|r| e("gaze action set", r))?;
        let action = set
            .create_action::<xr::Posef>("gaze_pose", "gaze pose", &[])
            .map_err(|r| e("gaze action", r))?;
        let profile = instance
            .string_to_path("/interaction_profiles/ext/eye_gaze_interaction")
            .map_err(|r| e("eye gaze profile path", r))?;
        let pose = instance
            .string_to_path("/user/eyes_ext/input/gaze_ext/pose")
            .map_err(|r| e("gaze pose path", r))?;
        instance
            .suggest_interaction_profile_bindings(profile, &[xr::Binding::new(&action, pose)])
            .map_err(|r| e("suggest gaze bindings", r))?;
        Ok(GazeInput {
            set,
            action,
            space: None,
        })
    }

    pub fn set(&self) -> &xr::ActionSet {
        &self.set
    }

    /// Create the action space (this is what starts the EyeChip in the service). After attaching the action sets.
    pub fn create_space<G>(&mut self, session: &xr::Session<G>) -> Result<(), String> {
        let s = self
            .action
            .create_space(session, xr::Path::NULL, xr::Posef::IDENTITY)
            .map_err(|r| format!("gaze space: {r:?}"))?;
        self.space = Some(s);
        Ok(())
    }

    /// The gaze in `view` space at `time`; `None` when the pose is not valid (eyes closed, tracker starting).
    pub fn locate(&self, view: &xr::Space, time: xr::Time) -> Option<Reading> {
        let loc = self.space.as_ref()?.locate(view, time).ok()?;
        reading_of(&loc)
    }
}

/// Direction of the pose's -Z axis (OpenXR "forward").
pub fn forward(q: xr::Quaternionf) -> V3 {
    let (u, w) = (
        [f64::from(q.x), f64::from(q.y), f64::from(q.z)],
        f64::from(q.w),
    );
    let v = [0.0, 0.0, -1.0];
    let cross = |a: V3, b: V3| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let t = cross(u, v);
    let t2 = cross(u, t);
    [
        v[0] + 2.0 * (w * t[0] + t2[0]),
        v[1] + 2.0 * (w * t[1] + t2[1]),
        v[2] + 2.0 * (w * t[2] + t2[2]),
    ]
}

/// Largest distance of the reported gaze origin from the head origin that is believed (metres); beyond it the head
/// origin is used (the test only needs the direction to a target 2 m away).
pub const MAX_ORIGIN_M: f64 = 0.08;

pub fn reading_of(loc: &xr::SpaceLocation) -> Option<Reading> {
    let f = loc.location_flags;
    if !f.contains(xr::SpaceLocationFlags::ORIENTATION_VALID) {
        return None;
    }
    let dir = forward(loc.pose.orientation);
    if !dir.iter().all(|c| c.is_finite()) {
        return None;
    }
    let p = loc.pose.position;
    let o = [f64::from(p.x), f64::from(p.y), f64::from(p.z)];
    let near = o.iter().map(|c| c * c).sum::<f64>().sqrt() <= MAX_ORIGIN_M;
    let origin = if f.contains(xr::SpaceLocationFlags::POSITION_VALID) && near {
        o
    } else {
        [0.0; 3]
    };
    Some(Reading { dir, origin })
}

/// What a key or button means during the gaze test. Space and A (also the trigger) go on, R and B restart, Y (and the
/// menu button) accept; everything else (Q to quit, ...) keeps its normal meaning.
pub fn command_of(e: Event) -> Option<Cmd> {
    match e {
        Event::NextPattern | Event::ToggleAuto => Some(Cmd::Next),
        Event::AlignReset | Event::PrevPattern => Some(Cmd::Restart),
        Event::Accept => Some(Cmd::Accept),
        _ => None,
    }
}

/// The correction file the service would apply (so that a measurement is known to be of corrected samples), from the
/// value of `PIMAX_EYE_CORRECTION` and `$HOME`.
pub fn service_correction_for(var: Option<&str>, home: Option<PathBuf>) -> Option<String> {
    match var.map(str::trim) {
        Some("none" | "off" | "0") => None,
        Some(p) if !p.is_empty() => Some(p.to_string()),
        _ => home
            .map(|h| h.join(".config/omarchyvr/eye-correction.toml"))
            .filter(|p| p.exists())
            .map(|p| p.display().to_string()),
    }
}

pub fn service_correction() -> Option<String> {
    service_correction_for(
        std::env::var("PIMAX_EYE_CORRECTION").ok().as_deref(),
        std::env::var_os("HOME").map(PathBuf::from),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(x: f32, y: f32, z: f32, w: f32) -> xr::Quaternionf {
        xr::Quaternionf { x, y, z, w }
    }

    #[test]
    fn identity_looks_down_minus_z_and_yaw_turns_right() {
        let f = forward(q(0.0, 0.0, 0.0, 1.0));
        assert!((f[2] + 1.0).abs() < 1e-9 && f[0].abs() < 1e-9);
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let f = forward(q(0.0, -h, 0.0, h));
        assert!((f[0] - 1.0).abs() < 1e-6, "{f:?}");
        // pitch up by 90 deg: forward becomes +y
        let f = forward(q(h, 0.0, 0.0, h));
        assert!((f[1] - 1.0).abs() < 1e-6, "{f:?}");
    }

    fn loc(flags: xr::SpaceLocationFlags, pos: [f32; 3]) -> xr::SpaceLocation {
        xr::SpaceLocation {
            location_flags: flags,
            pose: xr::Posef {
                orientation: q(0.0, 0.0, 0.0, 1.0),
                position: xr::Vector3f {
                    x: pos[0],
                    y: pos[1],
                    z: pos[2],
                },
            },
        }
    }

    #[test]
    fn invalid_orientation_is_no_reading_and_far_origins_are_ignored() {
        let none = xr::SpaceLocationFlags::EMPTY;
        assert!(reading_of(&loc(none, [0.0; 3])).is_none());
        let o = xr::SpaceLocationFlags::ORIENTATION_VALID;
        let r = reading_of(&loc(o, [0.01, 0.0, 0.0])).unwrap();
        assert_eq!(r.origin, [0.0; 3], "position not flagged valid");
        let both = o | xr::SpaceLocationFlags::POSITION_VALID;
        let r = reading_of(&loc(both, [0.01, 0.0, 0.02])).unwrap();
        assert!((r.origin[0] - 0.01).abs() < 1e-7 && (r.dir[2] + 1.0).abs() < 1e-9);
        let r = reading_of(&loc(both, [0.5, 0.0, 0.0])).unwrap();
        assert_eq!(
            r.origin, [0.0; 3],
            "an origin half a metre from the head is not believed"
        );
    }

    #[test]
    fn keys_map_to_commands() {
        assert_eq!(command_of(Event::NextPattern), Some(Cmd::Next)); // space, A button
        assert_eq!(command_of(Event::ToggleAuto), Some(Cmd::Next)); // 'a', trigger
        assert_eq!(command_of(Event::AlignReset), Some(Cmd::Restart)); // 'r'
        assert_eq!(command_of(Event::PrevPattern), Some(Cmd::Restart)); // B button
        assert_eq!(command_of(Event::Accept), Some(Cmd::Accept)); // 'y'
        assert_eq!(command_of(Event::Quit), None);
        assert_eq!(command_of(Event::ToggleHud), None);
    }

    #[test]
    fn service_correction_detection() {
        let dir = std::env::temp_dir().join(format!("pimax-gx-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".config/omarchyvr")).unwrap();
        let home = Some(dir.clone());
        assert_eq!(service_correction_for(None, home.clone()), None);
        std::fs::write(dir.join(".config/omarchyvr/eye-correction.toml"), "x").unwrap();
        assert!(service_correction_for(None, home.clone())
            .unwrap()
            .ends_with("eye-correction.toml"));
        assert_eq!(service_correction_for(Some("none"), home.clone()), None);
        assert_eq!(
            service_correction_for(Some("/a/b.toml"), home),
            Some("/a/b.toml".to_string())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
