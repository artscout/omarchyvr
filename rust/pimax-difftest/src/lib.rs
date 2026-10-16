//! FFI to the original C library (symbols renamed `c_*` by build.rs) and conversions to compare it with the Rust port.
#![allow(non_camel_case_types)]
use pimax_proto::{ControllerReport, ImuSample, Report};
use std::os::raw::{c_char, c_int};

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct c_imu_sample {
    pub ts_us: u32,
    pub raw_accel: [i16; 3],
    pub raw_gyro: [i16; 3],
    pub accel_g: [f32; 3],
    pub gyro_dps: [f32; 3],
    pub ipd_mm: f32,
    pub sample_in_report: c_int,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct c_ctrl_report {
    pub slot: u8,
    pub ts_us: u32,
    pub stick_x: c_int,
    pub stick_y: c_int,
    pub trigger: u8,
    pub grip: u8,
    pub battery: u8,
    pub buttons: u32,
    pub touch: u8,
    pub raw_accel: [i16; 3],
    pub raw_gyro: [i16; 3],
    pub raw: [u8; 64],
}

extern "C" {
    fn c_pimax_decode_imu(r: *const u8, out: *mut c_imu_sample) -> c_int;
    fn c_pimax_decode_controller(r: *const u8, out: *mut c_ctrl_report) -> c_int;
    fn c_pimax_find_interfaces(paths: *mut [c_char; 64]) -> c_int;
    pub fn shim_sizeof_imu() -> usize;
    pub fn shim_sizeof_ctrl() -> usize;
    pub fn shim_off_ctrl_battery() -> usize;
    pub fn shim_off_ctrl_raw() -> usize;
    pub fn shim_off_imu_ipd() -> usize;
}

/// C `pimax_decode_imu`.
pub fn c_decode_imu(r: &Report) -> Option<[c_imu_sample; 2]> {
    let mut out = [c_imu_sample::default(); 2];
    // SAFETY: r is 64 bytes, out holds two samples, as the C API requires.
    let ok = unsafe { c_pimax_decode_imu(r.as_ptr(), out.as_mut_ptr()) };
    (ok != 0).then_some(out)
}

/// C `pimax_decode_controller`.
pub fn c_decode_controller(r: &Report) -> Option<c_ctrl_report> {
    // SAFETY: zero is a valid bit pattern for the plain-data struct; the C function memsets it anyway.
    let mut out: c_ctrl_report = unsafe { std::mem::zeroed() };
    // SAFETY: r is 64 bytes, out is one report.
    let ok = unsafe { c_pimax_decode_controller(r.as_ptr(), &mut out) };
    (ok != 0).then_some(out)
}

/// C `pimax_find_interfaces`: (mask, paths).
pub fn c_find_interfaces() -> (i32, [String; 4]) {
    let mut p = [[0 as c_char; 64]; 4];
    // SAFETY: paths is char[4][64].
    let m = unsafe { c_pimax_find_interfaces(p.as_mut_ptr()) };
    let s = |i: usize| {
        let b: Vec<u8> = p[i]
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c as u8)
            .collect();
        String::from_utf8_lossy(&b).into_owned()
    };
    (m, [s(0), s(1), s(2), s(3)])
}

/// Bit-exact comparison of IMU decodes; returns a description of the first difference.
pub fn imu_diff(rust: &Option<[ImuSample; 2]>, c: &Option<[c_imu_sample; 2]>) -> Option<String> {
    match (rust, c) {
        (None, None) => None,
        (Some(r), Some(c)) => {
            for i in 0..2 {
                let (r, c) = (&r[i], &c[i]);
                let same = r.ts_us == c.ts_us
                    && r.raw_accel == c.raw_accel
                    && r.raw_gyro == c.raw_gyro
                    && r.sample_in_report == c.sample_in_report
                    && r.ipd_mm.to_bits() == c.ipd_mm.to_bits()
                    && (0..3).all(|k| {
                        r.accel_g[k].to_bits() == c.accel_g[k].to_bits()
                            && r.gyro_dps[k].to_bits() == c.gyro_dps[k].to_bits()
                    });
                if !same {
                    return Some(format!("sample {i}: rust {r:?} vs c ts={} acc={:?} gyro={:?} g={:?} dps={:?} ipd={}", c.ts_us, c.raw_accel, c.raw_gyro, c.accel_g, c.gyro_dps, c.ipd_mm));
                }
            }
            None
        }
        _ => Some(format!(
            "accept mismatch: rust {} c {}",
            rust.is_some(),
            c.is_some()
        )),
    }
}

/// Field-by-field comparison of controller decodes. `battery` differs by design: the C library keeps byte 46,
/// Rust reports percent (`& 0x7f`) and a separate charging flag; they must agree after recombining.
pub fn ctrl_diff(rust: &Option<ControllerReport>, c: &Option<c_ctrl_report>) -> Option<String> {
    match (rust, c) {
        (None, None) => None,
        (Some(r), Some(c)) => {
            let bat_ok = c.battery == (r.battery | if r.charging { 0x80 } else { 0 })
                && c.raw[46] == c.battery;
            let same = r.slot == c.slot
                && r.ts_us == c.ts_us
                && r.stick_x == c.stick_x
                && r.stick_y == c.stick_y
                && r.trigger == c.trigger
                && r.grip == c.grip
                && r.buttons == c.buttons
                && r.touch == c.touch
                && r.raw_accel == c.raw_accel
                && r.raw_gyro == c.raw_gyro
                && r.raw == c.raw
                && bat_ok;
            (!same).then(|| format!("rust {r:?}\n   c slot={} ts={} stick=({},{}) trig={} grip={} bat={} btn={} touch={} acc={:?} gyro={:?}", c.slot, c.ts_us, c.stick_x, c.stick_y, c.trigger, c.grip, c.battery, c.buttons, c.touch, c.raw_accel, c.raw_gyro))
        }
        _ => Some(format!(
            "accept mismatch: rust {} c {}",
            rust.is_some(),
            c.is_some()
        )),
    }
}
