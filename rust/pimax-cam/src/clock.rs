//! Device clock -> host `CLOCK_MONOTONIC`.
//!
//! The frame metadata carries the MCU time in microseconds (64 bit); the IMU reports the low 32 bits of the same
//! clock. Two mappings:
//! * [`imu_dev_to_mono_ns`]: the preferred one inside Monado. The HMD driver already maps IMU device time to
//!   monotonic (`pimax_hmd.c` keeps an unwrapped device time and a minimum-delay offset). Using the same reference
//!   puts camera and IMU samples on one timeline exactly (what SLAM needs).
//! * [`HostClockMap`]: fallback without the IMU (replays, camera-only runs). It tracks the minimum of
//!   `host_arrival - device_time` (the least delayed frame), relaxing slowly so drift is followed. Same rule as
//!   the HMD driver's IMU mapping.

/// IMU clock reference published by the HMD driver.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImuClockRef {
    /// Last IMU device timestamp (u32 µs, as reported).
    pub dev_ts_u32: u32,
    /// Non-zero when the reference is valid.
    pub valid: u32,
    /// The same timestamp unwrapped to 64 bit by the driver.
    pub dev_ext_us: u64,
    /// `monotonic_ns = dev_ext_us * 1000 + offset_ns`.
    pub offset_ns: i64,
}

/// Unwrap the low 32 bits of `dev_ts_us` around the IMU reference and convert to monotonic ns.
/// Valid while the camera timestamp is within ±35 min of the reference (u32 µs wraps every 71.6 min).
pub fn imu_dev_to_mono_ns(dev_ts_us: u64, r: &ImuClockRef) -> u64 {
    let delta = (dev_ts_us as u32).wrapping_sub(r.dev_ts_u32) as i32 as i64;
    let ext = r.dev_ext_us as i64 + delta;
    let ns = ext as i128 * 1000 + r.offset_ns as i128;
    ns.clamp(0, u64::MAX as i128) as u64
}

/// Minimum-delay mapping from device µs to host ns, from arrival times.
#[derive(Clone, Debug, Default)]
pub struct HostClockMap {
    have: bool,
    offset_ns: i64,
    last_dev_us: u64,
    last_out_ns: u64,
    /// Number of resets (device time went backwards or jumped).
    pub resets: u32,
}

impl HostClockMap {
    /// Relaxation shift: the offset moves `(candidate - offset) >> RELAX` per frame towards later arrivals
    /// (2^8 frames = ~5 s at 50 fps).
    pub const RELAX: u32 = 8;

    /// Map one frame; `host_ns` is when the host saw it (V4L2 buffer timestamp). Output is strictly increasing.
    pub fn map(&mut self, dev_us: u64, host_ns: u64) -> u64 {
        let jump =
            self.have && (dev_us < self.last_dev_us || dev_us - self.last_dev_us > 2_000_000);
        if jump {
            self.resets += 1;
        }
        let cand = host_ns as i64 - (dev_us as i64).wrapping_mul(1000);
        if !self.have || jump {
            self.have = true;
            self.offset_ns = cand;
        } else if cand < self.offset_ns {
            self.offset_ns = cand;
        } else {
            self.offset_ns += (cand - self.offset_ns) >> Self::RELAX;
        }
        self.last_dev_us = dev_us;
        let mut t = (dev_us as i64 * 1000 + self.offset_ns).max(0) as u64;
        if !jump && t <= self.last_out_ns {
            t = self.last_out_ns + 1;
        }
        self.last_out_ns = t;
        t
    }

    /// Current offset (`host - device`), ns.
    pub fn offset_ns(&self) -> i64 {
        self.offset_ns
    }
}

/// Monotonic clock now, ns.
pub fn monotonic_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: valid pointer to a timespec.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imu_mapping_unwraps() {
        // IMU: u32 0xffff_ff00 extended to 5_000_000_000_000, offset 1 s.
        let r = ImuClockRef {
            dev_ts_u32: 0xffff_ff00,
            valid: 1,
            dev_ext_us: 5_000_000_000_000,
            offset_ns: 1_000_000_000,
        };
        // camera frame 0x200 µs later, after the u32 wrap; its high word differs from the IMU's notion entirely.
        let cam = (7u64 << 32) | 0x100;
        let ns = imu_dev_to_mono_ns(cam, &r);
        assert_eq!(ns, (5_000_000_000_000u64 + 0x200) * 1000 + 1_000_000_000);
        // 20 ms earlier than the reference
        let cam = (0xffff_ff00u32.wrapping_sub(20_000)) as u64;
        assert_eq!(
            imu_dev_to_mono_ns(cam, &r),
            (5_000_000_000_000u64 - 20_000) * 1000 + 1_000_000_000
        );
    }

    #[test]
    fn host_map_tracks_minimum_delay() {
        let mut m = HostClockMap::default();
        // true offset 10 s; arrivals delayed by 11.8 ms +- jitter
        let jitter = [
            3_000_000i64,
            0,
            4_000_000,
            1_000_000,
            2_500_000,
            0,
            3_900_000,
        ];
        let mut out = Vec::new();
        for i in 0..2000u64 {
            let dev = 1_000_000 + i * 19_988;
            let host = (dev * 1000) as i64
                + 10_000_000_000
                + 11_800_000
                + jitter[i as usize % jitter.len()];
            out.push(m.map(dev, host as u64));
        }
        // after convergence the output equals device time + offset + minimum delay, jitter removed
        let last = *out.last().unwrap();
        let dev = 1_000_000 + 1999 * 19_988u64;
        let ideal = dev * 1000 + 10_000_000_000 + 11_800_000;
        assert!(
            (last as i64 - ideal as i64).abs() < 200_000,
            "err {} ns",
            last as i64 - ideal as i64
        );
        assert!(out.windows(2).all(|w| w[1] > w[0]));
        // frame spacing stays ~19.988 ms (no jitter transferred)
        let d = out[1999] - out[1998];
        assert!((d as i64 - 19_988_000).abs() < 50_000, "spacing {d}");
    }

    #[test]
    fn host_map_resets_on_jump() {
        let mut m = HostClockMap::default();
        let a = m.map(10_000_000, 50_000_000_000);
        let b = m.map(1_000, 51_000_000_000); // device time went back (replay loop)
        assert_eq!(m.resets, 1);
        assert_eq!(a, 50_000_000_000);
        assert_eq!(b, 51_000_000_000);
    }
}
