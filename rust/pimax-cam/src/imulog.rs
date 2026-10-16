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

    /// Log to `path`: always for `minutes`, then while `flag` exists.
    pub fn start(path: &Path, minutes: f64, flag: Option<PathBuf>) -> Option<HeadImuLog> {
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
}
