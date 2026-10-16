//! Grip and aim of a tracked controller, from the Oculus Touch (Quest 2) geometry SteamVR uses.
//!
//! The optical tracker (pimax-ctrltrack) gives one pose per controller: the *aim* frame (the LED-model frame pitched
//! so a naturally held controller points level). The OpenXR grip must be derived from it. Before 2026-10-16 the
//! driver used `grip = aim * pitch(45 deg)` at the same origin, which disagreed with SteamVR: SteamVR presents the
//! controllers as Oculus Touch, takes Monado's grip, computes the Touch raw pose `raw = grip * inverse(openxr_grip)`
//! and then derives its own aim as `raw * openxr_aim` (components of `oculus_quest2_controller_<hand>.json`). With the
//! 45 deg rule the SteamVR ray pointed 15 deg lower than the OpenXR (WayVR) ray and started about 10 cm away from it,
//! and the grip (the palm centre, where controller and hand models are drawn) sat at the LED ring instead of in the
//! handle.
//!
//! Here the grip comes from the same two components, so every path agrees: `grip = aim * aim_to_grip()` with
//! `aim_to_grip = inverse(openxr_aim) * openxr_grip`, and SteamVR's derived aim equals the driver's aim exactly.
//! Only numbers are taken from the SteamVR file (two component transforms), no model data.

use crate::pose::Pose;

type V3 = [f64; 3];

/// `openxr_grip` of the Quest 2 Touch render model in the Touch raw frame: origin (x = +0.007 left, -0.007 right),
/// rotation about +x (deg).
pub const OPENXR_GRIP_ORIGIN: V3 = [0.007, -0.001_829_41, 0.101_948_2];
pub const OPENXR_GRIP_PITCH_DEG: f64 = 20.6;
/// `openxr_aim` of the same model.
pub const OPENXR_AIM_ORIGIN: V3 = [0.007, -0.038_947_66, 0.009_496_94];
pub const OPENXR_AIM_PITCH_DEG: f64 = -39.4;

fn pitch(deg: f64) -> [f64; 4] {
    let h = deg.to_radians() * 0.5;
    [h.sin(), 0.0, 0.0, h.cos()]
}

fn mirror_x(p: V3, left: bool) -> V3 {
    if left {
        p
    } else {
        [-p[0], p[1], p[2]]
    }
}

/// `openxr_grip` component (pose of the grip in the Touch raw frame).
pub fn openxr_grip(left: bool) -> Pose {
    Pose {
        q: pitch(OPENXR_GRIP_PITCH_DEG),
        p: mirror_x(OPENXR_GRIP_ORIGIN, left),
    }
}

/// `openxr_aim` component (pose of the aim in the Touch raw frame).
pub fn openxr_aim(left: bool) -> Pose {
    Pose {
        q: pitch(OPENXR_AIM_PITCH_DEG),
        p: mirror_x(OPENXR_AIM_ORIGIN, left),
    }
}

/// Grip relative to aim of a tracked controller: `grip = aim * aim_to_grip()`. The same for both hands (the x
/// offsets of the two components cancel): 60 deg pitch up, the grip 3.0 cm below and 9.5 cm behind the aim origin.
pub fn aim_to_grip() -> Pose {
    openxr_aim(true).inverse().compose(&openxr_grip(true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rest::forward;

    fn close(a: V3, b: V3, tol: f64) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < tol)
    }

    #[test]
    fn aim_to_grip_is_sixty_degrees_and_the_handle_offset() {
        let t = aim_to_grip();
        // pure pitch of 60 deg (grip -Z tilted up from the aim ray)
        let f = forward(&t);
        assert!(close(f, [0.0, 60f64.to_radians().sin(), -60f64.to_radians().cos()], 1e-9), "{f:?}");
        assert!(t.rotate([1.0, 0.0, 0.0])[0] > 1.0 - 1e-12);
        // the handle centre: below and behind the ray origin, on the centre line
        assert!(close(t.p, [0.0, -0.0300, 0.0950], 5e-4), "{:?}", t.p);
        // same for the right hand
        let r = openxr_aim(false).inverse().compose(&openxr_grip(false));
        assert!(r.angle_to(&t) < 1e-12 && close(r.p, t.p, 1e-12));
    }

    #[test]
    fn steamvr_derived_aim_equals_the_driver_aim() {
        // any aim pose in the world
        let aim = Pose {
            q: {
                let a = [0.3f64, -0.5, 0.2];
                let n = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
                let h = 0.7f64;
                [a[0] / n * h.sin(), a[1] / n * h.sin(), a[2] / n * h.sin(), h.cos()]
            },
            p: [0.1, 1.2, -0.4],
        };
        for left in [true, false] {
            let grip = aim.compose(&aim_to_grip());
            // Monado steamvr_drv: raw = grip * inverse(openxr_grip); SteamVR: aim = raw * openxr_aim
            let raw = grip.compose(&openxr_grip(left).inverse());
            let steam_aim = raw.compose(&openxr_aim(left));
            assert!(steam_aim.angle_to(&aim) < 1e-9, "left={left}");
            assert!(close(steam_aim.p, aim.p, 1e-9), "left={left}");
        }
    }

    #[test]
    fn the_old_rule_was_fifteen_degrees_off_in_steamvr() {
        // documents the bug: grip = aim * pitch(45), same origin
        let aim = Pose::IDENTITY;
        let old_grip = aim.compose(&Pose {
            q: pitch(45.0),
            p: [0.0; 3],
        });
        let steam_aim = old_grip.compose(&openxr_grip(true).inverse()).compose(&openxr_aim(true));
        let deg = steam_aim.angle_to(&aim).to_degrees();
        assert!((deg - 15.0).abs() < 1e-6, "{deg}");
        let d = steam_aim.p;
        let off = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        assert!(off > 0.09, "{off}");
    }
}
