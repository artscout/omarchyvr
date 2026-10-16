//! C ABI of the eye gaze session (`pimax-eye/include/pimax_eye.h`), exported from `libpimax_capi` so the Monado driver
//! links one Rust staticlib. Thin: everything is in `pimax_eye::live` and `pimax_eye::track`.
#![allow(non_camel_case_types, clippy::missing_safety_doc)]

use ::pimax_eye::clock::monotonic_ns;
use ::pimax_eye::live::{Config, State, Tracker};
use ::pimax_eye::track::Sample;
use std::os::raw::c_int;

pub const PIMAX_EYE_ABI_VERSION: u32 = 1;
pub const PIMAX_EYE_START_WITH_CAL: u32 = 1;
pub const PIMAX_EYE_GAZE_VALID: u32 = 1;
pub const PIMAX_EYE_LEFT_VALID: u32 = 2;
pub const PIMAX_EYE_RIGHT_VALID: u32 = 4;
pub const PIMAX_EYE_OK: c_int = 0;
pub const PIMAX_EYE_NO_SAMPLE: c_int = -1;
pub const PIMAX_EYE_BAD_ARG: c_int = -2;
pub const PIMAX_EYE_STALE: c_int = -3;
/// A newest sample older than this makes `latest` / `sample_at` report `PIMAX_EYE_STALE`.
pub const STALE_NS: i64 = 300_000_000;

/// Opaque handle.
pub struct pimax_eye {
    tracker: Tracker,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct pimax_eye_sample {
    pub seq: u64,
    pub device_ts_us: i64,
    pub host_ts_ns: i64,
    pub flags: u32,
    pub _pad: u32,
    pub origin: [f32; 3],
    pub direction: [f32; 3],
    pub eye_origin: [[f32; 3]; 2],
    pub eye_direction: [[f32; 3]; 2],
    pub pupil_mm: [f32; 2],
}

pub fn to_c(s: &Sample) -> pimax_eye_sample {
    let mut o = pimax_eye_sample {
        seq: s.seq,
        device_ts_us: s.device_ts_us,
        host_ts_ns: s.host_ts_ns,
        ..Default::default()
    };
    if let Some(g) = &s.gaze {
        o.flags |= PIMAX_EYE_GAZE_VALID;
        o.origin = g.origin;
        o.direction = g.direction;
    }
    for i in 0..2 {
        o.eye_origin[i] = s.eyes[i].origin;
        o.eye_direction[i] = s.eyes[i].direction;
        o.pupil_mm[i] = s.eyes[i].pupil_mm;
        if s.eyes[i].origin_valid && s.eyes[i].direction_valid {
            o.flags |= if i == 0 {
                PIMAX_EYE_LEFT_VALID
            } else {
                PIMAX_EYE_RIGHT_VALID
            };
        }
    }
    o
}

fn fill(newest: Option<Sample>, picked: Option<Sample>, out: &mut pimax_eye_sample) -> c_int {
    let (Some(n), Some(p)) = (newest, picked) else {
        return PIMAX_EYE_NO_SAMPLE;
    };
    *out = to_c(&p);
    // staleness is judged against the real clock, by the newest sample's arrival
    if monotonic_ns() - n.arrival_ns > STALE_NS {
        PIMAX_EYE_STALE
    } else {
        PIMAX_EYE_OK
    }
}

#[no_mangle]
pub extern "C" fn pimax_eye_abi_version() -> u32 {
    PIMAX_EYE_ABI_VERSION
}

#[no_mangle]
pub extern "C" fn pimax_eye_start(flags: u32) -> *mut pimax_eye {
    let mut cfg = Config::from_env();
    cfg.with_cal |= flags & PIMAX_EYE_START_WITH_CAL != 0;
    Box::into_raw(Box::new(pimax_eye {
        tracker: Tracker::start(cfg),
    }))
}

#[no_mangle]
pub unsafe extern "C" fn pimax_eye_stop(h: *mut pimax_eye) {
    if !h.is_null() {
        // SAFETY: created by pimax_eye_start, stopped once (caller contract). Dropping joins the thread, which
        // unsubscribes and releases the interface.
        drop(unsafe { Box::from_raw(h) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn pimax_eye_state(h: *const pimax_eye) -> c_int {
    // SAFETY: live handle or NULL.
    match unsafe { h.as_ref() } {
        Some(h) => h.tracker.state() as c_int,
        None => State::Stopped as c_int,
    }
}

#[no_mangle]
pub unsafe extern "C" fn pimax_eye_latest(
    h: *const pimax_eye,
    out: *mut pimax_eye_sample,
) -> c_int {
    // SAFETY: live pointers or NULL.
    let (Some(h), Some(out)) = (unsafe { h.as_ref() }, unsafe { out.as_mut() }) else {
        return PIMAX_EYE_BAD_ARG;
    };
    let l = h.tracker.latest();
    fill(l, l, out)
}

#[no_mangle]
pub unsafe extern "C" fn pimax_eye_sample_at(
    h: *const pimax_eye,
    at_ns: i64,
    out: *mut pimax_eye_sample,
) -> c_int {
    // SAFETY: live pointers or NULL.
    let (Some(h), Some(out)) = (unsafe { h.as_ref() }, unsafe { out.as_mut() }) else {
        return PIMAX_EYE_BAD_ARG;
    };
    fill(h.tracker.latest(), h.tracker.sample_at(at_ns), out)
}

/// Drift-correction hook: the wearer was looking along `dir` (head frame, unit) at `at_ns`. Returns the number of
/// eyes that used the observation (0..2), `PIMAX_EYE_BAD_ARG` for NULL pointers, `PIMAX_EYE_NO_SAMPLE` when there is
/// no calibration or no sample at that time.
#[no_mangle]
pub unsafe extern "C" fn pimax_eye_observe_target(
    h: *const pimax_eye,
    at_ns: i64,
    dir: *const f32,
) -> c_int {
    // SAFETY: live handle or NULL; `dir` points at 3 floats or is NULL (caller contract).
    let (Some(h), false) = (unsafe { h.as_ref() }, dir.is_null()) else {
        return PIMAX_EYE_BAD_ARG;
    };
    // SAFETY: checked non-NULL above; the caller passes 3 floats.
    let d = unsafe { std::slice::from_raw_parts(dir, 3) };
    let d = [f64::from(d[0]), f64::from(d[1]), f64::from(d[2])];
    match h
        .tracker
        .observe_target(at_ns, d, &::pimax_eye::drift::DriftParams::default())
    {
        Ok(n) => n as c_int,
        Err(_) => PIMAX_EYE_NO_SAMPLE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::pimax_eye::fake::FakeChip;
    use std::time::{Duration, Instant};

    #[test]
    fn struct_layout_matches_the_header() {
        assert_eq!(
            std::mem::size_of::<pimax_eye_sample>(),
            8 + 8 + 8 + 4 + 4 + 12 + 12 + 24 + 24 + 8
        );
        assert_eq!(std::mem::align_of::<pimax_eye_sample>(), 8);
    }

    #[test]
    fn null_arguments_are_safe() {
        unsafe {
            assert_eq!(
                pimax_eye_latest(std::ptr::null(), std::ptr::null_mut()),
                PIMAX_EYE_BAD_ARG
            );
            assert_eq!(pimax_eye_state(std::ptr::null()), State::Stopped as c_int);
            assert_eq!(
                pimax_eye_observe_target(std::ptr::null(), 0, std::ptr::null()),
                PIMAX_EYE_BAD_ARG
            );
            pimax_eye_stop(std::ptr::null_mut());
        }
        assert_eq!(pimax_eye_abi_version(), PIMAX_EYE_ABI_VERSION);
    }

    #[test]
    #[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
    fn handle_round_trip_with_a_fake_chip() {
        let cfg = Config {
            pace: false,
            ..Config::default()
        };
        let tracker = Tracker::start_with(cfg, |_, _| {
            let mut c = FakeChip::new();
            c.start_on = Some(0x4c4);
            Ok(c)
        });
        let h = Box::into_raw(Box::new(pimax_eye { tracker }));
        let mut s = pimax_eye_sample::default();
        let t0 = Instant::now();
        unsafe {
            while pimax_eye_latest(h, &mut s) == PIMAX_EYE_NO_SAMPLE
                && t0.elapsed() < Duration::from_secs(10)
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(pimax_eye_latest(h, &mut s), PIMAX_EYE_OK);
            assert_eq!(s.flags, PIMAX_EYE_GAZE_VALID | PIMAX_EYE_RIGHT_VALID);
            assert!(s.direction[2] < -0.9);
            let mut m = pimax_eye_sample::default();
            assert_eq!(pimax_eye_sample_at(h, i64::MAX, &mut m), PIMAX_EYE_OK);
            assert!(m.seq >= s.seq);
            assert_eq!(pimax_eye_state(h), State::Streaming as c_int);
            pimax_eye_stop(h);
        }
    }
}
