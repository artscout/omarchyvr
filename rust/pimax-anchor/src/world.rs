//! Files shared with the Monado `desk` branch and the offset maths.
//!
//! * Head ring (`$XDG_RUNTIME_DIR/pimax-anchor.head`, written by Monado when `PIMAX_ANCHOR=1`): the raw SLAM head poses
//!   (before the offset) with their timestamps. 16 byte header (u32 magic 0x434E4150, u32 version 1, u32 seq (odd
//!   while writing), u32 writer pid) then 128 slots of 40 bytes: u64 ts_ns (CLOCK_MONOTONIC), f32 pos[3],
//!   f32 quat[4] (x y z w), u32 flags (bit 0 = position valid). Slots are filled round robin, empty ones have ts 0.
//! * Offset file (`$XDG_RUNTIME_DIR/pimax-anchor.offset`, written here, read by Monado): text,
//!   `pimax-anchor-offset 1`, `pid N`, `pose px py pz qx qy qz qw`. Monado applies `app_from_slam = offset * slam` to
//!   the head pose and ignores the file when the pid is not its own. Monado deletes the file at start.

use pimax_cam::geom::{self, Pose};
use pimax_ctrltrack::solve::quat_to_mat;
use std::path::PathBuf;

pub const RING_MAGIC: u32 = 0x434E_4150;
pub const RING_SLOTS: usize = 128;
pub const RING_HDR: usize = 16;
pub const RING_SLOT: usize = 40;
pub const RING_SIZE: usize = RING_HDR + RING_SLOTS * RING_SLOT;

fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

pub fn ring_path() -> PathBuf {
    match std::env::var_os("PIMAX_ANCHOR_HEAD_FILE").filter(|v| !v.is_empty()) {
        Some(p) => PathBuf::from(p),
        None => runtime_dir().join("pimax-anchor.head"),
    }
}

pub fn offset_path() -> PathBuf {
    match std::env::var_os("PIMAX_ANCHOR_FILE").filter(|v| !v.is_empty()) {
        Some(p) => PathBuf::from(p),
        None => runtime_dir().join("pimax-anchor.offset"),
    }
}

pub fn pq(p: &Pose) -> ([f64; 3], [f64; 4]) {
    (p.t, geom::quat_from_mat(&p.r))
}

pub fn from_pq(t: [f64; 3], q: [f64; 4]) -> Pose {
    Pose { r: quat_to_mat(q), t }
}

fn slerp(a: [f64; 4], mut b: [f64; 4], f: f64) -> [f64; 4] {
    let mut d: f64 = (0..4).map(|i| a[i] * b[i]).sum();
    if d < 0.0 {
        d = -d;
        for x in &mut b {
            *x = -*x;
        }
    }
    let (wa, wb) = if d > 0.9995 {
        (1.0 - f, f)
    } else {
        let th = d.acos();
        (((1.0 - f) * th).sin() / th.sin(), (f * th).sin() / th.sin())
    };
    let mut q = [0.0; 4];
    for i in 0..4 {
        q[i] = wa * a[i] + wb * b[i];
    }
    let n = q.iter().map(|x| x * x).sum::<f64>().sqrt();
    q.map(|x| x / n)
}

/// Interpolate between two poses.
pub fn interp(a: &Pose, b: &Pose, f: f64) -> Pose {
    let t = [0, 1, 2].map(|i| a.t[i] * (1.0 - f) + b.t[i] * f);
    from_pq(t, slerp(geom::quat_from_mat(&a.r), geom::quat_from_mat(&b.r), f))
}

#[derive(Clone, Debug)]
pub struct HeadRing {
    pub pid: u32,
    /// (ts_ns, world_from_head, position valid), sorted by time.
    pub samples: Vec<(u64, Pose, bool)>,
}

impl HeadRing {
    pub fn parse(b: &[u8]) -> Option<HeadRing> {
        if b.len() < RING_SIZE {
            return None;
        }
        let u32at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        if u32at(0) != RING_MAGIC || u32at(4) != 1 {
            return None;
        }
        let f32at = |o: usize| f32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as f64;
        let mut samples = Vec::new();
        for i in 0..RING_SLOTS {
            let o = RING_HDR + i * RING_SLOT;
            let ts = u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
            if ts == 0 {
                continue;
            }
            let t = [f32at(o + 8), f32at(o + 12), f32at(o + 16)];
            let q = [f32at(o + 20), f32at(o + 24), f32at(o + 28), f32at(o + 32)];
            if q.iter().any(|x| !x.is_finite()) {
                continue;
            }
            samples.push((ts, from_pq(t, q), u32at(o + 36) & 1 != 0));
        }
        samples.sort_by_key(|s| s.0);
        Some(HeadRing {
            pid: u32at(12),
            samples,
        })
    }

    /// Seqlock read of the file.
    pub fn read(path: &std::path::Path) -> Option<HeadRing> {
        for _ in 0..8 {
            let b = std::fs::read(path).ok()?;
            if b.len() < RING_SIZE {
                return None;
            }
            let seq = u32::from_le_bytes(b[8..12].try_into().unwrap());
            if seq & 1 == 1 {
                std::thread::sleep(std::time::Duration::from_millis(1));
                continue;
            }
            let b2 = std::fs::read(path).ok()?;
            if b2.len() >= 12 && b2[8..12] == b[8..12] {
                return HeadRing::parse(&b);
            }
        }
        None
    }

    /// Latest sample time.
    pub fn newest_ns(&self) -> u64 {
        self.samples.last().map(|s| s.0).unwrap_or(0)
    }

    /// Raw head pose at `ts_ns` (needs a valid-position sample on each side within `max_gap_ns`, else the nearest
    /// within `max_gap_ns / 2`).
    pub fn at(&self, ts_ns: u64, max_gap_ns: u64) -> Option<Pose> {
        let v: Vec<&(u64, Pose, bool)> = self.samples.iter().filter(|s| s.2).collect();
        let i = v.partition_point(|s| s.0 <= ts_ns);
        if i > 0 && i < v.len() {
            let (a, b) = (v[i - 1], v[i]);
            if b.0 - a.0 <= max_gap_ns {
                let f = (ts_ns - a.0) as f64 / (b.0 - a.0).max(1) as f64;
                return Some(interp(&a.1, &b.1, f));
            }
        }
        v.iter()
            .min_by_key(|s| s.0.abs_diff(ts_ns))
            .filter(|s| s.0.abs_diff(ts_ns) <= max_gap_ns / 2)
            .map(|s| s.1)
    }
}

/// Test helper / reference of the writer layout (Monado writes the same bytes).
pub fn ring_bytes(pid: u32, samples: &[(u64, Pose, bool)]) -> Vec<u8> {
    let mut b = vec![0u8; RING_SIZE];
    b[0..4].copy_from_slice(&RING_MAGIC.to_le_bytes());
    b[4..8].copy_from_slice(&1u32.to_le_bytes());
    b[12..16].copy_from_slice(&pid.to_le_bytes());
    for (i, (ts, p, valid)) in samples.iter().take(RING_SLOTS).enumerate() {
        let o = RING_HDR + i * RING_SLOT;
        b[o..o + 8].copy_from_slice(&ts.to_le_bytes());
        let (t, q) = pq(p);
        for k in 0..3 {
            b[o + 8 + 4 * k..o + 12 + 4 * k].copy_from_slice(&(t[k] as f32).to_le_bytes());
        }
        for k in 0..4 {
            b[o + 20 + 4 * k..o + 24 + 4 * k].copy_from_slice(&(q[k] as f32).to_le_bytes());
        }
        b[o + 36..o + 40].copy_from_slice(&(*valid as u32).to_le_bytes());
    }
    b
}

/// `offset` such that `offset * world_from_tag_now == reference` (the tag lands where it was at calibration).
/// With `yaw_only` only a rotation about the world up axis (+Y) and a translation are used: the SLAM world is
/// gravity aligned, so roll/pitch differences are measurement noise, not a real offset.
pub fn compute_offset(reference: &Pose, world_from_tag_now: &Pose, yaw_only: bool) -> Pose {
    let r_full = geom::mat_mul(&reference.r, &geom::transpose(&world_from_tag_now.r));
    let r = if yaw_only {
        let psi = (r_full[0][2] - r_full[2][0]).atan2(r_full[0][0] + r_full[2][2]);
        geom::rodrigues([0.0, psi, 0.0])
    } else {
        r_full
    };
    let t = geom::sub(reference.t, geom::mat_vec(&r, world_from_tag_now.t));
    Pose { r, t }
}

pub fn offset_text(pid: u32, o: &Pose) -> String {
    let (t, q) = pq(o);
    format!(
        "pimax-anchor-offset 1\npid {pid}\npose {:.6} {:.6} {:.6} {:.7} {:.7} {:.7} {:.7}\n",
        t[0], t[1], t[2], q[0], q[1], q[2], q[3]
    )
}

pub fn parse_offset(s: &str) -> Option<(u32, Pose)> {
    let mut l = s.lines();
    if l.next()?.trim() != "pimax-anchor-offset 1" {
        return None;
    }
    let pid: u32 = l.next()?.strip_prefix("pid ")?.trim().parse().ok()?;
    let v: Vec<f64> = l
        .next()?
        .strip_prefix("pose ")?
        .split_whitespace()
        .map(|x| x.parse().ok())
        .collect::<Option<_>>()?;
    if v.len() != 7 {
        return None;
    }
    Some((pid, from_pq([v[0], v[1], v[2]], [v[3], v[4], v[5], v[6]])))
}

pub fn write_offset(path: &std::path::Path, pid: u32, o: &Pose) -> std::io::Result<()> {
    let tmp = path.with_extension("offset.tmp");
    std::fs::write(&tmp, offset_text(pid, o))?;
    std::fs::rename(tmp, path)
}

pub fn read_offset(path: &std::path::Path) -> Option<(u32, Pose)> {
    parse_offset(&std::fs::read_to_string(path).ok()?)
}

/// Robust mean of poses: drops samples further than `pos_tol` m / `rot_tol` deg from the median-ish centre.
pub fn robust_mean(v: &[Pose], pos_tol: f64, rot_tol_deg: f64) -> Option<(Pose, usize, f64, f64)> {
    if v.is_empty() {
        return None;
    }
    let med = |k: usize| {
        let mut x: Vec<f64> = v.iter().map(|p| p.t[k]).collect();
        x.sort_by(|a, b| a.partial_cmp(b).unwrap());
        x[x.len() / 2]
    };
    let c = [med(0), med(1), med(2)];
    let keep: Vec<&Pose> = v
        .iter()
        .filter(|p| geom::norm(geom::sub(p.t, c)) <= pos_tol)
        .collect();
    if keep.is_empty() {
        return None;
    }
    let mut mean_t = [0.0; 3];
    let mut qs = [0.0; 4];
    let q0 = geom::quat_from_mat(&keep[0].r);
    for p in &keep {
        for i in 0..3 {
            mean_t[i] += p.t[i] / keep.len() as f64;
        }
        let mut q = geom::quat_from_mat(&p.r);
        if (0..4).map(|i| q[i] * q0[i]).sum::<f64>() < 0.0 {
            q = q.map(|x| -x);
        }
        for i in 0..4 {
            qs[i] += q[i];
        }
    }
    let n = qs.iter().map(|x| x * x).sum::<f64>().sqrt();
    let mean = from_pq(mean_t, qs.map(|x| x / n));
    let _ = rot_tol_deg;
    let sd_t = (keep
        .iter()
        .map(|p| geom::norm(geom::sub(p.t, mean_t)).powi(2))
        .sum::<f64>()
        / keep.len() as f64)
        .sqrt();
    let sd_r = (keep
        .iter()
        .map(|p| crate::detect::pose_error(&mean, p).1.powi(2))
        .sum::<f64>()
        / keep.len() as f64)
        .sqrt();
    Some((mean, keep.len(), sd_t, sd_r))
}

// ---------------------------------------------------------------------------------------------------------------
// Is the head ring the live feed of the running Monado?
//
// Live failure 2026-10-16: the room setup's in-headset panel went to the system OpenXR runtime (SteamVR), SteamVR's
// Monado driver opened the Pimax headset inside vrserver, its anchor hook recreated this file with vrserver's pid and
// then exited; the real monado-service kept writing into the replaced (unlinked) file, so the ring stood still with a
// dead pid and the setup saw "no head movement" and "0 with a Monado head pose". A reader must check who wrote the
// ring and how fresh it is, and say so in plain words.

/// A ring whose newest sample is older than this (s) is stale.
pub const RING_STALE_S: f64 = 5.0;

/// CLOCK_MONOTONIC now (ns), the clock of the ring's timestamps.
pub use pimax_ctrltrack::shm::monotonic_ns;

/// Pids of running processes whose name (`/proc/*/comm`) is `name`.
pub fn pids_named(name: &str) -> Vec<u32> {
    let mut v: Vec<u32> = std::fs::read_dir("/proc")
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()))
                .filter(|pid| std::fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|c| c.trim() == name))
                .collect()
        })
        .unwrap_or_default();
    v.sort_unstable();
    v
}

/// The processes that may own the head ring now: the running `monado-service` (Work/Play), else SteamVR's
/// `vrserver` (its Monado driver writes the ring when SteamVR runs without monado-service, docs/ROOM-SETUP.md).
pub fn tracker_pids() -> Vec<u32> {
    let m = pids_named("monado-service");
    if m.is_empty() {
        pids_named("vrserver")
    } else {
        m
    }
}

fn pid_alive(pid: u32) -> bool {
    pid != 0 && std::path::Path::new(&format!("/proc/{pid}")).exists()
}

/// State of the head-pose feed.
#[derive(Clone, Debug, PartialEq)]
pub enum Feed {
    /// No ring file (or not a ring).
    Missing,
    /// No monado-service runs; the ring is left over from `pid`.
    NoMonado { pid: u32 },
    /// The ring was written by `pid`, which is not the running Monado (`monado`): another program opened the
    /// headset with the anchor hook (SteamVR's Monado driver) and replaced the file. `alive`: that writer still runs.
    Foreign { pid: u32, monado: u32, alive: bool },
    /// The running Monado's ring, but its newest sample is `age_s` old (or it has none).
    Stale { pid: u32, age_s: Option<f64> },
    /// The running Monado's ring with fresh samples.
    Live { pid: u32 },
}

impl Feed {
    /// The Monado pid when the feed is live.
    pub fn live_pid(&self) -> Option<u32> {
        match self {
            Feed::Live { pid } => Some(*pid),
            _ => None,
        }
    }

    /// One sentence for the user.
    pub fn plain(&self) -> String {
        match self {
            Feed::Missing => "Monado's head-pose feed is missing (Monado does not run with 6DoF and the room anchor; start Work from the app)".into(),
            Feed::NoMonado { pid } => format!("Monado is not running (the head-pose feed is left over from pid {pid}); start Work from the app"),
            Feed::Foreign { pid, monado, alive } => format!(
                "the head-pose feed was taken over by another program (pid {pid}{}, e.g. SteamVR's driver opening the headset) and is not the running Monado's (pid {monado}); restart Work from the app to get it back",
                if *alive { ", still running" } else { ", already closed" }
            ),
            Feed::Stale { pid, age_s: Some(a) } => format!("Monado's head-pose feed (pid {pid}) stopped {a:.0} s ago (6DoF lost or the headset asleep?)"),
            Feed::Stale { pid, age_s: None } => format!("Monado's head-pose feed (pid {pid}) has no poses yet (6DoF not tracking?)"),
            Feed::Live { pid } => format!("Monado's head-pose feed is live (pid {pid})"),
        }
    }
}

/// Classify `ring` against the running tracker pids ([`tracker_pids`]: Monado, else SteamVR's vrserver) at `now_ns` (CLOCK_MONOTONIC). Pure: tests feed it anything.
pub fn check_feed(ring: Option<&HeadRing>, monado: &[u32], now_ns: u64, alive: impl Fn(u32) -> bool) -> Feed {
    let Some(r) = ring else { return Feed::Missing };
    if !monado.contains(&r.pid) {
        return match monado.first() {
            None => Feed::NoMonado { pid: r.pid },
            Some(&m) => Feed::Foreign { pid: r.pid, monado: m, alive: alive(r.pid) },
        };
    }
    let newest = r.newest_ns();
    if newest == 0 {
        return Feed::Stale { pid: r.pid, age_s: None };
    }
    let age = now_ns.saturating_sub(newest) as f64 / 1e9;
    if age > RING_STALE_S {
        return Feed::Stale { pid: r.pid, age_s: Some(age) };
    }
    Feed::Live { pid: r.pid }
}

/// The feed now: the ring file, the running monado-service processes and the clock.
pub fn feed_now() -> Feed {
    check_feed(HeadRing::read(&ring_path()).as_ref(), &tracker_pids(), monotonic_ns(), pid_alive)
}

#[cfg(test)]
mod feed_tests {
    use super::*;

    fn ring(pid: u32, newest: u64) -> HeadRing {
        let p = Pose { r: geom::rodrigues([0.0, 0.0, 0.0]), t: [0.0, 1.6, 0.0] };
        let s: Vec<(u64, Pose, bool)> = if newest == 0 { vec![] } else { vec![(newest - 3_000_000, p, true), (newest, p, true)] };
        HeadRing::parse(&ring_bytes(pid, &s)).unwrap()
    }

    #[test]
    fn live_ring_of_the_running_monado() {
        let r = ring(43262, 100_000_000_000);
        assert_eq!(check_feed(Some(&r), &[43262], 101_000_000_000, |_| true), Feed::Live { pid: 43262 });
    }

    #[test]
    fn foreign_writer_is_named() {
        // the 2026-10-16 case: SteamVR's driver (pid 182552) recreated the ring and closed; Monado 43262 runs on
        let r = ring(182552, 100_000_000_000);
        let f = check_feed(Some(&r), &[43262], 101_000_000_000, |_| false);
        assert_eq!(f, Feed::Foreign { pid: 182552, monado: 43262, alive: false });
        assert_eq!(f.live_pid(), None);
        let t = f.plain();
        assert!(t.contains("182552") && t.contains("43262") && t.contains("another program"), "{t}");
    }

    #[test]
    fn stale_and_missing_and_no_monado() {
        let r = ring(43262, 100_000_000_000);
        let f = check_feed(Some(&r), &[43262], 172_000_000_000, |_| true);
        assert!(matches!(f, Feed::Stale { pid: 43262, age_s: Some(a) } if (a - 72.0).abs() < 0.1), "{f:?}");
        assert!(f.plain().contains("stopped 72 s ago"));
        assert_eq!(check_feed(Some(&ring(43262, 0)), &[43262], 1, |_| true), Feed::Stale { pid: 43262, age_s: None });
        assert_eq!(check_feed(None, &[43262], 1, |_| true), Feed::Missing);
        assert_eq!(check_feed(Some(&r), &[], 1, |_| true), Feed::NoMonado { pid: 43262 });
    }

    #[test]
    fn stale_foreign_file_on_disk() {
        let d = std::env::temp_dir().join(format!("pimax-feed-test-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("pimax-anchor.head");
        let now = monotonic_ns();
        let pose = Pose { r: geom::rodrigues([0.0, 0.3, 0.0]), t: [0.1, 1.5, -0.2] };
        std::fs::write(&p, ring_bytes(999_999_999, &[(now - 60_000_000_000, pose, true)])).unwrap();
        let r = HeadRing::read(&p).unwrap();
        // no monado-service with that pid can exist: foreign (or no Monado at all on a test machine)
        let f = check_feed(Some(&r), &tracker_pids(), now, pid_alive);
        assert!(matches!(f, Feed::Foreign { pid: 999_999_999, alive: false, .. } | Feed::NoMonado { pid: 999_999_999 }), "{f:?}");
        // the same file written by "the running Monado" is stale
        let f = check_feed(Some(&r), &[999_999_999], now, |_| true);
        assert!(matches!(f, Feed::Stale { age_s: Some(a), .. } if a > 59.0), "{f:?}");
        let _ = std::fs::remove_dir_all(&d);
    }
}
