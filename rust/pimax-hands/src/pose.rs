//! Where the wrist is relative to a controller's grip pose.
//!
//! Monado's hand simulation (`u_hand_sim_simulate_generic`) puts the wrist at the origin of its joint set, fingers
//! along -Z, +Y out of the back of the hand, thumb on +X for the left hand (mirrored for the right). The OpenXR
//! `grip` pose of a controller is defined from the hand that holds it (+X into the palm for the left hand and out of
//! the back for the right, -Z from the little finger toward the thumb). Monado's `ht_ctrl_emu` derives the grip pose
//! from hand joints (palm joint, index and ring proximal joints); this module applies the same rule to the canonical
//! open hand of the simulation and inverts it, so `hand_pose = grip_pose * grip_to_wrist(hand)`.

/// Which hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hand {
    Left,
    Right,
}

/// Distance from the palm joint to the grip origin, towards the palm surface (same value as `ht_ctrl_emu`).
pub const GRIP_OFFSET_FROM_PALM_M: f64 = 0.03;

// Canonical open left hand of `u_hand_sim_simulate_generic` (hand size 0.095 m, natural splay), wrist frame, metres.
// Dumped from the real simulation; the right hand is the mirror image (x negated).
const PALM_LEFT: [f64; 3] = [0.00329, 0.00095, -0.06443];
const INDEX_PROXIMAL_LEFT: [f64; 3] = [0.02792, 0.0, -0.09429];
const RING_PROXIMAL_LEFT: [f64; 3] = [-0.01645, 0.00047, -0.08656];

type V3 = [f64; 3];

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: V3, s: f64) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn norm(a: V3) -> V3 {
    let l = dot(a, a).sqrt();
    if l > 1e-12 {
        scale(a, 1.0 / l)
    } else {
        a
    }
}

/// Rigid transform: rotation (unit quaternion x, y, z, w) and translation, f64 for the derivation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    /// x, y, z, w.
    pub q: [f64; 4],
    pub p: V3,
}

impl Pose {
    pub const IDENTITY: Pose = Pose {
        q: [0.0, 0.0, 0.0, 1.0],
        p: [0.0; 3],
    };

    pub fn from_position(p: V3) -> Self {
        Self {
            p,
            ..Self::IDENTITY
        }
    }

    /// Rotate a vector by this pose's rotation.
    pub fn rotate(&self, v: V3) -> V3 {
        let [x, y, z, w] = self.q;
        let u = [x, y, z];
        let t = scale(cross(u, v), 2.0);
        add(add(v, scale(t, w)), cross(u, t))
    }

    /// `self * other`: apply `other` first, then `self` (child pose `other` expressed in the parent of `self`).
    pub fn compose(&self, other: &Pose) -> Pose {
        let [ax, ay, az, aw] = self.q;
        let [bx, by, bz, bw] = other.q;
        let q = [
            aw * bx + ax * bw + ay * bz - az * by,
            aw * by - ax * bz + ay * bw + az * bx,
            aw * bz + ax * by - ay * bx + az * bw,
            aw * bw - ax * bx - ay * by - az * bz,
        ];
        let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
        Pose {
            q: [q[0] / n, q[1] / n, q[2] / n, q[3] / n],
            p: add(self.p, self.rotate(other.p)),
        }
    }

    pub fn inverse(&self) -> Pose {
        let qi = Pose {
            q: [-self.q[0], -self.q[1], -self.q[2], self.q[3]],
            p: [0.0; 3],
        };
        Pose {
            q: qi.q,
            p: scale(qi.rotate(self.p), -1.0),
        }
    }

    /// Rotation from orthonormal axes (columns of the rotation matrix).
    pub fn from_axes(x: V3, y: V3, z: V3, p: V3) -> Pose {
        // Row-major m[r][c]: column c is the image of unit axis c.
        let m = [[x[0], y[0], z[0]], [x[1], y[1], z[1]], [x[2], y[2], z[2]]];
        let tr = m[0][0] + m[1][1] + m[2][2];
        let q = if tr > 0.0 {
            let s = (tr + 1.0).sqrt() * 2.0;
            [
                (m[2][1] - m[1][2]) / s,
                (m[0][2] - m[2][0]) / s,
                (m[1][0] - m[0][1]) / s,
                0.25 * s,
            ]
        } else if m[0][0] > m[1][1] && m[0][0] > m[2][2] {
            let s = (1.0 + m[0][0] - m[1][1] - m[2][2]).sqrt() * 2.0;
            [
                0.25 * s,
                (m[0][1] + m[1][0]) / s,
                (m[0][2] + m[2][0]) / s,
                (m[2][1] - m[1][2]) / s,
            ]
        } else if m[1][1] > m[2][2] {
            let s = (1.0 + m[1][1] - m[0][0] - m[2][2]).sqrt() * 2.0;
            [
                (m[0][1] + m[1][0]) / s,
                0.25 * s,
                (m[1][2] + m[2][1]) / s,
                (m[0][2] - m[2][0]) / s,
            ]
        } else {
            let s = (1.0 + m[2][2] - m[0][0] - m[1][1]).sqrt() * 2.0;
            [
                (m[0][2] + m[2][0]) / s,
                (m[1][2] + m[2][1]) / s,
                0.25 * s,
                (m[1][0] - m[0][1]) / s,
            ]
        };
        let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
        Pose {
            q: [q[0] / n, q[1] / n, q[2] / n, q[3] / n],
            p,
        }
    }

    /// Angle in radians between the rotations of two poses.
    pub fn angle_to(&self, other: &Pose) -> f64 {
        let d: f64 = self.q.iter().zip(other.q.iter()).map(|(a, b)| a * b).sum();
        2.0 * d.abs().min(1.0).acos()
    }
}

/// The grip pose derived from hand joints, exactly like Monado's `ht_ctrl_emu` (`do_grip_pose`).
/// `palm` is the palm joint pose, `index_prox` / `ring_prox` the proximal joint positions, all in one frame.
pub fn grip_from_joints(palm: &Pose, index_prox: V3, ring_prox: V3, hand: Hand) -> Pose {
    let origin = add(palm.p, palm.rotate([0.0, -GRIP_OFFSET_FROM_PALM_M, 0.0]));
    let plus_z = norm(sub(ring_prox, index_prox));
    let side = if hand == Hand::Right { 1.0 } else { -1.0 };
    let want_x = palm.rotate([0.0, side, 0.0]);
    let plus_x = norm(sub(want_x, scale(plus_z, dot(want_x, plus_z))));
    let plus_y = cross(plus_z, plus_x);
    Pose::from_axes(plus_x, plus_y, plus_z, origin)
}

fn mirror_x(v: V3) -> V3 {
    [-v[0], v[1], v[2]]
}

/// Grip pose expressed in the wrist frame of the simulated open hand.
pub fn grip_in_wrist(hand: Hand) -> Pose {
    let m = |v: V3| if hand == Hand::Right { mirror_x(v) } else { v };
    grip_from_joints(
        &Pose::from_position(m(PALM_LEFT)),
        m(INDEX_PROXIMAL_LEFT),
        m(RING_PROXIMAL_LEFT),
        hand,
    )
}

/// Wrist (the origin of the simulated joint set) expressed in the grip frame:
/// `wrist_pose = grip_pose * grip_to_wrist(hand)`.
pub fn grip_to_wrist(hand: Hand) -> Pose {
    grip_in_wrist(hand).inverse()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: V3, b: V3, tol: f64) -> bool {
        sub(a, b).iter().all(|d| d.abs() <= tol)
    }
    fn deg(r: f64) -> f64 {
        r.to_degrees()
    }

    #[test]
    fn quaternions_are_unit_and_the_inverse_is_exact() {
        for h in [Hand::Left, Hand::Right] {
            let p = grip_to_wrist(h);
            let n: f64 = p.q.iter().map(|c| c * c).sum();
            assert!((n - 1.0).abs() < 1e-12);
            let id = p.compose(&p.inverse());
            assert!(id.p.iter().all(|c| c.abs() < 1e-12), "{id:?}");
            assert!(id.angle_to(&Pose::IDENTITY) < 1e-9);
            let id2 = grip_in_wrist(h).compose(&grip_to_wrist(h));
            assert!(id2.p.iter().all(|c| c.abs() < 1e-12));
        }
    }

    #[test]
    fn left_hand_matches_the_hand_derived_numbers() {
        // wrist in grip frame: 2.9 cm to -X (out of the palm side), 6.3 cm along +Y (up the forearm), 1.5 cm +Z.
        let p = grip_to_wrist(Hand::Left).p;
        assert!(close(p, [-0.0289, 0.0629, 0.0146], 0.0005), "{p:?}");
    }

    #[test]
    fn right_hand_is_the_mirror_image() {
        let (l, r) = (grip_to_wrist(Hand::Left), grip_to_wrist(Hand::Right));
        assert!(close(r.p, mirror_x(l.p), 1e-9), "{:?} {:?}", l.p, r.p);
        // mirroring a rotation across x negates the y and z components of the quaternion
        let m = Pose {
            q: [l.q[0], -l.q[1], -l.q[2], l.q[3]],
            p: [0.0; 3],
        };
        assert!(m.angle_to(&r) < 1e-6, "{} deg", deg(m.angle_to(&r)));
    }

    #[test]
    fn grip_axes_follow_the_openxr_definition() {
        // Express wrist-frame directions in the grip frame with the rotation of grip_to_wrist.
        for (hand, thumb_x) in [(Hand::Left, 1.0), (Hand::Right, -1.0)] {
            let r = grip_to_wrist(hand);
            let into_palm = r.rotate([0.0, -1.0, 0.0]); // -Y of the wrist frame is the palm side
            let thumb = r.rotate([thumb_x, 0.0, 0.0]); // pinky -> thumb direction (across the knuckles)
            let fingers = r.rotate([0.0, 0.0, -1.0]); // wrist -> fingertips
                                                      // +X into the palm for the left hand, out of the back of the hand for the right hand.
            let want_x = if hand == Hand::Left { 1.0 } else { -1.0 };
            assert!(
                into_palm[0] * want_x > 0.99,
                "{hand:?} palm normal {into_palm:?}"
            );
            // -Z from the little finger toward the thumb.
            assert!(thumb[2] < -0.97, "{hand:?} thumb {thumb:?}");
            // fingers point to -Y of the grip: the wrist is up the +Y axis.
            assert!(fingers[1] < -0.97, "{hand:?} fingers {fingers:?}");
            assert!(r.p[1] > 0.05);
        }
    }

    #[test]
    fn hand_hangs_naturally_with_identity_grip() {
        // Identity grip (controllers in the default 'fixed' mode): fingers point down, palm toward the body's middle.
        let l = grip_to_wrist(Hand::Left);
        let fingers = l.rotate([0.0, 0.0, -1.0]);
        let palm_normal = l.rotate([0.0, -1.0, 0.0]);
        assert!(fingers[1] < -0.97);
        assert!(
            palm_normal[0] > 0.99,
            "left palm faces +X (toward the middle)"
        );
        let r = grip_to_wrist(Hand::Right);
        assert!(
            r.rotate([0.0, -1.0, 0.0])[0] < -0.99,
            "right palm faces -X (toward the middle)"
        );
    }

    #[test]
    fn composing_with_a_world_pose_round_trips() {
        let world = Pose::from_axes(
            [0.0, 0.0, -1.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 2.0, 3.0],
        );
        for h in [Hand::Left, Hand::Right] {
            let wrist = world.compose(&grip_to_wrist(h));
            let grip_again = wrist.compose(&grip_in_wrist(h));
            assert!(close(grip_again.p, world.p, 1e-9));
            assert!(grip_again.angle_to(&world) < 1e-9);
        }
    }

    #[test]
    fn grip_from_joints_is_frame_independent() {
        // Move the whole canonical hand by a rigid transform: the derived grip moves with it.
        let m = Pose::from_axes(
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 0.0],
            [0.4, -0.2, 0.9],
        );
        for h in [Hand::Left, Hand::Right] {
            let sgn = if h == Hand::Right { -1.0 } else { 1.0 };
            let at = |v: V3| m.compose(&Pose::from_position([sgn * v[0], v[1], v[2]])).p;
            let palm = m.compose(&Pose::from_position([
                sgn * PALM_LEFT[0],
                PALM_LEFT[1],
                PALM_LEFT[2],
            ]));
            let moved = grip_from_joints(&palm, at(INDEX_PROXIMAL_LEFT), at(RING_PROXIMAL_LEFT), h);
            let expect = m.compose(&grip_in_wrist(h));
            assert!(close(moved.p, expect.p, 1e-9), "{h:?}");
            assert!(moved.angle_to(&expect) < 1e-9);
        }
    }

    #[test]
    fn from_axes_covers_all_branches() {
        // 90 degree rotations about each axis plus a 180 (exercises every Shepperd branch)
        let cases: [[V3; 3]; 4] = [
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            [[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, -1.0]],
            [[-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0]],
            [[-1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]],
        ];
        for [x, y, z] in cases {
            let p = Pose::from_axes(x, y, z, [0.0; 3]);
            let n: f64 = p.q.iter().map(|c| c * c).sum();
            assert!((n - 1.0).abs() < 1e-12);
            assert!(close(p.rotate([1.0, 0.0, 0.0]), x, 1e-12), "{x:?}");
            assert!(close(p.rotate([0.0, 1.0, 0.0]), y, 1e-12), "{y:?}");
            assert!(close(p.rotate([0.0, 0.0, 1.0]), z, 1e-12), "{z:?}");
        }
    }
}
