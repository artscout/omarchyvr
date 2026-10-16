//! Smoothing primitives: One Euro filter (analog inputs) and a critically damped smoother (thumb target).
//!
//! Both work on `f32` with an explicit `dt` in seconds, never read a clock and never panic: non-finite inputs and
//! non-positive `dt` leave the state untouched.

use core::f32::consts::PI;

/// One Euro filter (Casiez, Roussel, Vogel, CHI 2012): a low-pass whose cutoff rises with the filtered speed, so a
/// resting signal is smooth and a fast one has little lag.
///
/// `cutoff = min_cutoff + beta * |filtered speed|`, speed in units/s, cutoffs in Hz.
#[derive(Clone, Debug)]
pub struct OneEuro {
    pub min_cutoff: f32,
    pub beta: f32,
    pub d_cutoff: f32,
    x_hat: f32,
    dx_hat: f32,
    primed: bool,
}

/// Smoothing factor of a first-order low-pass with the given cutoff, for a step of `dt` seconds.
fn alpha(cutoff: f32, dt: f32) -> f32 {
    let tau = 1.0 / (2.0 * PI * cutoff.max(1e-3));
    dt / (dt + tau)
}

impl OneEuro {
    pub fn new(min_cutoff: f32, beta: f32, d_cutoff: f32) -> Self {
        Self {
            min_cutoff,
            beta,
            d_cutoff,
            x_hat: 0.0,
            dx_hat: 0.0,
            primed: false,
        }
    }

    /// Forget the history; the next sample passes through unchanged.
    pub fn reset(&mut self) {
        self.primed = false;
        self.x_hat = 0.0;
        self.dx_hat = 0.0;
    }

    /// Start from `x` right away (used after a long gap in the data).
    pub fn prime(&mut self, x: f32) {
        if x.is_finite() {
            self.x_hat = x;
            self.dx_hat = 0.0;
            self.primed = true;
        }
    }

    /// Latest output, `None` before the first sample.
    pub fn value(&self) -> Option<f32> {
        self.primed.then_some(self.x_hat)
    }

    /// Filtered speed estimate in units/s (0 before the second sample).
    pub fn speed(&self) -> f32 {
        self.dx_hat
    }

    /// Feed one sample taken `dt` seconds after the previous one; returns the filtered value.
    pub fn update(&mut self, x: f32, dt: f32) -> f32 {
        if !x.is_finite() {
            return self.x_hat;
        }
        if !self.primed {
            self.prime(x);
            return self.x_hat;
        }
        if !(dt.is_finite() && dt > 0.0) {
            return self.x_hat;
        }
        let dx = (x - self.x_hat) / dt;
        self.dx_hat += alpha(self.d_cutoff, dt) * (dx - self.dx_hat);
        let cutoff = self.min_cutoff + self.beta * self.dx_hat.abs();
        self.x_hat += alpha(cutoff, dt) * (x - self.x_hat);
        self.x_hat
    }
}

/// Critically damped smoother toward a moving target (same scheme as Unity's `SmoothDamp`): no overshoot, speed
/// limited, independent of the step size to first order.
#[derive(Clone, Debug)]
pub struct SmoothDamp {
    /// Approximate time to reach the target, seconds.
    pub smooth_time: f32,
    /// Speed limit, units/s.
    pub max_speed: f32,
    value: f32,
    velocity: f32,
}

impl SmoothDamp {
    pub fn new(initial: f32, smooth_time: f32, max_speed: f32) -> Self {
        Self {
            smooth_time,
            max_speed,
            value: if initial.is_finite() { initial } else { 0.0 },
            velocity: 0.0,
        }
    }

    pub fn value(&self) -> f32 {
        self.value
    }

    /// Jump to `v` with zero velocity.
    pub fn snap(&mut self, v: f32) {
        if v.is_finite() {
            self.value = v;
            self.velocity = 0.0;
        }
    }

    pub fn update(&mut self, target: f32, dt: f32) -> f32 {
        if !(target.is_finite() && dt.is_finite() && dt > 0.0) {
            return self.value;
        }
        let smooth = self.smooth_time.max(1e-4);
        let omega = 2.0 / smooth;
        let x = omega * dt;
        let exp = 1.0 / (1.0 + x + 0.48 * x * x + 0.235 * x * x * x);
        let max_change = self.max_speed.max(0.0) * smooth;
        let change = (self.value - target).clamp(-max_change, max_change);
        let clamped_target = self.value - change;
        let temp = (self.velocity + omega * change) * dt;
        self.velocity = (self.velocity - omega * temp) * exp;
        let mut out = clamped_target + (change + temp) * exp;
        // Never overshoot the (unclamped) target.
        if (target - self.value > 0.0) == (out > target) {
            out = target;
            self.velocity = 0.0;
        }
        self.value = out;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic pseudo noise in [-1, 1] (xorshift), so the tests need no RNG crate.
    struct Noise(u32);
    impl Noise {
        fn next(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 17;
            self.0 ^= self.0 << 5;
            (self.0 as f32 / u32::MAX as f32) * 2.0 - 1.0
        }
    }

    #[test]
    fn one_euro_first_sample_passes_through() {
        let mut f = OneEuro::new(1.0, 0.0, 1.0);
        assert_eq!(f.value(), None);
        assert_eq!(f.update(0.7, 0.004), 0.7);
        assert_eq!(f.value(), Some(0.7));
    }

    #[test]
    fn one_euro_constant_input_is_a_fixed_point() {
        let mut f = OneEuro::new(2.0, 5.0, 1.0);
        f.update(0.4, 0.004);
        for _ in 0..1000 {
            assert!((f.update(0.4, 0.004) - 0.4).abs() < 1e-6);
        }
        assert!(f.speed().abs() < 1e-6);
    }

    #[test]
    fn one_euro_attenuates_jitter_at_rest() {
        let mut f = OneEuro::new(1.5, 0.0, 1.0);
        let mut n = Noise(0x1234_5678);
        let (mut raw_sq, mut out_sq) = (0.0f64, 0.0f64);
        f.update(0.5, 0.004);
        for i in 0..4000 {
            let noise = 0.02 * n.next();
            let y = f.update(0.5 + noise, 0.004);
            if i > 500 {
                raw_sq += (noise as f64).powi(2);
                out_sq += ((y - 0.5) as f64).powi(2);
            }
        }
        assert!(out_sq < raw_sq * 0.2, "jitter {out_sq} vs raw {raw_sq}");
    }

    #[test]
    fn one_euro_speed_coefficient_reduces_lag_on_fast_moves() {
        // A 0 -> 1 step in 0.1 s; with beta the filter reaches 90 % sooner than the same filter without it.
        let reach = |beta: f32| {
            let mut f = OneEuro::new(1.0, beta, 1.0);
            f.update(0.0, 0.004);
            let mut t = 0.0;
            loop {
                t += 0.004;
                let x = (t / 0.1f32).min(1.0);
                let y = f.update(x, 0.004);
                if y >= 0.9 || t > 5.0 {
                    return t;
                }
            }
        };
        let slow = reach(0.0);
        let fast = reach(20.0);
        assert!(fast < slow * 0.6, "beta=20 {fast}s vs beta=0 {slow}s");
    }

    #[test]
    fn one_euro_never_leaves_the_input_range() {
        let mut f = OneEuro::new(1.0, 30.0, 1.0);
        let mut n = Noise(99);
        f.update(0.0, 0.004);
        for _ in 0..20_000 {
            let x = (n.next() * 0.5 + 0.5).clamp(0.0, 1.0);
            let y = f.update(x, 0.004);
            assert!((0.0..=1.0).contains(&y), "{y}");
        }
    }

    #[test]
    fn one_euro_ignores_garbage() {
        let mut f = OneEuro::new(1.0, 1.0, 1.0);
        f.update(0.3, 0.004);
        assert_eq!(f.update(f32::NAN, 0.004), 0.3);
        assert_eq!(f.update(f32::INFINITY, 0.004), 0.3);
        assert_eq!(f.update(0.9, 0.0), 0.3);
        assert_eq!(f.update(0.9, -1.0), 0.3);
        assert_eq!(f.update(0.9, f32::NAN), 0.3);
        assert!(f.update(0.9, 0.004) > 0.3);
    }

    #[test]
    fn one_euro_is_roughly_independent_of_the_sample_rate() {
        let run = |dt: f32| {
            let mut f = OneEuro::new(2.0, 3.0, 1.0);
            f.update(0.0, dt);
            let steps = (0.3 / dt) as usize;
            let mut y = 0.0;
            for _ in 0..steps {
                y = f.update(1.0, dt);
            }
            y
        };
        let (a, b) = (run(0.004), run(0.0111));
        assert!((a - b).abs() < 0.1, "{a} vs {b}");
    }

    #[test]
    fn smooth_damp_converges_without_overshoot() {
        let mut s = SmoothDamp::new(0.0, 0.08, 100.0);
        let mut prev = 0.0;
        for _ in 0..500 {
            let v = s.update(1.0, 0.004);
            assert!(v >= prev - 1e-6 && v <= 1.0 + 1e-6, "{v}");
            prev = v;
        }
        assert!((s.value() - 1.0).abs() < 1e-3);
    }

    #[test]
    fn smooth_damp_goes_down_as_well() {
        let mut s = SmoothDamp::new(1.0, 0.08, 100.0);
        for _ in 0..500 {
            let v = s.update(0.0, 0.004);
            assert!((-1e-6..=1.0 + 1e-6).contains(&v));
        }
        assert!(s.value() < 1e-3);
    }

    #[test]
    fn smooth_damp_respects_the_speed_limit() {
        let mut s = SmoothDamp::new(0.0, 0.05, 2.0);
        let mut prev = 0.0;
        for _ in 0..200 {
            let v = s.update(1.0, 0.004);
            assert!((v - prev) / 0.004 <= 2.0 * 1.2, "{}", (v - prev) / 0.004);
            prev = v;
        }
    }

    #[test]
    fn smooth_damp_reaches_the_target_in_about_the_smooth_time() {
        let mut s = SmoothDamp::new(0.0, 0.1, 100.0);
        let mut t = 0.0;
        while s.update(1.0, 0.001) < 0.95 {
            t += 0.001;
            assert!(t < 1.0);
        }
        assert!((0.08..0.4).contains(&t), "{t}");
    }

    #[test]
    fn smooth_damp_snap_and_garbage() {
        let mut s = SmoothDamp::new(0.0, 0.1, 10.0);
        s.snap(0.6);
        assert_eq!(s.value(), 0.6);
        assert_eq!(s.update(f32::NAN, 0.01), 0.6);
        assert_eq!(s.update(1.0, 0.0), 0.6);
        assert_eq!(s.update(1.0, -0.1), 0.6);
        s.snap(f32::NAN);
        assert_eq!(s.value(), 0.6);
    }
}
