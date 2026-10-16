//! C ABI of the controller pose fusion (`include/pimax_ctrlfuse.h`), see [`crate::fuse`]. Every function tolerates
//! NULL. One object per hand; all calls are serialised by an internal mutex (reports come from the session thread,
//! queries from the compositor).

#![allow(
    non_camel_case_types,
    clippy::missing_safety_doc,
    clippy::manual_is_multiple_of
)]

use crate::ffi::pimax_hands_pose;
use crate::fuse::{calib_or_default, calib_path, Config, Fuser, State, P};
use std::os::raw::{c_char, c_int};
use std::sync::{Arc, Mutex};

pub struct PimaxCtrlFuse {
    f: Arc<Mutex<Fuser>>,
    /// The other hand (body model: separation); set by [`pimax_ctrlfuse_link`]. Never locked while `f` is held.
    other: Mutex<Option<Arc<Mutex<Fuser>>>>,
    path: std::path::PathBuf,
}

impl PimaxCtrlFuse {
    fn other(&self) -> Option<Arc<Mutex<Fuser>>> {
        self.other.lock().ok().and_then(|o| o.clone())
    }
}

/// Extra information about the last [`pimax_ctrlfuse_get`].
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct pimax_ctrlfuse_info {
    /// 0 none (rest pose), 1 optical (fresh, extrapolated), 2 out of view (head-relative hold).
    pub state: u32,
    /// 1 when the orientation comes from the gyro between fixes.
    pub gyro: u32,
}

fn to_p(p: &pimax_hands_pose) -> P {
    let o = p.orientation;
    P {
        q: crate::fuse::qnorm([o[0] as f64, o[1] as f64, o[2] as f64, o[3] as f64]),
        p: [
            p.position[0] as f64,
            p.position[1] as f64,
            p.position[2] as f64,
        ],
    }
}

fn from_p(p: &P) -> pimax_hands_pose {
    pimax_hands_pose {
        orientation: [p.q[0] as f32, p.q[1] as f32, p.q[2] as f32, p.q[3] as f32],
        position: [p.p[0] as f32, p.p[1] as f32, p.p[2] as f32],
    }
}

/// New fuser for `hand` (0 left, 1 right): config from the environment, calibration from
/// `~/.config/omarchyvr/ctrl-imu.json` (or PIMAX_CTRL_IMU_CALIB), else the built-in one (gyro on by default;
/// PIMAX_CTRL_TRACK_IMU=0 turns it off), log to `$PIMAX_CTRL_IMU_LOG-<hand>.csv` when set.
#[no_mangle]
pub extern "C" fn pimax_ctrlfuse_create(hand: c_int) -> *mut PimaxCtrlFuse {
    let hand = if hand == 0 { 0 } else { 1 };
    let path = calib_path();
    let mut f = Fuser::new(hand, Config::from_env(), calib_or_default(&path, hand));
    if let Some(prefix) = std::env::var_os("PIMAX_CTRL_IMU_LOG").filter(|v| !v.is_empty()) {
        let mut p = prefix.into_string().unwrap_or_default();
        p += if hand == 0 { "-left.csv" } else { "-right.csv" };
        let minutes = std::env::var("PIMAX_CTRL_IMU_LOG_MIN")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10.0);
        let _ = f.start_log(std::path::Path::new(&p), minutes);
    }
    Box::into_raw(Box::new(PimaxCtrlFuse {
        f: Arc::new(Mutex::new(f)),
        other: Mutex::new(None),
        path,
    }))
}

#[no_mangle]
pub unsafe extern "C" fn pimax_ctrlfuse_destroy(h: *mut PimaxCtrlFuse) {
    if h.is_null() {
        return;
    }
    let b = Box::from_raw(h);
    let path = b.path.clone();
    // the other hand may still hold a reference to this fuser: work through the lock
    let mut f = match b.f.lock() {
        Ok(f) => f,
        Err(p) => p.into_inner(),
    };
    let _ = f.maybe_save(u64::MAX, &path, 0.0);
    f.flush_log();
}

/// One raw 64-byte controller report (EP 0x82) received at `host_ns` (CLOCK_MONOTONIC).
#[no_mangle]
pub unsafe extern "C" fn pimax_ctrlfuse_report(
    h: *mut PimaxCtrlFuse,
    host_ns: u64,
    raw: *const u8,
) {
    if h.is_null() || raw.is_null() {
        return;
    }
    let raw = &*(raw as *const [u8; 64]);
    if let Ok(mut f) = (*h).f.lock() {
        f.report(host_ns, raw);
        if f.stats.imu_samples % 2000 == 0 {
            let p = (*h).path.clone();
            let _ = f.maybe_save(host_ns, &p, 30.0);
            f.flush_log();
        }
    }
}

/// An optical fix: `world_from_aim` and the head pose, both at the LED frame time `frame_ns`.
#[no_mangle]
pub unsafe extern "C" fn pimax_ctrlfuse_optical(
    h: *mut PimaxCtrlFuse,
    frame_ns: u64,
    world_from_aim: *const pimax_hands_pose,
    world_from_head: *const pimax_hands_pose,
    rms_px: f32,
    leds: u32,
) {
    if h.is_null() || world_from_aim.is_null() || world_from_head.is_null() {
        return;
    }
    // the other hand's fix of (about) the same LED frame, for the separation rule
    let other = (*h)
        .other()
        .and_then(|o| o.lock().ok().and_then(|o| o.fresh_fix(frame_ns, 0.1)));
    if let Ok(mut f) = (*h).f.lock() {
        f.optical_with(
            frame_ns,
            to_p(&*world_from_aim),
            &to_p(&*world_from_head),
            rms_px,
            leds,
            other,
        );
    }
}

/// Let two fusers (left, right) see each other for the body model (separation). Call once after creating both.
#[no_mangle]
pub unsafe extern "C" fn pimax_ctrlfuse_link(a: *mut PimaxCtrlFuse, b: *mut PimaxCtrlFuse) {
    if a.is_null() || b.is_null() || a == b {
        return;
    }
    if let Ok(mut o) = (*a).other.lock() {
        *o = Some((*b).f.clone());
    }
    if let Ok(mut o) = (*b).other.lock() {
        *o = Some((*a).f.clone());
    }
}

/// Aim pose at `at_ns` given the head pose at `at_ns`. Returns the state (0 = none: use the rest pose).
#[no_mangle]
pub unsafe extern "C" fn pimax_ctrlfuse_get(
    h: *mut PimaxCtrlFuse,
    at_ns: u64,
    world_from_head: *const pimax_hands_pose,
    out: *mut pimax_hands_pose,
    info: *mut pimax_ctrlfuse_info,
) -> c_int {
    if h.is_null() || world_from_head.is_null() || out.is_null() {
        return 0;
    }
    let head = to_p(&*world_from_head);
    let other = (*h)
        .other()
        .and_then(|o| o.lock().ok().and_then(|mut o| o.peek(at_ns, &head)));
    let Ok(mut f) = (*h).f.lock() else { return 0 };
    let (s, p) = f.get_with(at_ns, &head, other);
    let gyro = f.gyro_active();
    drop(f);
    *out = from_p(&p);
    if !info.is_null() {
        *info = pimax_ctrlfuse_info {
            state: s as u32,
            gyro: (gyro && s != State::None) as u32,
        };
    }
    s as c_int
}

/// One-line status (calibration, counters) into `buf`. Returns the length written (without the NUL).
#[no_mangle]
pub unsafe extern "C" fn pimax_ctrlfuse_describe(
    h: *mut PimaxCtrlFuse,
    buf: *mut c_char,
    len: usize,
) -> usize {
    if h.is_null() || buf.is_null() || len == 0 {
        return 0;
    }
    let s = match (*h).f.lock() {
        Ok(f) => {
            let cal = match &f.calib {
                Some(c) => format!(
                    "gyro calibrated ({:.4} deg/s per LSB, axes {}, offset {} ms, {} pairs, r2 {:.3})",
                    c.scale_dps_per_lsb(),
                    c.axes(),
                    c.time_offset_ms,
                    c.pairs,
                    c.r2
                ),
                None => "gyro not calibrated yet (learning from the optical poses)".to_string(),
            };
            format!(
                "{cal}; gyro {}; {} fixes, {} dropped by the body model, {} by the prediction, {} reacquires, {} fit updates, out of view kept {:.1} s",
                if f.gyro_active() { "on" } else { "off" },
                f.stats.fixes,
                f.stats.rejected_body,
                f.stats.rejected,
                f.stats.reacquires,
                f.stats.fit_updates,
                f.cfg.lost_s
            )
        }
        Err(_) => "poisoned".to_string(),
    };
    let n = s.len().min(len - 1);
    std::ptr::copy_nonoverlapping(s.as_ptr(), buf as *mut u8, n);
    *buf.add(n) = 0;
    n
}
