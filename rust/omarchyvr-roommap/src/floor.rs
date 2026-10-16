//! Floor detection and the floor in Monado's spaces (design and numbers: `analysis/floor.md`).
//!
//! Frames are gravity aligned (y up), so the floor is a horizontal plane `y = h`. Three ways to measure `h`:
//!
//! * **Points** ([`detect`]): stereo points (room-map points or live stereo points in the tracker world). A height
//!   histogram finds the horizontal layers, a gravity-constrained RANSAC plane (tilt <= a few deg) refines each,
//!   and the floor is the **lowest** layer with enough support, horizontal extent and a plausible head height
//!   above it (0.4-2.1 m below the median head). Tables and desks are higher layers; sparse layers below the floor
//!   (reflections, mismatches) fail the support test. Height = median of the inliers (sub-cm with hundreds).
//! * **Controller touch** ([`touch_floor`]): a controller lying on the floor, still for 1 s.
//! * **Manual / standing height** ([`FloorSource::Manual`], [`FloorSource::Standing`]).
//!
//! The result is stored in the map directory (`floor.toml`, the height in the room frame) so it comes back with
//! the room. Monado (pimax driver, legacy builder) reports `head = (0, 1.6, 0) + offset * slam` with STAGE /
//! LOCAL_FLOOR at y = 0, so the floor lands at y = 0 when the offset's y puts the floor at app y = -1.6:
//! [`floor_offset`]. [`Follower`] keeps it there against vertical drift with a rate-limited, smooth correction.

use pimax_cam::geom::{self, Vec3};
use std::path::Path;

/// Height of Monado's tracking origin above STAGE (`u_builder_setup_tracking_origins`: nominal 1.6 m for a device
/// without a floor, plus `XRT_TRACKING_ORIGIN_OFFSET_Y`).
pub const MONADO_ORIGIN_HEIGHT: f64 = 1.6;

/// Detection parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct FloorParams {
    /// Histogram bin (m).
    pub bin_m: f64,
    /// Half width of a layer around its histogram peak (m).
    pub band_m: f64,
    /// Inlier distance of the plane fit (m).
    pub inlier_m: f64,
    /// Maximum tilt of a floor plane against gravity (deg).
    pub max_tilt_deg: f64,
    /// Inliers needed.
    pub min_support: usize,
    /// Inliers needed relative to the strongest horizontal layer.
    pub min_rel_support: f64,
    /// Horizontal extent needed: p10-p90 box area of the inliers (m^2).
    pub min_area_m2: f64,
    /// Plausible head height above the floor (m).
    pub head_min_m: f64,
    /// Plausible head height above the floor (m).
    pub head_max_m: f64,
    /// RANSAC iterations per layer.
    pub iters: usize,
}

impl Default for FloorParams {
    fn default() -> Self {
        FloorParams {
            bin_m: 0.01,
            band_m: 0.04,
            inlier_m: 0.02,
            max_tilt_deg: 4.0,
            min_support: 60,
            min_rel_support: 0.08,
            min_area_m2: 0.4,
            head_min_m: 0.4,
            head_max_m: 2.1,
            iters: 300,
        }
    }
}

/// A horizontal layer of points.
#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    /// Height (m): level median of the inliers.
    pub y: f64,
    /// Inliers.
    pub inliers: usize,
    /// Robust spread of the inliers (1.4826 MAD, m).
    pub spread_m: f64,
    /// Standard error of `y` (spread / sqrt(n), m).
    pub sigma_m: f64,
    /// Tilt of the fitted plane against gravity (deg).
    pub tilt_deg: f64,
    /// p10-p90 box area of the inliers (m^2).
    pub area_m2: f64,
}

/// A floor estimate.
#[derive(Clone, Debug, PartialEq)]
pub struct Floor {
    /// The floor layer.
    pub layer: Layer,
    /// Median head height above it (m).
    pub head_above_m: f64,
    /// 0..1.
    pub confidence: f64,
    /// Every horizontal layer found, lowest first (the floor is one of them).
    pub layers: Vec<Layer>,
    /// Why lower layers were not taken.
    pub rejected: Vec<String>,
}

fn median(v: &mut [f64]) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

fn quantile(v: &mut [f64], q: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    v[((v.len() - 1) as f64 * q).round() as usize]
}

/// Small deterministic generator (no rand dependency).
struct Lcg(u64);
impl Lcg {
    fn next(&mut self, n: usize) -> usize {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) as usize) % n.max(1)
    }
}

/// Plane `y = a x + b z + c` through three points.
fn plane3(p: &[Vec3; 3]) -> Option<(f64, f64, f64)> {
    let n = geom::cross(geom::sub(p[1], p[0]), geom::sub(p[2], p[0]));
    if n[1].abs() < 1e-9 || geom::norm(n) < 1e-6 {
        return None;
    }
    let (a, b) = (-n[0] / n[1], -n[2] / n[1]);
    Some((a, b, p[0][1] - a * p[0][0] - b * p[0][2]))
}

/// Least-squares plane `y = a x + b z + c` (centred at `c0`).
fn plane_ls(pts: &[Vec3], c0: [f64; 2]) -> Option<(f64, f64, f64)> {
    let (mut sxx, mut sxz, mut szz, mut sx, mut sz, mut sxy, mut szy, mut sy) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    let n = pts.len() as f64;
    for p in pts {
        let (x, z, y) = (p[0] - c0[0], p[2] - c0[1], p[1]);
        sxx += x * x;
        sxz += x * z;
        szz += z * z;
        sx += x;
        sz += z;
        sxy += x * y;
        szy += z * y;
        sy += y;
    }
    // normal equations [sxx sxz sx; sxz szz sz; sx sz n] [a b c] = [sxy szy sy]
    let m = [[sxx, sxz, sx], [sxz, szz, sz], [sx, sz, n]];
    let d = geom::det(&m);
    if d.abs() < 1e-12 {
        return None;
    }
    let solve = |col: usize| {
        let mut k = m;
        let r = [sxy, szy, sy];
        for i in 0..3 {
            k[i][col] = r[i];
        }
        geom::det(&k) / d
    };
    let (a, b, c) = (solve(0), solve(1), solve(2));
    Some((a, b, c - a * c0[0] - b * c0[1]))
}

fn tilt_deg(a: f64, b: f64) -> f64 {
    (a * a + b * b).sqrt().atan().to_degrees()
}

/// Horizontal layers of `pts`, lowest first. `at` = the reference position (x, z) the height is reported at.
pub fn layers(pts: &[Vec3], at: [f64; 2], p: &FloorParams) -> Vec<Layer> {
    if pts.len() < p.min_support {
        return Vec::new();
    }
    let mut ys: Vec<f64> = pts.iter().map(|q| q[1]).collect();
    let (lo, hi) = (quantile(&mut ys, 0.0), quantile(&mut ys, 1.0));
    let nb = (((hi - lo) / p.bin_m).ceil() as usize + 1).min(100_000);
    let mut h = vec![0.0f64; nb + 4];
    for q in pts {
        h[((q[1] - lo) / p.bin_m) as usize + 2] += 1.0;
    }
    let k = [1.0, 2.0, 3.0, 2.0, 1.0];
    let hs: Vec<f64> = (0..h.len())
        .map(|i| (0..5).map(|j| if i + j >= 2 && i + j - 2 < h.len() { h[i + j - 2] * k[j] } else { 0.0 }).sum::<f64>() / 9.0)
        .collect();
    // peaks: local maxima, non-max suppressed within the band
    let w = (p.band_m / p.bin_m).round().max(1.0) as usize;
    let mut peaks: Vec<usize> = (0..hs.len())
        .filter(|&i| {
            let a = i.saturating_sub(w);
            let b = (i + w).min(hs.len() - 1);
            hs[i] > 0.0 && (a..=b).all(|j| hs[j] < hs[i] || (hs[j] == hs[i] && j >= i))
        })
        .collect();
    // most supported first, at most 12 layers
    peaks.sort_by(|a, b| hs[*b].total_cmp(&hs[*a]));
    peaks.truncate(12);
    let mut out = Vec::new();
    let mut rng = Lcg(0x5eed);
    for pk in peaks {
        let yc = lo + (pk as f64 - 2.0 + 0.5) * p.bin_m;
        let band: Vec<Vec3> = pts.iter().filter(|q| (q[1] - yc).abs() <= p.band_m).copied().collect();
        if band.len() < p.min_support {
            continue;
        }
        // gravity-constrained RANSAC: tilt <= max_tilt, inliers within inlier_m
        let max_slope = p.max_tilt_deg.to_radians().tan();
        let count = |pl: (f64, f64, f64)| band.iter().filter(|q| (q[1] - (pl.0 * q[0] + pl.1 * q[2] + pl.2)).abs() <= p.inlier_m).count();
        let mut best = (0.0, 0.0, median(&mut band.iter().map(|q| q[1]).collect::<Vec<_>>()));
        let mut best_n = count(best);
        for _ in 0..p.iters {
            let s = [band[rng.next(band.len())], band[rng.next(band.len())], band[rng.next(band.len())]];
            // a sample spanning less than 30 cm says nothing about tilt
            let span = geom::norm(geom::sub(s[1], s[0])).min(geom::norm(geom::sub(s[2], s[0])));
            if span < 0.3 {
                continue;
            }
            let Some(pl) = plane3(&s) else { continue };
            if (pl.0 * pl.0 + pl.1 * pl.1).sqrt() > max_slope {
                continue;
            }
            let n = count(pl);
            if n > best_n {
                best = pl;
                best_n = n;
            }
        }
        let inl: Vec<Vec3> = band
            .iter()
            .filter(|q| (q[1] - (best.0 * q[0] + best.1 * q[2] + best.2)).abs() <= p.inlier_m)
            .copied()
            .collect();
        if inl.len() < p.min_support {
            continue;
        }
        // tilt from a least-squares refit (kept only when it stays within the gate)
        let c0 = [inl.iter().map(|q| q[0]).sum::<f64>() / inl.len() as f64, inl.iter().map(|q| q[2]).sum::<f64>() / inl.len() as f64];
        let (a, b) = match plane_ls(&inl, c0) {
            Some((a, b, _)) if (a * a + b * b).sqrt() <= max_slope => (a, b),
            _ => (best.0, best.1),
        };
        // height: the level median of the inliers. Monado's offset can only move the world, not tilt it, and a
        // fitted tilt (tracker attitude error or map bend, 0.1-1 deg) extrapolated to one spot moves the height by
        // cm (map "home": 1.7 cm between 2 m and 3.5 m point ranges); the level median is the best height over the
        // whole floor area. The tilt is reported, and selects the inliers. `at` is kept for the residual spread.
        let _ = at;
        let mut r: Vec<f64> = inl.iter().map(|q| q[1]).collect();
        let y = median(&mut r);
        let mut r: Vec<f64> = inl.iter().map(|q| q[1] - a * (q[0] - c0[0]) - b * (q[2] - c0[1])).collect();
        let yc = median(&mut r);
        let mut dev: Vec<f64> = r.iter().map(|v| (v - yc).abs()).collect();
        let spread = 1.4826 * median(&mut dev);
        let mut xs: Vec<f64> = inl.iter().map(|q| q[0]).collect();
        let mut zs: Vec<f64> = inl.iter().map(|q| q[2]).collect();
        let area = (quantile(&mut xs, 0.9) - quantile(&mut xs, 0.1)) * (quantile(&mut zs, 0.9) - quantile(&mut zs, 0.1));
        if out.iter().any(|l: &Layer| (l.y - y).abs() < p.band_m) {
            continue;
        }
        out.push(Layer {
            y,
            inliers: inl.len(),
            spread_m: spread,
            sigma_m: spread / (inl.len() as f64).sqrt(),
            tilt_deg: tilt_deg(a, b),
            area_m2: area,
        });
    }
    out.sort_by(|a, b| a.y.total_cmp(&b.y));
    out
}

/// The floor among the points: the lowest supported, extended layer 0.4-2.1 m below the median head.
/// `heads` = head (body) positions of the session in the same frame.
pub fn detect(pts: &[Vec3], heads: &[Vec3], p: &FloorParams) -> Result<Floor, String> {
    if heads.is_empty() {
        return Err("no head positions".into());
    }
    let head_y = median(&mut heads.iter().map(|h| h[1]).collect::<Vec<_>>());
    let at = [
        median(&mut heads.iter().map(|h| h[0]).collect::<Vec<_>>()),
        median(&mut heads.iter().map(|h| h[2]).collect::<Vec<_>>()),
    ];
    let ls = layers(pts, at, p);
    if ls.is_empty() {
        return Err(format!("no horizontal layer with {} points ({} points in all)", p.min_support, pts.len()));
    }
    let strongest = ls.iter().map(|l| l.inliers).max().unwrap_or(0) as f64;
    let mut rejected = Vec::new();
    for l in &ls {
        let above = head_y - l.y;
        let why = if above > p.head_max_m {
            Some(format!("{:.1} m below the head", above))
        } else if above < p.head_min_m {
            Some(format!("only {:.2} m below the head", above))
        } else if (l.inliers as f64) < p.min_rel_support * strongest {
            Some(format!("{} points vs {} in the strongest layer", l.inliers, strongest))
        } else if l.area_m2 < p.min_area_m2 {
            Some(format!("{:.2} m^2 only", l.area_m2))
        } else {
            None
        };
        match why {
            Some(w) => rejected.push(format!("layer at {:+.3} m: {w}", l.y)),
            None => {
                let confidence = confidence(l, above, &ls);
                return Ok(Floor { layer: l.clone(), head_above_m: above, confidence, layers: ls.clone(), rejected });
            }
        }
    }
    Err(format!("no layer passes: {}", rejected.join("; ")))
}

/// 0..1: support, extent, flatness and a usual head height (seated 1.0-1.4, standing 1.4-2.0 m).
fn confidence(l: &Layer, above: f64, all: &[Layer]) -> f64 {
    let s = |x: f64, half: f64| x / (x + half);
    let support = s(l.inliers as f64, 150.0);
    let extent = s(l.area_m2, 1.0);
    let flat = if l.tilt_deg < 2.0 { 1.0 } else { 0.7 };
    let noise = if l.spread_m < 0.02 { 1.0 } else { 0.02 / l.spread_m };
    let head = if (0.95..=2.0).contains(&above) { 1.0 } else { 0.6 };
    // a strong layer only a little lower that failed the gates makes it less sure
    let doubt = if all.iter().any(|o| o.y < l.y && o.inliers > l.inliers / 2) { 0.8 } else { 1.0 };
    ((support + extent) / 1.6).min(1.0) * flat * noise * head * doubt
}

/// Follow a known floor: starting at `prior`, the height moves to the median of the points within 5 cm, then
/// 2.5 cm (a robust mean shift), at most `gate_m` from the prior. Level, no tilt: a short window's points cover
/// too little floor to fit one. Used for windows of live points, where the lowest-layer rule could take a shoe or
/// a laptop seen more than the floor.
pub fn track(pts: &[Vec3], heads: &[Vec3], prior: f64, gate_m: f64, p: &FloorParams) -> Option<Floor> {
    if heads.is_empty() {
        return None;
    }
    let head_y = median(&mut heads.iter().map(|h| h[1]).collect::<Vec<_>>());
    let mut y = prior;
    let mut sel: Vec<Vec3> = Vec::new();
    for r in [0.05, 0.05, 0.025, 0.025, 0.025] {
        sel = pts.iter().filter(|q| (q[1] - y).abs() <= r).copied().collect();
        if sel.len() < p.min_support {
            return None;
        }
        y = median(&mut sel.iter().map(|q| q[1]).collect::<Vec<_>>());
        if (y - prior).abs() > gate_m {
            return None;
        }
    }
    // a layer, not a wall: denser (x1.33) than the 5-10 cm shells around it
    let shell = pts.iter().filter(|q| ((q[1] - y).abs() - 0.075).abs() <= 0.025).count();
    if sel.len() * 3 < shell * 2 {
        return None;
    }
    let mut dev: Vec<f64> = sel.iter().map(|q| (q[1] - y).abs()).collect();
    let spread = 1.4826 * median(&mut dev);
    let mut xs: Vec<f64> = sel.iter().map(|q| q[0]).collect();
    let mut zs: Vec<f64> = sel.iter().map(|q| q[2]).collect();
    let area = (quantile(&mut xs, 0.9) - quantile(&mut xs, 0.1)) * (quantile(&mut zs, 0.9) - quantile(&mut zs, 0.1));
    if area < p.min_area_m2 {
        return None;
    }
    let l = Layer { y, inliers: sel.len(), spread_m: spread, sigma_m: spread / (sel.len() as f64).sqrt(), tilt_deg: 0.0, area_m2: area };
    let above = head_y - y;
    Some(Floor { confidence: confidence(&l, above, &[]), layers: vec![l.clone()], layer: l, head_above_m: above, rejected: Vec::new() })
}

/// Room-map points and keyframe (head) positions, keeping only points within `max_range` of their keyframe
/// (stereo depth noise grows with the square of the range).
pub fn map_points(map: &omarchyvr_room::map::RoomMap, max_range: f64) -> (Vec<Vec3>, Vec<Vec3>) {
    let mut pts = Vec::new();
    let mut heads = Vec::new();
    for k in &map.keyframes {
        heads.push(k.pose.t);
        for m in &k.points {
            if geom::norm(geom::sub(m.p, k.pose.t)) <= max_range {
                pts.push(m.p);
            }
        }
    }
    (pts, heads)
}

/// How the floor was measured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloorSource {
    /// Map / stereo points.
    Points,
    /// A controller on the floor.
    Touch,
    /// Typed in (the head height above the floor at a known moment).
    Manual,
    /// Standing height: standing eye height entered, head measured.
    Standing,
}

impl FloorSource {
    /// Name in `floor.toml`.
    pub fn name(self) -> &'static str {
        match self {
            FloorSource::Points => "points",
            FloorSource::Touch => "touch",
            FloorSource::Manual => "manual",
            FloorSource::Standing => "standing",
        }
    }

    fn parse(s: &str) -> Option<FloorSource> {
        Some(match s {
            "points" => FloorSource::Points,
            "touch" => FloorSource::Touch,
            "manual" => FloorSource::Manual,
            "standing" => FloorSource::Standing,
            _ => return None,
        })
    }
}

/// The floor of a room map (`<map>/floor.toml`): height in the room frame.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredFloor {
    /// Floor height in the room frame (m).
    pub y: f64,
    /// Method.
    pub source: FloorSource,
    /// 0..1.
    pub confidence: f64,
    /// Standard error (m).
    pub sigma_m: f64,
    /// Points used (0 for touch/manual).
    pub inliers: usize,
    /// Median head height above it in the mapping session (m).
    pub head_above_m: f64,
    /// Unix time.
    pub when: u64,
}

impl StoredFloor {
    /// TOML text.
    pub fn to_toml(&self) -> String {
        format!(
            "# omarchyvr-roommap floor: floor plane y = floor_y in the room frame (y up, gravity aligned)\n\
             floor_y = {:.5}\nsource = \"{}\"\nconfidence = {:.3}\nsigma_m = {:.5}\ninliers = {}\nhead_above_m = {:.4}\nwhen = {}\n",
            self.y,
            self.source.name(),
            self.confidence,
            self.sigma_m,
            self.inliers,
            self.head_above_m,
            self.when
        )
    }

    /// Parse [`StoredFloor::to_toml`].
    pub fn parse(s: &str) -> Result<StoredFloor, String> {
        let mut f = StoredFloor { y: f64::NAN, source: FloorSource::Points, confidence: 0.0, sigma_m: 0.0, inliers: 0, head_above_m: f64::NAN, when: 0 };
        for l in s.lines() {
            let l = l.split('#').next().unwrap_or("").trim();
            let Some((k, v)) = l.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim().trim_matches('"'));
            let num = || v.parse::<f64>().map_err(|e| format!("{k}: {e}"));
            match k {
                "floor_y" => f.y = num()?,
                "source" => f.source = FloorSource::parse(v).ok_or_else(|| format!("source {v:?}"))?,
                "confidence" => f.confidence = num()?,
                "sigma_m" => f.sigma_m = num()?,
                "inliers" => f.inliers = num()? as usize,
                "head_above_m" => f.head_above_m = num()?,
                "when" => f.when = num()? as u64,
                _ => {}
            }
        }
        if !f.y.is_finite() {
            return Err("floor_y missing".into());
        }
        Ok(f)
    }

    /// Load `dir/floor.toml`.
    pub fn load(dir: &Path) -> Result<StoredFloor, String> {
        let p = dir.join("floor.toml");
        StoredFloor::parse(&std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?)
    }

    /// Save `dir/floor.toml` (atomic).
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join("floor.toml.tmp");
        std::fs::write(&tmp, self.to_toml())?;
        std::fs::rename(tmp, dir.join("floor.toml"))
    }
}

/// The offset y that puts a floor at `floor_y_app` (the floor height in the app world, i.e. after the current offset)
/// onto Monado's STAGE floor: Monado reports `origin_height + app`, so the floor must sit at `-origin_height`.
/// Returns the change to add to the offset's y.
pub fn floor_shift(floor_y_app: f64, origin_height: f64) -> f64 {
    -origin_height - floor_y_app
}

/// Monado's origin height: [`MONADO_ORIGIN_HEIGHT`] + `XRT_TRACKING_ORIGIN_OFFSET_Y` of the Monado process
/// (read from `/proc/<pid>/environ`, which needs only the same user), else of this process.
pub fn origin_height(monado_pid: u32) -> f64 {
    let from_env = |s: &str| s.parse::<f64>().ok();
    let proc_env = (monado_pid != 0)
        .then(|| std::fs::read(format!("/proc/{monado_pid}/environ")).ok())
        .flatten()
        .and_then(|b| {
            b.split(|c| *c == 0)
                .filter_map(|kv| std::str::from_utf8(kv).ok())
                .find_map(|kv| kv.strip_prefix("XRT_TRACKING_ORIGIN_OFFSET_Y=").and_then(from_env))
        });
    let own = std::env::var("XRT_TRACKING_ORIGIN_OFFSET_Y").ok().and_then(|v| from_env(&v));
    MONADO_ORIGIN_HEIGHT + proc_env.or(own).unwrap_or(0.0)
}

/// Controller-on-the-floor estimate: positions of the controller origin (tracker world) over ~1 s while it lies
/// still. Returns (floor y, spread) or why not.
pub fn touch_floor(origins: &[Vec3], bottom_m: f64, max_spread_m: f64) -> Result<(f64, f64), String> {
    if origins.len() < 10 {
        return Err(format!("only {} controller poses", origins.len()));
    }
    let mut ys: Vec<f64> = origins.iter().map(|p| p[1]).collect();
    let y = median(&mut ys);
    let cx = median(&mut origins.iter().map(|p| p[0]).collect::<Vec<_>>());
    let cz = median(&mut origins.iter().map(|p| p[2]).collect::<Vec<_>>());
    let mut d: Vec<f64> = origins.iter().map(|p| geom::norm(geom::sub(*p, [cx, y, cz]))).collect();
    let spread = quantile(&mut d, 0.9);
    if spread > max_spread_m {
        return Err(format!("the controller moved ({:.1} cm): lay it still on the floor", spread * 100.0));
    }
    Ok((y - bottom_m, spread))
}

/// Combine a point estimate and a touch: the touch wins when they disagree by more than `gate_m` (the user put
/// the controller on the real floor), otherwise a precision-weighted mean.
pub fn combine(points: Option<(f64, f64)>, touch: Option<(f64, f64)>, gate_m: f64) -> Option<(f64, f64)> {
    match (points, touch) {
        (Some((yp, sp)), Some((yt, st))) => {
            if (yp - yt).abs() > gate_m {
                Some((yt, st))
            } else {
                let (wp, wt) = (1.0 / (sp * sp).max(1e-8), 1.0 / (st * st).max(1e-8));
                Some(((yp * wp + yt * wt) / (wp + wt), (1.0 / (wp + wt)).sqrt()))
            }
        }
        (a, b) => a.or(b),
    }
}

/// Smooth, rate-limited vertical correction of the offset (continuous floor lock).
///
/// Each floor measurement gives a target shift; the follower low-passes it (time constant `tau_s`), ignores
/// measurements that disagree with the current state by more than `outlier_m` unless `confirm` of them agree, and
/// moves the applied value by at most `max_rate` m/s (`step` is called at the write rate), so the head never jumps
/// more than `max_rate / 90 Hz` per frame (0.01 m/s -> 0.11 mm per frame).
#[derive(Clone, Debug, PartialEq)]
pub struct Follower {
    /// Time constant of the target filter (s).
    pub tau_s: f64,
    /// Slew limit (m/s).
    pub max_rate: f64,
    /// A measurement further than this from the target is held back until confirmed (m).
    pub outlier_m: f64,
    /// Agreeing outliers needed to move the target there.
    pub confirm: usize,
    /// Filtered target shift (m).
    pub target: Option<f64>,
    /// Applied shift (m).
    pub applied: Option<f64>,
    pending: Vec<f64>,
}

impl Follower {
    /// Defaults: tau 10 s, 1 cm/s, outlier 5 cm confirmed by 3.
    pub fn new() -> Follower {
        Follower { tau_s: 10.0, max_rate: 0.01, outlier_m: 0.05, confirm: 3, target: None, applied: None, pending: Vec::new() }
    }

    /// A new measurement of the shift (with its time step since the last one, s).
    pub fn measure(&mut self, shift: f64, dt_s: f64) {
        let Some(t) = self.target else {
            self.target = Some(shift);
            return;
        };
        if (shift - t).abs() > self.outlier_m {
            self.pending.push(shift);
            if self.pending.len() >= self.confirm {
                let mut p = self.pending.clone();
                let m = median(&mut p);
                if p.iter().all(|x| (x - m).abs() <= self.outlier_m) {
                    self.target = Some(m);
                }
                self.pending.clear();
            }
            return;
        }
        self.pending.clear();
        let a = 1.0 - (-dt_s.max(0.0) / self.tau_s).exp();
        self.target = Some(t + a * (shift - t));
    }

    /// Advance the applied value by `dt_s` (first call: jumps to the target, that is the initial placement).
    pub fn step(&mut self, dt_s: f64) -> Option<f64> {
        let t = self.target?;
        let a = match self.applied {
            None => t,
            Some(a) => {
                let lim = self.max_rate * dt_s.max(0.0);
                a + (t - a).clamp(-lim, lim)
            }
        };
        self.applied = Some(a);
        Some(a)
    }
}

impl Default for Follower {
    fn default() -> Self {
        Follower::new()
    }
}

/// Downsampled points for a top-down preview (`<map>/preview.txt`): `x z y` in cm, one point per line, plus a
/// header with the floor and the keyframes. Plain text so the app needs no map parser.
pub fn preview_text(pts: &[Vec3], heads: &[Vec3], floor_y: Option<f64>, max_points: usize) -> String {
    let step = (pts.len() / max_points.max(1)).max(1);
    let mut s = String::from("# omarchyvr-roommap preview: room frame, cm. h = keyframe (head) x z y, p = point x z y\n");
    if let Some(f) = floor_y {
        s += &format!("floor {:.1}\n", f * 100.0);
    }
    for h in heads {
        s += &format!("h {:.0} {:.0} {:.0}\n", h[0] * 100.0, h[2] * 100.0, h[1] * 100.0);
    }
    for p in pts.iter().step_by(step) {
        s += &format!("p {:.0} {:.0} {:.0}\n", p[0] * 100.0, p[2] * 100.0, p[1] * 100.0);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rnd(s: &mut u64) -> f64 {
        *s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((*s >> 11) as f64) / ((1u64 << 53) as f64)
    }

    fn gauss(s: &mut u64) -> f64 {
        (0..6).map(|_| rnd(s)).sum::<f64>() - 3.0
    }

    /// floor at -1.62 (big, noisy), a desk at -0.88 with more points, a sparse reflection layer at -2.3, walls.
    fn room(seed: u64) -> (Vec<Vec3>, Vec<Vec3>) {
        let mut s = seed;
        let mut p = Vec::new();
        for _ in 0..900 {
            p.push([rnd(&mut s) * 5.0 - 2.5, -1.62 + 0.012 * gauss(&mut s) + 0.01 * (rnd(&mut s) - 0.5), rnd(&mut s) * 4.0 - 2.0]);
        }
        for _ in 0..1500 {
            p.push([rnd(&mut s) * 1.4 + 0.3, -0.88 + 0.006 * gauss(&mut s), rnd(&mut s) * 0.7 - 1.5]);
        }
        for _ in 0..30 {
            p.push([rnd(&mut s) * 2.0, -2.3 + 0.01 * gauss(&mut s), rnd(&mut s)]);
        }
        for _ in 0..3000 {
            let y = -1.62 + rnd(&mut s) * 2.6;
            p.push(if rnd(&mut s) < 0.5 { [2.5, y, rnd(&mut s) * 4.0 - 2.0] } else { [rnd(&mut s) * 5.0 - 2.5, y, -2.0] });
        }
        let heads = (0..40).map(|i| [i as f64 * 0.05, 0.02 * gauss(&mut s), 0.0]).collect();
        (p, heads)
    }

    #[test]
    fn floor_under_a_desk_and_above_reflections() {
        for seed in 1..6 {
            let (p, h) = room(seed);
            let f = detect(&p, &h, &FloorParams::default()).unwrap();
            assert!((f.layer.y + 1.62).abs() < 0.005, "seed {seed}: floor {:.4}", f.layer.y);
            assert!(f.layer.tilt_deg < 1.0 && f.confidence > 0.5, "{f:?}");
            assert!(f.layers.iter().any(|l| (l.y + 0.88).abs() < 0.01), "desk layer found");
            let t = track(&p, &h, -1.58, 0.1, &FloorParams::default()).unwrap();
            assert!((t.layer.y + 1.62).abs() < 0.005, "tracked {:.4}", t.layer.y);
            assert!(track(&p, &h, -1.3, 0.1, &FloorParams::default()).is_none(), "nothing near a wrong prior");
        }
    }

    #[test]
    fn tilted_tracker_floor_level_median() {
        // 0.5 deg tracker tilt: the inliers still come from the plane, the height is their level median
        let mut s = 9;
        let a = 0.5f64.to_radians().tan();
        let p: Vec<Vec3> = (0..800)
            .map(|_| {
                let (x, z) = (rnd(&mut s) * 6.0 - 1.0, rnd(&mut s) * 4.0 - 2.0);
                [x, -1.5 + a * (x - 1.0) + 0.01 * gauss(&mut s), z]
            })
            .collect();
        let f = detect(&p, &[[1.0, 0.0, 0.0]], &FloorParams::default()).unwrap();
        // points span x -1..5 (centre 2): level median = -1.5 + a * (2 - 1)
        assert!((f.layer.y - (-1.5 + a)).abs() < 0.004, "{}", f.layer.y);
        assert!((f.layer.tilt_deg - 0.5).abs() < 0.3, "{}", f.layer.tilt_deg);
    }

    #[test]
    fn no_floor_when_only_a_desk_is_seen() {
        let mut s = 3;
        let p: Vec<Vec3> = (0..500).map(|_| [rnd(&mut s), -0.3 + 0.005 * gauss(&mut s), rnd(&mut s)]).collect();
        assert!(detect(&p, &[[0.0; 3]], &FloorParams::default()).is_err(), "a layer 0.3 m below the head is no floor");
    }

    #[test]
    fn stored_floor_round_trip_and_shift() {
        let f = StoredFloor { y: -1.234, source: FloorSource::Touch, confidence: 0.9, sigma_m: 0.004, inliers: 0, head_above_m: 1.6, when: 7 };
        assert_eq!(StoredFloor::parse(&f.to_toml()).unwrap(), StoredFloor { y: -1.234, ..f });
        // floor seen at app y = -0.45 (headset started on the cushion): Monado's 1.6 m origin puts it 1.15 m too
        // high; the offset must move everything 1.15 m up
        assert!((floor_shift(-0.45, 1.6) + 1.15).abs() < 1e-12);
    }

    #[test]
    fn follower_is_smooth_and_robust() {
        let mut f = Follower::new();
        f.measure(0.30, 0.0);
        assert_eq!(f.step(0.0), Some(0.30), "first placement jumps");
        // drift: the target moves 2 cm; at 90 Hz no step may exceed 0.2 mm
        let mut last = 0.30;
        for i in 0..90 * 30 {
            if i % 45 == 0 {
                f.measure(0.32, 0.5);
            }
            let a = f.step(1.0 / 90.0).unwrap();
            assert!((a - last).abs() <= 0.01 / 90.0 + 1e-12);
            last = a;
        }
        assert!((last - 0.32).abs() < 0.003, "{last}");
        // one wild measurement is ignored, three agreeing ones move it
        f.measure(0.80, 0.5);
        assert!((f.target.unwrap() - 0.32).abs() < 0.003);
        f.measure(0.81, 0.5);
        f.measure(0.80, 0.5);
        assert!((f.target.unwrap() - 0.80).abs() < 0.011);
    }

    #[test]
    fn touch_and_combine() {
        let o: Vec<Vec3> = (0..30).map(|i| [0.5, -1.55 + 0.0005 * (i % 3) as f64, 0.2]).collect();
        let (y, sp) = touch_floor(&o, 0.035, 0.01).unwrap();
        assert!((y + 1.585).abs() < 0.002 && sp < 0.002);
        let moving: Vec<Vec3> = (0..30).map(|i| [0.5 + 0.01 * i as f64, -1.55, 0.2]).collect();
        assert!(touch_floor(&moving, 0.035, 0.01).is_err());
        assert_eq!(combine(Some((-1.60, 0.003)), Some((-1.50, 0.01)), 0.05), Some((-1.50, 0.01)));
        let (m, _) = combine(Some((-1.60, 0.01)), Some((-1.58, 0.01)), 0.05).unwrap();
        assert!((m + 1.59).abs() < 1e-9);
    }
}
