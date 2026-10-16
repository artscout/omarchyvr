//! Live input: tracking frames from `pimax-cam broker` and the tracker pose of each frame from Monado's head ring
//! (`$XDG_RUNTIME_DIR/pimax-anchor.head`, written by the `desk` build when `PIMAX_ANCHOR=1`; raw SLAM head poses,
//! y up, before the offset). Tiles 0 and 1 are the lower tracking pair, the same cameras as `cam0`/`cam1` of a
//! recording. Read-only: nothing is sent to the headset, Monado or the broker.

use pimax_anchor::world::{self, HeadRing};
use pimax_cam::frame::{self, FrameKind, Meta, TILE_H, TILE_W};
use pimax_cam::geom::Pose;
use pimax_cam::source::{self, ReplayOptions};
use pimax_room::pngread::Grey;
use pimax_room::session::Alignment;
use std::time::Instant;

/// One live frame set with its tracker pose.
pub struct LiveFrame {
    /// Host CLOCK_MONOTONIC (ns).
    pub ts_ns: i64,
    /// Tiles of cameras 0 and 1.
    pub imgs: [Grey; 2],
    /// Raw SLAM pose of the frame (Monado head ring).
    pub slam_from_body: Pose,
    /// Monado pid that wrote the ring.
    pub pid: u32,
}

/// Counters of a capture.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LiveStats {
    /// Tracking frames read.
    pub frames: usize,
    /// Frames with a head pose.
    pub posed: usize,
    /// Pid of the ring (0 = no ring).
    pub pid: u32,
}

/// Read frames from `spec` (normally `broker`) for `seconds`; `f` gets every frame with a pose and returns false to
/// stop early.
///
/// A live broker source reconnects by itself (pimax_cam::resilient): a broker restart or a camera drop during the
/// capture is a gap in the frames, not an error; only a replay's end returns early.
pub fn capture(spec: &str, seconds: f64, mut f: impl FnMut(LiveFrame) -> bool) -> Result<LiveStats, String> {
    let mut src = source::open(spec, 0, ReplayOptions { looping: false, realtime: false })
        .map_err(|e| format!("cannot open source {spec:?}: {e}"))?;
    eprintln!("source {}", src.describe());
    let ring_path = world::ring_path();
    let t0 = Instant::now();
    let mut st = LiveStats::default();
    while t0.elapsed().as_secs_f64() < seconds {
        let fr = match src.next(500) {
            Ok(Some(fr)) => fr,
            Ok(None) => {
                if src.finished() {
                    break;
                }
                continue;
            }
            Err(e) => return Err(format!("source: {e}")),
        };
        let Some(m) = Meta::parse(&fr.data[..frame::META_LEN]) else { continue };
        if m.kind() != FrameKind::Tracking {
            continue;
        }
        st.frames += 1;
        let ts = fr.host_ts_ns;
        let rows = frame::tile_rows(Some(&m), fr.data, fr.stride);
        let mut imgs: [Grey; 2] = std::array::from_fn(|_| Grey { w: TILE_W, h: TILE_H, data: vec![0; TILE_W * TILE_H] });
        for (ci, g) in imgs.iter_mut().enumerate() {
            frame::copy_tile(fr.data, fr.stride, ci, rows.y[ci], &mut g.data, TILE_W);
        }
        let Some(ring) = HeadRing::read(&ring_path) else { continue };
        st.pid = ring.pid;
        let Some(pose) = ring.at(ts, 100_000_000) else { continue };
        st.posed += 1;
        if !f(LiveFrame { ts_ns: ts as i64, imgs, slam_from_body: pose, pid: ring.pid }) {
            break;
        }
    }
    Ok(st)
}

/// The offset Monado applies now (identity when there is none for `pid`), as a yaw + translation.
pub fn current_offset(pid: u32) -> Alignment {
    match world::read_offset(&world::offset_path()) {
        Some((p, o)) if p == pid && pid != 0 => {
            let yaw = pimax_room::map::yaw_of_mat(&o.r);
            Alignment { yaw, t: o.t }
        }
        _ => Alignment { yaw: 0.0, t: [0.0; 3] },
    }
}
