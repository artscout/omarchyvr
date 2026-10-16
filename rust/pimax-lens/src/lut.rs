//! Exact distortion from Pimax's own samples ("LUT mode").
//!
//! `pimax-lens make-lut <dump.json> --out FILE` stores, per eye and colour channel, the 129x129 grid of the PVR
//! distortion dump as ray tangents (x right, y up), so a later change of the rendered FOV does not invalidate it.
//! lens.conf `lut = FILE` makes [`crate::Lens`] sample it instead of the radial polynomial.
//!
//! File format (little endian, version 1):
//!
//! ```text
//! 0   8   magic "PIMAXLUT"
//! 8   u32 version (1)
//! 12  u32 grid size n
//! 16  u32 channels (3)    20 u32 eyes (2)
//! 24  32  serial, NUL padded
//! 56  per eye (2 x 36 bytes): u32 orientation (index into pvrfit::ORIENTS), f32 panel width, f32 panel height (px),
//!     f32 FOV port tangents up, down, left, right, f32 reserved x2 (0)
//! 128 data: eye, channel (R G B), then n*n points row-major (panel v outer, panel u inner) of f32 tx, ty
//! end u32 CRC32 of everything before it
//! ```
//!
//! The data is in the dump's own panel order. `orientation` says how the panel axes map to the user-facing display;
//! [`Lut::from_bytes`] resamples (an exact permutation of grid nodes) into the user-display grid that
//! [`Lut::sample`] interpolates.

use crate::config::CHANNELS;
use crate::png::crc32;
use crate::pvrfit::{Dm, Dump, ORIENTS};

pub const MAGIC: &[u8; 8] = b"PIMAXLUT";
pub const VERSION: u32 = 1;
const HEADER: usize = 56 + 2 * 36 + 16;
/// Orientation of the left / right eye panel (indices into `pvrfit::ORIENTS`): left user x = panel v, user y = -panel u;
/// right user x = -panel v, user y = panel u.
pub const ORIENT_LEFT: u32 = 3;
pub const ORIENT_RIGHT: u32 = 1;

/// How the grid is interpolated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interp {
    Bilinear,
    Bicubic,
}

/// Per-eye metadata of a LUT file.
#[derive(Clone, Debug, PartialEq)]
pub struct EyeMeta {
    pub orient: u32,
    pub panel: [f32; 2],
    /// FOV port tangents: up, down, left, right.
    pub fov_tan: [f32; 4],
}

/// One eye in the user-display grid: `data[c][j * n + i]` = tangent at display (u, v) = (i, j) / (n - 1), v down.
#[derive(Clone)]
struct EyeLut {
    meta: EyeMeta,
    data: [Vec<[f32; 2]>; CHANNELS],
    /// Same as stored (panel order), kept for [`Lut::to_bytes`].
    raw: [Vec<[f32; 2]>; CHANNELS],
}

#[derive(Clone)]
pub struct Lut {
    pub serial: String,
    pub n: usize,
    eyes: [EyeLut; 2],
}

impl std::fmt::Debug for Lut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lut")
            .field("serial", &self.serial)
            .field("n", &self.n)
            .field("eyes", &[&self.eyes[0].meta, &self.eyes[1].meta])
            .finish()
    }
}

fn rd_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn rd_f32(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

/// Resample panel-order data into the user-display grid for orientation matrix `d` (a signed permutation).
fn reorient(raw: &[[f32; 2]], n: usize, d: &Dm) -> Vec<[f32; 2]> {
    let h = (n - 1) as f64;
    let mut out = vec![[0.0; 2]; n * n];
    for j in 0..n {
        for i in 0..n {
            // user_n = D panel_n, so panel_n = D^T user_n (D is orthogonal)
            let (x, y) = (i as f64 / h - 0.5, 0.5 - j as f64 / h);
            let pu = d[0][0] * x + d[1][0] * y + 0.5;
            let pv = d[0][1] * x + d[1][1] * y + 0.5;
            let (pi, pj) = ((pu * h).round() as usize, (pv * h).round() as usize);
            out[j * n + i] = raw[pj.min(n - 1) * n + pi.min(n - 1)];
        }
    }
    out
}

/// Panel UV -> user-facing display (u, v), v down, for orientation matrix `d`.
pub fn panel_to_display(d: &Dm, pu: f64, pv: f64) -> (f64, f64) {
    let (px, py) = (pu - 0.5, pv - 0.5);
    let x = d[0][0] * px + d[0][1] * py;
    let y = d[1][0] * px + d[1][1] * py;
    (0.5 + x, 0.5 - y)
}

impl Lut {
    /// Build from a parsed PVR dump (orientation of the Dream Air: left 3, right 1).
    pub fn from_dump(dump: &Dump) -> Result<Lut, String> {
        let n = dump.eyes[0].grid_n;
        if dump.eyes[1].grid_n != n || !(2..=1025).contains(&n) {
            return Err(format!("dump: unusable grid size {n}"));
        }
        let mut eyes = Vec::new();
        for (e, ed) in dump.eyes.iter().enumerate() {
            let [up, dn, l, r] = ed.fov_tan;
            // render UV V = 0 is the bottom of the FOV port (see analysis/pvr-distortion-fit.md)
            let mut raw: [Vec<[f32; 2]>; CHANNELS] = Default::default();
            for (c, rc) in raw.iter_mut().enumerate() {
                for k in 0..n * n {
                    let (ou, ov) = (ed.grid[c][2 * k], ed.grid[c][2 * k + 1]);
                    let t = [-l + ou * (l + r), -dn + ov * (up + dn)];
                    if !(t[0].is_finite() && t[1].is_finite()) {
                        return Err(format!(
                            "dump: eye {e} channel {c} sample {k} is not finite"
                        ));
                    }
                    rc.push([t[0] as f32, t[1] as f32]);
                }
            }
            let meta = EyeMeta {
                orient: if e == 0 { ORIENT_LEFT } else { ORIENT_RIGHT },
                panel: [ed.panel.0 as f32, ed.panel.1 as f32],
                fov_tan: [up as f32, dn as f32, l as f32, r as f32],
            };
            eyes.push(EyeLut::new(meta, raw, n));
        }
        let right = eyes.pop().unwrap();
        let left = eyes.pop().unwrap();
        Ok(Lut {
            serial: dump.serial.clone(),
            n,
            eyes: [left, right],
        })
    }

    pub fn meta(&self, eye: usize) -> &EyeMeta {
        &self.eyes[eye].meta
    }

    /// FOV port half-angles in degrees: left, right, up, down.
    pub fn fov_deg(&self, eye: usize) -> [f32; 4] {
        let [up, dn, l, r] = self.eyes[eye].meta.fov_tan;
        [l, r, up, dn].map(|t| t.atan().to_degrees())
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(HEADER + 2 * CHANNELS * self.n * self.n * 8 + 4);
        b.extend_from_slice(MAGIC);
        for v in [VERSION, self.n as u32, CHANNELS as u32, 2] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        let mut s = [0u8; 32];
        let sb = self.serial.as_bytes();
        s[..sb.len().min(31)].copy_from_slice(&sb[..sb.len().min(31)]);
        b.extend_from_slice(&s);
        for e in &self.eyes {
            b.extend_from_slice(&e.meta.orient.to_le_bytes());
            for v in e
                .meta
                .panel
                .iter()
                .chain(&e.meta.fov_tan)
                .chain(&[0.0, 0.0])
            {
                b.extend_from_slice(&v.to_le_bytes());
            }
        }
        b.resize(HEADER, 0);
        for e in &self.eyes {
            for ch in &e.raw {
                for p in ch {
                    b.extend_from_slice(&p[0].to_le_bytes());
                    b.extend_from_slice(&p[1].to_le_bytes());
                }
            }
        }
        let crc = crc32(&b);
        b.extend_from_slice(&crc.to_le_bytes());
        b
    }

    pub fn from_bytes(b: &[u8]) -> Result<Lut, String> {
        if b.len() < HEADER + 4 || &b[..8] != MAGIC {
            return Err("not a pimax LUT file (bad magic)".into());
        }
        let ver = rd_u32(b, 8);
        if ver != VERSION {
            return Err(format!(
                "unsupported LUT version {ver} (expected {VERSION})"
            ));
        }
        let n = rd_u32(b, 12) as usize;
        if rd_u32(b, 16) as usize != CHANNELS || rd_u32(b, 20) != 2 || !(2..=1025).contains(&n) {
            return Err("bad LUT header (channels/eyes/grid size)".into());
        }
        let want = HEADER + 2 * CHANNELS * n * n * 8 + 4;
        if b.len() != want {
            return Err(format!("bad LUT size {} (expected {want})", b.len()));
        }
        if crc32(&b[..want - 4]) != rd_u32(b, want - 4) {
            return Err("LUT checksum mismatch".into());
        }
        let serial = String::from_utf8_lossy(&b[24..56])
            .trim_end_matches('\0')
            .to_string();
        let mut off = HEADER;
        let mut eyes = Vec::new();
        for e in 0..2 {
            let h = 56 + e * 36;
            let orient = rd_u32(b, h);
            if orient as usize >= ORIENTS.len() {
                return Err(format!("bad LUT orientation {orient}"));
            }
            let meta = EyeMeta {
                orient,
                panel: [rd_f32(b, h + 4), rd_f32(b, h + 8)],
                fov_tan: [
                    rd_f32(b, h + 12),
                    rd_f32(b, h + 16),
                    rd_f32(b, h + 20),
                    rd_f32(b, h + 24),
                ],
            };
            if meta.fov_tan.iter().any(|t| !(t.is_finite() && *t > 0.0)) {
                return Err("bad LUT FOV port".into());
            }
            let mut raw: [Vec<[f32; 2]>; CHANNELS] = Default::default();
            for rc in raw.iter_mut() {
                for _ in 0..n * n {
                    let p = [rd_f32(b, off), rd_f32(b, off + 4)];
                    if !(p[0].is_finite() && p[1].is_finite()) {
                        return Err("non-finite value in LUT".into());
                    }
                    rc.push(p);
                    off += 8;
                }
            }
            eyes.push(EyeLut::new(meta, raw, n));
        }
        let right = eyes.pop().unwrap();
        let left = eyes.pop().unwrap();
        Ok(Lut {
            serial,
            n,
            eyes: [left, right],
        })
    }

    pub fn load(path: &std::path::Path) -> Result<Lut, String> {
        let b = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Lut::from_bytes(&b).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Ray tangent (x right, y up) that channel `c` shows at display point (u, v) of `eye` (v = 0 at the top).
    /// Points outside [0,1] are clamped to the grid.
    pub fn sample(&self, eye: usize, c: usize, u: f32, v: f32, interp: Interp) -> [f32; 2] {
        let n = self.n;
        let g = &self.eyes[eye].data[c];
        let h = (n - 1) as f32;
        let (fx, fy) = (u.clamp(0.0, 1.0) * h, v.clamp(0.0, 1.0) * h);
        let (i0, j0) = (
            (fx.floor() as usize).min(n - 2),
            (fy.floor() as usize).min(n - 2),
        );
        let (tx, ty) = (fx - i0 as f32, fy - j0 as f32);
        let at = |i: isize, j: isize| {
            g[(j.clamp(0, n as isize - 1) as usize) * n + i.clamp(0, n as isize - 1) as usize]
        };
        match interp {
            Interp::Bilinear => {
                let (a, b, c2, d) = (
                    at(i0 as isize, j0 as isize),
                    at(i0 as isize + 1, j0 as isize),
                    at(i0 as isize, j0 as isize + 1),
                    at(i0 as isize + 1, j0 as isize + 1),
                );
                let mut o = [0.0; 2];
                for k in 0..2 {
                    let top = a[k] + (b[k] - a[k]) * tx;
                    let bot = c2[k] + (d[k] - c2[k]) * tx;
                    o[k] = top + (bot - top) * ty;
                }
                o
            }
            Interp::Bicubic => {
                let wx = catmull_rom(tx);
                let wy = catmull_rom(ty);
                let mut o = [0.0f32; 2];
                for (dj, wyj) in wy.iter().enumerate() {
                    for (di, wxi) in wx.iter().enumerate() {
                        let p = at(i0 as isize + di as isize - 1, j0 as isize + dj as isize - 1);
                        let w = wxi * wyj;
                        o[0] += p[0] * w;
                        o[1] += p[1] * w;
                    }
                }
                o
            }
        }
    }
}

impl EyeLut {
    fn new(meta: EyeMeta, raw: [Vec<[f32; 2]>; CHANNELS], n: usize) -> EyeLut {
        let d = &ORIENTS[meta.orient as usize].0;
        let data = [
            reorient(&raw[0], n, d),
            reorient(&raw[1], n, d),
            reorient(&raw[2], n, d),
        ];
        EyeLut { meta, data, raw }
    }
}

/// Catmull-Rom weights for the four taps i-1, i, i+1, i+2 at fraction `t`.
fn catmull_rom(t: f32) -> [f32; 4] {
    let (t2, t3) = (t * t, t * t * t);
    [
        0.5 * (-t3 + 2.0 * t2 - t),
        0.5 * (3.0 * t3 - 5.0 * t2 + 2.0),
        0.5 * (-3.0 * t3 + 4.0 * t2 + t),
        0.5 * (t3 - t2),
    ]
}

fn ray_angle_deg(a: [f64; 2], b: [f64; 2]) -> f64 {
    let (na, nb) = (
        (1.0 + a[0] * a[0] + a[1] * a[1]).sqrt(),
        (1.0 + b[0] * b[0] + b[1] * b[1]).sqrt(),
    );
    let dot = (a[0] * b[0] + a[1] * b[1] + 1.0) / (na * nb);
    dot.clamp(-1.0, 1.0).acos().to_degrees()
}

/// Error statistics of a LUT against the dump's radial lines (the independent high-resolution samples).
#[derive(Clone, Debug, Default)]
pub struct LineError {
    pub samples: usize,
    pub rms_deg: f64,
    pub max_deg: f64,
    /// Where the maximum is: eye, line name, channel, position along the line 0..1.
    pub worst: (usize, String, usize, f64),
}

/// Compare `interp` sampling of `lut` with the radial lines of `dump`, visible samples only (output in [0,1]).
/// Every line sample is looked up through the same panel -> display orientation the driver uses.
pub fn line_error(lut: &Lut, dump: &Dump, interp: Interp) -> LineError {
    let (mut sum, mut max, mut cnt) = (0.0f64, 0.0f64, 0usize);
    let mut worst = (0, String::new(), 0, 0.0);
    for (e, ed) in dump.eyes.iter().enumerate() {
        let d = &ORIENTS[lut.meta(e).orient as usize].0;
        let [up, dn, l, r] = ed.fov_tan;
        for line in &ed.lines {
            for c in 0..CHANNELS {
                for k in 0..ed.line_n {
                    let (ou, ov) = (line.out[c][2 * k], line.out[c][2 * k + 1]);
                    if !(0.0..=1.0).contains(&ou) || !(0.0..=1.0).contains(&ov) {
                        continue;
                    }
                    let s = k as f64 / (ed.line_n - 1) as f64;
                    let (pu, pv) = (0.5 + line.dir[0] * 0.5 * s, 0.5 + line.dir[1] * 0.5 * s);
                    let (u, v) = panel_to_display(d, pu, pv);
                    let want = [-l + ou * (l + r), -dn + ov * (up + dn)];
                    let got = lut.sample(e, c, u as f32, v as f32, interp);
                    let a = ray_angle_deg(want, [f64::from(got[0]), f64::from(got[1])]);
                    sum += a * a;
                    if a > max {
                        max = a;
                        worst = (e, line.name.clone(), c, s);
                    }
                    cnt += 1;
                }
            }
        }
    }
    LineError {
        samples: cnt,
        rms_deg: if cnt > 0 {
            (sum / cnt as f64).sqrt()
        } else {
            0.0
        },
        max_deg: max,
        worst,
    }
}

pub const USAGE: &str = "pimax-lens make-lut <pimax-distortion-*.json> --out FILE";

/// `pimax-lens make-lut`: convert a PVR dump into a LUT file; returns the report text.
pub fn cli(args: &[String]) -> Result<String, String> {
    use std::fmt::Write as _;
    let mut input = None;
    let mut out = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--out" => {
                out = Some(args.get(i + 1).ok_or("--out needs a file")?.clone());
                i += 1;
            }
            a if a.starts_with("--") => return Err(format!("unknown option {a}; usage: {USAGE}")),
            a => input = input.or_else(|| Some(a.to_string())),
        }
        i += 1;
    }
    let input = input.ok_or_else(|| format!("usage: {USAGE}"))?;
    let out = out.ok_or_else(|| format!("--out is required; usage: {USAGE}"))?;
    let text = std::fs::read_to_string(&input).map_err(|e| format!("{input}: {e}"))?;
    let dump = crate::pvrfit::parse_dump(&text).map_err(|e| format!("{input}: {e}"))?;
    let lut = Lut::from_dump(&dump)?;
    let bytes = lut.to_bytes();
    if let Some(dir) = std::path::Path::new(&out)
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
    {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&out, &bytes).map_err(|e| format!("{out}: {e}"))?;
    let mut s = String::new();
    let _ = writeln!(
        s,
        "wrote {out}: {} bytes, serial {}, grid {n}x{n}, 2 eyes x 3 channels",
        bytes.len(),
        lut.serial,
        n = lut.n
    );
    for e in 0..2 {
        let f = lut.fov_deg(e);
        let _ = writeln!(
            s,
            "eye {e}: orientation {} ({}), fov L{:.2} R{:.2} U{:.2} D{:.2}",
            lut.meta(e).orient,
            ORIENTS[lut.meta(e).orient as usize].1,
            f[0],
            f[1],
            f[2],
            f[3]
        );
    }
    for (name, m) in [("bilinear", Interp::Bilinear), ("bicubic", Interp::Bicubic)] {
        let le = line_error(&lut, &dump, m);
        let _ = writeln!(
            s,
            "{name} vs the {}-sample radial lines ({} visible samples): rms {:.4} deg, max {:.4} deg (eye {} line {} ch {} at {:.3})",
            dump.eyes[0].line_n,
            le.samples,
            le.rms_deg,
            le.max_deg,
            le.worst.0,
            le.worst.1,
            le.worst.2,
            le.worst.3
        );
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pvrfit::parse_dump;

    const FIXTURE: &str = include_str!("../tests/data/pvr-dump-sub.json");
    /// The full Windows dump of the lab headset (private, not in the repo): the first `pimax-distortion-*.json` here.
    const REAL_DIR: &str = "../../pimax-data/windows-20261014/pimax-windows-dump-20261014-042210";

    fn real_dump() -> Option<Dump> {
        let d = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(REAL_DIR);
        let p = std::fs::read_dir(d).ok()?.filter_map(|e| e.ok().map(|e| e.path())).find(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("pimax-distortion-") && n.ends_with(".json"))
        })?;
        parse_dump(&std::fs::read_to_string(p).ok()?).ok()
    }

    /// Keep every `step`-th grid node (and nothing else changes).
    fn decimate(d: &Dump, step: usize) -> Dump {
        let mut d = d.clone();
        for e in d.eyes.iter_mut() {
            let n = e.grid_n;
            let m = (n - 1) / step + 1;
            for g in e.grid.iter_mut() {
                let mut out = Vec::with_capacity(m * m * 2);
                for j in 0..m {
                    for i in 0..m {
                        let k = (j * step * n + i * step) * 2;
                        out.extend_from_slice(&g[k..k + 2]);
                    }
                }
                *g = out;
            }
            e.grid_n = m;
        }
        d
    }

    /// Max / rms ray angle error of a coarse LUT against the fine LUT's own nodes (off the coarse nodes).
    fn coarse_vs_fine(fine: &Lut, coarse: &Lut, interp: Interp, interior_only: bool) -> (f64, f64) {
        let n = fine.n;
        let (mut max, mut sum, mut cnt) = (0.0f64, 0.0f64, 0usize);
        for e in 0..2 {
            let [up, dn, l, r] = fine.meta(e).fov_tan;
            for c in 0..CHANNELS {
                for j in 0..n {
                    for i in 0..n {
                        let (u, v) = (i as f32 / (n - 1) as f32, j as f32 / (n - 1) as f32);
                        let want = fine.sample(e, c, u, v, Interp::Bilinear);
                        let vis =
                            |t: [f32; 2]| t[0] >= -l && t[0] <= r && t[1] >= -dn && t[1] <= up;
                        if !vis(want) {
                            continue;
                        }
                        // optionally skip the band next to the visible-area boundary (kink of the data)
                        let m = 0.03;
                        if interior_only && !vis([want[0] * (1.0 + m), want[1] * (1.0 + m)]) {
                            continue;
                        }
                        let got = coarse.sample(e, c, u, v, interp);
                        let a = ray_angle_deg(
                            [f64::from(want[0]), f64::from(want[1])],
                            [f64::from(got[0]), f64::from(got[1])],
                        );
                        max = max.max(a);
                        sum += a * a;
                        cnt += 1;
                    }
                }
            }
        }
        (max, (sum / cnt as f64).sqrt())
    }

    #[test]
    fn round_trip_through_bytes_is_exact() {
        let dump = parse_dump(FIXTURE).unwrap();
        let lut = Lut::from_dump(&dump).unwrap();
        assert_eq!(lut.serial, "P511X0X000000000002");
        let b = lut.to_bytes();
        assert_eq!(&b[..8], MAGIC);
        let back = Lut::from_bytes(&b).unwrap();
        assert_eq!(back.n, lut.n);
        assert_eq!(back.serial, lut.serial);
        for e in 0..2 {
            assert_eq!(back.meta(e), lut.meta(e));
            for c in 0..CHANNELS {
                assert_eq!(back.eyes[e].raw[c], lut.eyes[e].raw[c]);
                assert_eq!(back.eyes[e].data[c], lut.eyes[e].data[c]);
            }
        }
        assert_eq!(back.to_bytes(), b);
        // exact FOV port of the Dream Air
        let f = back.fov_deg(0);
        assert!(
            (f[0] - 52.577).abs() < 0.01
                && (f[1] - 47.540).abs() < 0.01
                && (f[2] - 44.546).abs() < 0.01
        );
    }

    #[test]
    fn corrupt_files_are_rejected() {
        let dump = parse_dump(FIXTURE).unwrap();
        let b = Lut::from_dump(&dump).unwrap().to_bytes();
        assert!(Lut::from_bytes(&b[..b.len() - 1]).is_err());
        assert!(Lut::from_bytes(b"nope").is_err());
        let mut bad = b.clone();
        bad[200] ^= 0x40;
        assert!(Lut::from_bytes(&bad).unwrap_err().contains("checksum"));
        let mut bad = b.clone();
        bad[0] = b'X';
        assert!(Lut::from_bytes(&bad).unwrap_err().contains("magic"));
        let mut bad = b.clone();
        bad[8] = 9;
        assert!(Lut::from_bytes(&bad).unwrap_err().contains("version"));
    }

    /// The display (u, v) -> tangent must reproduce the dump at the nodes, through the orientation of each eye.
    #[test]
    fn orientation_of_both_eyes() {
        let dump = parse_dump(FIXTURE).unwrap();
        let lut = Lut::from_dump(&dump).unwrap();
        let n = lut.n;
        for (e, ed) in dump.eyes.iter().enumerate() {
            let d = &ORIENTS[lut.meta(e).orient as usize].0;
            let [up, dn, l, r] = ed.fov_tan;
            for (j, i) in [(0, 0), (3, 11), (16, 5), (8, 8), (12, 2)] {
                let (pu, pv) = (i as f64 / (n - 1) as f64, j as f64 / (n - 1) as f64);
                let k = (j * n + i) * 2;
                let want = [
                    -l + ed.grid[1][k] * (l + r),
                    -dn + ed.grid[1][k + 1] * (up + dn),
                ];
                let (u, v) = panel_to_display(d, pu, pv);
                let got = lut.sample(e, 1, u as f32, v as f32, Interp::Bilinear);
                assert!(
                    (f64::from(got[0]) - want[0]).abs() < 1e-5,
                    "eye {e} {i},{j}"
                );
                assert!(
                    (f64::from(got[1]) - want[1]).abs() < 1e-5,
                    "eye {e} {i},{j}"
                );
            }
        }
        // Left eye: user x = panel v, user y = -panel u. The panel's u = 1 edge (render UV x = 1 side) is at the bottom
        // of the left display (display v = 1), panel v = 1 at the right (display u = 1).
        assert_eq!(
            panel_to_display(&ORIENTS[ORIENT_LEFT as usize].0, 1.0, 0.5),
            (0.5, 1.0)
        );
        assert_eq!(
            panel_to_display(&ORIENTS[ORIENT_LEFT as usize].0, 0.5, 1.0),
            (1.0, 0.5)
        );
        // Right eye: user x = -panel v, user y = panel u: mirrored in both axes.
        assert_eq!(
            panel_to_display(&ORIENTS[ORIENT_RIGHT as usize].0, 1.0, 0.5),
            (0.5, 0.0)
        );
        assert_eq!(
            panel_to_display(&ORIENTS[ORIENT_RIGHT as usize].0, 0.5, 1.0),
            (0.0, 0.5)
        );
        // The two eyes are mirror images: left at display (u, v) equals right at (1-u, v) with x negated.
        let (a, b) = (
            lut.sample(0, 1, 0.3, 0.4, Interp::Bicubic),
            lut.sample(1, 1, 0.7, 0.4, Interp::Bicubic),
        );
        assert!(
            (a[0] + b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4,
            "{a:?} {b:?}"
        );
        // Outer side: left eye sees more to the left (FOV left 52.6), so display u = 0 maps to the most negative tangent.
        let edge = lut.sample(0, 1, 0.0, 0.5, Interp::Bilinear)[0];
        let other = lut.sample(0, 1, 1.0, 0.5, Interp::Bilinear)[0];
        assert!(
            edge < 0.0 && other > 0.0 && edge.abs() > other.abs() - 0.2,
            "{edge} {other}"
        );
    }

    #[test]
    fn fixture_lines_agree_with_the_grid() {
        // 17x17 grid (every 8th node): coarse, but bicubic must stay within a fraction of a degree
        let dump = parse_dump(FIXTURE).unwrap();
        let lut = Lut::from_dump(&dump).unwrap();
        let bl = line_error(&lut, &dump, Interp::Bilinear);
        let bc = line_error(&lut, &dump, Interp::Bicubic);
        assert!(bl.samples > 500);
        assert!(
            bc.rms_deg < 0.3 && bl.rms_deg < 0.6,
            "bilinear {bl:?} bicubic {bc:?}"
        );
    }

    #[test]
    fn interpolation_error_on_the_real_dump() {
        let Some(dump) = real_dump() else {
            eprintln!("real dump not present, skipping");
            return;
        };
        let lut = Lut::from_dump(&dump).unwrap();
        let bl = line_error(&lut, &dump, Interp::Bilinear);
        let bc = line_error(&lut, &dump, Interp::Bicubic);
        eprintln!("lines: bilinear {bl:?}\nlines: bicubic {bc:?}");
        // interpolation error between the nodes, estimated from a half resolution (65x65) LUT against the full grid
        let coarse = Lut::from_dump(&decimate(&dump, 2)).unwrap();
        for interior in [false, true] {
            let a = coarse_vs_fine(&lut, &coarse, Interp::Bilinear, interior);
            let b = coarse_vs_fine(&lut, &coarse, Interp::Bicubic, interior);
            eprintln!("half-res LUT, interior_only={interior}: bilinear max {:.4} rms {:.4}; bicubic max {:.4} rms {:.4}", a.0, a.1, b.0, b.1);
        }
        assert!(bl.rms_deg < 0.02 && bc.rms_deg < 0.02);
        assert!(bl.max_deg < 0.12 && bc.max_deg < 0.12);
    }
}
