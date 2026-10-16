//! Headset IMU report (IF2): `53 31 08 00 | ts u32 LE @4 | samples @12 and @28 | ipd u16 LE @45`.
use crate::{be16, le32, Report};

/// One IMU sample (a report holds two, 1 ms apart).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ImuSample {
    /// Device clock, microseconds; the second sample is `ts + 1000`.
    pub ts_us: u32,
    /// Raw accelerometer counts.
    pub raw_accel: [i16; 3],
    /// Raw gyro counts.
    pub raw_gyro: [i16; 3],
    /// `raw / 2048`.
    pub accel_g: [f32; 3],
    /// `raw / 16.4`.
    pub gyro_dps: [f32; 3],
    /// Live lens IPD in mm (bytes 45-46, u16 LE / 100).
    pub ipd_mm: f32,
    /// 0 or 1.
    pub sample_in_report: i32,
}

/// Decode an IMU report; `None` if it is not one.
pub fn decode_imu(r: &Report) -> Option<[ImuSample; 2]> {
    if r[0] != 0x53 || r[1] != 0x31 {
        return None;
    }
    let ts = le32(&r[4..]);
    let ipd = f32::from(u16::from(r[45]) | (u16::from(r[46]) << 8)) / 100.0;
    let mut out = [ImuSample::default(); 2];
    for (i, o) in out.iter_mut().enumerate() {
        let s = &r[12 + 16 * i + 2..];
        for k in 0..3 {
            o.raw_accel[k] = be16(&s[2 * k..]);
            o.raw_gyro[k] = be16(&s[6 + 2 * k..]);
            o.accel_g[k] = f32::from(o.raw_accel[k]) / 2048.0;
            o.gyro_dps[k] = f32::from(o.raw_gyro[k]) / 16.4;
        }
        o.ts_us = ts.wrapping_add(1000u32.wrapping_mul(i as u32));
        o.ipd_mm = ipd;
        o.sample_in_report = i as i32;
    }
    Some(out)
}
