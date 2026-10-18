//! The Monado builds on this machine (LESSONS rows 6, 7, 12, 16): does the service binary exist, is `PIMAX_CAM=ON` in
//! its `CMakeCache.txt`, does `getcap` show `cap_sys_nice=ep`, which branch is the source tree on, does the binary
//! contain the 6DoF code and the Basalt IMU-gate fix (Monado `slam` d3138d4, without which the first auto-exposure
//! push deadlocked the tracker).
use crate::caps;
use crate::cmake::CMakeCache;
use crate::machine::{FileMeta, Machine};
use crate::paths::BuildDir;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Strings searched in the service binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Markers {
    /// `PIMAX_SLAM` (the launcher's own `--6dof` test).
    pub slam: bool,
    /// `held from the tracker` (the IMU gate of d3138d4: auto exposure is safe).
    pub imu_gate: bool,
    /// `PIMAX_EYE` (eye gaze code).
    pub eye: bool,
    /// `PIMAX_CTRL_TRACK` (optical controller poses).
    pub ctrl_track: bool,
    /// `PIMAX_ANCHOR` (the room offset hook of the AprilTag and the room map).
    pub anchor: bool,
    /// `origin_gate_mm` (the eye calibration parser reads version 2 files: a refit may be saved).
    pub eye_v2: bool,
    /// `PIMAX_HT_HZ` (camera hand tracking, Mercury: the hands build).
    pub hands: bool,
    /// `PIMAX_CTRL_INHAND` (controllers shown only while in the hand, docs/CONTROLLERS-INHAND.md).
    pub ctrl_inhand: bool,
    /// `PIMAX_CTRL_ANGLE` (the controller angle setting, docs/CONTROLLERS-INHAND.md).
    pub ctrl_angle: bool,
    /// `PIMAX_ANCHOR_GLIDE` (room corrections glide instead of snapping, docs/ROOM-SETUP.md).
    pub anchor_glide: bool,
    /// `PIMAX_CAM_AE_LAW` (the new camera exposure law and its kill switch; status only, docs/TRACKING-STABILITY.md).
    pub ae_law: bool,
    /// `PIMAX_CTRL_TRACK_HYST` (the steadier controller fuser and its kill switch; status only).
    pub ctrl_hyst: bool,
    /// `PIMAX_SLAM_DRIFT_HOLD` (the head drift governor and its kill switch; status only).
    pub drift_hold: bool,
}

impl Markers {
    /// The features every build the app starts must carry (docs: one build that carries everything), in plain words,
    /// that this binary lacks. `hands`: the start uses camera hands, so their code is required too.
    pub fn missing(&self, hands: bool) -> Vec<&'static str> {
        let mut v = Vec::new();
        for (have, what) in [
            (self.slam, "6DoF head tracking (PIMAX_SLAM)"),
            (self.imu_gate, "the IMU gate that keeps 6DoF safe with auto exposure"),
            (self.eye, "eye tracking (PIMAX_EYE)"),
            (self.ctrl_track, "optical controller tracking (PIMAX_CTRL_TRACK)"),
            (self.anchor, "the room anchor (PIMAX_ANCHOR)"),
            (self.ctrl_inhand, "controllers only when picked up (PIMAX_CTRL_INHAND)"),
            (self.ctrl_angle, "the controller angle setting (PIMAX_CTRL_ANGLE)"),
            (self.anchor_glide, "smooth room corrections (PIMAX_ANCHOR_GLIDE)"),
            (self.hands || !hands, "camera hand tracking (PIMAX_HT_HZ)"),
        ] {
            if !have {
                v.push(what);
            }
        }
        v
    }
}

const NEEDLES: [&[u8]; 13] = [
    b"PIMAX_SLAM\0",
    b"held from the tracker",
    b"PIMAX_EYE\0",
    b"PIMAX_CTRL_TRACK",
    b"PIMAX_ANCHOR",
    b"origin_gate_mm",
    b"PIMAX_HT_HZ\0",
    b"PIMAX_CTRL_INHAND",
    b"PIMAX_CTRL_ANGLE",
    b"PIMAX_ANCHOR_GLIDE",
    b"PIMAX_CAM_AE_LAW",
    b"PIMAX_CTRL_TRACK_HYST",
    b"PIMAX_SLAM_DRIFT_HOLD",
];

/// Scan a binary in chunks (it is ~150 MB) for the marker strings.
pub fn scan_markers(m: &dyn Machine, path: &Path) -> Option<Markers> {
    const CH: usize = 8 << 20;
    let overlap = NEEDLES.iter().map(|n| n.len()).max().unwrap_or(1) - 1;
    let mut found = [false; NEEDLES.len()];
    let mut off = 0u64;
    let mut tail: Vec<u8> = Vec::new();
    loop {
        let b = m.read_range(path, off, CH)?;
        if b.is_empty() {
            break;
        }
        off += b.len() as u64;
        let mut hay = std::mem::take(&mut tail);
        hay.extend_from_slice(&b);
        for (i, n) in NEEDLES.iter().enumerate() {
            if !found[i] && memchr::memmem::find(&hay, n).is_some() {
                found[i] = true;
            }
        }
        if found.iter().all(|f| *f) || b.len() < CH {
            break;
        }
        tail = hay[hay.len().saturating_sub(overlap)..].to_vec();
    }
    Some(Markers {
        slam: found[0],
        imu_gate: found[1],
        eye: found[2],
        ctrl_track: found[3],
        anchor: found[4],
        eye_v2: found[5],
        hands: found[6],
        ctrl_inhand: found[7],
        ctrl_angle: found[8],
        anchor_glide: found[9],
        ae_law: found[10],
        ctrl_hyst: found[11],
        drift_hold: found[12],
    })
}

/// Marker scans keyed by (path, inode, size, mtime): a rebuilt binary is scanned again.
#[derive(Default)]
pub struct MarkerCache {
    map: Mutex<HashMap<PathBuf, (FileMeta, Markers)>>,
}

impl MarkerCache {
    /// Cached scan.
    pub fn get(&self, m: &dyn Machine, path: &Path) -> Option<Markers> {
        let meta = m.meta(path)?;
        if let Ok(map) = self.map.lock() {
            if let Some((k, v)) = map.get(path) {
                if *k == meta {
                    return Some(*v);
                }
            }
        }
        let v = scan_markers(m, path)?;
        if let Ok(mut map) = self.map.lock() {
            map.insert(path.to_path_buf(), (meta, v));
        }
        Some(v)
    }
}

/// Everything known about one build.
#[derive(Debug, Clone, PartialEq)]
pub struct BuildInfo {
    /// Which build.
    pub build: BuildDir,
    /// The service binary.
    pub service: PathBuf,
    /// Service binary metadata (None = not built).
    pub service_meta: Option<FileMeta>,
    /// Parsed cache (None = no CMakeCache.txt).
    pub cmake: Option<CMakeCache>,
    /// Branch of the source tree.
    pub branch: Option<String>,
    /// `getcap` verdict (None = could not run getcap or no binary).
    pub sys_nice: Option<bool>,
    /// Binary markers.
    pub markers: Option<Markers>,
}

impl BuildInfo {
    /// The service binary exists.
    pub fn built(&self) -> bool {
        self.service_meta.is_some()
    }

    /// `PIMAX_CAM` from the cache.
    pub fn pimax_cam(&self) -> Option<bool> {
        self.cmake.as_ref().and_then(CMakeCache::pimax_cam)
    }

    /// Source directory.
    pub fn source_dir(&self) -> Option<PathBuf> {
        self.cmake
            .as_ref()
            .and_then(|c| c.source_dir().map(PathBuf::from))
            .or_else(|| self.build.dir.parent().map(Path::to_path_buf))
    }
}

/// Branch of a git work tree (`.git/HEAD`, or a worktree's `gitdir:` file).
pub fn git_branch(m: &dyn Machine, src: &Path) -> Option<String> {
    let dotgit = src.join(".git");
    let head = match m.read_str(&dotgit.join("HEAD")) {
        Some(h) => h,
        None => {
            let link = m.read_str(&dotgit)?;
            let gitdir = link.strip_prefix("gitdir:")?.trim();
            let gd = if Path::new(gitdir).is_absolute() {
                PathBuf::from(gitdir)
            } else {
                src.join(gitdir)
            };
            m.read_str(&gd.join("HEAD"))?
        }
    };
    match head.strip_prefix("ref: refs/heads/") {
        Some(b) => Some(b.trim().to_string()),
        None => Some(format!("detached {}", head.chars().take(8).collect::<String>())),
    }
}

/// Inspect one build. `scan` = also look for the markers (slow on the first call, cached after).
pub fn inspect(m: &dyn Machine, b: &BuildDir, cache: Option<&MarkerCache>) -> BuildInfo {
    let service = b.service();
    let service_meta = m.meta(&service).filter(|x| !x.is_dir);
    let cmake = m.read(&b.dir.join("CMakeCache.txt")).map(|t| CMakeCache::parse(&String::from_utf8_lossy(&t)));
    let mut info = BuildInfo {
        build: b.clone(),
        service: service.clone(),
        service_meta,
        cmake,
        branch: None,
        sys_nice: None,
        markers: None,
    };
    if let Some(src) = info.source_dir() {
        info.branch = git_branch(m, &src);
    }
    if info.built() {
        let s = service.display().to_string();
        info.sys_nice = m.run("getcap", &[&s]).map(|o| caps::has_sys_nice(&o.stdout));
        if let Some(c) = cache {
            info.markers = c.get(m, &service);
        }
    }
    info
}

/// Every known build.
pub fn inspect_all(m: &dyn Machine, builds: &[BuildDir], cache: Option<&MarkerCache>) -> Vec<BuildInfo> {
    builds.iter().map(|b| inspect(m, b, cache)).collect()
}

/// The info of the build whose directory is `dir`, or a fresh inspection of an unknown directory.
pub fn for_dir(m: &dyn Machine, all: &[BuildInfo], dir: &Path, cache: Option<&MarkerCache>) -> BuildInfo {
    all.iter().find(|b| b.build.dir == dir).cloned().unwrap_or_else(|| {
        inspect(
            m,
            &BuildDir {
                key: "custom",
                label: "Monado (custom build directory)",
                dir: dir.to_path_buf(),
            },
            cache,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::FakeMachine;

    fn desk() -> BuildDir {
        BuildDir {
            key: "desk",
            label: "desk",
            dir: "/home/u/.local/lib/omarchyvr/monado".into(),
        }
    }

    fn machine(getcap: &str, cache: &str) -> FakeMachine {
        let mut m = FakeMachine::new();
        let svc = "/home/u/.local/lib/omarchyvr/monado/src/xrt/targets/service/monado-service";
        let mut bin = vec![0u8; 3000];
        bin.extend_from_slice(b"PIMAX_SLAM\0...IMU samples (%lu held from the tracker in %lu camera gaps)\0PIMAX_EYE\0");
        m.file(svc, bin);
        m.cmd(&format!("getcap {svc}"), 0, getcap);
        m.file("/home/u/.local/lib/omarchyvr/monado/CMakeCache.txt", cache);
        m.file("/home/u/src/monado-desk/.git/HEAD", "ref: refs/heads/desk\n");
        m
    }

    #[test]
    fn desk_build_with_everything() {
        let svc = "/home/u/.local/lib/omarchyvr/monado/src/xrt/targets/service/monado-service";
        let m = machine(&format!("{svc} cap_sys_nice=ep\n"), include_str!("../tests/fixtures/CMakeCache-desk.txt"));
        let c = MarkerCache::default();
        let i = inspect(&m, &desk(), Some(&c));
        assert!(i.built());
        assert_eq!(i.pimax_cam(), Some(true));
        assert_eq!(i.sys_nice, Some(true));
        assert_eq!(i.branch.as_deref(), Some("desk"));
        assert_eq!(
            i.markers,
            Some(Markers { slam: true, imu_gate: true, eye: true, ctrl_track: false, anchor: false, eye_v2: false, hands: false, ctrl_inhand: false, ..Markers::default() })
        );
        // cached: same answer without rescanning
        assert_eq!(c.get(&m, Path::new(svc)), i.markers);
    }

    #[test]
    fn relinked_build_lost_its_capability_and_plain_build_has_no_camera() {
        let m = machine("", include_str!("../tests/fixtures/CMakeCache-pimax.txt"));
        let i = inspect(&m, &desk(), None);
        assert_eq!(i.sys_nice, Some(false), "getcap prints nothing after a relink");
        assert_eq!(i.pimax_cam(), Some(false));
        assert_eq!(i.markers, None, "no scan without a cache");
        let missing = inspect(&FakeMachine::new(), &desk(), None);
        assert!(!missing.built() && missing.sys_nice.is_none() && missing.cmake.is_none());
    }

    #[test]
    fn markers_found_across_chunk_borders() {
        let mut m = FakeMachine::new();
        let mut big = vec![b'x'; (8 << 20) - 5];
        big.extend_from_slice(b"PIMAX_SLAM\0");
        m.file("/b", big);
        let k = scan_markers(&m, Path::new("/b")).unwrap();
        assert!(k.slam && !k.imu_gate);
    }

    #[test]
    fn worktree_branch() {
        let mut m = FakeMachine::new();
        m.file("/src/monado-slam/.git", "gitdir: /src/monado/.git/worktrees/monado-slam\n");
        m.file("/src/monado/.git/worktrees/monado-slam/HEAD", "ref: refs/heads/slam\n");
        assert_eq!(git_branch(&m, Path::new("/src/monado-slam")).as_deref(), Some("slam"));
        m.file("/src/x/.git/HEAD", "4ec0348aa\n");
        assert_eq!(git_branch(&m, Path::new("/src/x")).as_deref(), Some("detached 4ec0348a"));
    }
}
