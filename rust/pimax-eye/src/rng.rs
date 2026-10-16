//! A tiny deterministic random source (xorshift64*) with a normal deviate, for simulations and tests (no
//! dependency).

/// xorshift64* generator.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    /// New generator; the seed 0 is replaced by a constant.
    pub fn new(seed: u64) -> Rng {
        Rng(if seed == 0 { 0x9e37_79b9_7f4a_7c15 } else { seed })
    }

    /// Next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// Uniform in (0, 1).
    pub fn uniform(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    /// Standard normal (Box-Muller).
    pub fn gauss(&mut self) -> f64 {
        let (a, b) = (self.uniform(), self.uniform());
        (-2.0 * a.ln()).sqrt() * (2.0 * std::f64::consts::PI * b).cos()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moments_are_right() {
        let mut r = Rng::new(5);
        let n = 20000;
        let v: Vec<f64> = (0..n).map(|_| r.gauss()).collect();
        let m = v.iter().sum::<f64>() / n as f64;
        let s = (v.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / n as f64).sqrt();
        assert!(m.abs() < 0.03 && (s - 1.0).abs() < 0.03, "{m} {s}");
        let u: Vec<f64> = (0..n).map(|_| r.uniform()).collect();
        assert!(u.iter().all(|&x| x > 0.0 && x < 1.0));
    }
}
