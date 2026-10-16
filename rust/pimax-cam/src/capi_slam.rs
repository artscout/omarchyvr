//! C ABI of [`crate::slam`] (declared in `include/pimax_cam.h`, section "6DoF / SLAM").

use crate::capi::{from_c, guard, opt_str, CCalib};
use crate::headpose::{HeadParams, HeadTracker, ImuSample, Rel, VioSample};
use crate::slam::{self, ImuNoise, SlamSetup, SupervisorParams, TrackOutput};
use std::ffi::{c_char, c_int};
use std::path::Path;

/// `pimax_cam_slam_imu`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct CSlamImu {
    /// m/s²/√Hz.
    pub accel_noise_std: [f64; 3],
    /// rad/s/√Hz.
    pub gyro_noise_std: [f64; 3],
    /// m/s³/√Hz.
    pub accel_bias_std: [f64; 3],
    /// rad/s²/√Hz.
    pub gyro_bias_std: [f64; 3],
    /// Hz.
    pub rate_hz: f64,
    /// "defaults" or the file path (NUL terminated).
    pub source: [c_char; 256],
}

fn imu_to_c(n: &ImuNoise) -> CSlamImu {
    let mut c = CSlamImu {
        accel_noise_std: n.accel_noise_std,
        gyro_noise_std: n.gyro_noise_std,
        accel_bias_std: n.accel_bias_std,
        gyro_bias_std: n.gyro_bias_std,
        rate_hz: n.rate_hz,
        source: [0; 256],
    };
    for (d, b) in c.source.iter_mut().zip(n.source.bytes().take(255)) {
        *d = b as c_char;
    }
    c
}

fn imu_from_c(c: &CSlamImu) -> ImuNoise {
    let len = c
        .source
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(c.source.len());
    let bytes: Vec<u8> = c.source[..len].iter().map(|&b| b as u8).collect();
    ImuNoise {
        accel_noise_std: c.accel_noise_std,
        gyro_noise_std: c.gyro_noise_std,
        accel_bias_std: c.accel_bias_std,
        gyro_bias_std: c.gyro_bias_std,
        rate_hz: c.rate_hz,
        source: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

/// IMU noise model: `path` (NULL = `$PIMAX_SLAM_IMU` or `~/.config/pimax/slam-imu.json` if present). Returns 1 =
/// loaded from a file, 0 = defaults (no file), -1 = bad file (defaults written to `out`, error message set).
///
/// # Safety
/// `path` NULL or a C string; `out` writable.
#[no_mangle]
pub unsafe extern "C" fn pimax_cam_slam_imu_load(path: *const c_char, out: *mut CSlamImu) -> c_int {
    if out.is_null() {
        return -1;
    }
    // SAFETY: per contract.
    let p = unsafe { opt_str(path) };
    // SAFETY: out checked non-null, writable per contract.
    unsafe { *out = imu_to_c(&ImuNoise::default()) };
    guard(-1, || match ImuNoise::load(p.as_deref().map(Path::new))? {
        Some(n) => {
            // SAFETY: as above.
            unsafe { *out = imu_to_c(&n) };
            Ok(1)
        }
        None => Ok(0),
    })
}

/// `T_imu_cam` of factory camera `cam` as a column-major 4x4 (Monado `xrt_matrix_4x4` layout, doubles).
/// 0 or -1 (no extrinsic / bad id).
///
/// # Safety
/// `cal` valid; `out` 16 writable doubles.
#[no_mangle]
pub unsafe extern "C" fn pimax_cam_slam_t_imu_cam(
    cal: *const CCalib,
    cam: u32,
    out: *mut f64,
) -> c_int {
    guard(-1, || {
        // SAFETY: per contract.
        let c = unsafe { cal.as_ref() }.ok_or("null calib")?;
        if out.is_null() {
            return Err("null output".into());
        }
        let p = slam::t_imu_cam(&from_c(c), cam as usize)
            .ok_or("no camera-IMU extrinsic or bad camera id")?;
        // SAFETY: 16 writable doubles per contract.
        let o = unsafe { std::slice::from_raw_parts_mut(out, 16) };
        o.fill(0.0);
        for col in 0..3 {
            for row in 0..3 {
                o[col * 4 + row] = p.r[row][col];
            }
        }
        o[12] = p.t[0];
        o[13] = p.t[1];
        o[14] = p.t[2];
        o[15] = 1.0;
        Ok(0)
    })
}

/// Default head origin in the IMU frame (between the lower tracking cameras, 5 cm behind). 0 or -1.
///
/// # Safety
/// `cal` valid; `out` 3 writable doubles.
#[no_mangle]
pub unsafe extern "C" fn pimax_cam_slam_default_head(cal: *const CCalib, out: *mut f64) -> c_int {
    guard(-1, || {
        // SAFETY: per contract.
        let c = unsafe { cal.as_ref() }.ok_or("null calib")?;
        if out.is_null() {
            return Err("null output".into());
        }
        let h = slam::default_head_in_imu(&from_c(c));
        // SAFETY: 3 writable doubles.
        unsafe { std::ptr::copy_nonoverlapping(h.as_ptr(), out, 3) };
        Ok(0)
    })
}

/// Parse a camera list ("0,1", "0,1,2,3") into `out` (capacity `cap`). Returns the count or -1.
///
/// # Safety
/// `s` a C string; `out` `cap` writable u32.
#[no_mangle]
pub unsafe extern "C" fn pimax_cam_slam_parse_cams(
    s: *const c_char,
    count: u32,
    out: *mut u32,
    cap: u32,
) -> c_int {
    guard(-1, || {
        // SAFETY: per contract.
        let s = unsafe { opt_str(s) }.ok_or("null camera list")?;
        let v = slam::parse_cams(&s, count as usize)?;
        if out.is_null() || v.len() > cap as usize {
            return Err(format!("camera list {s:?}: more than {cap} cameras"));
        }
        for (k, &i) in v.iter().enumerate() {
            // SAFETY: k < cap.
            unsafe { *out.add(k) = i as u32 };
        }
        Ok(v.len() as c_int)
    })
}

/// Write `pimax-basalt-calib.json` + `pimax-slam-info.json` into `dir` for cameras `cams[0..n]` (tracker order).
/// `note` (may be NULL) is stored in the info file. 0 or -1.
///
/// # Safety
/// Pointers valid per their sizes; strings NUL terminated.
#[no_mangle]
pub unsafe extern "C" fn pimax_cam_slam_write_basalt(
    cal: *const CCalib,
    cams: *const u32,
    n: u32,
    cam_hz: f64,
    cam_time_offset_ns: i64,
    imu: *const CSlamImu,
    dir: *const c_char,
    note: *const c_char,
) -> c_int {
    guard(-1, || {
        // SAFETY: per contract.
        let c = unsafe { cal.as_ref() }.ok_or("null calib")?;
        // SAFETY: per contract.
        let imu = unsafe { imu.as_ref() }.map(imu_from_c).unwrap_or_default();
        // SAFETY: per contract.
        let dir = unsafe { opt_str(dir) }.ok_or("null dir")?;
        if cams.is_null() || n == 0 {
            return Err("no cameras".into());
        }
        // SAFETY: n readable u32.
        let ids = unsafe { std::slice::from_raw_parts(cams, n as usize) };
        let setup = SlamSetup {
            cams: ids.iter().map(|&i| i as usize).collect(),
            cam_hz,
            cam_time_offset_ns,
            // Same seed as `pimax-slam export` for live runs: the bias the driver cached from its last still window.
            gyro_bias: slam::gyro_bias_cache().unwrap_or([0.0; 3]),
        };
        // SAFETY: per contract.
        let note = unsafe { opt_str(note) }.unwrap_or_default();
        slam::write_basalt_files(Path::new(&dir), &from_c(c), &setup, &imu, &[("note", note)])?;
        Ok(0)
    })
}

/// `pimax_cam_track_sample`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct CTrackSample {
    /// Tracker pose time, monotonic ns.
    pub ts_ns: u64,
    /// IMU position in the tracker world (y up).
    pub pos: [f64; 3],
    /// IMU orientation in the tracker world, x y z w.
    pub q: [f64; 4],
    /// 3DoF IMU orientation at `ts_ns`, x y z w.
    pub q3dof: [f64; 4],
}

/// `pimax_cam_track_out`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct CTrackOut {
    /// 0 init, 1 tracking, 2 lost.
    pub state: u32,
    /// PIMAX_CAM_TRACK_EV_* bits.
    pub events: u32,
    /// world_from_slam translation.
    pub world_from_slam_pos: [f64; 3],
    /// world_from_slam rotation (yaw), x y z w.
    pub world_from_slam_q: [f64; 4],
    /// Yaw correction for the 3DoF orientation while not tracking, x y z w.
    pub fix3dof_q: [f64; 4],
    /// Head position while not tracking.
    pub held_pos: [f64; 3],
    /// Counters.
    pub acquisitions: u64,
    /// Counters.
    pub losses: u64,
    /// Counters.
    pub resets: u64,
    /// Counters.
    pub rejects: u64,
    /// Last state reason (NUL terminated).
    pub reason: [c_char; 48],
}

/// Opaque handle: supervisor + predictor + correction blending ([`HeadTracker`]).
pub struct CTrack {
    h: HeadTracker,
    /// Head IMU + tracker pose times for offline replays (`PIMAX_HEAD_IMU_LOG`), a leaf.
    log: Option<crate::imulog::HeadImuLog>,
    log_vio_ts: i64,
}

impl CTrack {
    fn log_vio(&mut self, s: &Option<VioSample>) {
        if let (Some(l), Some(s)) = (&mut self.log, s) {
            if s.ts_ns != self.log_vio_ts {
                self.log_vio_ts = s.ts_ns;
                l.vio(s.ts_ns, s.pos, s.q);
            }
        }
    }
}

/// `pimax_cam_track_vio`: newest tracker pose, y-up tracker world, IMU frame.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct CTrackVio {
    /// Pose time, monotonic ns.
    pub ts_ns: u64,
    /// IMU position.
    pub pos: [f64; 3],
    /// IMU orientation x y z w.
    pub q: [f64; 4],
    /// IMU velocity.
    pub vel: [f64; 3],
    /// 3DoF IMU orientation at `ts_ns`, x y z w.
    pub q3dof: [f64; 4],
}

/// `pimax_cam_rel`: a relation in the y-up world (flags = Monado `xrt_space_relation_flags` bits).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CRel {
    /// Position.
    pub pos: [f64; 3],
    /// Orientation x y z w.
    pub q: [f64; 4],
    /// Linear velocity.
    pub lin_vel: [f64; 3],
    /// Angular velocity (world).
    pub ang_vel: [f64; 3],
    /// Flags.
    pub flags: u32,
    /// Reserved, zero.
    pub reserved: u32,
}

/// `pimax_cam_track_info`: diagnostics.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CTrackInfo {
    /// Gyro bias learned from the tracker (rad/s, IMU frame).
    pub gyro_bias: [f64; 3],
    /// Current correction offset: position (m) and rotation vector (rad).
    pub corr_pos: [f64; 3],
    /// Rotation part.
    pub corr_rot: [f64; 3],
    /// Largest blended corrections so far (m, rad).
    pub max_corr_pos: f64,
    /// Rotation.
    pub max_corr_rot: f64,
    /// Corrections blended / applied at once.
    pub blended: u64,
    /// Snapped.
    pub snapped: u64,
    /// IMU samples buffered (newest timestamp ns).
    pub last_imu_ns: i64,
}

fn out_to_c(t: &CTrack, o: &TrackOutput) -> CTrackOut {
    let mut c = CTrackOut {
        state: o.state as u32,
        events: o.events,
        world_from_slam_pos: o.world_from_slam.0,
        world_from_slam_q: o.world_from_slam.1,
        fix3dof_q: o.fix3dof,
        held_pos: o.held_pos,
        acquisitions: t.h.sup.acquisitions,
        losses: t.h.sup.losses,
        resets: t.h.sup.resets,
        rejects: t.h.sup.rejects,
        reason: [0; 48],
    };
    for (d, b) in c.reason.iter_mut().zip(o.reason.bytes().take(47)) {
        *d = b as c_char;
    }
    c
}

/// Create a supervisor. `params` NULL = defaults; `head_in_imu` NULL = origin.
///
/// # Safety
/// Pointers NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn pimax_cam_track_create(
    params: *const SupervisorParams,
    head_in_imu: *const f64,
) -> *mut CTrack {
    guard(std::ptr::null_mut(), || {
        // SAFETY: per contract.
        let p = unsafe { params.as_ref() }.copied().unwrap_or_default();
        let mut h = [0.0; 3];
        if !head_in_imu.is_null() {
            // SAFETY: 3 readable doubles.
            unsafe { std::ptr::copy_nonoverlapping(head_in_imu, h.as_mut_ptr(), 3) };
        }
        Ok(Box::into_raw(Box::new(CTrack {
            h: HeadTracker::new(p, HeadParams::from_env(), h),
            log: crate::imulog::HeadImuLog::from_env(),
            log_vio_ts: i64::MIN,
        })))
    })
}

/// Default supervisor parameters.
///
/// # Safety
/// `out` writable.
#[no_mangle]
pub unsafe extern "C" fn pimax_cam_track_default_params(out: *mut SupervisorParams) {
    if !out.is_null() {
        // SAFETY: per contract.
        unsafe { *out = SupervisorParams::default() };
    }
}

/// Destroy (NULL ok).
///
/// # Safety
/// `t` from `pimax_cam_track_create`, not used afterwards.
#[no_mangle]
pub unsafe extern "C" fn pimax_cam_track_destroy(t: *mut CTrack) {
    if !t.is_null() {
        // SAFETY: created by Box::into_raw.
        drop(unsafe { Box::from_raw(t) });
    }
}

/// Recenter at the next update (head to the origin, heading unchanged).
///
/// # Safety
/// `t` valid.
#[no_mangle]
pub unsafe extern "C" fn pimax_cam_track_recenter(t: *mut CTrack) {
    // SAFETY: per contract.
    if let Some(t) = unsafe { t.as_mut() } {
        t.h.sup.request_recenter();
    }
}

/// One update (`sample` NULL = no tracker pose available). 0 or -1.
///
/// # Safety
/// `t` valid; `sample` NULL or valid; `out` writable.
#[no_mangle]
pub unsafe extern "C" fn pimax_cam_track_update(
    t: *mut CTrack,
    now_ns: u64,
    sample: *const CTrackSample,
    out: *mut CTrackOut,
) -> c_int {
    guard(-1, || {
        // SAFETY: per contract.
        let t = unsafe { t.as_mut() }.ok_or("null tracker")?;
        // SAFETY: per contract.
        // Phase-1 entry point: no velocity (the C side used Monado's own prediction with it).
        let s = unsafe { sample.as_ref() }.map(|s| VioSample {
            ts_ns: s.ts_ns as i64,
            pos: s.pos,
            q: s.q,
            vel: [0.0; 3],
            q3dof: s.q3dof,
        });
        t.log_vio(&s);
        let o = t.h.update(now_ns as i64, s);
        if !out.is_null() {
            // SAFETY: writable per contract.
            unsafe { *out = out_to_c(t, &o) };
        }
        Ok(0)
    })
}

/// Feed one IMU sample (the same samples, frame and clock the tracker gets: IMU frame, m/s², rad/s, raw).
///
/// # Safety
/// `t` valid; `accel`/`gyro` 3 readable doubles each.
#[no_mangle]
pub unsafe extern "C" fn pimax_cam_track_push_imu(
    t: *mut CTrack,
    ts_ns: i64,
    accel: *const f64,
    gyro: *const f64,
) {
    // SAFETY: per contract.
    let (Some(t), false, false) = (unsafe { t.as_mut() }, accel.is_null(), gyro.is_null()) else {
        return;
    };
    let mut a = [0.0; 3];
    let mut g = [0.0; 3];
    // SAFETY: 3 readable doubles each.
    unsafe {
        std::ptr::copy_nonoverlapping(accel, a.as_mut_ptr(), 3);
        std::ptr::copy_nonoverlapping(gyro, g.as_mut_ptr(), 3);
    }
    if let Some(l) = &mut t.log {
        l.imu(ts_ns, a, g);
    }
    t.h.push_imu(ImuSample {
        ts_ns,
        accel: a,
        gyro: g,
    });
}

/// Supervisor + predictor update with the newest tracker pose (with velocity; `sample` NULL = none). 0 or -1.
///
/// # Safety
/// `t` valid; `sample` NULL or valid; `out` NULL or writable.
#[no_mangle]
pub unsafe extern "C" fn pimax_cam_track_update_vio(
    t: *mut CTrack,
    now_ns: u64,
    sample: *const CTrackVio,
    out: *mut CTrackOut,
) -> c_int {
    guard(-1, || {
        // SAFETY: per contract.
        let t = unsafe { t.as_mut() }.ok_or("null tracker")?;
        // SAFETY: per contract.
        let s = unsafe { sample.as_ref() }.map(|s| VioSample {
            ts_ns: s.ts_ns as i64,
            pos: s.pos,
            q: s.q,
            vel: s.vel,
            q3dof: s.q3dof,
        });
        t.log_vio(&s);
        let o = t.h.update(now_ns as i64, s);
        if !out.is_null() {
            // SAFETY: writable per contract.
            unsafe { *out = out_to_c(t, &o) };
        }
        Ok(0)
    })
}

/// Head pose at `at_ns` (y-up world): tracker prediction + blended corrections while tracking, else the 3DoF
/// relation `rel3dof` (for the same time) with the heading fix at the held position. Returns the state
/// (PIMAX_CAM_TRACK_*) or -1.
///
/// # Safety
/// `t` valid; `rel3dof` valid; `out` writable.
#[no_mangle]
pub unsafe extern "C" fn pimax_cam_track_head(
    t: *mut CTrack,
    now_ns: u64,
    at_ns: i64,
    rel3dof: *const CRel,
    out: *mut CRel,
) -> c_int {
    guard(-1, || {
        // SAFETY: per contract.
        let t = unsafe { t.as_mut() }.ok_or("null tracker")?;
        // SAFETY: per contract.
        let r3 = unsafe { rel3dof.as_ref() }.ok_or("null 3DoF relation")?;
        if out.is_null() {
            return Err("null output".into());
        }
        let r = Rel {
            pos: r3.pos,
            q: r3.q,
            lin_vel: r3.lin_vel,
            ang_vel: r3.ang_vel,
            flags: r3.flags,
        };
        let o = t.h.head(now_ns as i64, at_ns, &r);
        // SAFETY: writable per contract.
        unsafe {
            *out = CRel {
                pos: o.pos,
                q: o.q,
                lin_vel: o.lin_vel,
                ang_vel: o.ang_vel,
                flags: o.flags,
                reserved: 0,
            }
        };
        Ok(t.h.output().state as c_int)
    })
}

/// Diagnostics (gyro bias, correction offset and counters). 0 or -1.
///
/// # Safety
/// `t` valid; `out` writable.
#[no_mangle]
pub unsafe extern "C" fn pimax_cam_track_get_info(t: *const CTrack, out: *mut CTrackInfo) -> c_int {
    guard(-1, || {
        // SAFETY: per contract.
        let t = unsafe { t.as_ref() }.ok_or("null tracker")?;
        if out.is_null() {
            return Err("null output".into());
        }
        let (cp, cr) = t.h.correction();
        // SAFETY: writable per contract.
        unsafe {
            *out = CTrackInfo {
                gyro_bias: t.h.pred.gyro_bias,
                corr_pos: cp,
                corr_rot: cr,
                max_corr_pos: t.h.corr.max_pos,
                max_corr_rot: t.h.corr.max_rot,
                blended: t.h.corr.blended,
                snapped: t.h.corr.snapped,
                last_imu_ns: t.h.pred.last_imu_ns().unwrap_or(0),
            }
        };
        Ok(0)
    })
}

/// `pimax_cam_track_corr`: one output correction step of at least `PIMAX_SLAM_BIG_CORR_MM` (default 20 mm).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CTrackCorr {
    /// Host time (ns).
    pub ts_ns: i64,
    /// The step (m, y-up world: [1] is vertical).
    pub step: [f64; 3],
    /// The whole pending offset after it (m).
    pub offset: [f64; 3],
    /// Rotation offset (rad).
    pub rot: f64,
    /// Head speed (m/s) and angular speed (rad/s) then.
    pub speed: f64,
    /// See `speed`.
    pub ang_speed: f64,
    /// 1 = only the tracker pose changed, 0 = a supervisor change (acquire, loss, re-zero, recenter, hold).
    pub pose_only: u32,
    /// Reserved, zero.
    pub reserved: u32,
}

/// Take the oldest queued big correction (the last 64 are kept). 1 = `out` filled, 0 = none, -1 = error.
///
/// # Safety
/// `t` from `pimax_cam_track_create`, `out` writable.
#[no_mangle]
pub unsafe extern "C" fn pimax_cam_track_take_big_corr(t: *mut CTrack, out: *mut CTrackCorr) -> c_int {
    guard(-1, || {
        // SAFETY: per contract.
        let t = unsafe { t.as_mut() }.ok_or("null tracker")?;
        if out.is_null() {
            return Err("null output".into());
        }
        let Some(e) = t.h.take_big() else {
            return Ok(0);
        };
        // SAFETY: writable per contract.
        unsafe {
            *out = CTrackCorr {
                ts_ns: e.ts_ns,
                step: e.step,
                offset: e.offset,
                rot: e.rot,
                speed: e.speed,
                ang_speed: e.ang_speed,
                pose_only: e.pose_only as u32,
                reserved: 0,
            }
        };
        Ok(1)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capi::pimax_cam_calib_load;
    use std::ffi::CString;

    fn load() -> CCalib {
        let p = CString::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../analysis/headset-calibration.xml"
        ))
        .unwrap();
        let mut c = std::mem::MaybeUninit::<CCalib>::zeroed();
        // SAFETY: valid pointers.
        let r =
            unsafe { pimax_cam_calib_load(p.as_ptr(), c.as_mut_ptr(), std::ptr::null_mut(), 0) };
        assert_eq!(r, 0);
        // SAFETY: filled by the call.
        unsafe { c.assume_init() }
    }

    #[test]
    #[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
    fn t_imu_cam_column_major() {
        let c = load();
        let mut m = [0.0; 16];
        // SAFETY: valid pointers.
        assert_eq!(
            unsafe { pimax_cam_slam_t_imu_cam(&c, 1, m.as_mut_ptr()) },
            0
        );
        let p = slam::t_imu_cam(&from_c(&c), 1).unwrap();
        assert_eq!(m[12..15], p.t[..]);
        assert_eq!(m[1], p.r[1][0]); // column 0, row 1
        assert_eq!(m[15], 1.0);
        // SAFETY: valid pointers.
        assert_eq!(
            unsafe { pimax_cam_slam_t_imu_cam(&c, 7, m.as_mut_ptr()) },
            -1
        );
    }

    #[test]
    #[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
    fn write_files_and_cams() {
        let c = load();
        let dir = std::env::temp_dir().join(format!("pimax-slam-capi-{}", std::process::id()));
        let d = CString::new(dir.to_str().unwrap()).unwrap();
        let mut imu = std::mem::MaybeUninit::<CSlamImu>::zeroed();
        let missing = CString::new("/nonexistent/slam-imu.json").unwrap();
        // SAFETY: valid pointers.
        assert_eq!(
            unsafe { pimax_cam_slam_imu_load(missing.as_ptr(), imu.as_mut_ptr()) },
            -1
        );
        // SAFETY: filled with defaults even on error.
        let imu = unsafe { imu.assume_init() };
        assert_eq!(imu.rate_hz, 1000.0);
        let mut ids = [0u32; 4];
        let s = CString::new("0,1,2,3").unwrap();
        // SAFETY: valid pointers.
        let n = unsafe { pimax_cam_slam_parse_cams(s.as_ptr(), 4, ids.as_mut_ptr(), 4) };
        assert_eq!(n, 4);
        // SAFETY: valid pointers.
        let r = unsafe {
            pimax_cam_slam_write_basalt(
                &c,
                ids.as_ptr(),
                2,
                25.0,
                0,
                &imu,
                d.as_ptr(),
                std::ptr::null(),
            )
        };
        assert_eq!(r, 0);
        let txt = std::fs::read_to_string(dir.join("pimax-basalt-calib.json")).unwrap();
        assert!(txt.contains("\"kb4\""));
        assert!(dir.join("pimax-slam-info.json").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn track_through_c() {
        // SAFETY: valid pointers throughout.
        unsafe {
            let mut p = std::mem::MaybeUninit::<SupervisorParams>::zeroed();
            pimax_cam_track_default_params(p.as_mut_ptr());
            let mut p = p.assume_init();
            p.acquire_poses = 2;
            let t = pimax_cam_track_create(&p, std::ptr::null());
            assert!(!t.is_null());
            let mut out = std::mem::MaybeUninit::<CTrackOut>::zeroed();
            for i in 0..3u64 {
                let s = CTrackSample {
                    ts_ns: 1_000_000_000 + i * 40_000_000,
                    pos: [1.0, 2.0, 3.0],
                    q: [0.0, 0.0, 0.0, 1.0],
                    q3dof: [0.0, 0.0, 0.0, 1.0],
                };
                assert_eq!(pimax_cam_track_update(t, s.ts_ns, &s, out.as_mut_ptr()), 0);
            }
            let o = out.assume_init();
            assert_eq!(o.state, 1);
            assert_eq!(o.world_from_slam_pos, [-1.0, -2.0, -3.0]);
            pimax_cam_track_destroy(t);

            // New path: IMU + VIO with velocity + head query.
            let t = pimax_cam_track_create(&p, std::ptr::null());
            let g = [0.0, 0.0, 0.0];
            let a = [0.0, 9.80665, 0.0];
            for i in 0..200i64 {
                pimax_cam_track_push_imu(t, 1_000_000_000 + i * 1_000_000, a.as_ptr(), g.as_ptr());
            }
            for i in 0..3u64 {
                let s = CTrackVio {
                    ts_ns: 1_000_000_000 + i * 40_000_000,
                    pos: [1.0, 2.0, 3.0],
                    q: [0.0, 0.0, 0.0, 1.0],
                    vel: [0.0; 3],
                    q3dof: [0.0, 0.0, 0.0, 1.0],
                };
                assert_eq!(
                    pimax_cam_track_update_vio(t, s.ts_ns, &s, out.as_mut_ptr()),
                    0
                );
            }
            let r3 = CRel {
                q: [0.0, 0.0, 0.0, 1.0],
                flags: 17,
                ..Default::default()
            };
            let mut r = CRel::default();
            assert_eq!(
                pimax_cam_track_head(t, 1_150_000_000, 1_170_000_000, &r3, &mut r),
                1
            );
            assert_eq!(r.flags, 63);
            assert!(r.pos.iter().all(|x| x.abs() < 1e-6), "{:?}", r.pos); // head at the origin at acquisition
            let mut info = CTrackInfo::default();
            assert_eq!(pimax_cam_track_get_info(t, &mut info), 0);
            assert_eq!(info.last_imu_ns, 1_199_000_000);
            pimax_cam_track_destroy(t);
        }
    }
}
