//! Host monotonic clock and the device-to-host timestamp map.
#![allow(unsafe_code)]

/// `CLOCK_MONOTONIC` in nanoseconds: the clock Monado stamps its data with (`os_monotonic_get_ns`).
pub fn monotonic_ns() -> i64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: ts is a valid timespec for the duration of the call.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec * 1_000_000_000 + ts.tv_nsec
}

/// Maps the device clock (microseconds, same crystal as the IMU) to the host `CLOCK_MONOTONIC`.
///
/// `arrival - device` is the clock offset plus a non-negative transport delay, so the true offset is the lower
/// envelope of that difference. The estimate follows it with a one-sided filter: it jumps down at once to any new
/// minimum and creeps up with a time constant of `1/alpha` samples, which tracks crystal drift (tens of ppm, a few
/// tens of microseconds per second) without being dragged up by USB jitter. The result is "arrival time of the
/// least-delayed sample", so it is late by the minimal transport latency (a ms or two); [`ClockMap::latency_ns`]
/// subtracts a configured allowance. Output is forced strictly increasing.
#[derive(Debug, Clone)]
pub struct ClockMap {
    offset_ns: Option<f64>,
    alpha: f64,
    last_dev_us: i64,
    last_out_ns: i64,
    /// Subtracted from every mapped time (configured transport latency).
    pub latency_ns: i64,
}

impl Default for ClockMap {
    fn default() -> Self {
        ClockMap::new(1.0 / 1024.0)
    }
}

impl ClockMap {
    /// New map; `alpha` is the upward creep per sample.
    pub fn new(alpha: f64) -> ClockMap {
        ClockMap {
            offset_ns: None,
            alpha,
            last_dev_us: i64::MIN,
            last_out_ns: i64::MIN,
            latency_ns: 0,
        }
    }

    /// Forget everything (device re-enumerated: its clock restarted).
    pub fn reset(&mut self) {
        let lat = self.latency_ns;
        *self = ClockMap::new(self.alpha);
        self.latency_ns = lat;
    }

    /// Current offset `host - device` in ns, if any sample was seen.
    pub fn offset_ns(&self) -> Option<f64> {
        self.offset_ns
    }

    /// Host time for a sample with device timestamp `dev_us` that arrived at host time `arrival_ns`.
    pub fn map(&mut self, dev_us: i64, arrival_ns: i64) -> i64 {
        // device clock went backwards by more than a second: new epoch
        if self.last_dev_us != i64::MIN && dev_us < self.last_dev_us - 1_000_000 {
            self.reset();
        }
        self.last_dev_us = dev_us;
        let x = arrival_ns as f64 - dev_us as f64 * 1000.0;
        let o = match self.offset_ns {
            None => x,
            Some(o) if x < o => x,
            Some(o) => o + self.alpha * (x - o),
        };
        self.offset_ns = Some(o);
        let mut out = (dev_us as f64 * 1000.0 + o) as i64 - self.latency_ns;
        if out <= self.last_out_ns {
            out = self.last_out_ns + 1000;
        }
        self.last_out_ns = out;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tiny deterministic LCG for jitter.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 33) as f64) / f64::from(1u32 << 31)
        }
    }

    #[test]
    fn tracks_offset_and_drift_through_jitter() {
        let mut m = ClockMap::default();
        let mut rng = Lcg(7);
        let host0 = 5_000_000_000_i64; // host clock at device t=0
        let drift = 40e-6; // device crystal 40 ppm slow relative to host
        let mut worst: f64 = 0.0;
        for i in 0..(120 * 120) {
            let dev_us = 1_000_000 + i * 8300;
            let true_host = host0 as f64 + (dev_us as f64 * 1000.0) * (1.0 + drift);
            // 0.3..3 ms transport delay, rare 15 ms hiccup
            let delay =
                300_000.0 + rng.next() * 2_700_000.0 + if i % 500 == 0 { 15e6 } else { 0.0 };
            let arrival = (true_host + delay) as i64;
            let mapped = m.map(dev_us, arrival);
            if i > 2000 {
                worst = worst.max((mapped as f64 - true_host).abs());
            }
        }
        // after warm-up the mapped time is within ~the minimum delay plus a bit of filter bias
        assert!(worst < 2_500_000.0, "worst error {worst} ns");
    }

    #[test]
    fn output_is_strictly_increasing_and_reset_on_epoch_change() {
        let mut m = ClockMap::default();
        let a = m.map(1_000_000, 10_000_000_000);
        let b = m.map(1_008_300, 10_008_000_000); // arrived "early": offset drops
        assert!(b > a);
        // device restarted
        let c = m.map(5_000, 20_000_000_000);
        assert!(c >= 20_000_000_000 - 1_000_000);
    }

    #[test]
    fn latency_allowance_is_subtracted() {
        let mut m = ClockMap {
            latency_ns: 2_000_000,
            ..ClockMap::default()
        };
        assert_eq!(m.map(1_000, 1_000_000_000), 1_000_000_000 - 2_000_000);
    }

    #[test]
    fn monotonic_clock_advances() {
        let a = monotonic_ns();
        std::thread::sleep(std::time::Duration::from_millis(2));
        assert!(monotonic_ns() > a);
    }
}
