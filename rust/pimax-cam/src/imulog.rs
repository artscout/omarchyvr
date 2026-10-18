//! Head IMU log for offline replays (`PIMAX_HEAD_IMU_LOG=<file>`): every IMU sample the 6DoF output gets and every
//! tracker pose time, so a "Record tracking" take (cameras.pxcam + this log) can be replayed through Basalt
//! (`omarchyvr-slam take-euroc`). Monado owns the headset's HID interface, so this is the only place the head IMU of a
//! running session can be saved.
//!
//! A leaf: the IMU thread only sends a small message into a channel; a writer thread owns the file. If the file
//! cannot be written the log stops and nothing else changes. It writes for the first `PIMAX_HEAD_IMU_LOG_MIN`
//! minutes (default 10) and afterwards only while `$XDG_RUNTIME_DIR/omarchyvr-imu-log.on` exists (the tracking
//! recorder creates it for the length of a take).
//!
//! Every start keeps the earlier logs ([`rotate`]): the previous one becomes `head-imu.<YYYYmmdd-HHMMSS>.csv` (its
//! own last write time, local) in [`keep_dir`] (`~/.local/state/omarchyvr/head-imu/`, on disk: the session dir
//! `/tmp/omarchyvr-session` is tmpfs, RAM, and gone at a reboot) and `head-imu.csv.prev` in the session dir links to
//! it; the newest [`KEEP`] old logs are kept (about 45 MB each for the first 10 minutes). Before 2026-10-18 every
//! start overwrote the only copy; the 0.1.3 release review moved the kept copies out of RAM.
//!
//! Format (text, one line each):
//! * `I,ts_ns,ax,ay,az,gx,gy,gz`: IMU frame, m/s² and rad/s, `CLOCK_MONOTONIC` ns, exactly as fed to the tracker;
//! * `V,ts_ns,px,py,pz,qx,qy,qz,qw`: a tracker pose; `ts_ns` is its camera frame time on the same clock, which
//!   ties the camera's device clock to the IMU's.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};

/// Header line of the file.
pub const HEADER: &str = "# pimax-head-imu v1: I,ts_ns,ax,ay,az,gx,gy,gz (IMU frame, m/s2, rad/s) | V,ts_ns,px,py,pz,qx,qy,qz,qw (tracker pose at its camera frame time); CLOCK_MONOTONIC ns";

enum Msg {
    Imu(i64, [f64; 3], [f64; 3]),
    Vio(i64, [f64; 3], [f64; 4]),
}

/// The sending side (owned by the head tracker).
pub struct HeadImuLog {
    tx: SyncSender<Msg>,
    /// Messages dropped because the writer fell behind.
    pub dropped: u64,
}

/// File that keeps the log writing after the first minutes (created by `omarchyvr record tracking`).
pub fn flag_path() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map(|d| PathBuf::from(d).join("omarchyvr-imu-log.on"))
}

impl HeadImuLog {
    /// From `PIMAX_HEAD_IMU_LOG` (and `PIMAX_HEAD_IMU_LOG_MIN`); None when unset or the file cannot be created.
    pub fn from_env() -> Option<HeadImuLog> {
        let p = std::env::var_os("PIMAX_HEAD_IMU_LOG").filter(|v| !v.is_empty())?;
        let minutes = std::env::var("PIMAX_HEAD_IMU_LOG_MIN")
            .ok()
            .and_then(|v| v.trim().parse::<f64>().ok())
            .unwrap_or(10.0);
        HeadImuLog::start(Path::new(&p), minutes, flag_path())
    }

    /// Log to `path`: always for `minutes`, then while `flag` exists. An earlier log at `path` is kept ([`rotate`]).
    pub fn start(path: &Path, minutes: f64, flag: Option<PathBuf>) -> Option<HeadImuLog> {
        HeadImuLog::start_keeping(path, minutes, flag, keep_dir(&|k| std::env::var(k).ok()))
    }

    /// [`HeadImuLog::start`] with the directory for the kept logs (`None`: they stay next to `path`). The earlier
    /// log is renamed at once (same directory, instant); the copy to `keep` runs on a thread of its own.
    pub fn start_keeping(path: &Path, minutes: f64, flag: Option<PathBuf>, keep: Option<PathBuf>) -> Option<HeadImuLog> {
        if let Some(staged) = stage(path) {
            let keep = keep.unwrap_or_else(|| path.parent().map(Path::to_path_buf).unwrap_or_default());
            let link = prev_link(path);
            let _ = std::thread::Builder::new().name("omarchyvr-imu-keep".into()).spawn(move || {
                let _ = finish(&staged, &keep, &link, KEEP);
            });
        }
        let f = std::fs::File::create(path).ok()?;
        let (tx, rx) = sync_channel::<Msg>(8192);
        let span_ns = (minutes.max(0.0) * 60e9) as i64;
        std::thread::Builder::new()
            .name("omarchyvr-imu-log".into())
            .spawn(move || {
                let mut w = std::io::BufWriter::with_capacity(1 << 16, f);
                if writeln!(w, "{HEADER}").is_err() {
                    return;
                }
                let mut first: Option<i64> = None;
                let (mut flag_on, mut flag_checked) = (false, None::<i64>);
                for m in rx {
                    let ts = match m {
                        Msg::Imu(t, ..) | Msg::Vio(t, ..) => t,
                    };
                    let t0 = *first.get_or_insert(ts);
                    if ts.saturating_sub(t0) > span_ns {
                        if flag_checked.is_none_or(|c| ts.saturating_sub(c) > 1_000_000_000) {
                            flag_checked = Some(ts);
                            let on = flag.as_ref().is_some_and(|f| f.exists());
                            if flag_on && !on {
                                let _ = w.flush();
                            }
                            flag_on = on;
                        }
                        if !flag_on {
                            continue;
                        }
                    }
                    let r = match m {
                        Msg::Imu(t, a, g) => writeln!(
                            w,
                            "I,{t},{:.5},{:.5},{:.5},{:.6},{:.6},{:.6}",
                            a[0], a[1], a[2], g[0], g[1], g[2]
                        ),
                        Msg::Vio(t, p, q) => writeln!(
                            w,
                            "V,{t},{:.5},{:.5},{:.5},{:.6},{:.6},{:.6},{:.6}",
                            p[0], p[1], p[2], q[0], q[1], q[2], q[3]
                        ),
                    };
                    if r.is_err() {
                        return;
                    }
                }
                let _ = w.flush();
            })
            .ok()?;
        Some(HeadImuLog { tx, dropped: 0 })
    }

    fn send(&mut self, m: Msg) {
        match self.tx.try_send(m) {
            Ok(()) | Err(TrySendError::Disconnected(_)) => {}
            Err(TrySendError::Full(_)) => self.dropped += 1,
        }
    }

    /// One IMU sample (never blocks).
    pub fn imu(&mut self, ts_ns: i64, accel: [f64; 3], gyro: [f64; 3]) {
        self.send(Msg::Imu(ts_ns, accel, gyro));
    }

    /// One tracker pose (never blocks).
    pub fn vio(&mut self, ts_ns: i64, pos: [f64; 3], q: [f64; 4]) {
        self.send(Msg::Vio(ts_ns, pos, q));
    }
}

/// Old logs kept next to the current one.
pub const KEEP: usize = 3;

/// Local time stamp `YYYYmmdd-HHMMSS` of a file's modification time (libc localtime_r).
fn stamp_of(path: &Path) -> Option<String> {
    let t = std::fs::metadata(path).ok()?.modified().ok()?;
    let secs = t.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: valid pointers to a local time_t and tm.
    if unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
        return None;
    }
    Some(format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    ))
}

/// Where the kept logs go: `$XDG_STATE_HOME/omarchyvr/head-imu`, else `$HOME/.local/state/omarchyvr/head-imu`
/// (None without either: they stay next to the log).
pub fn keep_dir(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let state = env("XDG_STATE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| env("HOME").filter(|v| !v.is_empty()).map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(state.join("omarchyvr/head-imu"))
}

/// `dir/name.csv.prev` for `dir/name.csv`.
fn prev_link(path: &Path) -> PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push(".prev");
    PathBuf::from(p)
}

/// Point `link` at `target` (a symlink; relative when they share a directory).
fn relink(link: &Path, target: &Path) {
    let t = if link.parent() == target.parent() { target.file_name().map(PathBuf::from) } else { Some(target.to_path_buf()) };
    let _ = std::fs::remove_file(link);
    if let Some(t) = t {
        let _ = std::os::unix::fs::symlink(t, link);
    }
}

/// Step 1 of keeping a log (instant, same directory): rename a non-empty `dir/name.csv` to
/// `dir/name.<stamp>.keeping` and point `name.csv.prev` at it. Returns the staged file.
fn stage(path: &Path) -> Option<PathBuf> {
    if !path.is_file() || std::fs::metadata(path).ok()?.len() == 0 {
        return None;
    }
    let dir = path.parent()?;
    let name = path.file_name()?.to_str()?;
    let stem = name.strip_suffix(".csv").unwrap_or(name);
    let stamp = stamp_of(path)?;
    let mut staged = dir.join(format!("{stem}.{stamp}.keeping"));
    let mut n = 1;
    while staged.exists() {
        staged = dir.join(format!("{stem}.{stamp}-{n}.keeping"));
        n += 1;
    }
    std::fs::rename(path, &staged).ok()?;
    relink(&prev_link(path), &staged);
    Some(staged)
}

/// Step 2: move a staged `name.<stamp>.keeping` into `keep_dir` as `name.<stamp>.csv` (a rename, or a copy across
/// filesystems), re-point `link` at it and delete all but the newest `keep` stamped copies there. Returns the kept
/// file. On an error the staged file is renamed to `name.<stamp>.csv` where it is (still linked).
fn finish(staged: &Path, keep_dir: &Path, link: &Path, keep: usize) -> Option<PathBuf> {
    let fname = staged.file_name()?.to_str()?;
    let base = fname.strip_suffix(".keeping")?;
    let (stem, _) = base.rsplit_once('.')?;
    let moved = (|| {
        std::fs::create_dir_all(keep_dir).ok()?;
        let mut dest = keep_dir.join(format!("{base}.csv"));
        let mut n = 1;
        while dest.exists() {
            dest = keep_dir.join(format!("{base}-{n}.csv"));
            n += 1;
        }
        if std::fs::rename(staged, &dest).is_err() {
            // tmpfs -> disk: copy, keep the write time (the copies are sorted by it), then drop the RAM copy
            let mtime = std::fs::metadata(staged).and_then(|m| m.modified()).ok();
            if std::fs::copy(staged, &dest).is_err() {
                let _ = std::fs::remove_file(&dest);
                return None;
            }
            if let (Some(t), Ok(f)) = (mtime, std::fs::File::options().write(true).open(&dest)) {
                let _ = f.set_modified(t);
            }
            let _ = std::fs::remove_file(staged);
        }
        Some(dest)
    })();
    let Some(dest) = moved else {
        let here = staged.with_file_name(format!("{base}.csv"));
        if std::fs::rename(staged, &here).is_ok() {
            relink(link, &here);
        }
        return None;
    };
    relink(link, &dest);
    prune(keep_dir, stem, keep);
    Some(dest)
}

/// Keep an existing log at `path` (`dir/name.csv`) now: move it to `keep_dir/name.<stamp>.csv`, point
/// `dir/name.csv.prev` at it (a symlink), and delete all but the newest `keep` stamped copies in `keep_dir`. Returns
/// the kept file. Errors leave the old log in `dir` (renamed to its stamp when possible).
pub fn rotate(path: &Path, keep_dir: &Path, keep: usize) -> Option<PathBuf> {
    let staged = stage(path)?;
    finish(&staged, keep_dir, &prev_link(path), keep)
}

/// Delete all but the newest `keep` copies `stem.<stamp>.csv` in `dir`.
fn prune(dir: &Path, stem: &str, keep: usize) -> Option<()> {
    // The stamped copies sort by time; the newest `keep` stay.
    let prefix = format!("{stem}.");
    let mut old: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name().and_then(|f| f.to_str()).is_some_and(|f| {
                f.starts_with(&prefix) && f.ends_with(".csv") && f.as_bytes().get(prefix.len()).is_some_and(|c| c.is_ascii_digit())
            })
        })
        .collect();
    // Oldest first: by write time, then by the collision number of copies stamped in the same second.
    let key = |p: &PathBuf| {
        let t = std::fs::metadata(p).and_then(|m| m.modified()).ok();
        let f = p.file_name().and_then(|f| f.to_str()).unwrap_or("");
        let n: u32 = f.trim_end_matches(".csv").rsplit_once('-').and_then(|(_, x)| (x.len() < 6).then(|| x.parse().ok()).flatten()).unwrap_or(0);
        (t, n)
    };
    old.sort_by_key(key);
    while old.len() > keep.max(1) {
        let _ = std::fs::remove_file(old.remove(0));
    }
    Some(())
}

/// A parsed head IMU log.
#[derive(Clone, Debug, Default)]
pub struct HeadImu {
    /// (ts ns, accel m/s², gyro rad/s)
    pub imu: Vec<(i64, [f64; 3], [f64; 3])>,
    /// Tracker pose times (ns).
    pub vio_ts: Vec<i64>,
}

/// Read a log (unreadable lines are skipped: a take's slice can start or end inside a line).
pub fn parse(text: &str) -> HeadImu {
    let mut h = HeadImu::default();
    for l in text.lines() {
        let f: Vec<&str> = l.split(',').collect();
        let n: Option<Vec<f64>> = f
            .get(2..)
            .map(|v| v.iter().filter_map(|x| x.trim().parse().ok()).collect());
        let ts = f.get(1).and_then(|v| v.trim().parse::<i64>().ok());
        match (f[0], ts, n) {
            ("I", Some(t), Some(v)) if v.len() == 6 && f.len() == 8 => {
                h.imu.push((t, [v[0], v[1], v[2]], [v[3], v[4], v[5]]))
            }
            ("V", Some(t), Some(v)) if v.len() == 7 && f.len() == 9 => h.vio_ts.push(t),
            _ => {}
        }
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_imu_and_poses_then_pauses_until_the_flag_exists() {
        let dir = std::env::temp_dir().join(format!("pimax-imulog-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("head-imu.csv");
        let flag = dir.join("on");
        {
            // 1 s always, then only with the flag
            let mut l = HeadImuLog::start(&p, 1.0 / 60.0, Some(flag.clone())).unwrap();
            l.imu(1_000_000_000, [0.0, 9.8, 0.0], [0.01, 0.0, 0.0]);
            l.vio(1_010_000_000, [0.0; 3], [0.0, 0.0, 0.0, 1.0]);
            l.imu(3_000_000_000, [1.0, 9.8, 0.0], [0.0; 3]); // after the minutes, no flag: dropped
            std::thread::sleep(std::time::Duration::from_millis(50));
            std::fs::write(&flag, "").unwrap();
            l.imu(5_000_000_000, [2.0, 9.8, 0.0], [0.0; 3]); // flag checked again (> 1 s later): written
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        let h = parse(&std::fs::read_to_string(&p).unwrap());
        assert_eq!(h.vio_ts, vec![1_010_000_000]);
        let ts: Vec<i64> = h.imu.iter().map(|x| x.0).collect();
        assert_eq!(ts, vec![1_000_000_000, 5_000_000_000]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_start_keeps_the_last_three_logs() {
        let root = std::env::temp_dir().join(format!("pimax-imulog-rot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("session");
        let keep = root.join("state/omarchyvr/head-imu");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("head-imu.csv");
        assert_eq!(rotate(&p, &keep, KEEP), None, "nothing to keep");
        for i in 0..5 {
            std::fs::write(&p, format!("session {i}\n")).unwrap();
            // within the same second the copies get -1, -2, ...
            let kept = rotate(&p, &keep, KEEP).unwrap();
            assert!(!p.exists());
            assert_eq!(kept.parent(), Some(keep.as_path()), "kept on disk, not in the session dir");
            assert_eq!(std::fs::read_to_string(&kept).unwrap(), format!("session {i}\n"));
            let prev = dir.join("head-imu.csv.prev");
            assert_eq!(std::fs::read_to_string(&prev).unwrap(), format!("session {i}\n"), "prev = the newest");
        }
        let names = |d: &Path| {
            let mut v: Vec<String> = std::fs::read_dir(d).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
            v.sort();
            v
        };
        let stamped: Vec<String> = names(&keep).into_iter().filter(|n| n.starts_with("head-imu.2")).collect();
        assert_eq!(stamped.len(), 3, "{stamped:?}");
        assert_eq!(names(&dir), vec!["head-imu.csv.prev".to_string()], "nothing else left in the session dir");
        let texts: Vec<String> = stamped.iter().map(|n| std::fs::read_to_string(keep.join(n)).unwrap()).collect();
        assert!(texts.contains(&"session 4\n".to_string()) && !texts.contains(&"session 0\n".to_string()), "{texts:?}");
        // start() keeps it by itself (the move runs on a thread: the link is right at once, then points into keep)
        std::fs::write(&p, "session 5\n").unwrap();
        drop(HeadImuLog::start_keeping(&p, 1.0, None, Some(keep.clone())).unwrap());
        assert_eq!(std::fs::read_to_string(dir.join("head-imu.csv.prev")).unwrap(), "session 5\n");
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while names(&dir).iter().any(|n| n.ends_with(".keeping")) && std::time::Instant::now() < end {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(std::fs::read_to_string(dir.join("head-imu.csv.prev")).unwrap(), "session 5\n");
        assert!(std::fs::read_link(dir.join("head-imu.csv.prev")).unwrap().starts_with(&keep));
        assert_eq!(names(&keep).len(), 3);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_kept_logs_go_to_the_state_dir() {
        let env = |m: &'static [(&'static str, &'static str)]| move |k: &str| m.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string());
        assert_eq!(keep_dir(&env(&[("HOME", "/home/u")])), Some(PathBuf::from("/home/u/.local/state/omarchyvr/head-imu")));
        assert_eq!(keep_dir(&env(&[("HOME", "/home/u"), ("XDG_STATE_HOME", "/s")])), Some(PathBuf::from("/s/omarchyvr/head-imu")));
        assert_eq!(keep_dir(&env(&[])), None);
    }
}
