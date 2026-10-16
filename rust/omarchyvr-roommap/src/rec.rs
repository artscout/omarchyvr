//! Offline input: a 6DoF recording (EuRoC layout written by the Monado pimax driver, `PIMAX_SLAM_RECORD`) and the
//! tracker trajectory of the same session (Basalt `tracking.csv` from `run_slam.sh replay`).
//!
//! Recording directory: `<dir>/euroc_<stamp>/mav0/{cam0,cam1,imu0}`; `cam0`/`cam1` are factory cameras 0 and 1
//! (the lower tracking pair), 640x480 grey PNG tiles, the same pixels the broker hands out. The trajectory is the
//! IMU body in Basalt's world (z up), converted here to the y-up world Monado uses.

use pimax_cam::geom::Pose;
use pimax_cam::slam::{mat_of, qmul, qnorm, qrot, Quat};
use std::path::{Path, PathBuf};

/// Basalt's world is z up, ours y up: rotate -90 deg about X (the driver's `SLAM_TO_YUP`).
pub const SLAM_TO_YUP: Quat = [
    -std::f64::consts::FRAC_1_SQRT_2,
    0.0,
    0.0,
    std::f64::consts::FRAC_1_SQRT_2,
];

/// One stereo frame set of a recording.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameRef {
    /// Frame time (ns, the recording's clock).
    pub ts_ns: i64,
    /// cam0 PNG.
    pub cam0: PathBuf,
    /// cam1 PNG.
    pub cam1: PathBuf,
}

/// A recording with its trajectory.
#[derive(Clone, Debug)]
pub struct Recording {
    /// EuRoC root (the directory holding `mav0`).
    pub root: PathBuf,
    /// Trajectory file used.
    pub tracking: PathBuf,
    /// Frame sets present in both cameras, sorted by time.
    pub frames: Vec<FrameRef>,
    /// `slam_from_body` (y up), sorted by time.
    pub traj: Vec<(i64, Pose)>,
}

/// EuRoC root of a recording directory (the directory itself if it holds `mav0`, else the newest `*/mav0`).
pub fn dataset_root(d: &Path) -> Result<PathBuf, String> {
    if d.join("mav0").is_dir() {
        return Ok(d.to_path_buf());
    }
    let mut v: Vec<PathBuf> = std::fs::read_dir(d)
        .map_err(|e| format!("{}: {e}", d.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join("mav0").is_dir())
        .collect();
    v.sort();
    v.pop().ok_or_else(|| format!("{}: no EuRoC dataset (mav0)", d.display()))
}

/// Newest `replay-*/tracking.csv` of a recording directory (what `run_slam.sh replay` writes).
pub fn find_tracking(d: &Path) -> Option<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(d)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("replay-"))
                && p.join("tracking.csv").is_file()
        })
        .collect();
    v.sort_by_key(|p| std::fs::metadata(p.join("tracking.csv")).and_then(|m| m.modified()).ok());
    v.pop().map(|p| p.join("tracking.csv"))
}

/// Parse a trajectory (`ts,px,py,pz,qw,qx,qy,qz`, Basalt world z up) into y-up `slam_from_body` poses.
pub fn parse_tracking(text: &str) -> Vec<(i64, Pose)> {
    let mut v = Vec::new();
    for l in text.lines() {
        let l = l.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let f: Vec<f64> = l.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        if f.len() < 8 || f.iter().any(|x| !x.is_finite()) {
            continue;
        }
        let q = qmul(SLAM_TO_YUP, qnorm([f[5], f[6], f[7], f[4]]));
        let p = qrot(SLAM_TO_YUP, [f[1], f[2], f[3]]);
        v.push((f[0] as i64, Pose { r: mat_of(q), t: p }));
    }
    v.sort_by_key(|x| x.0);
    v
}

/// Tracker pose at a frame time (nearest sample within 5 ms; Basalt writes one pose per frame).
pub fn pose_at(tr: &[(i64, Pose)], ts: i64) -> Option<Pose> {
    let i = tr.partition_point(|x| x.0 < ts);
    [i.saturating_sub(1), i]
        .iter()
        .filter_map(|&j| tr.get(j))
        .filter(|(t, _)| (t - ts).abs() <= 5_000_000)
        .min_by_key(|(t, _)| (t - ts).abs())
        .map(|x| x.1)
}

fn read_cam(root: &Path, c: usize) -> Result<Vec<(i64, String)>, String> {
    let p = root.join(format!("mav0/cam{c}/data.csv"));
    let t = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(t.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .filter_map(|l| {
            let (a, b) = l.split_once(',')?;
            Some((a.trim().parse().ok()?, b.trim().to_string()))
        })
        .collect())
}

impl Recording {
    /// Open a recording directory (or EuRoC root); `tracking` = trajectory file, default the newest replay.
    pub fn open(dir: &Path, tracking: Option<&Path>) -> Result<Recording, String> {
        let root = dataset_root(dir)?;
        let tracking = match tracking {
            Some(t) => t.to_path_buf(),
            None => find_tracking(dir)
                .or_else(|| root.parent().and_then(find_tracking))
                .ok_or_else(|| {
                    format!(
                        "{}: no replay-*/tracking.csv; run `analysis/tools/run_slam.sh replay {}` first or pass --tracking",
                        dir.display(),
                        dir.display()
                    )
                })?,
        };
        let text = std::fs::read_to_string(&tracking).map_err(|e| format!("{}: {e}", tracking.display()))?;
        let traj = parse_tracking(&text);
        if traj.is_empty() {
            return Err(format!("{}: no poses", tracking.display()));
        }
        let (c0, c1) = (read_cam(&root, 0)?, read_cam(&root, 1)?);
        let m1: std::collections::HashMap<i64, String> = c1.into_iter().collect();
        let frames = c0
            .into_iter()
            .filter_map(|(t, f)| {
                let g = m1.get(&t)?;
                Some(FrameRef {
                    ts_ns: t,
                    cam0: root.join("mav0/cam0/data").join(f),
                    cam1: root.join("mav0/cam1/data").join(g),
                })
            })
            .collect();
        Ok(Recording { root, tracking, frames, traj })
    }

    /// Short name (the recording directory name).
    pub fn name(&self) -> String {
        let d = if self.root.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("euroc_")) {
            self.root.parent().unwrap_or(&self.root)
        } else {
            &self.root
        };
        d.file_name().and_then(|n| n.to_str()).unwrap_or("recording").to_string()
    }
}
