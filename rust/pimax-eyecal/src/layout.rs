//! Where the targets are. Everything stays inside a comfortable eye rotation (about ±15 degrees horizontally and
//! ±12 vertically, well inside the sharp centre of the Dream Air lenses) so that the wearer never has to turn the
//! head: the targets are head-locked, a head turn moves them along.
//!
//! Angles: azimuth right, elevation up, degrees, from the head origin (between the eyes); targets sit on a sphere of
//! [`TARGET_DIST_M`].

/// Distance of the head-locked targets, metres (far enough that the per-eye parallax is small and the vergence
/// relaxed, near enough to fuse easily).
pub const TARGET_DIST_M: f64 = 2.0;

/// One target.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    /// Azimuth, degrees (right positive).
    pub az: f64,
    /// Elevation, degrees (up positive).
    pub el: f64,
    /// Shown on a white background (the pupil narrows: the fit sees another pupil size).
    pub bright: bool,
}

impl Target {
    /// New target.
    pub const fn new(az: f64, el: f64) -> Target {
        Target { az, el, bright: false }
    }

    /// New target on a white background.
    pub const fn on_white(az: f64, el: f64) -> Target {
        Target { az, el, bright: true }
    }

    /// Unit head-frame direction (x right, y up, -z forward).
    pub fn dir(&self) -> [f64; 3] {
        dir_az_el(self.az, self.el)
    }

    /// The target point, metres, head frame.
    pub fn point(&self) -> [f64; 3] {
        let d = self.dir();
        [d[0] * TARGET_DIST_M, d[1] * TARGET_DIST_M, d[2] * TARGET_DIST_M]
    }

    /// Angle from straight ahead, degrees.
    pub fn eccentricity(&self) -> f64 {
        let d = self.dir();
        (-d[2]).clamp(-1.0, 1.0).acos().to_degrees()
    }

    /// Short name, e.g. `+15,-12`.
    pub fn name(&self) -> String {
        format!("{:+.0},{:+.0}", self.az, self.el)
    }
}

/// Unit direction at azimuth `az` / elevation `el` (degrees).
pub fn dir_az_el(az: f64, el: f64) -> [f64; 3] {
    let (a, e) = (az.to_radians(), el.to_radians());
    [a.sin() * e.cos(), e.sin(), -a.cos() * e.cos()]
}

/// Horizontal and vertical extent of the calibration grid.
pub const CAL_AZ: f64 = 15.0;
/// Vertical extent of the calibration grid.
pub const CAL_EL: f64 = 12.0;

/// Calibration targets: 9 = 3 x 3 grid at (0, ±15) x (0, ±12); 13 adds the four inner diagonals (±7.5, ±6), which
/// lets the quadratic model see the field between the grid lines. Centre first, then a short path.
pub fn calibration_targets(points: usize) -> Vec<Target> {
    let mut v = vec![
        Target::new(0.0, 0.0),
        Target::new(-CAL_AZ, CAL_EL),
        Target::new(0.0, CAL_EL),
        Target::new(CAL_AZ, CAL_EL),
        Target::new(CAL_AZ, 0.0),
        Target::new(CAL_AZ, -CAL_EL),
        Target::new(0.0, -CAL_EL),
        Target::new(-CAL_AZ, -CAL_EL),
        Target::new(-CAL_AZ, 0.0),
    ];
    if points >= 13 {
        v.extend([
            Target::new(-7.5, 6.0),
            Target::new(7.5, 6.0),
            Target::new(7.5, -6.0),
            Target::new(-7.5, -6.0),
        ]);
    }
    v
}

/// The calibration path of Pimax Play (as the user saw it on Windows, 2026-10-18): a green dot on black at the
/// centre, then the corners bottom-left, top-right, bottom-right, top-left and back to the centre; then on white
/// left, right, centre, up, down. The dot travels from one place to the next. Same extent as the grid.
pub fn pimax_targets() -> Vec<Target> {
    vec![
        Target::new(0.0, 0.0),
        Target::new(-CAL_AZ, -CAL_EL),
        Target::new(CAL_AZ, CAL_EL),
        Target::new(CAL_AZ, -CAL_EL),
        Target::new(-CAL_AZ, CAL_EL),
        Target::new(0.0, 0.0),
        Target::on_white(-CAL_AZ, 0.0),
        Target::on_white(CAL_AZ, 0.0),
        Target::on_white(0.0, 0.0),
        Target::on_white(0.0, CAL_EL),
        Target::on_white(0.0, -CAL_EL),
    ]
}

/// Validation targets, all different from the calibration targets and inside the calibrated field: two near the
/// centre (5 degrees), four mid (8-10 degrees), four outer (15 degrees, between the grid points).
pub fn validation_targets() -> Vec<Target> {
    vec![
        Target::new(4.0, 3.0),
        Target::new(10.0, 0.0),
        Target::new(12.0, 9.0),
        Target::new(0.0, 8.0),
        Target::new(-12.0, 9.0),
        Target::new(-4.0, -3.0),
        Target::new(-10.0, 0.0),
        Target::new(-12.0, -9.0),
        Target::new(0.0, -8.0),
        Target::new(12.0, -9.0),
    ]
}

/// Recentre (drift) targets: centre and four at ±8 / ±6.
pub fn recenter_targets() -> Vec<Target> {
    vec![
        Target::new(0.0, 0.0),
        Target::new(8.0, 0.0),
        Target::new(0.0, 6.0),
        Target::new(-8.0, 0.0),
        Target::new(0.0, -6.0),
    ]
}

/// Service-path check targets (through the runtime, combined gaze).
pub fn service_targets() -> Vec<Target> {
    vec![
        Target::new(0.0, 0.0),
        Target::new(10.0, 0.0),
        Target::new(0.0, 8.0),
        Target::new(-10.0, 0.0),
        Target::new(0.0, -8.0),
    ]
}

/// Region of a target for the report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    /// Up to 7 degrees from the centre.
    Centre,
    /// 7 to 12 degrees.
    Mid,
    /// Further out.
    Outer,
}

impl Region {
    /// All regions.
    pub const ALL: [Region; 3] = [Region::Centre, Region::Mid, Region::Outer];

    /// Name.
    pub fn name(self) -> &'static str {
        match self {
            Region::Centre => "centre",
            Region::Mid => "mid",
            Region::Outer => "outer",
        }
    }

    /// Region of a target.
    pub fn of(t: &Target) -> Region {
        let e = t.eccentricity();
        if e <= 7.0 + 1e-9 {
            Region::Centre
        } else if e <= 12.5 {
            Region::Mid
        } else {
            Region::Outer
        }
    }
}

/// Smooth-pursuit path: a circle of [`PURSUIT_RADIUS_DEG`] around the centre, one turn in [`PURSUIT_PERIOD_S`]
/// (4.5 deg/s, an easy pursuit speed), starting at the right. `t` seconds from the start.
pub fn pursuit_at(t: f64) -> Target {
    let a = 2.0 * std::f64::consts::PI * t / PURSUIT_PERIOD_S;
    Target::new(PURSUIT_RADIUS_DEG * a.cos(), PURSUIT_RADIUS_DEG * 0.8 * a.sin())
}

/// Radius of the pursuit circle (horizontal; vertical is 0.8 of it).
pub const PURSUIT_RADIUS_DEG: f64 = 10.0;
/// Duration of one pursuit turn.
pub const PURSUIT_PERIOD_S: f64 = 12.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everything_is_inside_the_comfortable_field() {
        let all = calibration_targets(13)
            .into_iter()
            .chain(validation_targets())
            .chain(recenter_targets())
            .chain(service_targets());
        for t in all {
            assert!(t.az.abs() <= 15.0 && t.el.abs() <= 12.0, "{t:?}");
            assert!(t.eccentricity() < 20.0);
        }
        for i in 0..100 {
            let p = pursuit_at(i as f64 * 0.12);
            assert!(p.az.abs() <= 10.0 + 1e-9 && p.el.abs() <= 8.0 + 1e-9);
        }
    }

    #[test]
    fn validation_points_are_disjoint_from_calibration_and_cover_all_regions() {
        let cal = calibration_targets(13);
        let val = validation_targets();
        for v in &val {
            for c in &cal {
                let d = ((v.az - c.az).powi(2) + (v.el - c.el).powi(2)).sqrt();
                assert!(d > 2.5, "{v:?} too close to {c:?}");
            }
        }
        for r in Region::ALL {
            assert!(val.iter().filter(|t| Region::of(t) == r).count() >= 2, "{r:?}");
        }
        assert_eq!(calibration_targets(9).len(), 9);
        assert_eq!(Region::of(&Target::new(0.0, 0.0)), Region::Centre);
        assert_eq!(Region::of(&Target::new(15.0, 12.0)), Region::Outer);
    }

    #[test]
    fn directions_and_points() {
        let t = Target::new(15.0, -12.0);
        let d = t.dir();
        assert!(d[0] > 0.0 && d[1] < 0.0 && d[2] < -0.9);
        let p = t.point();
        let l = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
        assert!((l - TARGET_DIST_M).abs() < 1e-12);
        assert_eq!(t.name(), "+15,-12");
    }
}
