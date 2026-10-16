//! EuRoC/ASL datasets as Monado's recorder writes them (`mav0/imu0/data.csv`, `mav0/camN/data.csv` +
//! `mav0/camN/data/<ts>.png`), plus a consistency check for SLAM use.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// One IMU sample (EuRoC column order: gyro then accel).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImuRow {
    /// ns.
    pub ts: i64,
    /// rad/s.
    pub gyro: [f64; 3],
    /// m/s².
    pub accel: [f64; 3],
}

/// Find the dataset root (the directory holding `mav0`): `dir` itself, or the newest `dir/euroc_*` /
/// `dir/*` child with a `mav0` (the Pimax driver writes `<dir>/euroc_<datetime>/mav0`).
pub fn find_root(dir: &Path) -> Result<PathBuf, String> {
    if dir.join("mav0").is_dir() {
        return Ok(dir.to_path_buf());
    }
    let mut cands: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join("mav0").is_dir())
        .collect();
    cands.sort();
    cands
        .pop()
        .ok_or_else(|| format!("{}: no mav0 directory (not a EuRoC dataset)", dir.display()))
}

fn data_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l.trim()))
        .filter(|(_, l)| !l.is_empty() && !l.starts_with('#'))
}

/// Parse `imu0/data.csv` text.
pub fn parse_imu(text: &str) -> Result<Vec<ImuRow>, String> {
    let mut v = Vec::new();
    for (n, l) in data_lines(text) {
        let f: Vec<&str> = l.split(',').map(str::trim).collect();
        if f.len() < 7 {
            return Err(format!("imu line {n}: {} fields, want 7", f.len()));
        }
        let ts: i64 = f[0]
            .parse()
            .map_err(|_| format!("imu line {n}: bad timestamp {:?}", f[0]))?;
        let mut x = [0.0; 6];
        for k in 0..6 {
            x[k] = f[k + 1]
                .parse()
                .map_err(|_| format!("imu line {n}: bad number {:?}", f[k + 1]))?;
        }
        v.push(ImuRow {
            ts,
            gyro: [x[0], x[1], x[2]],
            accel: [x[3], x[4], x[5]],
        });
    }
    Ok(v)
}

/// Parse a `camN/data.csv` text into (timestamp, file name).
pub fn parse_cam(text: &str) -> Result<Vec<(i64, String)>, String> {
    let mut v = Vec::new();
    for (n, l) in data_lines(text) {
        let (a, b) = l
            .split_once(',')
            .ok_or_else(|| format!("cam line {n}: want ts,filename"))?;
        let ts: i64 = a
            .trim()
            .parse()
            .map_err(|_| format!("cam line {n}: bad timestamp {a:?}"))?;
        v.push((ts, b.trim().to_string()));
    }
    Ok(v)
}

/// A loaded dataset (images are not read).
#[derive(Clone, Debug, Default)]
pub struct Dataset {
    /// Root (holds mav0).
    pub root: PathBuf,
    /// IMU rows.
    pub imu: Vec<ImuRow>,
    /// Per camera: (ts, file).
    pub cams: Vec<Vec<(i64, String)>>,
}

impl Dataset {
    /// Load `imu0` and every `camN` found.
    pub fn load(dir: &Path) -> Result<Dataset, String> {
        let root = find_root(dir)?;
        let read =
            |p: PathBuf| std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()));
        let imu = parse_imu(&read(root.join("mav0/imu0/data.csv"))?)?;
        let mut cams = Vec::new();
        for i in 0.. {
            let p = root.join(format!("mav0/cam{i}/data.csv"));
            if !p.is_file() {
                break;
            }
            cams.push(parse_cam(&read(p)?)?);
        }
        Ok(Dataset { root, imu, cams })
    }
}

fn median(v: &mut [f64]) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

/// Result of [`check`].
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Text lines.
    pub text: String,
    /// Problems that make the dataset unusable or suspicious.
    pub problems: Vec<String>,
}

/// Interval statistics of a timestamp series: (median dt s, max dt s, gaps > 1.5 median, non-increasing).
fn intervals(ts: &[i64]) -> (f64, f64, usize, usize) {
    let mut d: Vec<f64> = ts.windows(2).map(|w| (w[1] - w[0]) as f64 / 1e9).collect();
    let back = d.iter().filter(|&&x| x <= 0.0).count();
    let max = d.iter().cloned().fold(0.0, f64::max);
    let med = median(&mut d.clone());
    let gaps = d.iter().filter(|&&x| x > 1.5 * med).count();
    d.clear();
    (med, max, gaps, back)
}

/// Check a dataset for SLAM: rates, gaps, ordering, camera sync, camera/IMU overlap, gravity magnitude, missing
/// images (when `check_files`).
pub fn check(ds: &Dataset, check_files: bool) -> Report {
    let mut r = Report::default();
    let mut w = |s: String| {
        r.text.push_str(&s);
        r.text.push('\n');
    };
    w(format!("dataset {}", ds.root.display()));
    let mut problems = Vec::new();
    if ds.imu.len() < 2 {
        problems.push("fewer than 2 IMU samples".to_string());
    } else {
        let ts: Vec<i64> = ds.imu.iter().map(|s| s.ts).collect();
        let (med, max, gaps, back) = intervals(&ts);
        let span = (ts[ts.len() - 1] - ts[0]) as f64 / 1e9;
        w(format!(
            "imu0: {} samples over {:.2} s, {:.1} Hz (median dt {:.3} ms, max {:.2} ms), {} gaps > 1.5x, {} non-increasing",
            ts.len(),
            span,
            1.0 / med,
            med * 1e3,
            max * 1e3,
            gaps,
            back
        ));
        if back > 0 {
            problems.push(format!("imu0: {back} non-increasing timestamps"));
        }
        if max > 0.02 {
            problems.push(format!("imu0: a {:.1} ms gap (USB stall?)", max * 1e3));
        }
        let mut g: Vec<f64> = ds
            .imu
            .iter()
            .map(|s| (s.accel[0].powi(2) + s.accel[1].powi(2) + s.accel[2].powi(2)).sqrt())
            .collect();
        let gm = median(&mut g);
        w(format!("imu0: median |accel| {gm:.3} m/s² (gravity 9.807)"));
        if (gm - 9.807).abs() > 0.5 {
            problems.push(format!("imu0: median |accel| {gm:.2} m/s², scale wrong?"));
        }
    }
    let first_imu = ds.imu.first().map(|s| s.ts).unwrap_or(0);
    let last_imu = ds.imu.last().map(|s| s.ts).unwrap_or(0);
    if ds.cams.is_empty() {
        problems.push("no camN/data.csv".into());
    }
    for (i, c) in ds.cams.iter().enumerate() {
        let ts: Vec<i64> = c.iter().map(|x| x.0).collect();
        if ts.len() < 2 {
            problems.push(format!("cam{i}: fewer than 2 frames"));
            continue;
        }
        let (med, max, gaps, back) = intervals(&ts);
        let outside = ts
            .iter()
            .filter(|&&t| t < first_imu || t > last_imu)
            .count();
        w(format!(
            "cam{i}: {} frames, {:.2} Hz (median dt {:.2} ms, max {:.1} ms), {} gaps > 1.5x, {} non-increasing, {} outside the IMU span",
            ts.len(),
            1.0 / med,
            med * 1e3,
            max * 1e3,
            gaps,
            back,
            outside
        ));
        if back > 0 {
            problems.push(format!("cam{i}: {back} non-increasing timestamps"));
        }
        if outside > 2 {
            problems.push(format!(
                "cam{i}: {outside} frames outside the IMU time span (clock mismatch?)"
            ));
        }
        if check_files {
            let dir = ds.root.join(format!("mav0/cam{i}/data"));
            let missing = c.iter().filter(|(_, f)| !dir.join(f).is_file()).count();
            if missing > 0 {
                problems.push(format!(
                    "cam{i}: {missing} image files missing (recording cut short?)"
                ));
            }
        }
    }
    if ds.cams.len() > 1 {
        let base: std::collections::HashSet<i64> = ds.cams[0].iter().map(|x| x.0).collect();
        for (i, c) in ds.cams.iter().enumerate().skip(1) {
            let common = c.iter().filter(|x| base.contains(&x.0)).count();
            w(format!(
                "cam{i}: {common} of {} timestamps shared with cam0 (synchronised stereo wants all)",
                c.len()
            ));
            if common * 10 < c.len() * 9 {
                problems.push(format!(
                    "cam{i}: only {common}/{} timestamps match cam0",
                    c.len()
                ));
            }
        }
    }
    for p in &problems {
        w(format!("PROBLEM: {p}"));
    }
    if problems.is_empty() {
        w("OK: usable for offline SLAM replay".into());
    }
    r.problems = problems;
    r
}

/// Write a dataset's CSV files (used by tests and the smoke-test generator).
pub fn write_csvs(root: &Path, imu: &[ImuRow], cams: &[Vec<(i64, String)>]) -> std::io::Result<()> {
    let d = root.join("mav0/imu0");
    std::fs::create_dir_all(&d)?;
    let mut s = String::from("#timestamp [ns],w_RS_S_x [rad s^-1],w_RS_S_y [rad s^-1],w_RS_S_z [rad s^-1],a_RS_S_x [m s^-2],a_RS_S_y [m s^-2],a_RS_S_z [m s^-2]\r\n");
    for r in imu {
        let _ = write!(
            s,
            "{},{},{},{},{},{},{}\r\n",
            r.ts, r.gyro[0], r.gyro[1], r.gyro[2], r.accel[0], r.accel[1], r.accel[2]
        );
    }
    std::fs::write(d.join("data.csv"), s)?;
    for (i, c) in cams.iter().enumerate() {
        let d = root.join(format!("mav0/cam{i}"));
        std::fs::create_dir_all(d.join("data"))?;
        let mut s = String::from("#timestamp [ns],filename\r\n");
        for (t, f) in c {
            let _ = write!(s, "{t},{f}\r\n");
        }
        std::fs::write(d.join("data.csv"), s)?;
    }
    Ok(())
}

/// Copy of a dataset with every camera timestamp shifted by `offset_ns` (images symlinked, IMU unchanged).
/// Basalt's VIO ignores `cam_time_offset_ns` (the correction is commented out in sqrt_keypoint_vio.cpp), so a
/// camera-IMU time offset sweep has to shift the data instead. Returns the new EuRoC root.
pub fn shift_cams(ds: &Dataset, out: &Path, offset_ns: i64) -> Result<PathBuf, String> {
    let e = |x: std::io::Error| format!("{}: {x}", out.display());
    let shifted: Vec<Vec<(i64, String)>> = ds
        .cams
        .iter()
        .map(|c| c.iter().map(|(t, f)| (t + offset_ns, f.clone())).collect())
        .collect();
    write_csvs(out, &ds.imu, &shifted).map_err(e)?;
    for i in 0..ds.cams.len() {
        let link = out.join(format!("mav0/cam{i}/data"));
        let _ = std::fs::remove_dir(&link);
        let target = ds.root.join(format!("mav0/cam{i}/data"));
        let target = target.canonicalize().unwrap_or(target);
        std::os::unix::fs::symlink(&target, &link).map_err(e)?;
    }
    Ok(out.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth(dir: &Path, cam_shift: i64) {
        let imu: Vec<ImuRow> = (0..2000)
            .map(|i| ImuRow {
                ts: 1_000_000_000 + i * 1_000_000,
                gyro: [0.001, 0.0, 0.0],
                accel: [0.0, 9.81, 0.0],
            })
            .collect();
        let cam: Vec<(i64, String)> = (0..40)
            .map(|i| {
                let t = 1_100_000_000 + i * 40_000_000;
                (t, format!("{t}.png"))
            })
            .collect();
        let cam1: Vec<(i64, String)> = cam
            .iter()
            .map(|(t, f)| (t + cam_shift, f.clone()))
            .collect();
        write_csvs(dir, &imu, &[cam, cam1]).unwrap();
    }

    #[test]
    fn good_dataset_passes() {
        let dir = std::env::temp_dir().join(format!("pimax-slam-euroc-ok-{}", std::process::id()));
        let root = dir.join("euroc_20261014120000");
        synth(&root, 0);
        let ds = Dataset::load(&dir).unwrap();
        assert_eq!(ds.root, root);
        assert_eq!(ds.imu.len(), 2000);
        let r = check(&ds, false);
        assert!(r.problems.is_empty(), "{}", r.text);
        assert!(r.text.contains("25.00 Hz"), "{}", r.text);
        assert!(r.text.contains("1000.0 Hz"), "{}", r.text);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unsynced_and_missing_files_flagged() {
        let dir = std::env::temp_dir().join(format!("pimax-slam-euroc-bad-{}", std::process::id()));
        synth(&dir, 3_000_000);
        let ds = Dataset::load(&dir).unwrap();
        let r = check(&ds, true);
        assert!(
            r.problems.iter().any(|p| p.contains("match cam0")),
            "{}",
            r.text
        );
        assert!(
            r.problems.iter().any(|p| p.contains("missing")),
            "{}",
            r.text
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
