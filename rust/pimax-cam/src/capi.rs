//! C ABI (`include/pimax_cam.h`). All functions catch panics; errors are reported through
//! `pimax_cam_last_error()` (thread-local). A `pimax_cam` handle must be used from one thread at a time.

use crate::calib::{DeviceCalib, RigConvention};
use crate::camera::{Camera, FrameInfo};
use crate::clock::{self, ImuClockRef};
use crate::source::ReplayOptions;
use std::cell::RefCell;
use std::ffi::{c_char, c_int, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;

thread_local! {
    static LAST_ERROR: RefCell<CString> = RefCell::new(CString::default());
}

pub(crate) fn set_error(msg: impl Into<String>) {
    let s = msg.into().replace('\0', " ");
    LAST_ERROR.with(|e| *e.borrow_mut() = CString::new(s).unwrap_or_default());
}

pub(crate) fn guard<T>(fallback: T, f: impl FnOnce() -> Result<T, String>) -> T {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            set_error(e);
            fallback
        }
        Err(_) => {
            set_error("panic inside pimax_cam");
            fallback
        }
    }
}

pub(crate) unsafe fn opt_str(p: *const c_char) -> Option<String> {
    if p.is_null() {
        None
    } else {
        // SAFETY: caller passes a NUL-terminated string.
        Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
    }
}

/// Opaque handle.
pub struct PimaxCam {
    pub(crate) cam: Camera,
}

/// Open options (`pimax_cam_open_params`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct OpenParams {
    /// Requested frame rate (0 = driver default).
    pub fps: u32,
    /// Bit 0: loop a replay; bit 1: pace a replay in real time.
    pub replay_flags: u32,
    /// Check each frame's tile rows against the picture (diagnostics).
    pub verify_rows: u32,
    /// Reserved, zero.
    pub reserved: u32,
}

/// Library version string.
#[no_mangle]
pub extern "C" fn pimax_cam_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr() as *const c_char
}

/// Last error message of this thread (valid until the next failing call on the same thread).
#[no_mangle]
pub extern "C" fn pimax_cam_last_error() -> *const c_char {
    LAST_ERROR.with(|e| e.borrow().as_ptr())
}

/// Open a source: `NULL`/`""`/`"auto"` = find the camera, `"/dev/videoN"`, or a `.pxcam` recording.
///
/// # Safety
/// `source` is NULL or a valid C string; `params` is NULL or points to an `OpenParams`.
#[no_mangle]
pub unsafe extern "C" fn pimax_cam_open(
    source: *const c_char,
    params: *const OpenParams,
) -> *mut PimaxCam {
    guard(std::ptr::null_mut(), || {
        // SAFETY: per contract.
        let spec = unsafe { opt_str(source) }.unwrap_or_default();
        // SAFETY: per contract.
        let p = if params.is_null() {
            OpenParams::default()
        } else {
            unsafe { *params }
        };
        let ro = ReplayOptions {
            looping: p.replay_flags & 1 != 0,
            realtime: p.replay_flags & 2 != 0,
        };
        let mut cam = Camera::open(&spec, p.fps, ro).map_err(|e| format!("open {spec:?}: {e}"))?;
        cam.verify_rows = p.verify_rows != 0;
        Ok(Box::into_raw(Box::new(PimaxCam { cam })))
    })
}

/// Close and free.
///
/// # Safety
/// `cam` is NULL or a handle from `pimax_cam_open` not used afterwards.
#[no_mangle]
pub unsafe extern "C" fn pimax_cam_close(cam: *mut PimaxCam) {
    if cam.is_null() {
        return;
    }
    guard((), || {
        // SAFETY: per contract.
        let mut c = unsafe { Box::from_raw(cam) };
        let _ = c.cam.stop_recording();
        Ok(())
    })
}

/// Describe the source into `buf` (NUL terminated, truncated). Returns the full length.
///
/// # Safety
/// `cam` valid; `buf` NULL or `len` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn pimax_cam_describe(
    cam: *const PimaxCam,
    buf: *mut c_char,
    len: usize,
) -> usize {
    guard(0, || {
        // SAFETY: per contract.
        let c = unsafe { cam.as_ref() }.ok_or("null handle")?;
        let s = c.cam.describe();
        if !buf.is_null() && len > 0 {
            let n = s.len().min(len - 1);
            // SAFETY: buf has len bytes.
            unsafe {
                std::ptr::copy_nonoverlapping(s.as_ptr(), buf as *mut u8, n);
                *buf.add(n) = 0;
            }
        }
        Ok(s.len())
    })
}

/// Wait up to `timeout_ms` for the next frame. Returns 1 = frame (info filled), 0 = timeout, -1 = error or end
/// of a replay (see `pimax_cam_last_error`).
///
/// # Safety
/// `cam` valid; `info` NULL or writable.
#[no_mangle]
pub unsafe extern "C" fn pimax_cam_next(
    cam: *mut PimaxCam,
    timeout_ms: c_int,
    info: *mut FrameInfo,
) -> c_int {
    guard(-1, || {
        // SAFETY: per contract.
        let c = unsafe { cam.as_mut() }.ok_or("null handle")?;
        match c.cam.next_frame(timeout_ms).map_err(|e| e.to_string())? {
            Some(i) => {
                if !info.is_null() {
                    // SAFETY: per contract.
                    unsafe { *info = i };
                }
                Ok(1)
            }
            None if c.cam.source().finished() => Err("end of replay".into()),
            None => Ok(0),
        }
    })
}

/// Copy tile `tile` (0..3) of the current frame: 480 rows of 640 bytes into `dst` with row pitch `stride`.
/// Returns 0 or -1.
///
/// # Safety
/// `cam` valid; `dst` has at least `stride * 479 + 640` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn pimax_cam_copy_tile(
    cam: *const PimaxCam,
    tile: u32,
    dst: *mut u8,
    stride: u32,
) -> c_int {
    guard(-1, || {
        // SAFETY: per contract.
        let c = unsafe { cam.as_ref() }.ok_or("null handle")?;
        if dst.is_null() {
            return Err("null destination".into());
        }
        let len = stride as usize * (crate::frame::TILE_H - 1) + crate::frame::TILE_W;
        // SAFETY: per contract.
        let d = unsafe { std::slice::from_raw_parts_mut(dst, len) };
        c.cam
            .copy_tile(tile as usize, d, stride as usize)
            .map_err(|e| e.to_string())?;
        Ok(0)
    })
}

/// Counters (`pimax_cam_stats`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CStats {
    /// Frames delivered.
    pub frames: u64,
    /// Tracking-exposure frames.
    pub tracking: u64,
    /// LED-exposure frames.
    pub led: u64,
    /// Unknown frames.
    pub unknown: u64,
    /// Lost frames (counter gaps).
    pub dropped: u64,
    /// Corrupt buffers skipped by the driver layer.
    pub corrupt: u64,
    /// Frames whose rows were found by scanning.
    pub scanned_rows: u64,
    /// Frames whose metadata row offsets disagreed with the picture.
    pub row_mismatch: u64,
    /// Non-alternating frame kinds.
    pub kind_repeats: u64,
    /// Fallback clock offset (host - device), ns.
    pub clock_offset_ns: i64,
}

/// Read the counters. Returns 0 or -1.
///
/// # Safety
/// `cam` valid, `out` writable.
#[no_mangle]
pub unsafe extern "C" fn pimax_cam_get_stats(cam: *const PimaxCam, out: *mut CStats) -> c_int {
    guard(-1, || {
        // SAFETY: per contract.
        let c = unsafe { cam.as_ref() }.ok_or("null handle")?;
        let s = &c.cam.stats;
        let v = CStats {
            frames: s.frames,
            tracking: s.tracking,
            led: s.led,
            unknown: s.unknown,
            dropped: s.dropped,
            corrupt: c.cam.corrupt_frames(),
            scanned_rows: s.scanned_rows,
            row_mismatch: s.row_mismatch,
            kind_repeats: s.kind_repeats,
            clock_offset_ns: c.cam.clock_offset_ns(),
        };
        if out.is_null() {
            return Err("null output".into());
        }
        // SAFETY: per contract.
        unsafe { *out = v };
        Ok(0)
    })
}

/// Start recording every following raw frame to `path` (`zlib_level` 0 = uncompressed, 1..9 = zlib).
///
/// # Safety
/// `cam` valid, `path` a C string.
#[no_mangle]
pub unsafe extern "C" fn pimax_cam_record_start(
    cam: *mut PimaxCam,
    path: *const c_char,
    zlib_level: u32,
) -> c_int {
    guard(-1, || {
        // SAFETY: per contract.
        let c = unsafe { cam.as_mut() }.ok_or("null handle")?;
        // SAFETY: per contract.
        let p = unsafe { opt_str(path) }.ok_or("null path")?;
        let lvl = if zlib_level == 0 {
            None
        } else {
            Some(zlib_level.min(9) as u8)
        };
        c.cam
            .start_recording(&PathBuf::from(&p), lvl)
            .map_err(|e| format!("{p}: {e}"))?;
        Ok(0)
    })
}

/// Stop recording. Returns the number of frames written, or -1.
///
/// # Safety
/// `cam` valid.
#[no_mangle]
pub unsafe extern "C" fn pimax_cam_record_stop(cam: *mut PimaxCam) -> i64 {
    guard(-1, || {
        // SAFETY: per contract.
        let c = unsafe { cam.as_mut() }.ok_or("null handle")?;
        let r = c.cam.stop_recording().map_err(|e| e.to_string())?;
        Ok(r.map(|(f, _)| f as i64).unwrap_or(0))
    })
}

/// Fisheye intrinsics (`pimax_cam_kb4`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CKb4 {
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
    /// fx.
    pub fx: f64,
    /// fy.
    pub fy: f64,
    /// cx.
    pub cx: f64,
    /// cy.
    pub cy: f64,
    /// k1..k4.
    pub k: [f64; 4],
    /// Unused extra coefficients from the XML.
    pub extra: [f64; 2],
}

/// One camera (`pimax_cam_camera`).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct CCamera {
    /// Name, NUL terminated.
    pub name: [c_char; 32],
    /// Id.
    pub id: i32,
    /// Reserved.
    pub reserved: u32,
    /// Intrinsics.
    pub intr: CKb4,
    /// Rig rotation, row major (`X_i = R X_0 + t`).
    pub rig_rotation: [f64; 9],
    /// Rig translation (m).
    pub rig_translation: [f64; 3],
    /// Time alignment delta (s).
    pub time_delta_s: f64,
}

/// Whole calibration (`pimax_cam_calib`).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct CCalib {
    /// Number of valid cameras.
    pub camera_count: u32,
    /// 1 when `ombc`/`tbc` are present.
    pub has_imu: u32,
    /// Cameras.
    pub cameras: [CCamera; 4],
    /// Rotation vector camera 0 -> body.
    pub ombc: [f64; 3],
    /// Camera 0 origin in the body frame (m).
    pub tbc: [f64; 3],
    /// Camera-IMU time offset (s).
    pub imu_cam_delta_s: f64,
    /// Device serial, NUL terminated.
    pub device_uid: [c_char; 32],
}

fn copy_name(dst: &mut [c_char; 32], s: &str) {
    for (d, b) in dst.iter_mut().zip(s.bytes().take(31)) {
        *d = b as c_char;
    }
}

fn to_c(c: &DeviceCalib) -> CCalib {
    let empty = CCamera {
        name: [0; 32],
        id: -1,
        reserved: 0,
        intr: CKb4::default(),
        rig_rotation: [0.0; 9],
        rig_translation: [0.0; 3],
        time_delta_s: 0.0,
    };
    let mut out = CCalib {
        camera_count: c.cameras.len().min(4) as u32,
        has_imu: c.imu.is_some() as u32,
        cameras: [empty; 4],
        ombc: c.imu.as_ref().map(|m| m.ombc).unwrap_or_default(),
        tbc: c.imu.as_ref().map(|m| m.tbc).unwrap_or_default(),
        imu_cam_delta_s: c.imu.as_ref().map(|m| m.delta_s).unwrap_or_default(),
        device_uid: [0; 32],
    };
    copy_name(&mut out.device_uid, &c.uid);
    for (o, cam) in out.cameras.iter_mut().zip(c.cameras.iter()) {
        copy_name(&mut o.name, &cam.name);
        o.id = cam.id as i32;
        let k = &cam.intr;
        o.intr = CKb4 {
            width: k.width,
            height: k.height,
            fx: k.fx,
            fy: k.fy,
            cx: k.cx,
            cy: k.cy,
            k: k.k,
            extra: [
                k.extra.first().copied().unwrap_or(0.0),
                k.extra.get(1).copied().unwrap_or(0.0),
            ],
        };
        for r in 0..3 {
            for col in 0..3 {
                o.rig_rotation[r * 3 + col] = cam.rig_r[r][col];
            }
        }
        o.rig_translation = cam.rig_t;
        o.time_delta_s = cam.time_delta;
    }
    out
}

pub(crate) fn from_c(c: &CCalib) -> DeviceCalib {
    let n = (c.camera_count as usize).min(4);
    let cameras = c.cameras[..n]
        .iter()
        .map(|o| {
            let r = o.rig_rotation;
            crate::calib::CameraCalib {
                id: o.id.max(0) as u32,
                name: String::new(),
                model: String::new(),
                intr: crate::kb4::Kb4::new(
                    o.intr.width,
                    o.intr.height,
                    o.intr.fx,
                    o.intr.fy,
                    o.intr.cx,
                    o.intr.cy,
                    o.intr.k,
                    vec![],
                ),
                rig_r: [[r[0], r[1], r[2]], [r[3], r[4], r[5]], [r[6], r[7], r[8]]],
                rig_t: o.rig_translation,
                time_delta: o.time_delta_s,
            }
        })
        .collect();
    DeviceCalib {
        uid: String::new(),
        sensor: String::new(),
        date: String::new(),
        version: String::new(),
        cameras,
        imu: (c.has_imu != 0).then(|| crate::calib::ImuExtrinsic {
            ombc: c.ombc,
            tbc: c.tbc,
            delta_s: c.imu_cam_delta_s,
            raw: vec![],
        }),
    }
}

/// Load the factory calibration XML (`path` NULL = default search: `$PIMAX_CAM_CALIB`, `$PIMAX_CALIB`,
/// `~/.config/pimax/headset-calibration.xml`, `~/pimax-re/analysis/headset-calibration.xml`).
/// `used_path` (optional) receives the file used. Returns 0 or -1.
///
/// # Safety
/// `path` NULL or a C string; `out` writable; `used_path` NULL or `used_len` bytes.
#[no_mangle]
pub unsafe extern "C" fn pimax_cam_calib_load(
    path: *const c_char,
    out: *mut CCalib,
    used_path: *mut c_char,
    used_len: usize,
) -> c_int {
    guard(-1, || {
        // SAFETY: per contract.
        let p = unsafe { opt_str(path) }
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        let (cal, used) = DeviceCalib::load_default(p.as_deref()).map_err(|e| e.to_string())?;
        if out.is_null() {
            return Err("null output".into());
        }
        // SAFETY: per contract.
        unsafe { *out = to_c(&cal) };
        if !used_path.is_null() && used_len > 0 {
            let s = used.to_string_lossy();
            let n = s.len().min(used_len - 1);
            // SAFETY: per contract.
            unsafe {
                std::ptr::copy_nonoverlapping(s.as_ptr(), used_path as *mut u8, n);
                *used_path.add(n) = 0;
            }
        }
        Ok(0)
    })
}

/// Parse calibration XML from memory. Returns 0 or -1.
///
/// # Safety
/// `xml` points to `len` bytes; `out` writable.
#[no_mangle]
pub unsafe extern "C" fn pimax_cam_calib_parse(
    xml: *const c_char,
    len: usize,
    out: *mut CCalib,
) -> c_int {
    guard(-1, || {
        if xml.is_null() || out.is_null() {
            return Err("null argument".into());
        }
        // SAFETY: per contract.
        let bytes = unsafe { std::slice::from_raw_parts(xml as *const u8, len) };
        let cal = DeviceCalib::parse(&String::from_utf8_lossy(bytes)).map_err(|e| e.to_string())?;
        // SAFETY: per contract.
        unsafe { *out = to_c(&cal) };
        Ok(0)
    })
}

fn write_pose(p: &crate::geom::Pose, r: *mut f64, t: *mut f64) {
    // SAFETY: callers pass 9 and 3 writable doubles.
    unsafe {
        for i in 0..3 {
            for j in 0..3 {
                *r.add(i * 3 + j) = p.r[i][j];
            }
            *t.add(i) = p.t[i];
        }
    }
}

/// Stereo extrinsic between cameras `a` and `b`: `X_b = R X_a + t` (OpenCV camera axes; row-major `R`), the
/// meaning of `t_stereo_camera_calibration::camera_rotation/translation`. Returns 0 or -1.
///
/// # Safety
/// `cal` valid; `r` 9 and `t` 3 writable doubles.
#[no_mangle]
pub unsafe extern "C" fn pimax_cam_calib_stereo(
    cal: *const CCalib,
    a: u32,
    b: u32,
    r: *mut f64,
    t: *mut f64,
) -> c_int {
    guard(-1, || {
        // SAFETY: per contract.
        let c = unsafe { cal.as_ref() }.ok_or("null calib")?;
        if a >= c.camera_count || b >= c.camera_count || r.is_null() || t.is_null() {
            return Err("bad camera index or null output".into());
        }
        let d = from_c(c);
        write_pose(
            &d.stereo(a as usize, b as usize, RigConvention::CamFromCam0),
            r,
            t,
        );
        Ok(0)
    })
}

/// Pose of camera `i` in the HMD body frame: `X_body = R X_cam + t` with body = OpenXR head axes (x right, y up,
/// z back; the driver's axis mode 1) and camera = OpenCV axes. Returns 0, or -1 without `ombc`.
///
/// # Safety
/// `cal` valid; `r` 9 and `t` 3 writable doubles.
#[no_mangle]
pub unsafe extern "C" fn pimax_cam_calib_body_from_cam(
    cal: *const CCalib,
    i: u32,
    r: *mut f64,
    t: *mut f64,
) -> c_int {
    guard(-1, || {
        // SAFETY: per contract.
        let c = unsafe { cal.as_ref() }.ok_or("null calib")?;
        if i >= c.camera_count || r.is_null() || t.is_null() {
            return Err("bad camera index or null output".into());
        }
        let p = from_c(c)
            .body_from_cam(i as usize, RigConvention::CamFromCam0)
            .ok_or("calibration has no ombc/tbc")?;
        write_pose(&p, r, t);
        Ok(0)
    })
}

/// Map a device timestamp (µs, IMU clock) to `CLOCK_MONOTONIC` ns with the HMD's IMU clock reference.
/// Returns 0 when `ref` is NULL or invalid.
///
/// # Safety
/// `r` NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn pimax_cam_dev_to_mono_ns(dev_ts_us: u64, r: *const ImuClockRef) -> u64 {
    // SAFETY: per contract.
    match unsafe { r.as_ref() } {
        Some(r) if r.valid != 0 => clock::imu_dev_to_mono_ns(dev_ts_us, r),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
    fn calib_through_c_abi() {
        let xml = include_str!("../tests/fixtures/headset-calibration.xml");
        // SAFETY: test buffers.
        unsafe {
            let mut c = std::mem::zeroed::<CCalib>();
            assert_eq!(
                pimax_cam_calib_parse(xml.as_ptr() as *const c_char, xml.len(), &mut c),
                0
            );
            assert_eq!(c.camera_count, 4);
            assert_eq!(c.has_imu, 1);
            let name = CStr::from_ptr(c.cameras[1].name.as_ptr()).to_str().unwrap();
            assert_eq!(name, "trackingB");
            assert_eq!(
                CStr::from_ptr(c.device_uid.as_ptr()).to_str().unwrap(),
                "P511X0X000000000002"
            );
            assert_eq!(c.cameras[0].intr.extra, [0.0011599741, -0.0001122173]);
            let (mut r, mut t) = ([0.0; 9], [0.0; 3]);
            assert_eq!(
                pimax_cam_calib_stereo(&c, 0, 1, r.as_mut_ptr(), t.as_mut_ptr()),
                0
            );
            assert_eq!(r[0], -0.5777754392);
            assert_eq!(t, [-0.0996975113, 0.0070973858, -0.0497311326]);
            assert_eq!(
                pimax_cam_calib_body_from_cam(&c, 0, r.as_mut_ptr(), t.as_mut_ptr()),
                0
            );
            assert_eq!(t, [-0.006352548001, -0.01406156932, -0.01943735371]);
            assert_eq!(
                pimax_cam_calib_stereo(&c, 0, 7, r.as_mut_ptr(), t.as_mut_ptr()),
                -1
            );
            let msg = CStr::from_ptr(pimax_cam_last_error()).to_str().unwrap();
            assert!(msg.contains("bad camera index"), "{msg}");
            assert_eq!(
                pimax_cam_calib_parse(b"<x/>".as_ptr() as *const c_char, 4, &mut c),
                -1
            );
        }
    }

    #[test]
    fn open_failure_sets_error() {
        let p = CString::new("/nonexistent/clip.pxcam").unwrap();
        // SAFETY: valid strings, null params allowed.
        unsafe {
            assert!(pimax_cam_open(p.as_ptr(), std::ptr::null()).is_null());
            let msg = CStr::from_ptr(pimax_cam_last_error()).to_str().unwrap();
            assert!(msg.contains("open"), "{msg}");
            pimax_cam_close(std::ptr::null_mut());
            assert_eq!(
                pimax_cam_next(std::ptr::null_mut(), 0, std::ptr::null_mut()),
                -1
            );
        }
    }

    #[test]
    fn clock_through_c_abi() {
        let r = ImuClockRef {
            dev_ts_u32: 1000,
            valid: 1,
            dev_ext_us: 1000,
            offset_ns: 5,
        };
        // SAFETY: valid pointers.
        unsafe {
            assert_eq!(pimax_cam_dev_to_mono_ns(3000, &r), 3_000_005);
            assert_eq!(pimax_cam_dev_to_mono_ns(3000, std::ptr::null()), 0);
        }
    }
}
