//! Convert a distortion dump of Pimax's own PVR runtime (`windows/pimax-dump-distortion.ps1`) into our radial
//! tangent-space lens model.
//!
//! The dump holds, per eye and colour channel, `pvr_getHmdDistortedUV` sampled on a grid and along four radial lines:
//! panel UV (0..1 over the eye's native panel) -> UV (0..1) into the undistorted render texture that spans the
//! eye's FOV port. This module
//!
//! 1. turns the output UV into a ray tangent (x right, y up) with the eye's FOV port,
//! 2. finds how the native panel axes map to the user-facing frame (one of the 8 flips/rotations of the square, and
//!    which panel size convention fits) and the optical centre, by a nonlinear least squares over the centre with an
//!    inner linear least squares for `tan = a0 r + a1 r^3 + ... + a5 r^11` (`base = a0`, `k_i = a_i / a0`),
//! 3. fits the red and blue channels as lateral chromatic aberration (`ca`, `shift`) of the green polynomial
//!    (or, with `per_channel`, as their own polynomial),
//! 4. reports residuals in degrees per radius, tests radial symmetry, and writes a `lens.conf` variant.
//!
//! The panel unit is the one of `lens.conf`: half of the smaller panel dimension.

use crate::config::{self, LensConfig, CHANNELS, K_TERMS};
use crate::model::radial_tangent;
use std::fmt::Write as _;

// ---------------------------------------------------------------------------------------------------------------
// Minimal JSON reader (the workspace has no serde; the dump is machine written and trusted only as far as parsing).
// ---------------------------------------------------------------------------------------------------------------

pub mod json {
    #[derive(Debug, Clone, PartialEq)]
    pub enum Value {
        Null,
        Bool(bool),
        Num(f64),
        Str(String),
        Arr(Vec<Value>),
        Obj(Vec<(String, Value)>),
    }

    impl Value {
        pub fn get(&self, key: &str) -> Option<&Value> {
            match self {
                Value::Obj(v) => v.iter().find(|(k, _)| k == key).map(|(_, v)| v),
                _ => None,
            }
        }
        /// Follow a `/`-separated path of object keys.
        pub fn path(&self, p: &str) -> Option<&Value> {
            let mut cur = self;
            for part in p.split('/') {
                cur = cur.get(part)?;
            }
            Some(cur)
        }
        pub fn f64(&self) -> Option<f64> {
            match self {
                Value::Num(n) => Some(*n),
                _ => None,
            }
        }
        pub fn str(&self) -> Option<&str> {
            match self {
                Value::Str(s) => Some(s),
                _ => None,
            }
        }
        pub fn arr(&self) -> Option<&[Value]> {
            match self {
                Value::Arr(a) => Some(a),
                _ => None,
            }
        }
        /// Array of numbers; `null` (NaN/Inf written by the tool) becomes NaN.
        pub fn floats(&self) -> Option<Vec<f64>> {
            self.arr()?
                .iter()
                .map(|v| match v {
                    Value::Num(n) => Some(*n),
                    Value::Null => Some(f64::NAN),
                    _ => None,
                })
                .collect()
        }
    }

    struct P<'a> {
        s: &'a [u8],
        i: usize,
    }

    impl P<'_> {
        fn ws(&mut self) {
            while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
                self.i += 1;
            }
        }
        fn err<T>(&self, m: &str) -> Result<T, String> {
            Err(format!("json: {m} at byte {}", self.i))
        }
        fn lit(&mut self, word: &str, v: Value) -> Result<Value, String> {
            if self.s[self.i..].starts_with(word.as_bytes()) {
                self.i += word.len();
                Ok(v)
            } else {
                self.err("bad literal")
            }
        }
        fn string(&mut self) -> Result<String, String> {
            // at the opening quote
            self.i += 1;
            let mut out = Vec::new();
            loop {
                let Some(&c) = self.s.get(self.i) else {
                    return self.err("unterminated string");
                };
                self.i += 1;
                match c {
                    b'"' => break,
                    b'\\' => {
                        let Some(&e) = self.s.get(self.i) else {
                            return self.err("bad escape");
                        };
                        self.i += 1;
                        match e {
                            b'n' => out.push(b'\n'),
                            b'r' => out.push(b'\r'),
                            b't' => out.push(b'\t'),
                            b'b' => out.push(8),
                            b'f' => out.push(12),
                            b'u' => {
                                let h = self
                                    .s
                                    .get(self.i..self.i + 4)
                                    .and_then(|h| std::str::from_utf8(h).ok())
                                    .and_then(|h| u32::from_str_radix(h, 16).ok());
                                let Some(cp) = h else {
                                    return self.err("bad \\u escape");
                                };
                                self.i += 4;
                                let ch = char::from_u32(cp).unwrap_or('?');
                                let mut b = [0u8; 4];
                                out.extend_from_slice(ch.encode_utf8(&mut b).as_bytes());
                            }
                            other => out.push(other),
                        }
                    }
                    _ => out.push(c),
                }
            }
            Ok(String::from_utf8_lossy(&out).into_owned())
        }
        fn value(&mut self) -> Result<Value, String> {
            self.ws();
            let Some(&c) = self.s.get(self.i) else {
                return self.err("unexpected end");
            };
            match c {
                b'{' => {
                    self.i += 1;
                    let mut v = Vec::new();
                    self.ws();
                    if self.s.get(self.i) == Some(&b'}') {
                        self.i += 1;
                        return Ok(Value::Obj(v));
                    }
                    loop {
                        self.ws();
                        if self.s.get(self.i) != Some(&b'"') {
                            return self.err("expected key");
                        }
                        let k = self.string()?;
                        self.ws();
                        if self.s.get(self.i) != Some(&b':') {
                            return self.err("expected ':'");
                        }
                        self.i += 1;
                        v.push((k, self.value()?));
                        self.ws();
                        match self.s.get(self.i) {
                            Some(b',') => self.i += 1,
                            Some(b'}') => {
                                self.i += 1;
                                return Ok(Value::Obj(v));
                            }
                            _ => return self.err("expected ',' or '}'"),
                        }
                    }
                }
                b'[' => {
                    self.i += 1;
                    let mut v = Vec::new();
                    self.ws();
                    if self.s.get(self.i) == Some(&b']') {
                        self.i += 1;
                        return Ok(Value::Arr(v));
                    }
                    loop {
                        v.push(self.value()?);
                        self.ws();
                        match self.s.get(self.i) {
                            Some(b',') => self.i += 1,
                            Some(b']') => {
                                self.i += 1;
                                return Ok(Value::Arr(v));
                            }
                            _ => return self.err("expected ',' or ']'"),
                        }
                    }
                }
                b'"' => Ok(Value::Str(self.string()?)),
                b't' => self.lit("true", Value::Bool(true)),
                b'f' => self.lit("false", Value::Bool(false)),
                b'n' => self.lit("null", Value::Null),
                _ => {
                    let st = self.i;
                    while self.i < self.s.len()
                        && matches!(
                            self.s[self.i],
                            b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'
                        )
                    {
                        self.i += 1;
                    }
                    std::str::from_utf8(&self.s[st..self.i])
                        .ok()
                        .and_then(|t| t.parse::<f64>().ok())
                        .map(Value::Num)
                        .ok_or_else(|| format!("json: bad number at byte {st}"))
                }
            }
        }
    }

    pub fn parse(text: &str) -> Result<Value, String> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut p = P {
            s: text.as_bytes(),
            i: 0,
        };
        let v = p.value()?;
        p.ws();
        if p.i != p.s.len() {
            return p.err("trailing data");
        }
        Ok(v)
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The dump
// ---------------------------------------------------------------------------------------------------------------

/// One radial line of the dump: direction in panel UV (`+x`, `-x`, `+y`, `-y`) and the output UVs per channel.
#[derive(Clone, Debug)]
pub struct LineData {
    pub name: String,
    pub dir: [f64; 2],
    pub out: [Vec<f64>; CHANNELS],
}

/// Everything the fit needs of one eye.
#[derive(Clone, Debug)]
pub struct EyeDump {
    pub name: String,
    /// FOV port tangents: up, down, left, right (all positive).
    pub fov_tan: [f64; 4],
    /// HmdToEyePose orientation (x, y, z, w) and position.
    pub quat: [f64; 4],
    pub pos: [f64; 3],
    /// Native panel size of this eye in pixels as the runtime reports it.
    pub panel: (f64, f64),
    pub grid_n: usize,
    /// Output UV pairs `[u0, v0, u1, v1, ...]`, row-major with v outer, per channel (R, G, B).
    pub grid: [Vec<f64>; CHANNELS],
    pub lines: Vec<LineData>,
    pub line_n: usize,
    pub failed_calls: i64,
}

#[derive(Clone, Debug)]
pub struct Dump {
    pub serial: String,
    pub product: String,
    pub eyes: [EyeDump; 2],
    /// `view_rotation_fix` config value read from the runtime (1 = parallel projection), if present.
    pub view_rotation_fix: Option<i64>,
}

fn num(v: &json::Value, path: &str) -> Result<f64, String> {
    v.path(path)
        .and_then(json::Value::f64)
        .ok_or_else(|| format!("dump: missing number '{path}'"))
}

fn floats(v: &json::Value, path: &str) -> Result<Vec<f64>, String> {
    v.path(path)
        .and_then(json::Value::floats)
        .ok_or_else(|| format!("dump: missing number array '{path}'"))
}

/// Parse the JSON text of a `pimax-distortion-*.json`.
pub fn parse_dump(text: &str) -> Result<Dump, String> {
    let root = json::parse(text)?;
    if root.get("eyes").is_none() {
        return Err("dump: no 'eyes' array; is this a pimax-distortion-*.json?".into());
    }
    let eyes_v = root
        .get("eyes")
        .and_then(json::Value::arr)
        .ok_or("dump: 'eyes' is not an array")?;
    if eyes_v.len() != 2 {
        return Err(format!("dump: expected 2 eyes, found {}", eyes_v.len()));
    }
    let mut eyes: Vec<EyeDump> = Vec::new();
    for (i, ev) in eyes_v.iter().enumerate() {
        let name = ev
            .get("eye")
            .and_then(json::Value::str)
            .unwrap_or(if i == 0 { "left" } else { "right" })
            .to_string();
        let fov_tan = [
            num(ev, "fov_tan/up")?,
            num(ev, "fov_tan/down")?,
            num(ev, "fov_tan/left")?,
            num(ev, "fov_tan/right")?,
        ];
        let q = floats(ev, "hmd_to_eye_pose/orientation_xyzw")?;
        let p = floats(ev, "hmd_to_eye_pose/position")?;
        if q.len() != 4 || p.len() != 3 {
            return Err("dump: bad hmd_to_eye_pose".into());
        }
        // native panel size of this eye
        let disp = root.path("display").and_then(|d| d.get(&name));
        let (mut w, mut h) = (0.0, 0.0);
        if let Some(d) = disp {
            let (dw, dh) = (
                d.get("width").and_then(json::Value::f64).unwrap_or(0.0),
                d.get("height").and_then(json::Value::f64).unwrap_or(0.0),
            );
            match d
                .get("eye_display")
                .and_then(json::Value::str)
                .unwrap_or("")
            {
                "horz_side_by_side" => (w, h) = (dw / 2.0, dh),
                "vert_side_by_side" => (w, h) = (dw, dh / 2.0),
                _ => (w, h) = (dw, dh),
            }
        }
        if w <= 0.0 || h <= 0.0 {
            (w, h) = (3840.0, 3552.0);
        }
        let dist = ev
            .get("distortion")
            .ok_or("dump: eye without 'distortion'")?;
        let grid_n = num(dist, "grid_n")? as usize;
        let grid = [
            floats(dist, "grid_r")?,
            floats(dist, "grid_g")?,
            floats(dist, "grid_b")?,
        ];
        if grid.iter().any(|g| g.len() != grid_n * grid_n * 2) {
            return Err(format!(
                "dump: grid arrays do not hold {grid_n}x{grid_n} points"
            ));
        }
        let line_n = dist.get("line_n").and_then(json::Value::f64).unwrap_or(0.0) as usize;
        let mut lines = Vec::new();
        for (name, dir) in [
            ("+x", [1.0, 0.0]),
            ("-x", [-1.0, 0.0]),
            ("+y", [0.0, 1.0]),
            ("-y", [0.0, -1.0]),
        ] {
            if let Some(l) = dist.path("lines").and_then(|l| l.get(name)) {
                let out = [floats(l, "r")?, floats(l, "g")?, floats(l, "b")?];
                if out.iter().any(|o| o.len() != line_n * 2) {
                    return Err(format!("dump: line {name} does not hold {line_n} points"));
                }
                lines.push(LineData {
                    name: name.to_string(),
                    dir,
                    out,
                });
            }
        }
        eyes.push(EyeDump {
            name,
            fov_tan,
            quat: [q[0], q[1], q[2], q[3]],
            pos: [p[0], p[1], p[2]],
            panel: (w, h),
            grid_n,
            grid,
            lines,
            line_n,
            failed_calls: dist
                .get("failed_calls")
                .and_then(json::Value::f64)
                .unwrap_or(0.0) as i64,
        });
    }
    let right = eyes.pop().unwrap();
    let left = eyes.pop().unwrap();
    Ok(Dump {
        serial: root
            .path("hmd/serial")
            .and_then(json::Value::str)
            .unwrap_or("")
            .to_string(),
        product: root
            .path("hmd/product_name")
            .and_then(json::Value::str)
            .unwrap_or("")
            .to_string(),
        eyes: [left, right],
        view_rotation_fix: root
            .path("config/view_rotation_fix_default_-1")
            .and_then(json::Value::f64)
            .map(|v| v as i64),
    })
}

/// Yaw (degrees, rotation about +Y; positive turns the view to the left) of a quaternion (x, y, z, w).
pub fn yaw_deg(q: [f64; 4]) -> f64 {
    let [x, y, z, w] = q;
    (2.0 * (w * y + x * z))
        .atan2(1.0 - 2.0 * (x * x + y * y))
        .to_degrees()
}

/// Pitch (about +X) in degrees.
pub fn pitch_deg(q: [f64; 4]) -> f64 {
    let [x, y, z, w] = q;
    (2.0 * (w * x - y * z)).clamp(-1.0, 1.0).asin().to_degrees()
}

/// Outward cant of each display from the two HmdToEye orientations: `(yaw_left - yaw_right) / 2`, the sign
/// convention of `cant_deg` (left view yawed +, right -).
pub fn cant_from_poses(d: &Dump) -> f64 {
    (yaw_deg(d.eyes[0].quat) - yaw_deg(d.eyes[1].quat)) / 2.0
}

/// Distance between the two eye positions in millimetres.
pub fn ipd_mm(d: &Dump) -> f64 {
    let (a, b) = (d.eyes[0].pos, d.eyes[1].pos);
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt() * 1000.0
}

// ---------------------------------------------------------------------------------------------------------------
// Fitting
// ---------------------------------------------------------------------------------------------------------------

/// Options of [`fit_dump`].
#[derive(Clone, Debug)]
pub struct Options {
    /// Polynomial terms k1..kN (1..=5).
    pub terms: usize,
    /// Output V runs downwards (0 = top) instead of upwards (0 = bottom of the FOV port).
    pub v_down: bool,
    /// Fit red and blue as their own polynomials instead of `ca`/`shift` of the green one.
    pub per_channel: bool,
    /// Largest accepted ray angle error for a sample to count as inlier, degrees.
    pub inlier_deg: f64,
    /// Declare radial symmetry when the green rms error and the sector spread are below this, degrees.
    pub sym_deg: f64,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            terms: K_TERMS,
            v_down: false,
            per_channel: false,
            inlier_deg: 3.0,
            sym_deg: 0.15,
        }
    }
}

pub(crate) type Dm = [[f64; 2]; 2];

/// The 8 symmetries of the square, with a description of what the user-facing axes are in panel axes.
pub(crate) const ORIENTS: [(Dm, &str); 8] = [
    (
        [[1.0, 0.0], [0.0, 1.0]],
        "user x = panel u, user y = panel v",
    ),
    (
        [[0.0, -1.0], [1.0, 0.0]],
        "user x = -panel v, user y = panel u",
    ),
    (
        [[-1.0, 0.0], [0.0, -1.0]],
        "user x = -panel u, user y = -panel v",
    ),
    (
        [[0.0, 1.0], [-1.0, 0.0]],
        "user x = panel v, user y = -panel u",
    ),
    (
        [[-1.0, 0.0], [0.0, 1.0]],
        "user x = -panel u, user y = panel v",
    ),
    (
        [[1.0, 0.0], [0.0, -1.0]],
        "user x = panel u, user y = -panel v",
    ),
    (
        [[0.0, 1.0], [1.0, 0.0]],
        "user x = panel v, user y = panel u",
    ),
    (
        [[0.0, -1.0], [-1.0, 0.0]],
        "user x = -panel v, user y = -panel u",
    ),
];

fn apply(d: &Dm, p: [f64; 2]) -> [f64; 2] {
    [
        d[0][0] * p[0] + d[0][1] * p[1],
        d[1][0] * p[0] + d[1][1] * p[1],
    ]
}

/// One input/output sample: panel UV relative to the panel centre, and the ray tangent seen there.
#[derive(Clone, Copy, Debug)]
struct Sample {
    uv: [f64; 2],
    t: [f64; 2],
    w: f64,
}

fn samples_of(eye: &EyeDump, c: usize, v_down: bool) -> Vec<Sample> {
    let [up, dn, l, r] = eye.fov_tan;
    let mut out = Vec::new();
    let mut push = |u: f64, v: f64, ou: f64, ov: f64| {
        if !(ou.is_finite() && ov.is_finite())
            || !(-0.002..=1.002).contains(&ou)
            || !(-0.002..=1.002).contains(&ov)
        {
            return;
        }
        let tx = -l + ou * (l + r);
        let ty = if v_down {
            up - ov * (up + dn)
        } else {
            -dn + ov * (up + dn)
        };
        let t2 = tx * tx + ty * ty;
        out.push(Sample {
            uv: [u - 0.5, v - 0.5],
            t: [tx, ty],
            w: 1.0 / ((1.0 + t2) * (1.0 + t2)),
        });
    };
    let n = eye.grid_n;
    for j in 0..n {
        for i in 0..n {
            let k = (j * n + i) * 2;
            let (u, v) = (i as f64 / (n - 1) as f64, j as f64 / (n - 1) as f64);
            push(u, v, eye.grid[c][k], eye.grid[c][k + 1]);
        }
    }
    for l in &eye.lines {
        for k in 0..eye.line_n {
            let s = k as f64 / (eye.line_n - 1) as f64;
            let (u, v) = (0.5 + l.dir[0] * 0.5 * s, 0.5 + l.dir[1] * 0.5 * s);
            push(u, v, l.out[c][2 * k], l.out[c][2 * k + 1]);
        }
    }
    out
}

/// Solve `A x = b` by Gaussian elimination with partial pivoting.
fn solve(a: Vec<Vec<f64>>, b: Vec<f64>) -> Option<Vec<f64>> {
    crate::fit::solve(a, b)
}

/// Weighted linear least squares of `t = sum_j a_j rho^(2j) d` for a fixed centre. Returns the coefficients and the
/// weighted squared residual.
fn radial_ls(
    s: &[Sample],
    panel: (f64, f64),
    unit: f64,
    dm: &Dm,
    c: [f64; 2],
    nterms: usize,
) -> Option<(Vec<f64>, f64)> {
    let n = nterms + 1;
    let mut a = vec![vec![0.0; n]; n];
    let mut b = vec![0.0; n];
    let mut ttw = 0.0;
    let mut pw = vec![0.0; 2 * n];
    for sm in s {
        let q = apply(dm, [sm.uv[0] * panel.0 / unit, sm.uv[1] * panel.1 / unit]);
        let d = [q[0] - c[0], q[1] - c[1]];
        let r2 = d[0] * d[0] + d[1] * d[1];
        pw[0] = 1.0;
        for i in 1..2 * n {
            pw[i] = pw[i - 1] * r2;
        }
        let dt = d[0] * sm.t[0] + d[1] * sm.t[1];
        for j in 0..n {
            for k in 0..n {
                a[j][k] += sm.w * pw[j + k] * r2;
            }
            b[j] += sm.w * pw[j] * dt;
        }
        ttw += sm.w * (sm.t[0] * sm.t[0] + sm.t[1] * sm.t[1]);
    }
    let x = solve(a.clone(), b.clone())?;
    if x[0] <= 0.0 {
        // a flipped orientation fits with a negative focal term: not a physical lens
        return None;
    }
    let mut cost = ttw;
    for j in 0..n {
        cost -= 2.0 * x[j] * b[j];
        for k in 0..n {
            cost += x[j] * a[j][k] * x[k];
        }
    }
    Some((x, cost.max(0.0)))
}

fn nelder_mead(
    mut f: impl FnMut(&[f64]) -> f64,
    x0: &[f64],
    step: f64,
    iters: usize,
) -> (Vec<f64>, f64) {
    let n = x0.len();
    let mut pts: Vec<Vec<f64>> = vec![x0.to_vec()];
    for i in 0..n {
        let mut p = x0.to_vec();
        p[i] += step;
        pts.push(p);
    }
    let mut vals: Vec<f64> = pts.iter().map(|p| f(p)).collect();
    for _ in 0..iters {
        let mut idx: Vec<usize> = (0..=n).collect();
        idx.sort_by(|&a, &b| vals[a].total_cmp(&vals[b]));
        pts = idx.iter().map(|&i| pts[i].clone()).collect();
        vals = idx.iter().map(|&i| vals[i]).collect();
        if (vals[n] - vals[0]).abs() <= 1e-15 * (1.0 + vals[0].abs()) {
            break;
        }
        let mut cen = vec![0.0; n];
        for p in &pts[..n] {
            for (c, x) in cen.iter_mut().zip(p) {
                *c += x / n as f64;
            }
        }
        let mix = |t: f64, from: &[f64]| -> Vec<f64> {
            cen.iter().zip(from).map(|(c, w)| c + t * (c - w)).collect()
        };
        let worst = pts[n].clone();
        let xr = mix(1.0, &worst);
        let fr = f(&xr);
        if fr < vals[0] {
            let xe = mix(2.0, &worst);
            let fe = f(&xe);
            if fe < fr {
                pts[n] = xe;
                vals[n] = fe;
            } else {
                pts[n] = xr;
                vals[n] = fr;
            }
        } else if fr < vals[n - 1] {
            pts[n] = xr;
            vals[n] = fr;
        } else {
            let xc = if fr < vals[n] {
                mix(0.5, &worst)
            } else {
                mix(-0.5, &worst)
            };
            let fc = f(&xc);
            if fc < vals[n].min(fr) {
                pts[n] = xc;
                vals[n] = fc;
            } else {
                let best = pts[0].clone();
                for i in 1..=n {
                    for (x, b) in pts[i].iter_mut().zip(&best) {
                        *x = b + 0.5 * (*x - b);
                    }
                    vals[i] = f(&pts[i]);
                }
            }
        }
    }
    let mut bi = 0;
    for i in 1..=n {
        if vals[i] < vals[bi] {
            bi = i;
        }
    }
    (pts[bi].clone(), vals[bi])
}

/// Angle in degrees between the rays (t, 1) and (m, 1).
fn ray_angle_deg(t: [f64; 2], m: [f64; 2]) -> f64 {
    let (u, v) = ([t[0], t[1], 1.0], [m[0], m[1], 1.0]);
    let cr = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    let cn = (cr[0] * cr[0] + cr[1] * cr[1] + cr[2] * cr[2]).sqrt();
    let dot = u[0] * v[0] + u[1] * v[1] + u[2] * v[2];
    cn.atan2(dot).to_degrees()
}

/// Polynomial `sum a_j rho^(2j)` applied to the vector `d`: the model's tangent.
fn model_t(a: &[f64], d: [f64; 2]) -> [f64; 2] {
    let r2 = d[0] * d[0] + d[1] * d[1];
    let mut f = 0.0;
    let mut pw = 1.0;
    for aj in a {
        f += aj * pw;
        pw *= r2;
    }
    [f * d[0], f * d[1]]
}

/// Residual of one radius bin.
#[derive(Clone, Debug, PartialEq)]
pub struct Bin {
    pub r_lo: f64,
    pub r_hi: f64,
    pub n: usize,
    pub rms_deg: f64,
    pub max_deg: f64,
}

/// Fit result for one colour channel of one eye.
#[derive(Clone, Debug)]
pub struct ChanFit {
    pub base: f64,
    pub k: [f64; K_TERMS],
    /// Radius divisor and offset of the channel relative to the green centre (green: 1 and 0).
    pub ca: f64,
    pub shift: [f64; 2],
    /// Optical centre of this fit in the user-facing frame (half-min-dimension units, +x right, +y up).
    pub centre: [f64; 2],
    pub n: usize,
    pub rms_deg: f64,
    pub max_deg: f64,
    /// Largest radius (in p' units) of an inlier.
    pub rmax: f64,
    pub bins: Vec<Bin>,
}

/// Radial symmetry analysis of the green channel.
#[derive(Clone, Debug)]
pub struct Symmetry {
    pub symmetric: bool,
    pub rms_deg: f64,
    pub max_deg: f64,
    /// Largest difference between the mean radial error of any two azimuth sectors, over the radial bins.
    pub sector_spread_deg: f64,
    /// rms of the azimuthal (twist) deviation of the data vectors from the radial direction, degrees.
    pub twist_rms_deg: f64,
    /// Mean radial error [radial bin][sector], degrees (NaN where empty); bins `[0,.3) [.3,.6) [.6,.9) [.9,inf)`.
    pub table: [[f64; 8]; 4],
    pub verdict: String,
}

#[derive(Clone, Debug)]
pub struct EyeFit {
    pub name: String,
    pub orient: usize,
    pub panel_used: (f64, f64),
    pub unit_px: f64,
    pub chans: [ChanFit; CHANNELS],
    pub symmetry: Symmetry,
    /// Render FOV half-angles needed to cover the valid data: left, right, up, down (degrees).
    pub fov_deg: [f64; 4],
    pub valid_points: usize,
    pub inlier_points: usize,
}

fn orient_desc(i: usize) -> &'static str {
    ORIENTS[i].1
}

/// Which orientation / panel size was chosen, human readable.
pub fn describe_orientation(e: &EyeFit) -> String {
    format!(
        "{} (panel {}x{} px, unit {} px)",
        orient_desc(e.orient),
        e.panel_used.0,
        e.panel_used.1,
        e.unit_px
    )
}

#[allow(clippy::too_many_arguments)]
fn residual_stats(
    s: &[Sample],
    panel: (f64, f64),
    unit: f64,
    dm: &Dm,
    c: [f64; 2],
    a: &[f64],
    ca: f64,
    shift: [f64; 2],
) -> Vec<f64> {
    s.iter()
        .map(|sm| {
            let q = apply(dm, [sm.uv[0] * panel.0 / unit, sm.uv[1] * panel.1 / unit]);
            let d = [(q[0] - c[0] - shift[0]) / ca, (q[1] - c[1] - shift[1]) / ca];
            ray_angle_deg(sm.t, model_t(a, d))
        })
        .collect()
}

fn bins_of(r: &[f64], err: &[f64]) -> Vec<Bin> {
    let rmax = r.iter().cloned().fold(0.0, f64::max);
    let nb = (rmax / 0.1).ceil().max(1.0) as usize;
    let mut v: Vec<(usize, f64, f64)> = vec![(0, 0.0, 0.0); nb];
    for (&rr, &e) in r.iter().zip(err) {
        let b = ((rr / 0.1) as usize).min(nb - 1);
        v[b].0 += 1;
        v[b].1 += e * e;
        v[b].2 = f64::max(v[b].2, e);
    }
    v.iter()
        .enumerate()
        .filter(|(_, x)| x.0 > 0)
        .map(|(i, x)| Bin {
            r_lo: i as f64 * 0.1,
            r_hi: (i + 1) as f64 * 0.1,
            n: x.0,
            rms_deg: (x.1 / x.0 as f64).sqrt(),
            max_deg: x.2,
        })
        .collect()
}

fn rms_max(err: &[f64]) -> (f64, f64) {
    if err.is_empty() {
        return (f64::NAN, f64::NAN);
    }
    let sq: f64 = err.iter().map(|e| e * e).sum();
    (
        (sq / err.len() as f64).sqrt(),
        err.iter().cloned().fold(0.0, f64::max),
    )
}

struct Core {
    orient: usize,
    panel: (f64, f64),
    centre: [f64; 2],
    a: Vec<f64>,
    inl: Vec<bool>,
}

/// Fit a free radial model (centre + polynomial) on the samples selected by `mask`.
#[allow(clippy::too_many_arguments)]
fn fit_free(
    s: &[Sample],
    mask: &[bool],
    panel: (f64, f64),
    unit: f64,
    dm: &Dm,
    c0: [f64; 2],
    nterms: usize,
    iters: usize,
) -> Option<([f64; 2], Vec<f64>, f64)> {
    let sel: Vec<Sample> = s
        .iter()
        .zip(mask)
        .filter(|(_, &m)| m)
        .map(|(x, _)| *x)
        .collect();
    if sel.len() < 4 * (nterms + 1) {
        return None;
    }
    let ttw: f64 = sel
        .iter()
        .map(|x| x.w * (x.t[0] * x.t[0] + x.t[1] * x.t[1]))
        .sum();
    let f = |c: &[f64]| {
        radial_ls(&sel, panel, unit, dm, [c[0], c[1]], nterms)
            .map_or(f64::MAX, |r| r.1 / ttw.max(1e-30))
    };
    let (c, _) = nelder_mead(f, &c0, 0.05, iters);
    let (a, cost) = radial_ls(&sel, panel, unit, dm, [c[0], c[1]], nterms)?;
    Some(([c[0], c[1]], a, cost / ttw.max(1e-30)))
}

/// Find orientation, panel convention, centre and polynomial of the green channel.
fn fit_core(s: &[Sample], panels: &[(f64, f64)], unit: f64, opt: &Options) -> Option<Core> {
    // the central part of the panel, thinned, is enough to pick the orientation
    let nterms = opt.terms.min(3);
    type Cand = (f64, usize, (f64, f64), [f64; 2]);
    let mut best: Option<Cand> = None;
    for &panel in panels {
        let central: Vec<bool> = s
            .iter()
            .map(|x| {
                let p = [x.uv[0] * panel.0 / unit, x.uv[1] * panel.1 / unit];
                p[0] * p[0] + p[1] * p[1] < 0.6 * 0.6
            })
            .collect();
        let thin: Vec<bool> = central
            .iter()
            .enumerate()
            .map(|(i, &c)| c && i % 3 == 0)
            .collect();
        for (oi, (dm, _)) in ORIENTS.iter().enumerate() {
            if let Some((c, _a, rel)) = fit_free(s, &thin, panel, unit, dm, [0.0, 0.0], nterms, 90)
            {
                // a physical lens has its centre near the middle of the panel and a sane focal term; fits that run
                // away (huge centre, tiny or huge a0) match nothing and only look good through numeric cancellation
                if !(c[0].hypot(c[1]) < 0.35 && (0.1..5.0).contains(&_a[0]) && rel.is_finite()) {
                    continue;
                }
                if !best.as_ref().is_some_and(|b| rel >= b.0) {
                    best = Some((rel, oi, panel, c));
                }
            }
        }
    }
    let (_, orient, panel, c0) = best?;
    let dm = &ORIENTS[orient].0;
    // grow the inlier set: start with the centre, then everything the model explains
    let mut mask: Vec<bool> = s
        .iter()
        .map(|x| {
            let q = apply(dm, [x.uv[0] * panel.0 / unit, x.uv[1] * panel.1 / unit]);
            (q[0] - c0[0]).hypot(q[1] - c0[1]) < 0.6
        })
        .collect();
    let mut centre = c0;
    let mut a = Vec::new();
    for round in 0..5 {
        let (c, aa, _) = fit_free(s, &mask, panel, unit, dm, centre, opt.terms, 160)?;
        centre = c;
        a = aa;
        let err = residual_stats(s, panel, unit, dm, centre, &a, 1.0, [0.0, 0.0]);
        let thr = if round < 2 {
            opt.inlier_deg * 2.0
        } else {
            opt.inlier_deg
        };
        mask = err.iter().map(|&e| e < thr).collect();
    }
    Some(Core {
        orient,
        panel,
        centre,
        a,
        inl: mask,
    })
}

fn chan_from(
    a: &[f64],
    centre: [f64; 2],
    ca: f64,
    shift: [f64; 2],
    err: &[f64],
    rad: &[f64],
) -> ChanFit {
    let (rms, mx) = rms_max(err);
    let mut k = [0.0; K_TERMS];
    for j in 1..a.len().min(K_TERMS + 1) {
        k[j - 1] = a[j] / a[0];
    }
    ChanFit {
        base: a[0],
        k,
        ca,
        shift,
        centre,
        n: err.len(),
        rms_deg: rms,
        max_deg: mx,
        rmax: rad.iter().cloned().fold(0.0, f64::max),
        bins: bins_of(rad, err),
    }
}

/// Fit both eyes of a dump.
pub fn fit_dump(dump: &Dump, opt: &Options) -> Result<[EyeFit; 2], String> {
    let mut out = Vec::new();
    for eye in &dump.eyes {
        out.push(fit_eye(eye, opt)?);
    }
    let r = out.pop().unwrap();
    let l = out.pop().unwrap();
    Ok([l, r])
}

fn fit_eye(eye: &EyeDump, opt: &Options) -> Result<EyeFit, String> {
    let (w, h) = eye.panel;
    let unit = w.min(h) / 2.0;
    let mut panels = vec![(w, h), (h, w), (3840.0, 3552.0), (3552.0, 3840.0)];
    panels.dedup();
    let sg = samples_of(eye, 1, opt.v_down);
    if sg.len() < 100 {
        return Err(format!(
            "eye {}: only {} usable green samples (failed calls: {})",
            eye.name,
            sg.len(),
            eye.failed_calls
        ));
    }
    let core = fit_core(&sg, &panels, unit, opt)
        .ok_or_else(|| format!("eye {}: the radial fit did not converge", eye.name))?;
    let dm = ORIENTS[core.orient].0;
    let rad_of = |s: &[Sample], c: [f64; 2], ca: f64, sh: [f64; 2]| -> Vec<f64> {
        s.iter()
            .map(|x| {
                let q = apply(
                    &dm,
                    [x.uv[0] * core.panel.0 / unit, x.uv[1] * core.panel.1 / unit],
                );
                ((q[0] - c[0] - sh[0]) / ca).hypot((q[1] - c[1] - sh[1]) / ca)
            })
            .collect()
    };
    // green statistics over its inliers
    let gsel: Vec<Sample> = sg
        .iter()
        .zip(&core.inl)
        .filter(|(_, &m)| m)
        .map(|(x, _)| *x)
        .collect();
    let gerr = residual_stats(
        &gsel,
        core.panel,
        unit,
        &dm,
        core.centre,
        &core.a,
        1.0,
        [0.0, 0.0],
    );
    let grad = rad_of(&gsel, core.centre, 1.0, [0.0, 0.0]);
    let green = chan_from(&core.a, core.centre, 1.0, [0.0, 0.0], &gerr, &grad);

    let mut chans: Vec<ChanFit> = Vec::new();
    for c in 0..CHANNELS {
        if c == 1 {
            chans.push(green.clone());
            continue;
        }
        let s = samples_of(eye, c, opt.v_down);
        // the green inlier set is defined on the sample list of green; rebuild the same selection by position:
        // R/B drop the same invalid points only when valid everywhere, so select by the model instead
        let rad0 = rad_of(&s, core.centre, 1.0, [0.0, 0.0]);
        let rlim = green.rmax * 1.02;
        let sel: Vec<Sample> = s
            .iter()
            .zip(&rad0)
            .filter(|(_, &r)| r <= rlim)
            .map(|(x, _)| *x)
            .collect();
        if sel.len() < 100 {
            return Err(format!(
                "eye {}: channel {c} has only {} usable samples",
                eye.name,
                sel.len()
            ));
        }
        if opt.per_channel {
            let mask = vec![true; sel.len()];
            // robust: start from the green centre, two inlier rounds
            let mut m = mask;
            let mut centre = core.centre;
            let mut a = core.a.clone();
            for round in 0..3 {
                let (cc, aa, _) = fit_free(&sel, &m, core.panel, unit, &dm, centre, opt.terms, 160)
                    .ok_or("per-channel fit failed")?;
                centre = cc;
                a = aa;
                let e = residual_stats(&sel, core.panel, unit, &dm, centre, &a, 1.0, [0.0, 0.0]);
                let thr = if round < 1 {
                    opt.inlier_deg * 2.0
                } else {
                    opt.inlier_deg
                };
                m = e.iter().map(|&x| x < thr).collect();
            }
            let isel: Vec<Sample> = sel
                .iter()
                .zip(&m)
                .filter(|(_, &q)| q)
                .map(|(x, _)| *x)
                .collect();
            let e = residual_stats(&isel, core.panel, unit, &dm, centre, &a, 1.0, [0.0, 0.0]);
            let rr = rad_of(&isel, centre, 1.0, [0.0, 0.0]);
            let mut cf = chan_from(
                &a,
                centre,
                1.0,
                [centre[0] - core.centre[0], centre[1] - core.centre[1]],
                &e,
                &rr,
            );
            cf.centre = centre;
            chans.push(cf);
        } else {
            // chromatic aberration of the green polynomial: radius divisor and shift, robustly
            let mut m = vec![true; sel.len()];
            let mut p = [1.0, 0.0, 0.0];
            for round in 0..3 {
                let cur: Vec<Sample> = sel
                    .iter()
                    .zip(&m)
                    .filter(|(_, &q)| q)
                    .map(|(x, _)| *x)
                    .collect();
                let ttw: f64 = cur
                    .iter()
                    .map(|x| x.w * (x.t[0] * x.t[0] + x.t[1] * x.t[1]))
                    .sum::<f64>()
                    .max(1e-30);
                let f = |v: &[f64]| -> f64 {
                    if !(0.5..=2.0).contains(&v[0]) {
                        return f64::MAX;
                    }
                    let mut sum = 0.0;
                    for x in &cur {
                        let q = apply(
                            &dm,
                            [x.uv[0] * core.panel.0 / unit, x.uv[1] * core.panel.1 / unit],
                        );
                        let d = [
                            (q[0] - core.centre[0] - v[1]) / v[0],
                            (q[1] - core.centre[1] - v[2]) / v[0],
                        ];
                        let mt = model_t(&core.a, d);
                        sum += x.w * ((x.t[0] - mt[0]).powi(2) + (x.t[1] - mt[1]).powi(2));
                    }
                    sum / ttw
                };
                let (best, _) = nelder_mead(f, &p, 0.01, 250);
                p = [best[0], best[1], best[2]];
                let e = residual_stats(
                    &sel,
                    core.panel,
                    unit,
                    &dm,
                    core.centre,
                    &core.a,
                    p[0],
                    [p[1], p[2]],
                );
                let thr = if round < 1 {
                    opt.inlier_deg * 2.0
                } else {
                    opt.inlier_deg
                };
                m = e.iter().map(|&x| x < thr).collect();
            }
            let isel: Vec<Sample> = sel
                .iter()
                .zip(&m)
                .filter(|(_, &q)| q)
                .map(|(x, _)| *x)
                .collect();
            let e = residual_stats(
                &isel,
                core.panel,
                unit,
                &dm,
                core.centre,
                &core.a,
                p[0],
                [p[1], p[2]],
            );
            let rr = rad_of(&isel, core.centre, p[0], [p[1], p[2]]);
            chans.push(chan_from(&core.a, core.centre, p[0], [p[1], p[2]], &e, &rr));
        }
    }

    // symmetry of the green channel
    let mut table_sum = [[0.0f64; 8]; 4];
    let mut table_n = [[0usize; 8]; 4];
    let mut twist_sq = 0.0;
    let mut twist_n = 0usize;
    for sm in &gsel {
        let q = apply(
            &dm,
            [
                sm.uv[0] * core.panel.0 / unit,
                sm.uv[1] * core.panel.1 / unit,
            ],
        );
        let d = [q[0] - core.centre[0], q[1] - core.centre[1]];
        let rho = d[0].hypot(d[1]);
        let mt = model_t(&core.a, d);
        let rad_err =
            sm.t[0].hypot(sm.t[1]).atan().to_degrees() - mt[0].hypot(mt[1]).atan().to_degrees();
        let az = d[1].atan2(d[0]);
        let sector =
            (((az + std::f64::consts::PI) / (2.0 * std::f64::consts::PI) * 8.0) as usize).min(7);
        let rb = if rho < 0.3 {
            0
        } else if rho < 0.6 {
            1
        } else if rho < 0.9 {
            2
        } else {
            3
        };
        table_sum[rb][sector] += rad_err;
        table_n[rb][sector] += 1;
        let tl = sm.t[0].hypot(sm.t[1]);
        if tl > 1e-3 && rho > 1e-3 {
            let tw = (d[0] * sm.t[1] - d[1] * sm.t[0])
                .atan2(d[0] * sm.t[0] + d[1] * sm.t[1])
                .to_degrees();
            twist_sq += tw * tw;
            twist_n += 1;
        }
    }
    let mut table = [[f64::NAN; 8]; 4];
    let mut spread: f64 = 0.0;
    for rb in 0..4 {
        let (mut lo, mut hi) = (f64::MAX, f64::MIN);
        for sct in 0..8 {
            if table_n[rb][sct] >= 5 {
                let m = table_sum[rb][sct] / table_n[rb][sct] as f64;
                table[rb][sct] = m;
                lo = lo.min(m);
                hi = hi.max(m);
            }
        }
        if hi >= lo {
            spread = spread.max(hi - lo);
        }
    }
    let twist_rms = if twist_n > 0 {
        (twist_sq / twist_n as f64).sqrt()
    } else {
        f64::NAN
    };
    let symmetric = green.rms_deg < opt.sym_deg && spread < opt.sym_deg && twist_rms < 1.0;
    let verdict = if symmetric {
        format!("radially symmetric: green rms {:.3} deg, sector spread {:.3} deg; the polynomial model is enough", green.rms_deg, spread)
    } else if green.rms_deg < 3.0 * opt.sym_deg && spread < 3.0 * opt.sym_deg {
        format!(
            "approximately symmetric (green rms {:.3} deg, sector spread {:.3} deg, max {:.3} deg): the polynomial is usable, a mesh/LUT mode would remove the remaining error",
            green.rms_deg, spread, green.max_deg
        )
    } else {
        format!(
            "NOT radially symmetric (green rms {:.3} deg, sector spread {:.3} deg, twist rms {:.2} deg, max {:.3} deg): a mesh/LUT mode is needed to follow Pimax's curve",
            green.rms_deg, spread, twist_rms, green.max_deg
        )
    };

    // FOV needed for the inliers (tangent frame: x right, y up)
    let (mut l, mut r, mut u, mut d) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for sm in &gsel {
        l = l.max(-sm.t[0]);
        r = r.max(sm.t[0]);
        u = u.max(sm.t[1]);
        d = d.max(-sm.t[1]);
    }
    let fov_deg = [l, r, u, d].map(|t| t.atan().to_degrees());
    let chans: [ChanFit; CHANNELS] = chans
        .try_into()
        .map_err(|_| "internal: channel count".to_string())?;
    Ok(EyeFit {
        name: eye.name.clone(),
        orient: core.orient,
        panel_used: core.panel,
        unit_px: unit,
        chans,
        symmetry: Symmetry {
            symmetric,
            rms_deg: green.rms_deg,
            max_deg: green.max_deg,
            sector_spread_deg: spread,
            twist_rms_deg: twist_rms,
            table,
            verdict,
        },
        fov_deg,
        valid_points: sg.len(),
        inlier_points: gsel.len(),
    })
}

// ---------------------------------------------------------------------------------------------------------------
// Report, comparison and lens.conf
// ---------------------------------------------------------------------------------------------------------------

/// Angle (degrees) a channel fit shows at p' radius `r` (before `ca`).
pub fn chan_angle_deg(c: &ChanFit, r: f64) -> f64 {
    radial_tangent(c.base, 1.0, &c.k.map(|x| x as f32), r)
        .0
        .atan()
        .to_degrees()
}

fn chan_name(c: usize) -> &'static str {
    ["red", "green", "blue"][c]
}

/// The full text report of a fit (what `pimax-lens fit-pvr` prints before the comparison).
pub fn report(dump: &Dump, fits: &[EyeFit; 2], opt: &Options) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "PVR dump: {} serial {}", dump.product, dump.serial);
    let _ = writeln!(s, "IPD from the eye poses: {:.2} mm", ipd_mm(dump));
    for e in &dump.eyes {
        let _ = writeln!(
            s,
            "eye {}: pose yaw {:+.3} deg, pitch {:+.3} deg; PVR FOV port left {:.2} right {:.2} up {:.2} down {:.2} deg; panel {}x{} px; failed calls {}",
            e.name,
            yaw_deg(e.quat),
            pitch_deg(e.quat),
            e.fov_tan[2].atan().to_degrees(),
            e.fov_tan[3].atan().to_degrees(),
            e.fov_tan[0].atan().to_degrees(),
            e.fov_tan[1].atan().to_degrees(),
            e.panel.0,
            e.panel.1,
            e.failed_calls
        );
    }
    let cant = cant_from_poses(dump);
    let _ = writeln!(
        s,
        "cant_deg from the poses: {cant:+.3} (left yaw - right yaw) / 2"
    );
    match dump.view_rotation_fix {
        Some(1) => {
            let _ = writeln!(s, "NOTE: the runtime reports view_rotation_fix = 1 (parallel projection). The poses then carry no canting; the distortion is then relative to parallel views and cant_deg 0 is consistent.");
        }
        Some(v) => {
            let _ = writeln!(s, "view_rotation_fix = {v}");
        }
        None => {}
    }
    let _ = writeln!(
        s,
        "output V convention assumed: {}",
        if opt.v_down {
            "0 = top"
        } else {
            "0 = bottom of the FOV port (CustomHeadsetOpenVR flips top/bottom for PVR)"
        }
    );
    for e in fits {
        let _ = writeln!(s, "\n=== eye {} ===", e.name);
        let _ = writeln!(s, "orientation: {}", describe_orientation(e));
        let _ = writeln!(
            s,
            "usable points {} inliers {}",
            e.valid_points, e.inlier_points
        );
        for c in 0..CHANNELS {
            let f = &e.chans[c];
            let _ = writeln!(
                s,
                "{:>5}: base {:.5} k {}  ca {:.5} shift {:+.5} {:+.5}  centre {:+.5} {:+.5}  n {} rms {:.4} deg max {:.4} deg rmax {:.3}",
                chan_name(c),
                f.base,
                f.k.iter().map(|k| format!("{k:+.5}")).collect::<Vec<_>>().join(" "),
                f.ca,
                f.shift[0],
                f.shift[1],
                f.centre[0],
                f.centre[1],
                f.n,
                f.rms_deg,
                f.max_deg,
                f.rmax
            );
        }
        let _ = writeln!(
            s,
            "green residual by radius (rho = |p - centre| in half-min-dimension units):"
        );
        let _ = writeln!(s, "   rho        n   rms deg   max deg");
        for b in &e.chans[1].bins {
            let _ = writeln!(
                s,
                "  {:.1}-{:.1} {:>6}   {:7.4}   {:7.4}",
                b.r_lo, b.r_hi, b.n, b.rms_deg, b.max_deg
            );
        }
        let _ = writeln!(s, "symmetry: {}", e.symmetry.verdict);
        let _ = writeln!(s, "mean radial error (deg) by azimuth sector, centres -157 -112 -67 -22 22 67 112 157 deg (0 = +x right, 90 = +y up), per radius bin:");
        for (rb, name) in ["rho<0.3", "0.3-0.6", "0.6-0.9", "rho>=0.9"]
            .iter()
            .enumerate()
        {
            let row: Vec<String> = e.symmetry.table[rb]
                .iter()
                .map(|v| {
                    if v.is_nan() {
                        "    -  ".to_string()
                    } else {
                        format!("{v:+7.3}")
                    }
                })
                .collect();
            let _ = writeln!(s, "  {name:>8}: {}", row.join(" "));
        }
        let _ = writeln!(
            s,
            "render FOV that covers the valid data (left right up down): {:.1} {:.1} {:.1} {:.1} deg",
            e.fov_deg[0], e.fov_deg[1], e.fov_deg[2], e.fov_deg[3]
        );
    }
    s
}

/// Compare the fit with the polynomial of the current lens config: angle at the same panel radius.
pub fn compare(fits: &[EyeFit; 2], cfg: &LensConfig, conf_name: &str) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "\n=== comparison with {conf_name} (green channel, same panel radius) ==="
    );
    for (i, e) in fits.iter().enumerate() {
        let cur = &cfg.eye[i];
        let g = &e.chans[1];
        let _ =
            writeln!(
            s,
            "eye {}: centre PVR {:+.4} {:+.4} vs conf {:+.4} {:+.4}; base PVR {:.5} vs conf {:.5}",
            e.name, g.centre[0], g.centre[1], cur.cx, cur.cy, g.base, cur.base[1] * cur.scale
        );
        let _ = writeln!(s, "   rho   PVR deg  conf deg   diff   (diff = conf - PVR; positive: current model shows more angle)");
        let mut r = 0.0;
        while r <= g.rmax + 1e-9 {
            let pvr = chan_angle_deg(g, r);
            let conf = radial_tangent(
                f64::from(cur.base[1]),
                f64::from(cur.scale),
                &cur.k[1],
                r / f64::from(cur.ca[1]),
            )
            .0
            .atan()
            .to_degrees();
            let _ = writeln!(s, "  {r:4.2}  {pvr:8.3}  {conf:8.3}  {:+7.3}", conf - pvr);
            r += 0.1;
        }
        // radius needed for given angles
        let _ = writeln!(s, "   angle  PVR rho  conf rho");
        for ang in [5.0f64, 10.0, 15.0, 20.0, 25.0, 30.0, 40.0, 50.0] {
            let want = ang.to_radians().tan();
            // first radius (scan, then bisect) where the model reaches the wanted angle
            let solve_r = |f: &dyn Fn(f64) -> f64| -> Option<f64> {
                let step = 0.005;
                let mut hi = step;
                while f(hi) < want {
                    hi += step;
                    if hi > 2.5 {
                        return None;
                    }
                }
                let mut lo = hi - step;
                for _ in 0..50 {
                    let m = 0.5 * (lo + hi);
                    if f(m) < want {
                        lo = m;
                    } else {
                        hi = m;
                    }
                }
                Some(0.5 * (lo + hi))
            };
            let rp = solve_r(&|r| radial_tangent(g.base, 1.0, &g.k.map(|x| x as f32), r).0)
                .filter(|&r| r <= g.rmax * 1.001);
            let rc = solve_r(&|r| {
                radial_tangent(f64::from(cur.base[1]), f64::from(cur.scale), &cur.k[1], r).0
            });
            let fmt = |v: Option<f64>| v.map_or("   -   ".to_string(), |v| format!("{v:7.4}"));
            let _ = writeln!(
                s,
                "  {ang:5.1}  {}  {}",
                fmt(rp),
                fmt(rc.map(|r| r * f64::from(cur.ca[1])))
            );
        }
    }
    s
}

/// A complete `lens.conf` variant for the fit. `cant_deg` is written as given.
pub fn conf_text(dump: &Dump, fits: &[EyeFit; 2], cant_deg: f64, opt: &Options) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "# Pimax Dream Air lens model from Pimax's own PVR distortion (pvr_getHmdDistortedUV), written by `pimax-lens fit-pvr`.");
    let _ = writeln!(s, "# headset {} serial {}; cant_deg from the HmdToEye poses; terms {}, per-channel polynomials {}.", dump.product, dump.serial, opt.terms, opt.per_channel);
    let _ = writeln!(s, "# Test:  PIMAX_LENS_CONF=$HOME/.config/pimax/lens-ipd/pvr.conf   (lens.conf itself is not touched)");
    let _ = writeln!(s, "cant_deg = {cant_deg:.4}\n");
    let _ = writeln!(s, "[common]");
    let _ = writeln!(s, "scale = 1.0");
    for (i, e) in fits.iter().enumerate() {
        let _ = writeln!(s, "\n[{}]", ["left", "right"][i]);
        // Pimax's own FOV port: the data span exactly that tangent rectangle, so it is the FOV that covers the
        // whole panel. Our `fov` is left right up down (tangent frame, x right, y up). Fall back to the extent of
        // the data when the dump has no port.
        let [up, dn, l, r] = dump.eyes[i].fov_tan;
        let port = [l, r, up, dn].map(|t| t.atan().to_degrees());
        let f = if port.iter().all(|&d| d.is_finite() && d > 1.0) {
            port
        } else {
            e.fov_deg
        }
        .map(|d| d.clamp(5.0, 89.0));
        let _ = writeln!(s, "fov = {:.2} {:.2} {:.2} {:.2}", f[0], f[1], f[2], f[3]);
        let g = &e.chans[1];
        let _ = writeln!(s, "center = {:.5} {:.5}", g.centre[0], g.centre[1]);
        let _ = writeln!(s, "fit_range = {:.3}", (g.rmax * 0.98).max(0.5));
        for (c, tag) in ["r", "g", "b"].iter().enumerate() {
            let cf = &e.chans[c];
            let used =
                cf.k.iter()
                    .rposition(|&k| k.abs() > 1e-5)
                    .map_or(3, |p| p + 1)
                    .max(3);
            let ks: Vec<String> = cf.k[..used].iter().map(|k| format!("{k:.6}")).collect();
            let _ = writeln!(s, "base_{tag} = {:.6}", cf.base);
            let _ = writeln!(s, "k_{tag} = {}", ks.join(" "));
            let _ = writeln!(s, "ca_{tag} = {:.6}", cf.ca);
            let _ = writeln!(s, "shift_{tag} = {:.6} {:.6}", cf.shift[0], cf.shift[1]);
        }
    }
    s
}

/// Parse the generated conf with the real parser; returns warnings (should be empty).
pub fn check_conf(text: &str) -> Vec<String> {
    let (_, w) = config::parse(text, 3840.0 / 3552.0);
    w.into_iter().map(|w| w.message).collect()
}

// ---------------------------------------------------------------------------------------------------------------
// Command line: `pimax-lens fit-pvr`
// ---------------------------------------------------------------------------------------------------------------

pub const USAGE: &str = "pimax-lens fit-pvr <pimax-distortion-*.json> [--out FILE] [--conf FILE] [--dry-run] [--v-down] [--per-channel] [--terms N] [--inlier-deg D]";

fn opt_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// Run `fit-pvr`. Returns the text for stdout, or an error text for stderr. Reads the current lens config
/// (`--conf`, else the usual `PIMAX_LENS_CONF` / `~/.config/pimax/lens.conf`) for the comparison but never writes
/// it; writes the variant to `--out` (default `~/.config/pimax/lens-ipd/pvr.conf`) unless `--dry-run`.
pub fn cli(args: &[String], env: &dyn Fn(&str) -> Option<String>) -> Result<String, String> {
    let flags_with_value = ["--out", "--conf", "--terms", "--inlier-deg"];
    let mut input = None;
    let mut i = 0;
    while i < args.len() {
        if flags_with_value.contains(&args[i].as_str()) {
            i += 2;
            continue;
        }
        if !args[i].starts_with("--") && input.is_none() {
            input = Some(args[i].clone());
        }
        i += 1;
    }
    let input = input.ok_or_else(|| format!("usage: {USAGE}"))?;
    let mut opt = Options::default();
    if let Some(t) = opt_value(args, "--terms") {
        opt.terms = t
            .parse()
            .ok()
            .filter(|t| (1..=K_TERMS).contains(t))
            .ok_or("--terms must be 1..=5")?;
    }
    if let Some(d) = opt_value(args, "--inlier-deg") {
        opt.inlier_deg = d
            .parse()
            .ok()
            .filter(|d: &f64| *d > 0.0)
            .ok_or("--inlier-deg must be a positive number")?;
    }
    opt.v_down = args.iter().any(|a| a == "--v-down");
    opt.per_channel = args.iter().any(|a| a == "--per-channel");

    let text = std::fs::read_to_string(&input).map_err(|e| format!("{input}: {e}"))?;
    let dump = parse_dump(&text).map_err(|e| format!("{input}: {e}"))?;
    let fits = fit_dump(&dump, &opt)?;
    let mut out = report(&dump, &fits, &opt);

    let conf_path = opt_value(args, "--conf")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| config::config_path(env));
    match std::fs::read_to_string(&conf_path) {
        Ok(t) => {
            let (cfg, _) = config::parse(&t, 3840.0 / 3552.0);
            out.push_str(&compare(&fits, &cfg, &conf_path.display().to_string()));
            let _ = writeln!(
                out,
                "cant_deg: poses {:+.3} vs conf {:+.3}",
                cant_from_poses(&dump),
                cfg.cant_deg
            );
        }
        Err(e) => {
            let _ = writeln!(
                out,
                "\n(no comparison: cannot read {}: {e})",
                conf_path.display()
            );
        }
    }

    let mut cant = cant_from_poses(&dump);
    if cant.abs() > f64::from(config::MAX_CANT_DEG) {
        let _ = writeln!(
            out,
            "WARNING: cant {cant:.2} deg is outside +-{} deg; written as 0",
            config::MAX_CANT_DEG
        );
        cant = 0.0;
    }
    let conf = conf_text(&dump, &fits, cant, &opt);
    let warnings = check_conf(&conf);
    if !warnings.is_empty() {
        return Err(format!(
            "internal: generated config does not parse cleanly: {warnings:?}"
        ));
    }
    if args.iter().any(|a| a == "--dry-run") {
        let _ = writeln!(
            out,
            "\n--dry-run: not written. The variant would be:\n\n{conf}"
        );
    } else {
        let path = opt_value(args, "--out")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                let base = match env("XDG_CONFIG_HOME").filter(|x| !x.is_empty()) {
                    Some(x) => std::path::PathBuf::from(x),
                    None => std::path::PathBuf::from(env("HOME").unwrap_or_else(|| ".".into()))
                        .join(".config"),
                };
                base.join("pimax/lens-ipd/pvr.conf")
            });
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        std::fs::write(&path, &conf).map_err(|e| format!("{}: {e}", path.display()))?;
        let _ = writeln!(
            out,
            "\nwrote {} (lens.conf untouched). Test with: PIMAX_LENS_CONF={} ...",
            path.display(),
            path.display()
        );
    }
    for (e, f) in dump.eyes.iter().zip(&fits) {
        let _ = writeln!(
            out,
            "SUMMARY eye {}: green rms {:.3} deg max {:.3} deg; {}",
            e.name, f.chans[1].rms_deg, f.chans[1].max_deg, f.symmetry.verdict
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Our own model: tangent at user-facing point q (half-min-dimension units).
    #[allow(clippy::too_many_arguments)]
    fn model(
        base: f64,
        k: &[f32; K_TERMS],
        centre: [f64; 2],
        ca: f64,
        shift: [f64; 2],
        q: [f64; 2],
    ) -> [f64; 2] {
        let p = [
            (q[0] - centre[0] - shift[0]) / ca,
            (q[1] - centre[1] - shift[1]) / ca,
        ];
        let r = p[0].hypot(p[1]);
        let t = radial_tangent(base, 1.0, k, r).0;
        if r < 1e-12 {
            [0.0, 0.0]
        } else {
            [p[0] / r * t, p[1] / r * t]
        }
    }

    struct Synth {
        base: f64,
        k: [f32; K_TERMS],
        centre: [f64; 2],
        ca: [f64; 3],
        orient: usize,
        // add a non-radial term (x-dependent scale) to test the symmetry verdict
        asym: f64,
    }

    fn synth_json(sy: &Synth, grid_n: usize, line_n: usize) -> String {
        let fov_tan = [
            (44.5f64).to_radians().tan(),
            (44.5f64).to_radians().tan(),
            (50f64).to_radians().tan(),
            (50f64).to_radians().tan(),
        ];
        let [up, dn, l, r] = fov_tan;
        let (pw, ph) = (3552.0, 3840.0); // native panel of the eye, rotated against the user-facing 3840x3552
        let unit = 3552.0 / 2.0;
        let dm = ORIENTS[sy.orient].0;
        // inverse of the orientation (all entries 0/+-1, so the transpose)
        let inv = [[dm[0][0], dm[1][0]], [dm[0][1], dm[1][1]]];
        let out_uv = |u: f64, v: f64, c: usize| -> [f64; 2] {
            let p = [(u - 0.5) * pw / unit, (v - 0.5) * ph / unit];
            let q = apply(&dm, p);
            let mut t = model(sy.base, &sy.k, sy.centre, sy.ca[c], [0.0, 0.0], q);
            t[0] *= 1.0 + sy.asym * q[0];
            let _ = inv;
            [(t[0] + l) / (l + r), (t[1] + dn) / (up + dn)]
        };
        let mut s = String::new();
        s.push_str("{\"hmd\":{\"serial\":\"TEST\",\"product_name\":\"synthetic\"},\"config\":{\"view_rotation_fix_default_-1\":0},");
        s.push_str("\"display\":{\"left\":{\"width\":7104,\"height\":3840,\"eye_display\":\"horz_side_by_side\"},\"right\":{\"width\":7104,\"height\":3840,\"eye_display\":\"horz_side_by_side\"}},");
        s.push_str("\"eyes\":[");
        for (ei, name) in ["left", "right"].iter().enumerate() {
            if ei > 0 {
                s.push(',');
            }
            let yaw = if ei == 0 { 2.0f64 } else { -2.0 }.to_radians() / 2.0;
            let _ = write!(
                s,
                "{{\"eye\":\"{name}\",\"fov_tan\":{{\"up\":{up},\"down\":{dn},\"left\":{l},\"right\":{r}}},\"hmd_to_eye_pose\":{{\"position\":[{},0,0],\"orientation_xyzw\":[0,{},0,{}]}},",
                if ei == 0 { -0.0315 } else { 0.0315 },
                yaw.sin(),
                yaw.cos()
            );
            let _ = write!(s, "\"distortion\":{{\"grid_n\":{grid_n},");
            for (c, tag) in ["r", "g", "b"].iter().enumerate() {
                let _ = write!(s, "\"grid_{tag}\":[");
                let mut first = true;
                for j in 0..grid_n {
                    for i in 0..grid_n {
                        let o = out_uv(
                            i as f64 / (grid_n - 1) as f64,
                            j as f64 / (grid_n - 1) as f64,
                            c,
                        );
                        if !first {
                            s.push(',');
                        }
                        first = false;
                        let _ = write!(s, "{},{}", o[0], o[1]);
                    }
                }
                s.push_str("],");
            }
            let _ = write!(s, "\"line_n\":{line_n},\"lines\":{{");
            for (di, (nm, d)) in [
                ("+x", [1.0, 0.0]),
                ("-x", [-1.0, 0.0]),
                ("+y", [0.0, 1.0]),
                ("-y", [0.0, -1.0]),
            ]
            .iter()
            .enumerate()
            {
                if di > 0 {
                    s.push(',');
                }
                let _ = write!(s, "\"{nm}\":{{");
                for (c, tag) in ["r", "g", "b"].iter().enumerate() {
                    if c > 0 {
                        s.push(',');
                    }
                    let _ = write!(s, "\"{tag}\":[");
                    for k in 0..line_n {
                        let sp = k as f64 / (line_n - 1) as f64;
                        let o = out_uv(0.5 + d[0] * 0.5 * sp, 0.5 + d[1] * 0.5 * sp, c);
                        if k > 0 {
                            s.push(',');
                        }
                        let _ = write!(s, "{},{}", o[0], o[1]);
                    }
                    s.push(']');
                }
                s.push('}');
            }
            s.push_str("},\"failed_calls\":0}}");
        }
        s.push_str("]}");
        s
    }

    fn std_synth(orient: usize) -> Synth {
        Synth {
            base: 0.62,
            k: [0.55, -0.6, 0.45, 0.0, 0.0],
            centre: [0.1, 0.012],
            ca: [1.0045, 1.0, 0.995],
            orient,
            asym: 0.0,
        }
    }

    #[test]
    fn json_parser_handles_nesting_nulls_and_escapes() {
        let v = json::parse("{\"a\":[1,2.5e-1,null,-3],\"b\":{\"c\":\"x\\ny\\u0041\"},\"d\":true}")
            .unwrap();
        assert_eq!(v.path("a").unwrap().floats().unwrap()[1], 0.25);
        assert!(v.path("a").unwrap().floats().unwrap()[2].is_nan());
        assert_eq!(v.path("b/c").unwrap().str(), Some("x\nyA"));
        assert!(json::parse("{\"a\":}").is_err());
        assert!(json::parse("[1,2] x").is_err());
    }

    #[test]
    fn recovers_the_parameters_of_our_own_model() {
        let sy = std_synth(1);
        let dump = parse_dump(&synth_json(&sy, 41, 129)).unwrap();
        let fits = fit_dump(&dump, &Options::default()).unwrap();
        for f in &fits {
            assert_eq!(f.orient, 1, "orientation");
            let g = &f.chans[1];
            assert!(
                (g.base - sy.base).abs() < 2e-3,
                "base {} vs {}",
                g.base,
                sy.base
            );
            for j in 0..3 {
                assert!(
                    (g.k[j] - f64::from(sy.k[j])).abs() < 0.03,
                    "k{} {} vs {}",
                    j + 1,
                    g.k[j],
                    sy.k[j]
                );
            }
            assert!(
                (g.centre[0] - sy.centre[0]).abs() < 2e-3
                    && (g.centre[1] - sy.centre[1]).abs() < 2e-3,
                "{:?}",
                g.centre
            );
            assert!(g.max_deg < 0.02, "green max {}", g.max_deg);
            assert!(
                (f.chans[0].ca - sy.ca[0]).abs() < 1e-3,
                "ca_r {}",
                f.chans[0].ca
            );
            assert!(
                (f.chans[2].ca - sy.ca[2]).abs() < 1e-3,
                "ca_b {}",
                f.chans[2].ca
            );
            assert!(f.chans[0].max_deg < 0.05 && f.chans[2].max_deg < 0.05);
            assert!(f.symmetry.symmetric, "{}", f.symmetry.verdict);
        }
    }

    #[test]
    fn finds_other_panel_orientations() {
        for o in [0usize, 3, 5, 6] {
            let sy = std_synth(o);
            let dump = parse_dump(&synth_json(&sy, 33, 65)).unwrap();
            let fits = fit_dump(&dump, &Options::default()).unwrap();
            assert_eq!(fits[0].orient, o);
            assert!((fits[0].chans[1].base - sy.base).abs() < 3e-3);
            assert!((fits[0].chans[1].centre[0] - sy.centre[0]).abs() < 3e-3);
        }
    }

    #[test]
    fn cant_and_ipd_come_from_the_poses() {
        let dump = parse_dump(&synth_json(&std_synth(1), 17, 17)).unwrap();
        assert!(
            (cant_from_poses(&dump) - 2.0).abs() < 1e-6,
            "{}",
            cant_from_poses(&dump)
        );
        assert!((ipd_mm(&dump) - 63.0).abs() < 1e-6);
    }

    #[test]
    fn a_non_radial_distortion_is_reported_as_asymmetric() {
        let mut sy = std_synth(1);
        sy.asym = 0.12;
        let dump = parse_dump(&synth_json(&sy, 41, 65)).unwrap();
        let fits = fit_dump(&dump, &Options::default()).unwrap();
        assert!(!fits[0].symmetry.symmetric, "{}", fits[0].symmetry.verdict);
        assert!(
            fits[0].symmetry.verdict.contains("mesh/LUT"),
            "{}",
            fits[0].symmetry.verdict
        );
    }

    #[test]
    fn written_conf_parses_cleanly_and_reproduces_the_fit() {
        let sy = std_synth(1);
        let dump = parse_dump(&synth_json(&sy, 33, 65)).unwrap();
        let opt = Options::default();
        let fits = fit_dump(&dump, &opt).unwrap();
        let text = conf_text(&dump, &fits, cant_from_poses(&dump), &opt);
        assert!(
            check_conf(&text).is_empty(),
            "{:?}\n{text}",
            check_conf(&text)
        );
        let (cfg, _) = config::parse(&text, 3840.0 / 3552.0);
        assert!((cfg.cant_deg - 2.0).abs() < 1e-3);
        assert!((cfg.eye[0].base[1] - sy.base as f32).abs() < 3e-3);
        assert!((f64::from(cfg.eye[0].cx) - sy.centre[0]).abs() < 3e-3);
        assert!((cfg.eye[0].ca[0] - sy.ca[0] as f32).abs() < 2e-3);
        // the comparison against the same model finds no difference at small radius
        let cmp = compare(&fits, &cfg, "synthetic");
        assert!(cmp.contains("comparison"));
    }

    #[test]
    fn per_channel_mode_recovers_the_channels_too() {
        let sy = std_synth(1);
        let dump = parse_dump(&synth_json(&sy, 33, 65)).unwrap();
        let opt = Options {
            per_channel: true,
            ..Options::default()
        };
        let fits = fit_dump(&dump, &opt).unwrap();
        assert!(
            fits[0].chans[0].max_deg < 0.05,
            "{}",
            fits[0].chans[0].max_deg
        );
        let text = conf_text(&dump, &fits, 0.0, &opt);
        assert!(check_conf(&text).is_empty());
    }

    #[test]
    fn cli_fits_a_file_compares_and_writes_only_the_variant() {
        let dir = std::env::temp_dir().join(format!("pimax-fit-pvr-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("d.json");
        std::fs::write(&input, synth_json(&std_synth(1), 33, 65)).unwrap();
        let cur = dir.join("lens.conf");
        std::fs::write(&cur, "base = 0.65307\nk = 0.60779 -0.72489 0.58858\n").unwrap();
        let out = dir.join("sub/pvr.conf");
        let args: Vec<String> = ["fit-pvr-ignored".to_string()]
            .into_iter()
            .skip(1)
            .chain([
                input.display().to_string(),
                "--out".into(),
                out.display().to_string(),
                "--conf".into(),
                cur.display().to_string(),
            ])
            .collect();
        let text = cli(&args, &|_| None).unwrap();
        assert!(text.contains("comparison with"), "{text}");
        assert!(text.contains("radially symmetric"), "{text}");
        assert!(text.contains("cant_deg from the poses: +2.000"), "{text}");
        let written = std::fs::read_to_string(&out).unwrap();
        assert!(written.contains("cant_deg = 2.0000"));
        assert_eq!(
            std::fs::read_to_string(&cur).unwrap(),
            "base = 0.65307\nk = 0.60779 -0.72489 0.58858\n"
        );
        assert!(cli(&[], &|_| None).unwrap_err().contains("usage"));
        assert!(cli(&["/nonexistent/x.json".into()], &|_| None).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Real Dream Air data (P511X0X000000000002, PVR 1.33.1), every 8th grid point and 32 samples per radial line.
    /// Regression: the right eye used to end in "the radial fit did not converge" because a runaway candidate
    /// (centre far outside the panel, numerically zero cost) won the orientation search.
    #[test]
    fn fits_the_real_dream_air_dump_for_both_eyes() {
        let dump = parse_dump(include_str!("../tests/data/pvr-dump-sub.json")).unwrap();
        let fits = fit_dump(&dump, &Options::default()).unwrap();
        assert_eq!((fits[0].orient, fits[1].orient), (3, 1));
        for (i, f) in fits.iter().enumerate() {
            let g = &f.chans[1];
            let side = if i == 0 { 1.0 } else { -1.0 };
            assert!((g.centre[0] - side * 0.027).abs() < 0.004, "{:?}", g.centre);
            assert!((g.centre[1] - 0.009).abs() < 0.005, "{:?}", g.centre);
            assert!((g.base - 0.686).abs() < 0.012, "base {}", g.base);
            assert!(
                g.rms_deg < 0.5 && g.max_deg < 2.0,
                "{} {}",
                g.rms_deg,
                g.max_deg
            );
            assert!(f.chans[0].ca > 1.0 && f.chans[2].ca < 1.0);
        }
        let conf = conf_text(&dump, &fits, cant_from_poses(&dump), &Options::default());
        assert!(check_conf(&conf).is_empty());
        assert!(conf.contains("fov = 52.58 47.54 44.55 44.55"), "{conf}");
        assert!(conf.contains("fov = 47.54 52.58 44.55 44.55"), "{conf}");
    }

    #[test]
    fn rejects_files_that_are_not_dumps() {
        assert!(parse_dump("{\"a\":1}").is_err());
        assert!(parse_dump("not json").is_err());
    }
}
