//! Files: the map directory (`~/.local/share/omarchyvr/roommap/<name>/`), relocalisation results, and the Monado world
//! offset (written through `omarchyvr_anchor::world`, the same `PIMAX_ANCHOR=1` hook the AprilTag anchor uses).
//!
//! Map directory: `map.bin` (the `omarchyvr-room` map format: stereo keyframes in room coordinates, y up, gravity
//! aligned), `roommap.toml` (what it was built from), `reloc-<source>.toml` and `last-reloc.toml` (results).

use pimax_cam::geom::Pose;
use omarchyvr_room::session::Alignment;
use std::path::{Path, PathBuf};

/// Base directory of the room maps.
pub fn base_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("PIMAX_ROOMMAP_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(p);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join(".local/share/omarchyvr/roommap")
}

/// Directory of the map `name` (a path with a `/` is used as is).
pub fn map_dir(name: &str) -> PathBuf {
    if name.contains('/') {
        PathBuf::from(name)
    } else {
        base_dir().join(name)
    }
}

/// A relocalisation result (`room_from_slam` = the offset to write).
#[derive(Clone, Debug, PartialEq)]
pub struct RelocResult {
    /// Map directory.
    pub map: String,
    /// What was relocalised (recording path or `broker`).
    pub source: String,
    /// `room_from_slam`.
    pub align: Alignment,
    /// Frames pooled.
    pub frames: usize,
    /// Correspondences used.
    pub inliers: usize,
    /// RMS residual (deg).
    pub rms_deg: f64,
    /// Formal 1-sigma yaw (deg).
    pub sigma_deg: f64,
    /// Formal 1-sigma translation (m).
    pub sigma_m: f64,
    /// Seconds from the first frame to the accepted fix.
    pub fix_after_s: f64,
    /// Monado pid the tracker world belongs to (live), 0 offline.
    pub pid: u32,
}

impl RelocResult {
    /// TOML text.
    pub fn to_toml(&self) -> String {
        format!(
            "# written by omarchyvr-roommap relocalize: room_from_slam = the Monado world offset (app_from_slam)\n\
             map = \"{}\"\nsource = \"{}\"\nyaw_deg = {:.6}\nt = [{:.6}, {:.6}, {:.6}]\nframes = {}\ninliers = {}\n\
             rms_deg = {:.4}\nsigma_deg = {:.4}\nsigma_m = {:.5}\nfix_after_s = {:.3}\npid = {}\n",
            self.map,
            self.source,
            self.align.yaw.to_degrees(),
            self.align.t[0],
            self.align.t[1],
            self.align.t[2],
            self.frames,
            self.inliers,
            self.rms_deg,
            self.sigma_deg,
            self.sigma_m,
            self.fix_after_s,
            self.pid
        )
    }

    /// Parse [`RelocResult::to_toml`] output.
    pub fn parse(s: &str) -> Result<RelocResult, String> {
        let mut r = RelocResult {
            map: String::new(),
            source: String::new(),
            align: Alignment { yaw: f64::NAN, t: [f64::NAN; 3] },
            frames: 0,
            inliers: 0,
            rms_deg: f64::NAN,
            sigma_deg: f64::NAN,
            sigma_m: f64::NAN,
            fix_after_s: f64::NAN,
            pid: 0,
        };
        for l in s.lines() {
            let l = l.split('#').next().unwrap_or("").trim();
            let Some((k, v)) = l.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            let num = |v: &str| v.parse::<f64>().map_err(|e| format!("{k}: {e}"));
            match k {
                "map" => r.map = v.trim_matches('"').to_string(),
                "source" => r.source = v.trim_matches('"').to_string(),
                "yaw_deg" => r.align.yaw = num(v)?.to_radians(),
                "t" => {
                    let x: Vec<f64> = v
                        .trim_matches(|c| c == '[' || c == ']')
                        .split(',')
                        .map(|x| x.trim().parse::<f64>().map_err(|e| format!("t: {e}")))
                        .collect::<Result<_, _>>()?;
                    if x.len() != 3 {
                        return Err("t needs 3 numbers".into());
                    }
                    r.align.t = [x[0], x[1], x[2]];
                }
                "frames" => r.frames = num(v)? as usize,
                "inliers" => r.inliers = num(v)? as usize,
                "rms_deg" => r.rms_deg = num(v)?,
                "sigma_deg" => r.sigma_deg = num(v)?,
                "sigma_m" => r.sigma_m = num(v)?,
                "fix_after_s" => r.fix_after_s = num(v)?,
                "pid" => r.pid = num(v)? as u32,
                _ => {}
            }
        }
        if !r.align.yaw.is_finite() || r.align.t.iter().any(|x| !x.is_finite()) {
            return Err("yaw_deg / t missing".into());
        }
        Ok(r)
    }

    /// Load from a file.
    pub fn load(p: &Path) -> Result<RelocResult, String> {
        RelocResult::parse(&std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?)
    }

    /// Save (atomic).
    pub fn save(&self, p: &Path) -> std::io::Result<()> {
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = p.with_extension("toml.tmp");
        std::fs::write(&tmp, self.to_toml())?;
        std::fs::rename(tmp, p)
    }
}

/// The offset Monado applies (`app_from_slam = offset * slam`) for a relocalisation: the room frame becomes the
/// app world, so everything placed in the room frame (WayVR's stage and its windows) lands on the same real spot.
pub fn offset_pose(a: &Alignment) -> Pose {
    a.pose()
}

/// Write the offset file for Monado `pid` (atomic). Returns the path.
pub fn write_offset(a: &Alignment, pid: u32) -> std::io::Result<PathBuf> {
    let p = omarchyvr_anchor::world::offset_path();
    omarchyvr_anchor::world::write_offset(&p, pid, &offset_pose(a))?;
    Ok(p)
}

/// Compose two alignments: `a * b` (both yaw + translation).
pub fn compose(a: &Alignment, b: &Alignment) -> Alignment {
    let p = a.pose().compose(&b.pose());
    Alignment { yaw: crate::align::wrap(a.yaw + b.yaw), t: p.t }
}

/// Inverse of an alignment.
pub fn inverse(a: &Alignment) -> Alignment {
    let p = a.pose().inverse();
    Alignment { yaw: crate::align::wrap(-a.yaw), t: p.t }
}

/// `roommap.toml` text.
pub fn meta_text(source: &str, keyframes: usize, points: usize, created_unix: u64) -> String {
    format!(
        "# omarchyvr-roommap map: room frame = the tracker world of the mapping session (y up, gravity aligned)\n\
         source = \"{source}\"\nkeyframes = {keyframes}\npoints = {points}\ncreated = \"unix {created_unix}\"\n"
    )
}

/// Save the mapping session's trajectory corrections (`correction.csv`: ts_ns, yaw_rad, tx, ty, tz).
pub fn save_corrections(dir: &Path, c: &[crate::loopclose::Correction]) -> std::io::Result<()> {
    let mut t = String::from("# ts_ns,yaw_rad,tx,ty,tz: room_from_slam of the mapping session after loop closure\n");
    for x in c {
        t += &format!("{},{:.9},{:.6},{:.6},{:.6}\n", x.ts_ns, x.c.yaw, x.c.t[0], x.c.t[1], x.c.t[2]);
    }
    std::fs::write(dir.join("correction.csv"), t)
}

/// Load `correction.csv` (empty when absent: room frame = the mapping session's world).
pub fn load_corrections(dir: &Path) -> Vec<crate::loopclose::Correction> {
    let Ok(t) = std::fs::read_to_string(dir.join("correction.csv")) else { return Vec::new() };
    t.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let f: Vec<f64> = l.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            (f.len() == 5).then(|| crate::loopclose::Correction {
                ts_ns: f[0] as i64,
                c: Alignment { yaw: f[1], t: [f[2], f[3], f[4]] },
            })
        })
        .collect()
}
