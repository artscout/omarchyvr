//! C ABI used by the Monado driver (`include/pimax_hands.h`). Every function tolerates NULL.

#![allow(non_camel_case_types, clippy::missing_safety_doc)]

use crate::demo::demo_input;
use crate::mapping::{to_joints, FingerJoints, HandState, Input, Params};
use crate::pose::{grip_to_wrist, Hand, Pose};
use crate::report::{input_from_report, Report};
use crate::rest::{controller_poses, AimMode, Rest};
use std::os::raw::{c_char, c_int};
use std::sync::OnceLock;

/// Bumped on every incompatible change of the structs below; the driver checks it at start-up.
pub const PIMAX_HANDS_ABI_VERSION: u32 = 1;

/// One decoded controller report.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct pimax_hands_input {
    pub trigger: f32,
    pub grip: f32,
    pub stick_x: f32,
    pub stick_y: f32,
    /// `PIMAX_HANDS_*` bits (see `mapping::flags`).
    pub flags: u32,
}

/// Joint curls of one finger, same layout as Monado's `struct u_hand_tracking_finger_value` without `joint_count`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct pimax_hands_finger {
    pub splay: f32,
    pub joint_curls: [f32; 4],
}

/// Result of [`pimax_hands_get`].
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct pimax_hands_out {
    pub little: pimax_hands_finger,
    pub ring: pimax_hands_finger,
    pub middle: pimax_hands_finger,
    pub index: pimax_hands_finger,
    pub thumb: pimax_hands_finger,
    /// Normalised curl 0..1: little, ring, middle, index, thumb.
    pub curl: [f32; 5],
    /// 0 rest, 1 A/X, 2 B/Y, 3 stick, 4 menu.
    pub thumb_state: u32,
}

/// Rigid transform, `orientation` is x, y, z, w.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct pimax_hands_pose {
    pub orientation: [f32; 4],
    pub position: [f32; 3],
}

/// Opaque per-hand state (`struct pimax_hands` in the C header).
pub struct PimaxHands {
    state: HandState,
}

fn finger(f: FingerJoints) -> pimax_hands_finger {
    pimax_hands_finger {
        splay: f.splay,
        joint_curls: f.joint_curls,
    }
}

#[no_mangle]
pub extern "C" fn pimax_hands_abi_version() -> u32 {
    PIMAX_HANDS_ABI_VERSION
}

/// New hand in the relaxed rest pose. Free with [`pimax_hands_destroy`].
#[no_mangle]
pub extern "C" fn pimax_hands_create() -> *mut PimaxHands {
    Box::into_raw(Box::new(PimaxHands {
        state: HandState::new(Params::default()),
    }))
}

#[no_mangle]
pub unsafe extern "C" fn pimax_hands_destroy(h: *mut PimaxHands) {
    if !h.is_null() {
        // SAFETY: created by pimax_hands_create, destroyed once (caller contract).
        drop(unsafe { Box::from_raw(h) });
    }
}

/// Forget all history (controller reconnected, mode changed).
#[no_mangle]
pub unsafe extern "C" fn pimax_hands_reset(h: *mut PimaxHands) {
    // SAFETY: caller passes a live handle or NULL.
    if let Some(h) = unsafe { h.as_mut() } {
        h.state.reset();
    }
}

/// Feed one report; `now_ns` is any monotonic clock in nanoseconds.
#[no_mangle]
pub unsafe extern "C" fn pimax_hands_update(
    h: *mut PimaxHands,
    now_ns: u64,
    input: *const pimax_hands_input,
) {
    // SAFETY: caller passes live pointers or NULL.
    let (Some(h), Some(i)) = (unsafe { h.as_mut() }, unsafe { input.as_ref() }) else {
        return;
    };
    h.state.update(
        now_ns,
        &Input {
            trigger: i.trigger,
            grip: i.grip,
            stick_x: i.stick_x,
            stick_y: i.stick_y,
            flags: i.flags,
        },
    );
}

/// Feed one controller report straight from libpimax's `pimax_controller_report` fields (the bit meanings of the
/// protocol are decoded here, not in the driver): `trigger` and `grip` 0..255, `stick_x`/`stick_y` -128..127,
/// `buttons` (`BTN_*` bits of pimax.h), `touch` (raw report byte 49).
#[no_mangle]
pub unsafe extern "C" fn pimax_hands_update_report(
    h: *mut PimaxHands,
    now_ns: u64,
    trigger: u8,
    grip: u8,
    stick_x: c_int,
    stick_y: c_int,
    buttons: u32,
    touch: u8,
) {
    // SAFETY: caller passes a live handle or NULL.
    let Some(h) = (unsafe { h.as_mut() }) else {
        return;
    };
    let input = input_from_report(&Report {
        trigger,
        grip,
        stick_x,
        stick_y,
        buttons,
        touch,
    });
    h.state.update(now_ns, &input);
}

/// Latest curls and joint curls. Leaves `out` untouched for NULL arguments.
#[no_mangle]
pub unsafe extern "C" fn pimax_hands_get(h: *const PimaxHands, out: *mut pimax_hands_out) {
    // SAFETY: caller passes live pointers or NULL.
    let (Some(h), Some(out)) = (unsafe { h.as_ref() }, unsafe { out.as_mut() }) else {
        return;
    };
    let c = h.state.curls();
    let j = to_joints(&c);
    *out = pimax_hands_out {
        little: finger(j.little),
        ring: finger(j.ring),
        middle: finger(j.middle),
        index: finger(j.index),
        thumb: finger(j.thumb),
        curl: c.to_array(),
        thumb_state: h.state.thumb_state() as u32,
    };
}

/// Pose of the hand-simulation root (the wrist) in the controller's grip frame:
/// `wrist = grip_pose * out`. `is_right` != 0 for the right hand.
#[no_mangle]
pub unsafe extern "C" fn pimax_hands_grip_to_wrist(is_right: c_int, out: *mut pimax_hands_pose) {
    // SAFETY: caller passes a live pointer or NULL.
    let Some(out) = (unsafe { out.as_mut() }) else {
        return;
    };
    let p = grip_to_wrist(if is_right != 0 {
        Hand::Right
    } else {
        Hand::Left
    });
    *out = pimax_hands_pose {
        orientation: [p.q[0] as f32, p.q[1] as f32, p.q[2] as f32, p.q[3] as f32],
        position: [p.p[0] as f32, p.p[1] as f32, p.p[2] as f32],
    };
}

/// Scripted test input for `now_ns` (see `demo.rs`).
#[no_mangle]
pub unsafe extern "C" fn pimax_hands_demo_input(
    now_ns: u64,
    is_right: c_int,
    out: *mut pimax_hands_input,
) {
    // SAFETY: caller passes a live pointer or NULL.
    let Some(out) = (unsafe { out.as_mut() }) else {
        return;
    };
    let i = demo_input(now_ns, is_right != 0);
    *out = pimax_hands_input {
        trigger: i.trigger,
        grip: i.grip,
        stick_x: i.stick_x,
        stick_y: i.stick_y,
        flags: i.flags,
    };
}

/// Placement of the synthesized controllers, read once from `PIMAX_CTRL_REST` and `PIMAX_CTRL_AIM`.
#[derive(Clone, Debug, PartialEq)]
struct ControllerConfig {
    rest: Rest,
    aim: AimMode,
    error: Option<String>,
}

fn config_from(rest: Option<String>, aim: Option<String>) -> ControllerConfig {
    let mut errs = vec![];
    let rest = match rest {
        Some(s) => Rest::parse(&s).unwrap_or_else(|e| {
            errs.push(e);
            Rest::DESK
        }),
        None => Rest::DESK,
    };
    let aim = match aim {
        Some(s) => AimMode::parse(&s).unwrap_or_else(|e| {
            errs.push(e);
            AimMode::Level
        }),
        None => AimMode::Level,
    };
    ControllerConfig {
        rest,
        aim,
        error: (!errs.is_empty()).then(|| errs.join("; ")),
    }
}

fn controller_config() -> &'static ControllerConfig {
    static CFG: OnceLock<ControllerConfig> = OnceLock::new();
    CFG.get_or_init(|| {
        config_from(
            std::env::var("PIMAX_CTRL_REST").ok(),
            std::env::var("PIMAX_CTRL_AIM").ok(),
        )
    })
}

fn c_pose(p: &Pose) -> pimax_hands_pose {
    pimax_hands_pose {
        orientation: [p.q[0] as f32, p.q[1] as f32, p.q[2] as f32, p.q[3] as f32],
        position: [p.p[0] as f32, p.p[1] as f32, p.p[2] as f32],
    }
}

fn describe(c: &ControllerConfig) -> String {
    let aim = match c.aim {
        AimMode::Level => "level (yaw only)".to_string(),
        AimMode::Head { distance } => format!("head (rays meet {distance:.2} m ahead of the view)"),
    };
    let mut s = format!(
        "rest {:.2} m side, {:.2} m down, {:.2} m forward, grip pitch {:.0} deg{}; aim {aim}",
        c.rest.side,
        c.rest.down,
        c.rest.fwd,
        c.rest.grip_pitch_deg,
        if c.rest == Rest::LEGACY {
            " (legacy)"
        } else {
            ""
        },
    );
    if let Some(e) = &c.error {
        s += &format!("; INVALID SETTING IGNORED: {e}");
    }
    s
}

/// Aim and grip pose of a synthesized (untracked) controller for the head orientation `head_orientation` (x, y, z, w;
/// NULL = identity), in the tracking-origin frame with the head at the origin. Placement from `PIMAX_CTRL_REST`,
/// aim from `PIMAX_CTRL_AIM` (read once). Either output may be NULL.
#[no_mangle]
pub unsafe extern "C" fn pimax_hands_controller_poses(
    is_right: c_int,
    head_orientation: *const f32,
    aim: *mut pimax_hands_pose,
    grip: *mut pimax_hands_pose,
) {
    // SAFETY: caller passes NULL or a pointer to 4 floats.
    let q = match unsafe { head_orientation.cast::<[f32; 4]>().as_ref() } {
        Some(q) => {
            let q = [q[0] as f64, q[1] as f64, q[2] as f64, q[3] as f64];
            let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
            if n.is_finite() && n > 1e-6 {
                [q[0] / n, q[1] / n, q[2] / n, q[3] / n]
            } else {
                Pose::IDENTITY.q
            }
        }
        None => Pose::IDENTITY.q,
    };
    let c = controller_config();
    let hand = if is_right != 0 {
        Hand::Right
    } else {
        Hand::Left
    };
    let (a, g) = controller_poses(hand, &c.rest, c.aim, q);
    // SAFETY: caller passes live pointers or NULL.
    if let Some(out) = unsafe { aim.as_mut() } {
        *out = c_pose(&a);
    }
    // SAFETY: as above.
    if let Some(out) = unsafe { grip.as_mut() } {
        *out = c_pose(&g);
    }
}

/// Grip relative to aim (the fixed handle pitch of the current placement): `grip = aim * out`, for orientation sources
/// other than the head (controller IMU mode).
#[no_mangle]
pub unsafe extern "C" fn pimax_hands_aim_to_grip(out: *mut pimax_hands_pose) {
    // SAFETY: caller passes a live pointer or NULL.
    if let Some(out) = unsafe { out.as_mut() } {
        *out = c_pose(&controller_config().rest.aim_to_grip());
    }
}

/// Grip relative to aim of a TRACKED controller (optical or IMU orientation): `grip = aim * out`, rotation and
/// translation, from the Oculus Touch (Quest 2) `openxr_aim` / `openxr_grip` components SteamVR uses, so SteamVR's
/// derived aim equals the driver's aim ([`crate::touch`]).
#[no_mangle]
pub unsafe extern "C" fn pimax_hands_tracked_aim_to_grip(out: *mut pimax_hands_pose) {
    // SAFETY: caller passes a live pointer or NULL.
    if let Some(out) = unsafe { out.as_mut() } {
        *out = c_pose(&crate::touch::aim_to_grip());
    }
}

/// One line describing the placement in use, NUL-terminated into `buf` (truncated to `len`). Returns 1 when
/// `PIMAX_CTRL_REST` or `PIMAX_CTRL_AIM` was invalid (the defaults are used), else 0.
#[no_mangle]
pub unsafe extern "C" fn pimax_hands_controller_describe(buf: *mut c_char, len: usize) -> c_int {
    let c = controller_config();
    if !buf.is_null() && len > 0 {
        let d = describe(c);
        let n = d.len().min(len - 1);
        // SAFETY: buf holds len bytes (caller contract); n < len.
        unsafe {
            std::ptr::copy_nonoverlapping(d.as_ptr(), buf.cast::<u8>(), n);
            *buf.add(n) = 0;
        }
    }
    c_int::from(c.error.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::flags;

    const MS: u64 = 1_000_000;

    #[test]
    fn controller_config_defaults_overrides_and_errors() {
        let d = config_from(None, None);
        assert_eq!(
            (d.rest, d.aim, d.error.clone()),
            (Rest::DESK, AimMode::Level, None)
        );
        let l = config_from(Some("legacy".into()), Some("head:2".into()));
        assert_eq!(
            (l.rest, l.aim),
            (Rest::LEGACY, AimMode::Head { distance: 2.0 })
        );
        assert!(describe(&l).contains("(legacy)") && describe(&l).contains("2.00 m"));
        let bad = config_from(Some("nope".into()), Some("sideways".into()));
        assert_eq!((bad.rest, bad.aim), (Rest::DESK, AimMode::Level));
        assert!(describe(&bad).contains("INVALID SETTING IGNORED"));
    }

    #[test]
    fn controller_poses_ffi_matches_rust_and_tolerates_null() {
        unsafe {
            pimax_hands_controller_poses(
                1,
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            pimax_hands_aim_to_grip(std::ptr::null_mut());
            pimax_hands_tracked_aim_to_grip(std::ptr::null_mut());
            let mut t = pimax_hands_pose::default();
            pimax_hands_tracked_aim_to_grip(&mut t);
            assert_eq!(t, c_pose(&crate::touch::aim_to_grip()));
            assert!(pimax_hands_controller_describe(std::ptr::null_mut(), 0) <= 1);
            let (mut a, mut g) = (pimax_hands_pose::default(), pimax_hands_pose::default());
            let zero = [0.0f32; 4]; // degenerate quaternion -> identity
            pimax_hands_controller_poses(0, zero.as_ptr(), &mut a, &mut g);
            let c = controller_config();
            let (ra, rg) = controller_poses(Hand::Left, &c.rest, c.aim, Pose::IDENTITY.q);
            assert_eq!((a, g), (c_pose(&ra), c_pose(&rg)));
            let mut buf = [0 as c_char; 8];
            pimax_hands_controller_describe(buf.as_mut_ptr(), buf.len());
            assert_eq!(buf[7], 0); // truncated and terminated
        }
    }

    #[test]
    fn lifecycle_and_null_arguments() {
        assert_eq!(pimax_hands_abi_version(), PIMAX_HANDS_ABI_VERSION);
        unsafe {
            pimax_hands_destroy(std::ptr::null_mut());
            pimax_hands_reset(std::ptr::null_mut());
            pimax_hands_update(std::ptr::null_mut(), 1, std::ptr::null());
            let mut out = pimax_hands_out::default();
            pimax_hands_get(std::ptr::null(), &mut out);
            assert_eq!(out, pimax_hands_out::default());
            pimax_hands_grip_to_wrist(0, std::ptr::null_mut());
            pimax_hands_demo_input(0, 0, std::ptr::null_mut());

            let h = pimax_hands_create();
            assert!(!h.is_null());
            pimax_hands_update(h, 1, std::ptr::null());
            pimax_hands_get(h, std::ptr::null_mut());
            pimax_hands_reset(h);
            pimax_hands_destroy(h);
        }
    }

    #[test]
    fn update_and_get_round_trip() {
        unsafe {
            let h = pimax_hands_create();
            let mut i = pimax_hands_input {
                trigger: 1.0,
                grip: 1.0,
                flags: flags::TOUCH_STICK,
                ..Default::default()
            };
            let mut t = 100 * MS;
            for _ in 0..200 {
                t += 4 * MS;
                pimax_hands_update(h, t, &i);
            }
            let mut out = pimax_hands_out::default();
            pimax_hands_get(h, &mut out);
            assert!(
                (out.curl[3] - Params::default().index_full).abs() < 0.01,
                "{out:?}"
            );
            assert!(
                (out.curl[2] - 1.0).abs() < 0.01
                    && out.curl[0] == out.curl[2]
                    && out.curl[1] == out.curl[2]
            );
            assert!((out.curl[4] - Params::default().thumb_stick).abs() < 0.02);
            assert_eq!(out.thumb_state, 3);
            // joint curls are consistent with the normalised curls
            assert!((out.middle.joint_curls[1] - out.curl[2] * 1.859).abs() < 1e-3);
            assert!((out.index.joint_curls[1] - out.curl[3] * 1.43).abs() < 1e-3);
            assert!((out.thumb.joint_curls[1] - out.curl[4] * 1.57).abs() < 1e-3);
            i.flags = 0;
            for _ in 0..200 {
                t += 4 * MS;
                pimax_hands_update(h, t, &i);
            }
            pimax_hands_get(h, &mut out);
            assert_eq!(out.thumb_state, 0);
            pimax_hands_destroy(h);
        }
    }

    #[test]
    fn grip_to_wrist_matches_the_rust_api() {
        for right in [0, 1] {
            let mut p = pimax_hands_pose::default();
            unsafe { pimax_hands_grip_to_wrist(right, &mut p) };
            let want = grip_to_wrist(if right == 1 { Hand::Right } else { Hand::Left });
            for k in 0..3 {
                assert!((f64::from(p.position[k]) - want.p[k]).abs() < 1e-6);
            }
            let n: f32 = p.orientation.iter().map(|c| c * c).sum();
            assert!((n - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn demo_input_matches_the_rust_api() {
        for (ns, right) in [
            (0u64, 0),
            (2_000 * MS, 0),
            (4_000 * MS, 1),
            (6_700 * MS, 0),
            (7_500 * MS, 1),
        ] {
            let mut o = pimax_hands_input::default();
            unsafe { pimax_hands_demo_input(ns, right, &mut o) };
            let want = demo_input(ns, right != 0);
            assert_eq!(
                (o.trigger, o.grip, o.stick_x, o.stick_y, o.flags),
                (
                    want.trigger,
                    want.grip,
                    want.stick_x,
                    want.stick_y,
                    want.flags
                )
            );
        }
    }
}
