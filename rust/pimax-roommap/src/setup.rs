//! Room setup (docs/ROOM-SETUP.md): the play area (boundary), its centre and forward direction, stored with the room
//! map, so every session (Monado and SteamVR) puts the same floor, centre and forward on the same real place.
//!
//! * **Setup frame**: origin = the play area's centre on the floor, -Z = forward, y up. The room offset Monado
//!   applies is `setup_from_room * floor shift * room_from_slam` ([`final_offset`]), so the app world (OpenXR STAGE,
//!   SteamVR's raw universe) *is* the setup frame once the room is recognised: STAGE origin = centre, floor y = 0.
//! * **File**: `<map dir>/room-setup.toml` ([`RoomSetup`]): centre and forward in the room frame (stable between
//!   sessions: the room frame is the map's), the boundary polygon in the room frame, the derived play area size.
//! * **Geometry**: boundary clean-up ([`clean_boundary`]), the largest centred rectangle inside it ([`play_area`]), the
//!   free area around a standing user from the map's obstacle points ([`free_area`]), SteamVR's collision walls
//!   ([`walls`]).
//! * **Guided steps** ([`confirm_forward`], [`trace_boundary`]) run against the [`Guide`] trait: prompts out, head
//!   and controller samples in (room frame). The binary drives them with the in-headset HUD; tests with a script.

use pimax_room::session::Alignment;
use std::path::Path;

/// A point on the floor plane: (x, z) in metres.
pub type P2 = [f64; 2];

/// Height of SteamVR's collision walls (what SteamVR's own room setup writes).
pub const WALL_HEIGHT_M: f64 = 2.43;

/// Default play area of the fast setup without map points: 2 x 2 m.
pub const DEFAULT_HALF_M: f64 = 1.0;

/// How the setup was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupMode {
    /// Fast: look around in the middle, automatic floor and play area.
    Fast,
    /// Full: map walk, floor touch, traced boundary, forward.
    Full,
}

impl SetupMode {
    pub fn name(self) -> &'static str {
        match self {
            SetupMode::Fast => "fast",
            SetupMode::Full => "full",
        }
    }

    pub fn parse(s: &str) -> Option<SetupMode> {
        match s {
            "fast" => Some(SetupMode::Fast),
            "full" => Some(SetupMode::Full),
            _ => None,
        }
    }
}

/// `room-setup.toml` of a room map.
#[derive(Clone, Debug, PartialEq)]
pub struct RoomSetup {
    /// Name for people ("Living room"); the map directory name is the id.
    pub label: String,
    pub mode: SetupMode,
    /// Play area centre in the room frame (x, z).
    pub centre: P2,
    /// Forward heading in the room frame: the rotation about +y that turns the setup frame's -Z into forward.
    pub yaw: f64,
    /// Boundary polygon in the room frame (x, z), not closed (the last point connects to the first).
    pub boundary: Vec<P2>,
    /// Play area: the largest rectangle centred on `centre`, along forward, inside the boundary (width x, depth z).
    pub play_area: [f64; 2],
    /// Head height above the floor when forward was confirmed (m; SteamVR's seated pose), NaN if unknown.
    pub head_m: f64,
    /// Unix time.
    pub when: u64,
}

impl RoomSetup {
    /// `room_from_setup`.
    pub fn room_from_setup(&self) -> Alignment {
        Alignment { yaw: self.yaw, t: [self.centre[0], 0.0, self.centre[1]] }
    }

    /// `setup_from_room` (the extra part of Monado's offset).
    pub fn setup_from_room(&self) -> Alignment {
        crate::store::inverse(&self.room_from_setup())
    }

    /// The boundary in the setup frame (centre at the origin, forward = -Z).
    pub fn boundary_setup(&self) -> Vec<P2> {
        self.boundary.iter().map(|p| to_setup(self.centre, self.yaw, *p)).collect()
    }

    /// Area of the boundary (m^2).
    pub fn area(&self) -> f64 {
        area(&self.boundary).abs()
    }

    /// For people: "3.1 x 2.4 m play area (8.2 m^2 inside the boundary), fast setup".
    pub fn describe(&self) -> String {
        format!(
            "play area {:.1} x {:.1} m ({:.1} m^2 inside the boundary), {} setup",
            self.play_area[0],
            self.play_area[1],
            self.area(),
            self.mode.name()
        )
    }

    pub fn to_toml(&self) -> String {
        let pts: Vec<String> = self.boundary.iter().map(|p| format!("[{:.4}, {:.4}]", p[0], p[1])).collect();
        format!(
            "# pimax-roommap room setup (docs/ROOM-SETUP.md): room frame of the map (y up); setup frame = centre on the floor, -Z forward\n\
             label = \"{}\"\nmode = \"{}\"\ncentre = [{:.4}, {:.4}]\nyaw_deg = {:.4}\nplay_area = [{:.3}, {:.3}]\nhead_m = {:.4}\nwhen = {}\nboundary = [{}]\n",
            self.label.replace(['"', '\\', '\n'], " "),
            self.mode.name(),
            self.centre[0],
            self.centre[1],
            self.yaw.to_degrees(),
            self.play_area[0],
            self.play_area[1],
            self.head_m,
            self.when,
            pts.join(", ")
        )
    }

    pub fn parse(s: &str) -> Result<RoomSetup, String> {
        let mut r = RoomSetup {
            label: String::new(),
            mode: SetupMode::Fast,
            centre: [f64::NAN; 2],
            yaw: f64::NAN,
            boundary: Vec::new(),
            play_area: [0.0; 2],
            head_m: f64::NAN,
            when: 0,
        };
        let nums = |v: &str| -> Vec<f64> {
            v.split(['[', ']', ',']).filter_map(|x| x.trim().parse::<f64>().ok()).collect()
        };
        for l in s.lines() {
            let l = l.trim();
            if l.starts_with('#') {
                continue;
            }
            let Some((k, v)) = l.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "label" => r.label = v.trim_matches('"').to_string(),
                "mode" => r.mode = SetupMode::parse(v.trim_matches('"')).ok_or_else(|| format!("mode {v}"))?,
                "centre" => {
                    let n = nums(v);
                    if n.len() == 2 {
                        r.centre = [n[0], n[1]];
                    }
                }
                "yaw_deg" => r.yaw = v.parse::<f64>().map_err(|e| format!("yaw_deg: {e}"))?.to_radians(),
                "play_area" => {
                    let n = nums(v);
                    if n.len() == 2 {
                        r.play_area = [n[0], n[1]];
                    }
                }
                "head_m" => r.head_m = v.parse().unwrap_or(f64::NAN),
                "when" => r.when = v.parse().unwrap_or(0),
                "boundary" => {
                    let n = nums(v);
                    r.boundary = n.as_chunks::<2>().0.to_vec();
                }
                _ => {}
            }
        }
        if !r.centre.iter().all(|x| x.is_finite()) || !r.yaw.is_finite() {
            return Err("centre / yaw_deg missing".into());
        }
        if r.boundary.len() < 3 {
            return Err("boundary needs at least 3 points".into());
        }
        Ok(r)
    }

    /// Load `dir/room-setup.toml`.
    pub fn load(dir: &Path) -> Result<RoomSetup, String> {
        let p = dir.join(FILE);
        RoomSetup::parse(&std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?)
    }

    /// Save `dir/room-setup.toml` (atomic).
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!("{FILE}.tmp"));
        std::fs::write(&tmp, self.to_toml())?;
        std::fs::rename(tmp, dir.join(FILE))
    }
}

/// File name in the map directory.
pub const FILE: &str = "room-setup.toml";

/// A room-frame point in the setup frame of (`centre`, `yaw`).
pub fn to_setup(centre: P2, yaw: f64, p: P2) -> P2 {
    let (s, c) = yaw.sin_cos();
    let (dx, dz) = (p[0] - centre[0], p[1] - centre[1]);
    // inverse of R_y(yaw): x = c dx - s dz, z = s dx + c dz
    [c * dx - s * dz, s * dx + c * dz]
}

/// A setup-frame point in the room frame.
pub fn to_room(centre: P2, yaw: f64, p: P2) -> P2 {
    let (s, c) = yaw.sin_cos();
    [c * p[0] + s * p[1] + centre[0], -s * p[0] + c * p[1] + centre[1]]
}

/// The heading (rotation about +y turning -Z into it) of a horizontal direction (x, z).
pub fn heading(dir_x: f64, dir_z: f64) -> f64 {
    (-dir_x).atan2(-dir_z)
}

/// The offset Monado applies for a room fix: the floor shift (`floor_dy`, from `floor.toml`) and the setup
/// (`setup_from_room`) on top of `room_from_slam`, so app = setup frame with the floor at Monado's STAGE y = 0.
pub fn final_offset(room_from_slam: &Alignment, floor_dy: f64, setup: Option<&RoomSetup>) -> Alignment {
    let a = Alignment { yaw: room_from_slam.yaw, t: [room_from_slam.t[0], room_from_slam.t[1] + floor_dy, room_from_slam.t[2]] };
    match setup {
        Some(s) => crate::store::compose(&s.setup_from_room(), &a),
        None => a,
    }
}

/// Signed area (counter-clockwise in x-z seen from above with z down = positive; only the magnitude matters here).
pub fn area(p: &[P2]) -> f64 {
    let n = p.len();
    if n < 3 {
        return 0.0;
    }
    (0..n).map(|i| p[i][0] * p[(i + 1) % n][1] - p[(i + 1) % n][0] * p[i][1]).sum::<f64>() / 2.0
}

/// Closed perimeter (m).
pub fn perimeter(p: &[P2]) -> f64 {
    let n = p.len();
    (0..n).map(|i| dist(p[i], p[(i + 1) % n])).sum()
}

pub fn dist(a: P2, b: P2) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// Point in polygon (even-odd).
pub fn inside(poly: &[P2], q: P2) -> bool {
    let n = poly.len();
    let mut c = false;
    let mut j = n.wrapping_sub(1);
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a[1] > q[1]) != (b[1] > q[1]) && q[0] < (b[0] - a[0]) * (q[1] - a[1]) / (b[1] - a[1]) + a[0] {
            c = !c;
        }
        j = i;
    }
    c
}

fn cross(o: P2, a: P2, b: P2) -> f64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

/// Proper or touching intersection of segments ab and cd.
pub fn segments_cross(a: P2, b: P2, c: P2, d: P2) -> bool {
    let (d1, d2, d3, d4) = (cross(c, d, a), cross(c, d, b), cross(a, b, c), cross(a, b, d));
    ((d1 > 0.0) != (d2 > 0.0)) && ((d3 > 0.0) != (d4 > 0.0)) && d1 != 0.0 && d2 != 0.0 && d3 != 0.0 && d4 != 0.0
}

/// Does the closed polygon cross itself?
pub fn self_intersects(p: &[P2]) -> bool {
    let n = p.len();
    for i in 0..n {
        for j in i + 1..n {
            // neighbours share a vertex
            if j == i + 1 || (i == 0 && j == n - 1) {
                continue;
            }
            if segments_cross(p[i], p[(i + 1) % n], p[j], p[(j + 1) % n]) {
                return true;
            }
        }
    }
    false
}

/// Convex hull (Andrew's monotone chain), counter-clockwise.
pub fn convex_hull(pts: &[P2]) -> Vec<P2> {
    let mut v: Vec<P2> = pts.to_vec();
    v.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    v.dedup();
    if v.len() < 3 {
        return v;
    }
    let mut h: Vec<P2> = Vec::new();
    for pass in 0..2 {
        let start = h.len();
        let it: Box<dyn Iterator<Item = &P2>> = if pass == 0 { Box::new(v.iter()) } else { Box::new(v.iter().rev()) };
        for &p in it {
            while h.len() >= start + 2 && cross(h[h.len() - 2], h[h.len() - 1], p) <= 0.0 {
                h.pop();
            }
            h.push(p);
        }
        h.pop();
    }
    h
}

/// Douglas-Peucker on an open polyline.
fn dp(p: &[P2], tol: f64, out: &mut Vec<P2>) {
    if p.len() < 3 {
        out.extend_from_slice(&p[..p.len().saturating_sub(1)]);
        return;
    }
    let (a, b) = (p[0], p[p.len() - 1]);
    let len = dist(a, b).max(1e-9);
    let (mut best, mut bi) = (0.0, 0);
    for (i, q) in p.iter().enumerate().take(p.len() - 1).skip(1) {
        let d = cross(a, b, *q).abs() / len;
        if d > best {
            best = d;
            bi = i;
        }
    }
    if best > tol {
        dp(&p[..=bi], tol, out);
        dp(&p[bi..], tol, out);
    } else {
        out.push(a);
    }
}

/// A traced path (room frame, x z) as a clean boundary polygon: points closer than 5 cm merged, simplified to
/// `tol` (Douglas-Peucker), a self-crossing trace replaced by its convex hull. Errors in plain words.
pub fn clean_boundary(trace: &[P2], tol: f64) -> Result<Vec<P2>, String> {
    let mut v: Vec<P2> = Vec::new();
    for &p in trace {
        if v.last().is_none_or(|l| dist(*l, p) >= 0.05) {
            v.push(p);
        }
    }
    if v.len() < 3 || perimeter(&v) < 2.0 {
        return Err("the traced line is too short: walk all the way around the play space".into());
    }
    // close: split the closed loop at the farthest point from the start so both halves simplify well
    let far = (0..v.len()).max_by(|&i, &j| dist(v[0], v[i]).total_cmp(&dist(v[0], v[j]))).unwrap_or(0);
    let mut a: Vec<P2> = v[..=far].to_vec();
    let mut b: Vec<P2> = v[far..].to_vec();
    b.push(v[0]);
    let mut out = Vec::new();
    dp(&a, tol, &mut out);
    dp(&b, tol, &mut out);
    a.clear();
    if out.len() < 3 {
        out = convex_hull(&v);
    }
    if self_intersects(&out) {
        out = convex_hull(&out);
    }
    if area(&out).abs() < 1.0 {
        return Err(format!("the traced area is only {:.1} m^2: trace at least 1 x 1 m", area(&out).abs()));
    }
    Ok(out)
}

/// Is the rectangle |x| <= hx, |z| <= hz inside the polygon (setup frame)?
pub fn rect_inside(poly: &[P2], hx: f64, hz: f64) -> bool {
    let c = [[-hx, -hz], [hx, -hz], [hx, hz], [-hx, hz]];
    if !c.iter().all(|q| inside(poly, *q)) {
        return false;
    }
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if a[0].abs() < hx && a[1].abs() < hz {
            return false;
        }
        for k in 0..4 {
            if segments_cross(a, b, c[k], c[(k + 1) % 4]) {
                return false;
            }
        }
    }
    true
}

/// The play area: the largest rectangle centred at the setup origin, along the setup axes, inside the boundary
/// (setup frame). Returns (width x, depth z); (0, 0) when the centre is outside.
pub fn play_area(poly: &[P2]) -> [f64; 2] {
    if poly.len() < 3 || !inside(poly, [0.0, 0.0]) {
        return [0.0, 0.0];
    }
    let mut best = (0.0, 0.0, 0.0);
    for k in 1..60 {
        let th = k as f64 / 60.0 * std::f64::consts::FRAC_PI_2;
        let (dx, dz) = (th.cos(), th.sin());
        let (mut lo, mut hi) = (0.0, 30.0);
        for _ in 0..40 {
            let m = 0.5 * (lo + hi);
            if rect_inside(poly, m * dx, m * dz) {
                lo = m;
            } else {
                hi = m;
            }
        }
        let (hx, hz) = (lo * dx, lo * dz);
        if hx * hz > best.0 {
            best = (hx * hz, hx, hz);
        }
    }
    [2.0 * best.1, 2.0 * best.2]
}

/// The free area around a user standing at the setup origin (fast setup): a rectangle along the setup axes, each
/// side grown from 0.4 m in 5 cm steps up to `max_half` while the strip it adds holds fewer than 3 obstacle points
/// (setup frame: furniture and walls from the map) and stays inside `extent` (min x, max x, min z, max z: where the
/// map has seen the room). Without obstacles and extent: the default 2 x 2 m square. Corners in the setup frame.
pub fn free_area(obstacles: &[P2], extent: Option<[f64; 4]>, max_half: f64) -> Vec<P2> {
    let (mut h, grow) = if obstacles.is_empty() && extent.is_none() {
        ([DEFAULT_HALF_M; 4], false)
    } else {
        ([0.4f64; 4], true)
    };
    // sides: 0 = -x, 1 = +x, 2 = -z (forward), 3 = +z
    if grow {
        let step = 0.05;
        let mut open = [true; 4];
        while open.iter().any(|o| *o) {
            for s in 0..4 {
                if !open[s] {
                    continue;
                }
                let nh = h[s] + step;
                let (x0, x1, z0, z1) = (-h[0], h[1], -h[2], h[3]);
                let strip = match s {
                    0 => (-nh, x0, z0, z1),
                    1 => (x1, nh, z0, z1),
                    2 => (x0, x1, -nh, z0),
                    _ => (x0, x1, z1, nh),
                };
                let n = obstacles.iter().filter(|p| p[0] >= strip.0 && p[0] <= strip.1 && p[1] >= strip.2 && p[1] <= strip.3).count();
                let within = match (extent, s) {
                    (None, _) => true,
                    (Some(e), 0) => -nh >= e[0],
                    (Some(e), 1) => nh <= e[1],
                    (Some(e), 2) => -nh >= e[2],
                    (Some(e), _) => nh <= e[3],
                };
                if n >= 3 || !within || nh > max_half + 1e-9 {
                    open[s] = false;
                } else {
                    h[s] = nh;
                }
            }
        }
    }
    vec![[-h[0], -h[2]], [h[1], -h[2]], [h[1], h[3]], [-h[0], h[3]]]
}

/// Obstacles for [`free_area`]: map points (room frame, x y z) between 0.15 m and 2.0 m above the floor and within
/// 4 m of the centre, in the setup frame; and the extent of everything the map saw (2nd-98th percentile, setup frame).
pub fn obstacles_from_points(points: &[[f64; 3]], floor_y: f64, centre: P2, yaw: f64) -> (Vec<P2>, Option<[f64; 4]>) {
    let mut obs = Vec::new();
    let (mut xs, mut zs) = (Vec::new(), Vec::new());
    for p in points {
        let q = to_setup(centre, yaw, [p[0], p[2]]);
        if q[0].hypot(q[1]) > 4.0 {
            continue;
        }
        xs.push(q[0]);
        zs.push(q[1]);
        let h = p[1] - floor_y;
        if (0.15..=2.0).contains(&h) {
            obs.push(q);
        }
    }
    if xs.len() < 50 {
        return (obs, None);
    }
    let q = |v: &mut Vec<f64>, f: f64| {
        v.sort_by(|a, b| a.total_cmp(b));
        v[((v.len() - 1) as f64 * f).round() as usize]
    };
    let e = [q(&mut xs, 0.02), q(&mut xs, 0.98), q(&mut zs, 0.02), q(&mut zs, 0.98)];
    (obs, Some(e))
}

/// SteamVR's collision bounds of a boundary (setup frame): one wall quad per edge, bottom then top of the first
/// corner, top then bottom of the next ([a, 0, az], [a, h], [b, h], [b, 0]).
pub fn walls(poly: &[P2], height: f64) -> Vec<[[f64; 3]; 4]> {
    let n = poly.len();
    (0..n)
        .map(|i| {
            let (a, b) = (poly[i], poly[(i + 1) % n]);
            [[a[0], 0.0, a[1]], [a[0], height, a[1]], [b[0], height, b[1]], [b[0], 0.0, b[1]]]
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------------------------
// Guided steps

/// One hand's sample (room frame).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HandSample {
    /// Aim position (room frame), when tracked.
    pub pos: Option<[f64; 3]>,
    /// Trigger / pinch 0..1.
    pub value: f64,
    /// The input is bound (a controller or a tracked hand is there).
    pub active: bool,
}

/// One input sample.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sample {
    /// Seconds since the step started.
    pub t: f64,
    /// Head position (room frame) and heading (rad, [`heading`]), when tracked.
    pub head: Option<([f64; 3], f64)>,
    pub hands: [HandSample; 2],
}

/// What the guided steps need: show a prompt in the headset (and print it for the app), get the next sample
/// (about 20 ms apart; blocks).
pub trait Guide {
    fn say(&mut self, title: &str, text: &str, progress: Option<f32>);
    fn sample(&mut self) -> Sample;
}

/// Trigger pressed / released thresholds.
const PRESS: f64 = 0.7;
const RELEASE: f64 = 0.3;

/// Forward and centre: stand where the middle should be, look straight ahead, pull a trigger (or pinch) - or, with
/// no controller in use, keep still for 3 s. Returns (centre x z, heading, head height y in the room frame).
pub fn confirm_forward(g: &mut dyn Guide, timeout_s: f64) -> Result<(P2, f64, f64), String> {
    g.say(
        "Centre and forward",
        "Stand in the middle of your play space and look straight ahead (where games should face). Pull a trigger, or hold still for 3 seconds.",
        None,
    );
    let mut was_down = [true, true];
    let mut still_since: Option<(f64, [f64; 3], f64)> = None;
    loop {
        let s = g.sample();
        if s.t > timeout_s {
            return Err("no confirmation in time: run the setup again".into());
        }
        let Some((hp, hy)) = s.head else {
            still_since = None;
            continue;
        };
        for i in 0..2 {
            let down = s.hands[i].active && s.hands[i].value >= PRESS;
            let up = !s.hands[i].active || s.hands[i].value <= RELEASE;
            if down && !was_down[i] && s.t > 0.5 {
                return Ok(([hp[0], hp[2]], hy, hp[1]));
            }
            if up {
                was_down[i] = false;
            } else if down {
                was_down[i] = true;
            }
        }
        // still: within 3 cm and 3 deg for 3 s (after the first 2 s, so the user has heard the prompt)
        match still_since {
            Some((t0, p0, y0)) if dist([p0[0], p0[2]], [hp[0], hp[2]]) < 0.03 && crate::align::wrap(hy - y0).abs() < 3f64.to_radians() => {
                let left = 3.0 - (s.t - t0);
                if s.t > 2.0 && left <= 0.0 {
                    return Ok(([hp[0], hp[2]], hy, hp[1]));
                }
                if s.t > 2.0 {
                    g.say("Centre and forward", &format!("Hold still... {:.0}", left.ceil()), Some((1.0 - left / 3.0).clamp(0.0, 1.0) as f32));
                }
            }
            _ => still_since = Some((s.t, hp, hy)),
        }
    }
}

/// Trace the boundary: hold a trigger (or pinch) and walk along the edge of the free space with the controller
/// pointing at the floor edge; release when back at the start. The loop also closes by itself once the path
/// is longer than 3 m and comes back within 30 cm of the start. Returns the traced points (room frame, x z).
pub fn trace_boundary(g: &mut dyn Guide, timeout_s: f64) -> Result<Vec<P2>, String> {
    g.say(
        "Play area boundary",
        "Hold a trigger and walk along the edge of your free space, controller at hip height. Let go when you are back at the start.",
        None,
    );
    let mut hand: Option<usize> = None;
    let mut path: Vec<P2> = Vec::new();
    let mut length = 0.0;
    let mut saw_input = false;
    loop {
        let s = g.sample();
        if s.t > timeout_s {
            return Err(if saw_input {
                "the boundary was not finished in time: run the setup again".into()
            } else {
                "no controller or hand input: switch a controller on (or turn on camera hands) and run the setup again".into()
            });
        }
        saw_input |= s.hands.iter().any(|h| h.active);
        match hand {
            None => {
                if let Some(i) = (0..2).find(|&i| s.hands[i].active && s.hands[i].value >= PRESS && s.hands[i].pos.is_some()) {
                    hand = Some(i);
                    let p = s.hands[i].pos.unwrap();
                    path = vec![[p[0], p[2]]];
                    length = 0.0;
                }
            }
            Some(i) => {
                let h = s.hands[i];
                if let Some(p) = h.pos {
                    let q = [p[0], p[2]];
                    let d = dist(*path.last().unwrap(), q);
                    if d >= 0.02 {
                        length += d;
                        path.push(q);
                        g.say("Play area boundary", &format!("Tracing... {length:.1} m. Let go when you are back at the start."), None);
                    }
                }
                let released = !h.active || h.value <= RELEASE;
                let closed = length > 3.0 && dist(path[0], *path.last().unwrap()) < 0.3;
                if released || closed {
                    if length < 2.0 {
                        g.say("Play area boundary", "That was too short. Hold the trigger again and walk all the way around.", None);
                        hand = None;
                        path.clear();
                        continue;
                    }
                    return Ok(path);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Script {
        t: f64,
        f: Box<dyn FnMut(f64) -> Sample>,
        said: Vec<String>,
    }

    impl Guide for Script {
        fn say(&mut self, title: &str, text: &str, _p: Option<f32>) {
            self.said.push(format!("{title}: {text}"));
        }
        fn sample(&mut self) -> Sample {
            self.t += 0.02;
            let mut s = (self.f)(self.t);
            s.t = self.t;
            s
        }
    }

    fn script(f: impl FnMut(f64) -> Sample + 'static) -> Script {
        Script { t: 0.0, f: Box::new(f), said: Vec::new() }
    }

    #[test]
    fn frames_round_trip_and_heading() {
        let (c, yaw) = ([1.0, -2.0], 0.7);
        let p = [0.3, 0.9];
        let q = to_room(c, yaw, to_setup(c, yaw, p));
        assert!(dist(p, q) < 1e-12);
        // forward (-Z of the setup frame) in the room frame has the heading yaw
        let f = to_room([0.0, 0.0], yaw, [0.0, -1.0]);
        assert!((heading(f[0], f[1]) - yaw).abs() < 1e-12);
        // the same through the Alignment of the setup (pimax_room's R_y convention)
        let s = RoomSetup { label: "x".into(), mode: SetupMode::Fast, centre: c, yaw, boundary: vec![[0.0; 2]; 3], play_area: [1.0; 2], head_m: 1.6, when: 0 };
        let r = s.room_from_setup().pose().apply([p[0], 0.0, p[1]]);
        let q = to_room(c, yaw, p);
        assert!((r[0] - q[0]).abs() < 1e-12 && (r[2] - q[1]).abs() < 1e-12, "{r:?} {q:?}");
        let back = s.setup_from_room().pose().apply(r);
        assert!((back[0] - p[0]).abs() < 1e-12 && (back[2] - p[1]).abs() < 1e-12);
    }

    #[test]
    fn final_offset_puts_centre_on_the_origin_and_floor_at_zero() {
        // room_from_slam: some yaw and shift; floor at room y = -1.4; Monado origin height 1.6
        let a = Alignment { yaw: 0.3, t: [0.5, 0.1, -0.2] };
        let floor_y = -1.4;
        let dy = crate::floor::floor_shift(floor_y, 1.6);
        let s = RoomSetup { label: "x".into(), mode: SetupMode::Full, centre: [2.0, 1.0], yaw: 1.1, boundary: vec![[0.0; 2]; 3], play_area: [1.0; 2], head_m: 1.6, when: 0 };
        let off = final_offset(&a, dy, Some(&s));
        // a slam point that is the centre on the floor in the room frame
        let room_pt = [2.0, floor_y, 1.0];
        let slam_pt = a.pose().inverse().apply(room_pt);
        let app = off.pose().apply(slam_pt);
        assert!(app[0].abs() < 1e-9 && app[2].abs() < 1e-9 && (app[1] + 1.6).abs() < 1e-9, "{app:?}");
        // forward of the setup in the room frame becomes -Z in the app
        let f = to_room(s.centre, s.yaw, [0.0, -1.0]);
        let app_f = off.pose().apply(a.pose().inverse().apply([f[0], floor_y, f[1]]));
        assert!((app_f[0]).abs() < 1e-9 && (app_f[2] + 1.0).abs() < 1e-9, "{app_f:?}");
        // without a setup: the old offset (floor only)
        let o2 = final_offset(&a, dy, None);
        assert!((o2.t[1] - (a.t[1] + dy)).abs() < 1e-12 && o2.yaw == a.yaw);
    }

    #[test]
    fn toml_round_trip() {
        let s = RoomSetup {
            label: "Living \"room\"".into(),
            mode: SetupMode::Full,
            centre: [0.25, -1.5],
            yaw: 0.5,
            boundary: vec![[0.0, 0.0], [2.0, 0.0], [2.0, 3.0], [0.0, 3.0]],
            play_area: [1.8, 2.6],
            head_m: 1.65,
            when: 1_792_000_000,
        };
        let t = s.to_toml();
        let r = RoomSetup::parse(&t).unwrap();
        assert_eq!(r.label, "Living  room ");
        assert_eq!(r.mode, SetupMode::Full);
        assert_eq!(r.boundary, s.boundary);
        assert!((r.yaw - 0.5).abs() < 1e-6 && r.centre == s.centre && r.play_area == s.play_area && r.when == s.when);
        assert!(RoomSetup::parse("centre = [0, 0]\nyaw_deg = 0\nboundary = [[0,0],[1,0]]\n").is_err());
        assert!(r.describe().contains("1.8 x 2.6 m"), "{}", r.describe());
    }

    #[test]
    fn play_area_of_a_square_an_l_and_an_offset_centre() {
        let sq = vec![[-1.5, -1.0], [1.5, -1.0], [1.5, 1.0], [-1.5, 1.0]];
        let pa = play_area(&sq);
        assert!((pa[0] - 3.0).abs() < 0.12 && (pa[1] - 2.0).abs() < 0.12, "{pa:?}");
        // L shape: the notch at +x +z limits the centred rectangle
        let l = vec![[-2.0, -2.0], [2.0, -2.0], [2.0, 0.5], [0.5, 0.5], [0.5, 2.0], [-2.0, 2.0]];
        let pa = play_area(&l);
        assert!(rect_inside(&l, pa[0] / 2.0 - 1e-6, pa[1] / 2.0 - 1e-6));
        assert!(pa[0] <= 1.0 + 1e-6 || pa[1] <= 1.0 + 1e-6, "{pa:?}");
        assert!(pa[0] * pa[1] > 1.9, "{pa:?}");
        // centre outside: nothing
        let away = vec![[1.0, 1.0], [2.0, 1.0], [2.0, 2.0], [1.0, 2.0]];
        assert_eq!(play_area(&away), [0.0, 0.0]);
    }

    #[test]
    fn boundary_cleanup() {
        // a noisy walk around a 3 x 2 m rectangle, 2 cm steps
        let mut tr = Vec::new();
        let corners = [[0.0, 0.0], [3.0, 0.0], [3.0, 2.0], [0.0, 2.0], [0.0, 0.0]];
        let mut k = 0u32;
        for w in corners.windows(2) {
            let n = (dist(w[0], w[1]) / 0.02) as usize;
            for i in 0..n {
                let f = i as f64 / n as f64;
                k = k.wrapping_mul(1103515245).wrapping_add(12345);
                let jit = ((k >> 16) % 100) as f64 / 100.0 * 0.02 - 0.01;
                tr.push([w[0][0] + (w[1][0] - w[0][0]) * f + jit, w[0][1] + (w[1][1] - w[0][1]) * f - jit]);
            }
        }
        let b = clean_boundary(&tr, 0.05).unwrap();
        assert!(b.len() >= 4 && b.len() <= 8, "{} points", b.len());
        assert!((area(&b).abs() - 6.0).abs() < 0.3, "{}", area(&b));
        assert!(!self_intersects(&b));
        // a figure eight becomes its hull
        let eight = vec![[0.0, 0.0], [2.0, 2.0], [2.0, 0.0], [0.0, 2.0]];
        assert!(self_intersects(&eight));
        let mut tr8 = Vec::new();
        for w in [eight[0], eight[1], eight[2], eight[3], eight[0]].windows(2) {
            for i in 0..50 {
                let f = i as f64 / 50.0;
                tr8.push([w[0][0] + (w[1][0] - w[0][0]) * f, w[0][1] + (w[1][1] - w[0][1]) * f]);
            }
        }
        let b8 = clean_boundary(&tr8, 0.05).unwrap();
        assert!(!self_intersects(&b8) && (area(&b8).abs() - 4.0).abs() < 0.2, "{b8:?}");
        assert!(clean_boundary(&[[0.0, 0.0], [0.5, 0.0], [0.5, 0.5]], 0.05).unwrap_err().contains("too short"));
        let small: Vec<P2> = (0..200).map(|i| { let a = i as f64 / 200.0 * std::f64::consts::TAU; [0.4 * a.cos(), 0.4 * a.sin()] }).collect();
        assert!(clean_boundary(&small, 0.05).unwrap_err().contains("m^2"));
    }

    #[test]
    fn free_area_stops_at_furniture_and_the_map_edge() {
        // nothing known: the default 2 x 2 m square
        let r = free_area(&[], None, 2.5);
        assert!((area(&r).abs() - 4.0).abs() < 1e-9);
        // a sofa 1.2 m in front (z = -1.2), a wall 2.0 m to the right, the map seen up to 1.6 m behind
        let mut obs = Vec::new();
        for i in 0..40 {
            obs.push([-1.0 + i as f64 * 0.05, -1.25]);
            obs.push([2.02, -2.0 + i as f64 * 0.1]);
        }
        let r = free_area(&obs, Some([-3.0, 3.0, -3.0, 1.6]), 2.5);
        let (x0, x1, z0, z1) = (r[0][0], r[1][0], r[0][1], r[2][1]);
        assert!(z0 > -1.26 && z0 < -1.1, "front {z0}");
        assert!(x1 > 1.85 && x1 < 2.02, "right {x1}");
        assert!(z1 <= 1.6 + 1e-9 && z1 > 1.5, "back {z1}");
        assert!((x0 + 2.5).abs() < 1e-9, "left free up to the cap {x0}");
        // obstacles from map points: heights relative to the floor, setup frame
        let pts = vec![[0.0, -1.4, -1.0], [0.0, -0.8, -1.0], [0.0, 1.0, -1.0], [9.0, -0.8, 0.0]];
        let (o, e) = obstacles_from_points(&pts, -1.4, [0.0, 0.0], 0.0);
        assert_eq!(o, vec![[0.0, -1.0]], "floor, ceiling and far points are not obstacles");
        assert!(e.is_none(), "too few points for an extent");
    }

    #[test]
    fn walls_like_steamvr() {
        let w = walls(&[[-0.5, -0.5], [-0.5, 0.5], [0.5, 0.5], [0.5, -0.5]], WALL_HEIGHT_M);
        assert_eq!(w.len(), 4);
        assert_eq!(w[0], [[-0.5, 0.0, -0.5], [-0.5, 2.43, -0.5], [-0.5, 2.43, 0.5], [-0.5, 0.0, 0.5]]);
        assert_eq!(w[3][2], [-0.5, 2.43, -0.5]);
    }

    #[test]
    fn forward_by_trigger_and_by_holding_still() {
        // trigger pulled at 1.5 s while facing +x (heading -90 deg)
        let mut g = script(|t| Sample {
            t,
            head: Some(([0.4, 1.7, -0.3], -std::f64::consts::FRAC_PI_2)),
            hands: [HandSample::default(), HandSample { pos: Some([0.0; 3]), value: if t > 1.5 { 1.0 } else { 0.0 }, active: true }],
        });
        let (c, y, h) = confirm_forward(&mut g, 30.0).unwrap();
        assert_eq!(c, [0.4, -0.3]);
        assert!((y + std::f64::consts::FRAC_PI_2).abs() < 1e-12 && (h - 1.7).abs() < 1e-12);
        assert!(g.t < 1.6);
        // a trigger already held at the start does not count; still for 3 s does
        let mut g = script(|t| Sample {
            t,
            head: Some(([0.0, 1.6, 0.0], 0.2)),
            hands: [HandSample { pos: None, value: 1.0, active: true }, HandSample::default()],
        });
        let (_, y, _) = confirm_forward(&mut g, 30.0).unwrap();
        assert!((y - 0.2).abs() < 1e-12);
        assert!(g.t >= 3.0 && g.t < 3.2, "{}", g.t);
        assert!(g.said.iter().any(|s| s.contains("Hold still")));
        // moving all the time: timeout in plain words
        let mut g = script(|t| Sample { t, head: Some(([t, 1.6, 0.0], t)), hands: Default::default() });
        assert!(confirm_forward(&mut g, 10.0).unwrap_err().contains("run the setup again"));
    }

    #[test]
    fn trace_closes_by_release_and_by_itself() {
        // walk a 2 x 1.5 m rectangle at 0.5 m/s holding the right trigger, release at the end
        let per = 7.0;
        let at = move |d: f64| -> [f64; 3] {
            let d = d.rem_euclid(per);
            let (x, z) = if d < 2.0 { (d, 0.0) } else if d < 3.5 { (2.0, d - 2.0) } else if d < 5.5 { (5.5 - d, 1.5) } else { (0.0, 7.0 - d) };
            [x, 1.0, z]
        };
        let mut g = script(move |t| {
            let d = (t - 1.0).max(0.0) * 0.5;
            let held = t > 1.0 && d < per - 0.5;
            Sample { t, head: None, hands: [HandSample::default(), HandSample { pos: Some(at(d)), value: if held { 1.0 } else { 0.0 }, active: true }] }
        });
        let p = trace_boundary(&mut g, 60.0).unwrap();
        let b = clean_boundary(&p, 0.05).unwrap();
        assert!((area(&b).abs() - 3.0).abs() < 0.3, "{}", area(&b));
        // held all the way: closes by itself when back near the start
        let mut g = script(move |t| {
            let d = (t - 1.0).max(0.0) * 0.5;
            Sample { t, head: None, hands: [HandSample { pos: Some(at(d)), value: if t > 1.0 { 1.0 } else { 0.0 }, active: true }, HandSample::default()] }
        });
        let p = trace_boundary(&mut g, 60.0).unwrap();
        assert!(g.t < 1.0 + per / 0.5 + 0.5, "closed by itself at {}", g.t);
        assert!(dist(p[0], *p.last().unwrap()) < 0.3);
        // a too short trace is restarted, not accepted
        let mut g = script(move |t| {
            let d = (t - 1.0).max(0.0) * 0.5;
            let held = (1.0..2.0).contains(&t) || t > 3.0;
            Sample { t, head: None, hands: [HandSample { pos: Some(at(d)), value: if held { 1.0 } else { 0.0 }, active: true }, HandSample::default()] }
        });
        let _ = trace_boundary(&mut g, 60.0).unwrap();
        assert!(g.said.iter().any(|s| s.contains("too short")));
        // no controller at all
        let mut g = script(|t| Sample { t, head: None, hands: Default::default() });
        assert!(trace_boundary(&mut g, 5.0).unwrap_err().contains("no controller"));
    }
}
