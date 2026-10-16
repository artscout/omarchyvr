//! Where the controllers are assumed to be.
//!
//! The Dream Air controllers have no positional tracking, so the Monado driver synthesizes their poses: a fixed place
//! in the head-yaw frame (x right, y up, -z forward, metres, head at the origin) that follows the head's yaw. The aim
//! pose is level and looks forward (or, with [`AimMode::Head`], at the spot the head looks at), the grip pose is the
//! aim pose pitched up like the handle of a held controller, so the simulated open hand points forward-down instead of
//! hanging straight down.
//!
//! Why (analysis/hands-desk.md): the first fixed offset (0.20 m side, 0.30 m down, 0.35 m forward, grip = aim = level)
//! put the wrist about 35 deg below and 34 deg beside the view centre and the open fingers hung straight down to about
//! 48 deg below: at or past the edge of the 44.55 deg lower FOV and the lens, so the hands WayVR drew were not seen.
//! `PIMAX_CTRL_REST=legacy` brings that placement back for A/B tests.

use crate::pose::{Hand, Pose};

type V3 = [f64; 3];

/// Placement of one controller's grip in the head-yaw frame (mirrored for the left hand).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rest {
    /// Distance of the grip from the head's centre line, outwards (m).
    pub side: f64,
    /// Grip height below the head (m).
    pub down: f64,
    /// Grip distance in front of the head (m).
    pub fwd: f64,
    /// Grip pitch relative to the level aim pose (deg; + = handle tilted forward-up, as a held controller's is).
    pub grip_pitch_deg: f64,
}

impl Rest {
    /// Default: lower part of the view, about 22 deg below and 22 deg beside the centre, at a comfortable reach.
    pub const DESK: Rest = Rest {
        side: 0.15,
        down: 0.16,
        fwd: 0.38,
        grip_pitch_deg: 45.0,
    };
    /// The placement before 2026-10-14 (hands at the lower edge of the view, fingers hanging down).
    pub const LEGACY: Rest = Rest {
        side: 0.20,
        down: 0.30,
        fwd: 0.35,
        grip_pitch_deg: 0.0,
    };

    /// `desk`, `legacy` or `side,down,fwd[,grip_pitch_deg]` (metres, degrees).
    pub fn parse(s: &str) -> Result<Rest, String> {
        let t = s.trim();
        match t {
            "" | "desk" | "default" => return Ok(Rest::DESK),
            "legacy" => return Ok(Rest::LEGACY),
            _ => {}
        }
        let v: Vec<f64> = t
            .split(',')
            .map(|x| x.trim().parse::<f64>())
            .collect::<Result<_, _>>()
            .map_err(|_| {
                format!("PIMAX_CTRL_REST={t:?}: want desk, legacy or side,down,fwd[,pitch_deg]")
            })?;
        let r = match v.as_slice() {
            [side, down, fwd] => Rest {
                side: *side,
                down: *down,
                fwd: *fwd,
                grip_pitch_deg: Rest::DESK.grip_pitch_deg,
            },
            [side, down, fwd, pitch] => Rest {
                side: *side,
                down: *down,
                fwd: *fwd,
                grip_pitch_deg: *pitch,
            },
            _ => return Err(format!("PIMAX_CTRL_REST={t:?}: want 3 or 4 numbers")),
        };
        let sane = |x: f64, lo: f64, hi: f64| x.is_finite() && (lo..=hi).contains(&x);
        if !(sane(r.side, -1.0, 1.0)
            && sane(r.down, -1.0, 1.5)
            && sane(r.fwd, -0.5, 1.5)
            && sane(r.grip_pitch_deg, -180.0, 180.0))
        {
            return Err(format!(
                "PIMAX_CTRL_REST={t:?}: out of range (|m| <= ~1, pitch in +-180)"
            ));
        }
        Ok(r)
    }

    /// Grip position in the head-yaw frame.
    pub fn position(&self, hand: Hand) -> V3 {
        let s = if hand == Hand::Left {
            -self.side
        } else {
            self.side
        };
        [s, -self.down, -self.fwd]
    }

    /// Grip relative to the aim pose: a pure pitch about +x.
    pub fn aim_to_grip(&self) -> Pose {
        let h = self.grip_pitch_deg.to_radians() * 0.5;
        Pose {
            q: [h.sin(), 0.0, 0.0, h.cos()],
            p: [0.0; 3],
        }
    }
}

/// Where the synthesized aim ray points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AimMode {
    /// Level and straight ahead in the head-yaw frame (follows yaw only; the original behaviour).
    Level,
    /// At the spot `distance` metres ahead of the head along its view direction (pitch included): look to aim, both
    /// rays meet where you look.
    Head { distance: f64 },
}

impl AimMode {
    pub const DEFAULT_HEAD_DISTANCE: f64 = 1.0;

    /// `level`, `head` or `head:<metres>`.
    pub fn parse(s: &str) -> Result<AimMode, String> {
        let t = s.trim();
        match t {
            "" | "level" => Ok(AimMode::Level),
            "head" => Ok(AimMode::Head {
                distance: Self::DEFAULT_HEAD_DISTANCE,
            }),
            _ => {
                let d = t
                    .strip_prefix("head:")
                    .and_then(|d| d.trim().parse::<f64>().ok())
                    .filter(|d| d.is_finite() && (0.2..=20.0).contains(d))
                    .ok_or_else(|| {
                        format!("PIMAX_CTRL_AIM={t:?}: want level, head or head:<0.2..20 m>")
                    })?;
                Ok(AimMode::Head { distance: d })
            }
        }
    }
}

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: V3, s: f64) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn len(a: V3) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

/// Yaw-only part of a head orientation (rotation about +y that keeps the forward direction's heading). Identity when
/// the head looks straight up or down (heading undefined).
pub fn yaw_of(head_q: [f64; 4]) -> Pose {
    let head = Pose {
        q: head_q,
        p: [0.0; 3],
    };
    let f = head.rotate([0.0, 0.0, -1.0]);
    if f[0].abs() + f[2].abs() < 1e-3 {
        return Pose::IDENTITY;
    }
    let yaw = (-f[0]).atan2(-f[2]);
    let h = yaw * 0.5;
    Pose {
        q: [0.0, h.sin(), 0.0, h.cos()],
        p: [0.0; 3],
    }
}

/// Orientation whose -z looks along `dir`, +y as close to world up as possible.
fn look_along(dir: V3, at: V3) -> Pose {
    let l = len(dir);
    if l < 1e-9 {
        return Pose::from_position(at);
    }
    let z = scale(dir, -1.0 / l);
    let mut x = cross([0.0, 1.0, 0.0], z);
    if len(x) < 1e-6 {
        x = [1.0, 0.0, 0.0]; // looking straight up or down
    }
    let x = scale(x, 1.0 / len(x));
    let y = cross(z, x);
    Pose::from_axes(x, y, z, at)
}

/// Aim and grip pose of one controller in the tracking-origin frame with the head at the origin (the driver adds no
/// head position: the controllers share the HMD's tracking origin and the HMD has no positional tracking).
pub fn controller_poses(hand: Hand, rest: &Rest, aim: AimMode, head_q: [f64; 4]) -> (Pose, Pose) {
    let yaw = yaw_of(head_q);
    let pos = yaw.rotate(rest.position(hand));
    let aim_pose = match aim {
        AimMode::Level => Pose { q: yaw.q, p: pos },
        AimMode::Head { distance } => {
            let head = Pose {
                q: head_q,
                p: [0.0; 3],
            };
            let target = scale(head.rotate([0.0, 0.0, -1.0]), distance);
            look_along(sub(target, pos), pos)
        }
    };
    // The grip keeps the level heading (a held controller does not tilt with your gaze), pitched like a handle.
    let level = Pose { q: yaw.q, p: pos };
    let grip = level.compose(&rest.aim_to_grip());
    (aim_pose, grip)
}

/// Direction (unit vector) of a pose's -z axis.
pub fn forward(p: &Pose) -> V3 {
    p.rotate([0.0, 0.0, -1.0])
}

/// Angles of a point seen from the head (deg): (right of centre, below centre), in the head's own frame.
pub fn view_angles(head_q: [f64; 4], point: V3) -> (f64, f64) {
    let head = Pose {
        q: head_q,
        p: [0.0; 3],
    };
    let v = head.inverse().rotate(point);
    let fwd = -v[2];
    (
        v[0].atan2(fwd).to_degrees(),
        (-v[1]).atan2(fwd).to_degrees(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pose::grip_to_wrist;

    const ID: [f64; 4] = [0.0, 0.0, 0.0, 1.0];

    fn add(a: V3, b: V3) -> V3 {
        [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
    }

    fn quat_axis(axis: V3, deg: f64) -> [f64; 4] {
        let h = deg.to_radians() * 0.5;
        let l = len(axis);
        [
            axis[0] / l * h.sin(),
            axis[1] / l * h.sin(),
            axis[2] / l * h.sin(),
            h.cos(),
        ]
    }

    fn close(a: V3, b: V3, tol: f64) -> bool {
        len(sub(a, b)) < tol
    }

    #[test]
    fn parse_names_numbers_and_errors() {
        assert_eq!(Rest::parse("").unwrap(), Rest::DESK);
        assert_eq!(Rest::parse("legacy").unwrap(), Rest::LEGACY);
        let r = Rest::parse("0.1, 0.2,0.3").unwrap();
        assert_eq!((r.side, r.down, r.fwd), (0.1, 0.2, 0.3));
        assert_eq!(r.grip_pitch_deg, Rest::DESK.grip_pitch_deg);
        assert_eq!(
            Rest::parse("0.1,0.2,0.3,-10").unwrap().grip_pitch_deg,
            -10.0
        );
        for bad in [
            "x",
            "0.1,0.2",
            "1,2,3,4,5",
            "0.1,0.2,9",
            "0.1,nan,0.3",
            "0,0,0,400",
        ] {
            assert!(Rest::parse(bad).is_err(), "{bad}");
        }
        assert_eq!(AimMode::parse("").unwrap(), AimMode::Level);
        assert_eq!(
            AimMode::parse("head").unwrap(),
            AimMode::Head {
                distance: AimMode::DEFAULT_HEAD_DISTANCE
            }
        );
        assert_eq!(
            AimMode::parse("head:2.5").unwrap(),
            AimMode::Head { distance: 2.5 }
        );
        for bad in ["up", "head:", "head:0.01", "head:x"] {
            assert!(AimMode::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn legacy_reproduces_the_old_fixed_offset() {
        let (aim, grip) = controller_poses(Hand::Right, &Rest::LEGACY, AimMode::Level, ID);
        assert!(close(aim.p, [0.20, -0.30, -0.35], 1e-12));
        assert!(close(grip.p, aim.p, 1e-12));
        assert!(aim.angle_to(&Pose::IDENTITY) < 1e-9 && grip.angle_to(&Pose::IDENTITY) < 1e-9);
        let (l, _) = controller_poses(Hand::Left, &Rest::LEGACY, AimMode::Level, ID);
        assert!(close(l.p, [-0.20, -0.30, -0.35], 1e-12));
    }

    /// The finger direction of the simulated open hand (wrist -z) in the head frame for a given grip.
    fn finger_dir(grip: &Pose, hand: Hand) -> V3 {
        forward(&grip.compose(&grip_to_wrist(hand)))
    }

    #[test]
    fn legacy_fingers_hang_down_and_desk_fingers_point_forward_down() {
        for hand in [Hand::Left, Hand::Right] {
            let (_, g) = controller_poses(hand, &Rest::LEGACY, AimMode::Level, ID);
            let d = finger_dir(&g, hand);
            assert!(d[1] < -0.95, "legacy fingers point down: {d:?}");
            let (_, g) = controller_poses(hand, &Rest::DESK, AimMode::Level, ID);
            let d = finger_dir(&g, hand);
            let below = (-d[1]).atan2(-d[2]).to_degrees();
            assert!(
                d[2] < -0.75 && (20.0..50.0).contains(&below),
                "desk fingers forward-down: {d:?} {below}"
            );
        }
    }

    #[test]
    fn desk_hand_is_well_inside_the_view_and_legacy_is_at_its_edge() {
        // Wrist and a point 0.19 m along the fingers (open hand, middle finger tip) seen from the head.
        let span = |rest: &Rest, hand: Hand| {
            let (_, g) = controller_poses(hand, rest, AimMode::Level, ID);
            let w = g.compose(&grip_to_wrist(hand));
            let tip = add(w.p, scale(forward(&w), 0.19));
            [view_angles(ID, w.p), view_angles(ID, tip)]
        };
        for hand in [Hand::Left, Hand::Right] {
            for (side, below) in span(&Rest::DESK, hand) {
                assert!(
                    side.abs() < 33.0 && (10.0..32.0).contains(&below),
                    "desk {side} {below}"
                );
            }
            let legacy = span(&Rest::LEGACY, hand);
            assert!(
                legacy[1].1 > 44.55,
                "legacy finger tips below the 44.55 deg FOV: {legacy:?}"
            );
            assert!(
                legacy[0].1 > 33.0 && legacy[0].0.abs() > 33.0,
                "legacy wrist low and outside: {legacy:?}"
            );
        }
    }

    #[test]
    fn placement_follows_head_yaw_but_not_pitch() {
        let yaw90 = quat_axis([0.0, 1.0, 0.0], 90.0); // looking left (+90 about +y)
        let (aim, _) = controller_poses(Hand::Right, &Rest::DESK, AimMode::Level, yaw90);
        let p = Rest::DESK.position(Hand::Right);
        // +90 deg about +y maps (x, y, z) -> (z, y, -x)
        assert!(close(aim.p, [p[2], p[1], -p[0]], 1e-9), "{:?}", aim.p);
        assert!(close(forward(&aim), [-1.0, 0.0, 0.0], 1e-9));
        let pitch = quat_axis([1.0, 0.0, 0.0], -40.0); // looking down
        let (aim2, grip2) = controller_poses(Hand::Right, &Rest::DESK, AimMode::Level, pitch);
        let (aim0, grip0) = controller_poses(Hand::Right, &Rest::DESK, AimMode::Level, ID);
        assert!(
            close(aim2.p, aim0.p, 1e-9)
                && aim2.angle_to(&aim0) < 1e-9
                && grip2.angle_to(&grip0) < 1e-9
        );
    }

    #[test]
    fn head_aim_rays_meet_at_the_look_point() {
        let mode = AimMode::Head { distance: 1.2 };
        for head in [
            ID,
            quat_axis([1.0, 0.0, 0.0], -25.0),
            quat_axis([0.3, 1.0, 0.0], 35.0),
        ] {
            let look = scale(
                Pose {
                    q: head,
                    p: [0.0; 3],
                }
                .rotate([0.0, 0.0, -1.0]),
                1.2,
            );
            for hand in [Hand::Left, Hand::Right] {
                let (aim, grip) = controller_poses(hand, &Rest::DESK, mode, head);
                let to = sub(look, aim.p);
                let hit = add(aim.p, scale(forward(&aim), len(to)));
                assert!(close(hit, look, 1e-9), "{hand:?} {hit:?} {look:?}");
                // the aim stays upright (no roll): its +x is horizontal
                assert!(aim.rotate([1.0, 0.0, 0.0])[1].abs() < 1e-9);
                // the grip does not follow the gaze
                let (_, g_level) = controller_poses(hand, &Rest::DESK, AimMode::Level, head);
                assert!(grip.angle_to(&g_level) < 1e-12);
            }
        }
    }

    #[test]
    fn aim_to_grip_pitches_the_handle_forward_up() {
        let g = Rest::DESK.aim_to_grip();
        let f = forward(&g);
        assert!(close(
            f,
            [0.0, 45f64.to_radians().sin(), -45f64.to_radians().cos()],
            1e-12
        ));
        assert!(Rest::LEGACY.aim_to_grip().angle_to(&Pose::IDENTITY) < 1e-12);
    }

    #[test]
    fn yaw_of_drops_pitch_and_roll() {
        let q = Pose {
            q: quat_axis([0.0, 1.0, 0.0], 30.0),
            p: [0.0; 3],
        }
        .compose(&Pose {
            q: quat_axis([1.0, 0.0, 0.0], 50.0),
            p: [0.0; 3],
        })
        .q;
        assert!(
            yaw_of(q).angle_to(&Pose {
                q: quat_axis([0.0, 1.0, 0.0], 30.0),
                p: [0.0; 3]
            }) < 1e-9
        );
        assert_eq!(yaw_of(quat_axis([1.0, 0.0, 0.0], 90.0)), Pose::IDENTITY);
    }

    #[test]
    fn view_angles_signs() {
        let (s, b) = view_angles(ID, [0.1, -0.1, -0.1]);
        assert!((s - 45.0).abs() < 1e-9 && (b - 45.0).abs() < 1e-9);
    }
}
