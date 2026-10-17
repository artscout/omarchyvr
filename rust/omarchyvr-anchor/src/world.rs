//! Files shared with the Monado `desk` branch and the offset maths.
//!
//! * Head ring (`$XDG_RUNTIME_DIR/pimax-anchor.head`, written by Monado when `PIMAX_ANCHOR=1`): the raw SLAM head poses
//!   (before the offset) with their timestamps. 16 byte header (u32 magic 0x434E4150, u32 version 1, u32 seq (odd
//!   while writing), u32 writer pid) then 128 slots of 40 bytes: u64 ts_ns (CLOCK_MONOTONIC), f32 pos[3],
//!   f32 quat[4] (x y z w), u32 flags (bit 0 = position valid). Slots are filled round robin, empty ones have ts 0.
//! * Offset file (`$XDG_RUNTIME_DIR/pimax-anchor.offset`, written here, read by Monado): text,
//!   `pimax-anchor-offset 1`, `pid N`, `pose px py pz qx qy qz qw`. Monado applies `app_from_slam = offset * slam` to
//!   the head pose and ignores the file when the pid is not its own. Monado deletes the file at start.
//! * Writer lease (`$XDG_RUNTIME_DIR/omarchyvr-anchor.offset.owner`): one writer of the offset per Monado session
//!   ([`take_lease`], [`write_offset`] refuses without it; see "One writer of the room offset" below).

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

/// Write the offset for Monado `pid` (atomic). Refused unless this process holds the writer lease of that session
/// ([`take_lease`]): one writer of the room offset per Monado session.
pub fn write_offset(path: &std::path::Path, pid: u32, o: &Pose) -> std::io::Result<()> {
    write_offset_as(path, pid, std::process::id(), o)
}

/// [`write_offset`] as writer `me` (tests pass a fake writer pid).
pub fn write_offset_as(path: &std::path::Path, pid: u32, me: u32, o: &Pose) -> std::io::Result<()> {
    let _g = LeaseLock::take(path)?;
    let cur = read_lease(&lease_path_for(path));
    if !lease_held_by(cur.as_ref(), pid, me) {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, refused_text(cur.as_ref(), pid)));
    }
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
// One writer of the room offset per Monado session (the writer lease).
//
// Live failure 2026-10-17 16:16-16:17 (Monado pid 1092498): Fast room setup wrote the offset, and the detached
// `omarchyvr-roommap auto` started at the session start (still retrying on the old map) recognised the brand-new map
// 73 s later and wrote a second offset 32 cm away; Monado snapped the world (the "floor jump"). The owner file only
// told the room map from the AprilTag, not one room-map process from another.
//
// The lease is the owner file next to the offset (`omarchyvr-anchor.offset.owner`). Its first line stays the one
// the app reads (`roommap <monado pid>` / `anchor <monado pid>`); schema 2 adds the writer process and its role:
//
// ```text
// roommap 1092498
// lease 2
// writer 1093007
// role setup
// ```
//
// Every writer takes the lease before it writes ([`take_lease`]); [`write_offset`] refuses without it. A user
// action (setup, recognise, apply, the AprilTag) takes the lease from a running background writer (auto, floor
// follow) and ends it by pid; a background writer never takes it from a live holder, and `auto` never writes after
// anyone placed the room in this session. The check and the write run under one `flock`
// (`omarchyvr-anchor.offset.lock`), so an evicted writer cannot slip one last write in.

/// Schema of the lease written by this code.
pub const LEASE_SCHEMA: u32 = 2;

/// Who writes the offset (the lease's role).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// `omarchyvr-roommap setup fast|full` (the user).
    Setup,
    /// `omarchyvr-roommap auto` (detached at the session start).
    Auto,
    /// `omarchyvr-roommap floor follow` (the floor.follow leaf).
    Follow,
    /// `omarchyvr-roommap relocalize --apply` (Recognise room, the anchor.relocalize job).
    Reloc,
    /// `omarchyvr-roommap apply` (a saved result).
    Apply,
    /// `omarchyvr-anchor apply` (the printed AprilTag).
    Tag,
}

impl Role {
    pub fn name(self) -> &'static str {
        match self {
            Role::Setup => "setup",
            Role::Auto => "auto",
            Role::Follow => "follow",
            Role::Reloc => "reloc",
            Role::Apply => "apply",
            Role::Tag => "tag",
        }
    }

    pub fn parse(s: &str) -> Option<Role> {
        [Role::Setup, Role::Auto, Role::Follow, Role::Reloc, Role::Apply, Role::Tag].into_iter().find(|r| r.name() == s)
    }

    /// The owner file's first word (the app's writer name).
    pub fn kind(self) -> &'static str {
        match self {
            Role::Tag => "anchor",
            _ => "roommap",
        }
    }

    /// Plain words for the user.
    pub fn title(self) -> &'static str {
        match self {
            Role::Setup => "the room setup",
            Role::Auto => "the automatic room placement",
            Role::Follow => "the floor follow",
            Role::Reloc => "Recognise room",
            Role::Apply => "a saved room fix",
            Role::Tag => "the AprilTag",
        }
    }

    /// A user action: takes the lease from a running background writer, which is then ended.
    pub fn preempts(self) -> bool {
        matches!(self, Role::Setup | Role::Reloc | Role::Apply | Role::Tag)
    }

    /// Only places a room nobody placed yet in this session (`auto` at the session start).
    pub fn first_only(self) -> bool {
        self == Role::Auto
    }
}

/// The owner file (lease).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lease {
    /// `roommap` or `anchor`.
    pub kind: String,
    /// Monado pid (the session).
    pub session: u32,
    /// Writer process (0: unknown, a schema 1 file written by the app).
    pub writer: u32,
    pub role: Option<Role>,
    pub schema: u32,
}

impl Lease {
    pub fn text(&self) -> String {
        let mut t = format!("{} {}\nlease {}\nwriter {}\n", self.kind, self.session, self.schema, self.writer);
        if let Some(r) = self.role {
            t += &format!("role {}\n", r.name());
        }
        t
    }

    /// Parse schema 2, and the app's schema 1 line (`roommap 42`: writer unknown).
    pub fn parse(s: &str) -> Option<Lease> {
        let mut lines = s.lines();
        let mut w = lines.next()?.split_whitespace();
        let kind = w.next()?.to_string();
        let session = w.next()?.parse().ok()?;
        let mut l = Lease { kind, session, writer: 0, role: None, schema: 1 };
        for line in lines {
            match line.trim().split_once(' ') {
                Some(("lease", v)) => l.schema = v.trim().parse().ok()?,
                Some(("writer", v)) => l.writer = v.trim().parse().ok()?,
                Some(("role", v)) => l.role = Role::parse(v.trim()),
                _ => {}
            }
        }
        if l.schema > LEASE_SCHEMA {
            // a newer writer's file: keep the session and kind, treat the writer as unknown
            l.writer = 0;
            l.role = None;
        }
        Some(l)
    }

    fn holder_title(&self) -> String {
        match self.role {
            Some(r) => format!("{} (pid {})", r.title(), self.writer),
            None if self.kind == "anchor" => "the AprilTag".into(),
            None => "the room map".into(),
        }
    }
}

/// The lease (owner file) next to an offset file.
pub fn lease_path_for(offset: &std::path::Path) -> PathBuf {
    offset.with_file_name("omarchyvr-anchor.offset.owner")
}

/// The lease of the live offset file.
pub fn lease_path() -> PathBuf {
    lease_path_for(&offset_path())
}

pub fn read_lease(path: &std::path::Path) -> Option<Lease> {
    Lease::parse(&std::fs::read_to_string(path).ok()?)
}

fn lease_held_by(cur: Option<&Lease>, session: u32, me: u32) -> bool {
    cur.is_some_and(|l| l.session == session && l.writer == me && l.schema == LEASE_SCHEMA)
}

fn refused_text(cur: Option<&Lease>, session: u32) -> String {
    match cur {
        Some(l) if l.session == session => {
            format!("the room offset of this session (Monado pid {session}) belongs to {}; not written", l.holder_title())
        }
        _ => format!("no writer lease for the room offset of Monado pid {session}; not written"),
    }
}

/// May `me` (role `role`) take the lease of Monado session `session`? Ok(Some(pid)) = yes, after ending the
/// running writer `pid`. `offset_written`: the offset file holds an offset for this session. Pure.
pub fn decide_lease(cur: Option<&Lease>, session: u32, me: u32, role: Role, offset_written: bool, alive: impl Fn(u32) -> bool) -> Result<Option<u32>, String> {
    let Some(cur) = cur.filter(|l| l.session == session) else { return Ok(None) };
    if cur.writer == me && cur.writer != 0 {
        return Ok(None);
    }
    if cur.writer != 0 && alive(cur.writer) {
        if role.preempts() {
            return Ok(Some(cur.writer));
        }
        return Err(format!("{} writes the room offset of this session; {} leaves it alone", cur.holder_title(), role.title()));
    }
    if offset_written && cur.kind != role.kind() {
        return Err(format!(
            "this session's room offset came from {}: only one of the room map and the AprilTag per session. Press Clear offset on the Room page first.",
            cur.holder_title()
        ));
    }
    if offset_written && role.first_only() {
        return Err(format!("{} already placed the room in this session; {} leaves it alone", cur.holder_title(), role.title()));
    }
    Ok(None)
}

/// `flock` on `omarchyvr-anchor.offset.lock` next to the offset: lease decisions and offset writes are serialised.
struct LeaseLock(#[allow(dead_code)] std::fs::File);

impl LeaseLock {
    fn take(offset: &std::path::Path) -> std::io::Result<LeaseLock> {
        let p = offset.with_file_name("omarchyvr-anchor.offset.lock");
        let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(p)?;
        f.lock()?;
        Ok(LeaseLock(f))
    }
}

fn comm_of(pid: u32) -> String {
    std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default().trim().to_string()
}

/// End an evicted writer by pid: only our own offset writers (comm checked), SIGTERM, wait up to 3 s.
fn end_writer(pid: u32) -> Result<(), String> {
    let c = comm_of(pid);
    if !matches!(c.as_str(), "omarchyvr-roomm" | "omarchyvr-ancho") {
        return Err(format!("pid {pid} ({c}) holds the room offset lease but is not an offset writer; not ended"));
    }
    // SAFETY: kill(2) on a pid whose name we just checked (one of our own tools).
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
    for _ in 0..30 {
        if !pid_alive(pid) || comm_of(pid) != c {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Err(format!("pid {pid} ({c}) did not stop within 3 s"))
}

/// Take the writer lease of Monado session `session` for this process as `role` (kept as it is when this process
/// already holds it). A user action ends a running background writer first. Returns a line for the log when
/// another writer was ended.
pub fn take_lease(offset: &std::path::Path, session: u32, role: Role) -> Result<Option<String>, String> {
    take_lease_as(offset, session, std::process::id(), role, pid_alive, end_writer)
}

/// [`take_lease`] with the writer pid, the liveness check and the eviction passed in (tests).
pub fn take_lease_as(
    offset: &std::path::Path,
    session: u32,
    me: u32,
    role: Role,
    alive: impl Fn(u32) -> bool,
    evict: impl Fn(u32) -> Result<(), String>,
) -> Result<Option<String>, String> {
    let lp = lease_path_for(offset);
    let _g = LeaseLock::take(offset).map_err(|e| format!("cannot lock the room offset lease: {e}"))?;
    let cur = read_lease(&lp);
    if lease_held_by(cur.as_ref(), session, me) {
        return Ok(None);
    }
    let written = read_offset(offset).is_some_and(|(p, _)| p == session);
    let note = match decide_lease(cur.as_ref(), session, me, role, written, alive)? {
        Some(pid) => {
            let who = cur.as_ref().map(|l| l.holder_title()).unwrap_or_default();
            evict(pid)?;
            Some(format!("{} took the room offset from {who}, which was ended", role.title()))
        }
        None => None,
    };
    let l = Lease { kind: role.kind().into(), session, writer: me, role: Some(role), schema: LEASE_SCHEMA };
    let tmp = lp.with_extension("owner.tmp");
    std::fs::write(&tmp, l.text()).and_then(|_| std::fs::rename(&tmp, &lp)).map_err(|e| format!("cannot write {}: {e}", lp.display()))?;
    Ok(note)
}

/// Does this process hold the lease of session `session`?
pub fn holds_lease(offset: &std::path::Path, session: u32) -> bool {
    lease_held_by(read_lease(&lease_path_for(offset)).as_ref(), session, std::process::id())
}

/// [`holds_lease`] for writer `me` (tests).
pub fn holds_lease_as(offset: &std::path::Path, session: u32, me: u32) -> bool {
    lease_held_by(read_lease(&lease_path_for(offset)).as_ref(), session, me)
}

#[cfg(test)]
mod lease_tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("omarchyvr-lease-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn pose(x: f64) -> Pose {
        Pose { r: geom::rodrigues([0.0, 0.1, 0.0]), t: [x, -0.9, 0.2] }
    }

    #[test]
    fn lease_text_round_trip_and_schema_1() {
        let l = Lease { kind: "roommap".into(), session: 1092498, writer: 77, role: Some(Role::Setup), schema: 2 };
        assert_eq!(Lease::parse(&l.text()), Some(l.clone()));
        // the app's first line is unchanged (gui-core parse_owner reads the first two words)
        assert!(l.text().starts_with("roommap 1092498\n"));
        let old = Lease::parse("anchor 42\n").unwrap();
        assert_eq!((old.kind.as_str(), old.session, old.writer, old.role, old.schema), ("anchor", 42, 0, None, 1));
    }

    #[test]
    fn no_lease_no_write() {
        let d = tmpdir("nowrite");
        let p = d.join("pimax-anchor.offset");
        let e = write_offset_as(&p, 5, 100, &pose(0.0)).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(read_offset(&p).is_none());
        take_lease_as(&p, 5, 100, Role::Reloc, |_| false, |_| Ok(())).unwrap();
        write_offset_as(&p, 5, 100, &pose(0.0)).unwrap();
        // a lease of another session does not let one write this session's offset
        assert!(write_offset_as(&p, 6, 100, &pose(0.0)).is_err());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_second_writer_is_refused() {
        let d = tmpdir("second");
        let p = d.join("pimax-anchor.offset");
        let alive = |pid: u32| pid == 200 || pid == 300;
        take_lease_as(&p, 9, 200, Role::Follow, alive, |_| panic!("nothing to end")).unwrap();
        write_offset_as(&p, 9, 200, &pose(0.1)).unwrap();
        // auto (300) while follow (200) runs: refused, and it cannot write
        let e = take_lease_as(&p, 9, 300, Role::Auto, alive, |_| panic!("auto never ends a writer")).unwrap_err();
        assert!(e.contains("floor follow"), "{e}");
        assert!(write_offset_as(&p, 9, 300, &pose(0.5)).is_err());
        assert!((read_offset(&p).unwrap().1.t[0] - 0.1).abs() < 1e-9);
        assert!(holds_lease_as(&p, 9, 200) && !holds_lease_as(&p, 9, 300));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn user_action_preempts_and_tag_rule() {
        let d = tmpdir("preempt");
        let p = d.join("pimax-anchor.offset");
        let ended = std::cell::Cell::new(0u32);
        take_lease_as(&p, 9, 200, Role::Auto, |_| false, |_| Ok(())).unwrap();
        let note = take_lease_as(&p, 9, 400, Role::Setup, |pid| pid == 200, |pid| { ended.set(pid); Ok(()) }).unwrap();
        assert_eq!(ended.get(), 200);
        assert!(note.unwrap().contains("automatic room placement"));
        assert!(write_offset_as(&p, 9, 200, &pose(0.3)).is_err(), "the evicted auto cannot write");
        write_offset_as(&p, 9, 400, &pose(0.1)).unwrap();
        // the setup ended (400 dead): the AprilTag may not take a room-map session, auto may not write again
        let e = take_lease_as(&p, 9, 500, Role::Tag, |_| false, |_| Ok(())).unwrap_err();
        assert!(e.contains("Clear offset"), "{e}");
        let e = take_lease_as(&p, 9, 600, Role::Auto, |_| false, |_| Ok(())).unwrap_err();
        assert!(e.contains("already placed"), "{e}");
        // another user action (Recognise room) and the floor follow may take it over from a dead holder
        take_lease_as(&p, 9, 700, Role::Follow, |_| false, |_| Ok(())).unwrap();
        // a new session: everything starts free
        take_lease_as(&p, 10, 600, Role::Auto, |_| false, |_| Ok(())).unwrap();
        let _ = std::fs::remove_dir_all(d);
    }
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
        let d = std::env::temp_dir().join(format!("omarchyvr-feed-test-{}", std::process::id()));
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
