//! Finger flexion and curl estimates from an OpenXR joint set (26 joints, `XR_HAND_JOINT_PALM_EXT` first).
//!
//! The Monado hand simulation bends a finger by rotating consecutive bones about a common axis, so the angle between
//! consecutive bone vectors is the joint flexion and their sum grows linearly with the normalised curl the driver
//! asked for (fitted on golden data from the real simulation, see `tests/golden`).

use pimax_hands::pose::{grip_from_joints, Hand, Pose};

pub const JOINT_COUNT: usize = 26;
pub const PALM: usize = 0;
pub const WRIST: usize = 1;

/// One located joint (position metres, rotation x, y, z, w).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Joint {
    pub p: [f64; 3],
    pub q: [f64; 4],
}

impl Joint {
    pub fn pose(&self) -> Pose {
        Pose {
            q: self.q,
            p: self.p,
        }
    }
}

pub type Joints = [Joint; JOINT_COUNT];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Finger {
    Little,
    Ring,
    Middle,
    Index,
    Thumb,
}

impl Finger {
    /// The order of `u_hand_tracking_values` / `pimax_hands_out.curl`.
    pub const ALL: [Finger; 5] = [
        Finger::Little,
        Finger::Ring,
        Finger::Middle,
        Finger::Index,
        Finger::Thumb,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Finger::Little => "little",
            Finger::Ring => "ring",
            Finger::Middle => "middle",
            Finger::Index => "index",
            Finger::Thumb => "thumb",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Finger::Little => "L",
            Finger::Ring => "R",
            Finger::Middle => "M",
            Finger::Index => "I",
            Finger::Thumb => "T",
        }
    }

    /// Joint indices from the base: thumb metacarpal..tip (4), other fingers metacarpal..tip (5).
    pub fn joint_indices(self) -> &'static [usize] {
        match self {
            Finger::Thumb => &[2, 3, 4, 5],
            Finger::Index => &[6, 7, 8, 9, 10],
            Finger::Middle => &[11, 12, 13, 14, 15],
            Finger::Ring => &[16, 17, 18, 19, 20],
            Finger::Little => &[21, 22, 23, 24, 25],
        }
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn angle(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let (na, nb) = (
        (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt(),
        (b[0] * b[0] + b[1] * b[1] + b[2] * b[2]).sqrt(),
    );
    if na < 1e-9 || nb < 1e-9 {
        return 0.0;
    }
    (d / (na * nb)).clamp(-1.0, 1.0).acos()
}

/// Angles (radians) between consecutive bones: 3 for the four-bone fingers, 2 for the thumb.
pub fn bend_angles(j: &Joints, f: Finger) -> Vec<f64> {
    let idx = f.joint_indices();
    let bones: Vec<[f64; 3]> = idx.windows(2).map(|w| sub(j[w[1]].p, j[w[0]].p)).collect();
    bones.windows(2).map(|w| angle(w[0], w[1])).collect()
}

/// Total flexion of the finger in radians.
pub fn flexion(j: &Joints, f: Finger) -> f64 {
    bend_angles(j, f).iter().sum()
}

/// Radians of total flexion per unit curl for the fingers (fitted on the simulation: index 1.43+1.1+1.1 rad ...).
pub const INDEX_FLEXION_PER_CURL: f64 = 3.6296;
pub const OTHER_FLEXION_PER_CURL: f64 = 3.861;
/// The simulation starts the thumb 10 degrees hyperextended at both joints; curl bends it back.
const THUMB_BASE: f64 = 10.0 * std::f64::consts::PI / 180.0;

/// Flexion the simulation produces for a driver curl (what a perfect round trip would measure).
pub fn expected_flexion(f: Finger, curl: f64) -> f64 {
    let c = curl.clamp(0.0, 1.0);
    match f {
        Finger::Index => INDEX_FLEXION_PER_CURL * c,
        Finger::Thumb => (THUMB_BASE - 1.57 * c).abs() + (THUMB_BASE - 1.4 * c).abs(),
        _ => OTHER_FLEXION_PER_CURL * c,
    }
}

/// Curl (0..1) estimated from a measured flexion. The thumb is only unambiguous above ~0.25 curl (below that its
/// flexion is not monotonic, see [`expected_flexion`]); there the estimate is 0.1.
pub fn curl_estimate(f: Finger, flexion: f64) -> f64 {
    match f {
        Finger::Index => (flexion / INDEX_FLEXION_PER_CURL).clamp(0.0, 1.0),
        Finger::Thumb => {
            if flexion <= 2.0 * THUMB_BASE + 0.02 {
                0.1
            } else {
                ((flexion + 2.0 * THUMB_BASE) / 2.97).clamp(0.0, 1.0)
            }
        }
        _ => (flexion / OTHER_FLEXION_PER_CURL).clamp(0.0, 1.0),
    }
}

/// All five curl estimates in `Finger::ALL` order.
pub fn curl_estimates(j: &Joints) -> [f64; 5] {
    let mut out = [0.0; 5];
    for (o, f) in out.iter_mut().zip(Finger::ALL) {
        *o = curl_estimate(f, flexion(j, f));
    }
    out
}

/// The controller grip pose reconstructed from the joints with Monado's `ht_ctrl_emu` rule (palm joint + index and
/// ring proximal joints). Joints may be in any frame; the result is in the same frame.
pub fn grip_from_joint_set(j: &Joints, hand: Hand) -> Pose {
    grip_from_joints(&j[PALM].pose(), j[7].p, j[17].p, hand)
}

/// Position error (metres) and rotation error (radians) between two poses.
pub fn pose_error(a: &Pose, b: &Pose) -> (f64, f64) {
    let d = sub(a.p, b.p);
    (
        (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt(),
        a.angle_to(b),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Straight finger along -Z: no bend.
    fn straight(f: Finger) -> Joints {
        let mut j = [Joint::default(); JOINT_COUNT];
        for (n, idx) in f.joint_indices().iter().enumerate() {
            j[*idx].p = [0.0, 0.0, -0.03 * n as f64];
        }
        j
    }

    #[test]
    fn straight_finger_has_no_flexion() {
        for f in Finger::ALL {
            assert!(flexion(&straight(f), f) < 1e-9, "{f:?}");
        }
    }

    #[test]
    fn bent_finger_measures_the_bend() {
        // Bone 1 straight down -Z, bone 2 turned by 30 degrees, bone 3 by 70 degrees from the start direction.
        let mut j = straight(Finger::Index);
        let idx = Finger::Index.joint_indices();
        let a = 30.0_f64.to_radians();
        let b = 70.0_f64.to_radians();
        j[idx[0]].p = [0.0, 0.0, 0.0];
        j[idx[1]].p = [0.0, 0.0, -0.03];
        j[idx[2]].p = [0.0, -0.03 * a.sin(), -0.03 - 0.03 * a.cos()];
        j[idx[3]].p = [
            0.0,
            j[idx[2]].p[1] - 0.03 * b.sin(),
            j[idx[2]].p[2] - 0.03 * b.cos(),
        ];
        j[idx[4]].p = [
            0.0,
            j[idx[3]].p[1] - 0.01 * b.sin(),
            j[idx[3]].p[2] - 0.01 * b.cos(),
        ]; // tip continues straight
        let angles = bend_angles(&j, Finger::Index);
        assert_eq!(angles.len(), 3);
        assert!((angles[0] - a).abs() < 1e-9, "{angles:?}");
        assert!((angles[1] - (b - a)).abs() < 1e-9, "{angles:?}");
        assert!(angles[2].abs() < 1e-6, "{angles:?}"); // acos near 1 is only good to ~1e-8
        assert!((flexion(&j, Finger::Index) - b).abs() < 1e-6);
    }

    #[test]
    fn expected_flexion_is_linear_for_fingers_and_v_shaped_for_the_thumb() {
        assert!(
            (expected_flexion(Finger::Index, 0.5) * 2.0 - expected_flexion(Finger::Index, 1.0))
                .abs()
                < 1e-9
        );
        assert!((expected_flexion(Finger::Middle, 1.0) - 3.861).abs() < 1e-9);
        // thumb: hyperextended 10 deg at both joints at curl 0, dips to ~0 near 0.115, then rises linearly
        let t0 = expected_flexion(Finger::Thumb, 0.0);
        assert!((t0 - 20.0_f64.to_radians()).abs() < 1e-9);
        assert!(expected_flexion(Finger::Thumb, 0.115) < 0.05);
        assert!(expected_flexion(Finger::Thumb, 0.5) > 1.0);
        assert!(expected_flexion(Finger::Thumb, 0.7) > expected_flexion(Finger::Thumb, 0.5));
    }

    #[test]
    fn curl_estimate_inverts_expected_flexion_for_fingers() {
        for f in [Finger::Little, Finger::Ring, Finger::Middle, Finger::Index] {
            for k in 0..=10 {
                let c = f64::from(k) / 10.0;
                assert!((curl_estimate(f, expected_flexion(f, c)) - c).abs() < 1e-9);
            }
        }
        for k in 3..=10 {
            let c = f64::from(k) / 10.0;
            assert!(
                (curl_estimate(Finger::Thumb, expected_flexion(Finger::Thumb, c)) - c).abs() < 0.01,
                "{c}"
            );
        }
    }

    #[test]
    fn pose_error_of_identical_and_shifted_poses() {
        let a = Pose::IDENTITY;
        let b = Pose::from_position([0.003, 0.0, 0.004]);
        let (dp, da) = pose_error(&a, &b);
        assert!((dp - 0.005).abs() < 1e-12 && da < 1e-12);
    }
}
