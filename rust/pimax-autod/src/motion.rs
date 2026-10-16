//! Wear detection from IMU micro-motion: a headset on a head is never still (breathing, pulse, balance), one on a
//! desk or a cushion is. Measured on the 14 Oct SLAM recordings (`~/.local/share/pimax/slam-recordings/*/euroc_*/
//! mav0/imu0/data.csv`, 0.5 s windows): the gyro noise floor at rest is 0.09-0.10 deg/s std, worn windows are
//! 0.3 deg/s and up (typically 1-60). Thresholds `still_dps` 0.2 / `worn_dps` 0.3 with hysteresis in between.

/// Motion class of one window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    /// No data (headset asleep, IMU source off, read failed).
    Unknown,
    /// Lying still: not worn.
    Still,
    /// Small motion: on a head.
    Worn,
    /// Large motion (> 5 deg/s std): handled, carried, or a head turning.
    Moving,
}

impl Motion {
    pub fn name(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Still => "still",
            Self::Worn => "worn",
            Self::Moving => "moving",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        [Self::Unknown, Self::Still, Self::Worn, Self::Moving]
            .into_iter()
            .find(|m| m.name() == s)
    }
    /// Evidence that the headset is on a head (Moving counts: put-on itself is a large motion).
    pub fn is_wear(self) -> bool {
        matches!(self, Self::Worn | Self::Moving)
    }
}

/// Above this the motion is Moving.
pub const MOVING_DPS: f64 = 5.0;

/// Running gyro statistics of one window (deg/s).
#[derive(Debug, Default, Clone)]
pub struct Window {
    n: u64,
    s: [f64; 3],
    q: [f64; 3],
}

impl Window {
    pub fn push(&mut self, gyro_dps: [f64; 3]) {
        self.n += 1;
        for (k, g) in gyro_dps.into_iter().enumerate() {
            self.s[k] += g;
            self.q[k] += g * g;
        }
    }
    pub fn len(&self) -> u64 {
        self.n
    }
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }
    /// Root of the summed per-axis variances (bias removed), deg/s.
    pub fn std_dps(&self) -> Option<f64> {
        if self.n < 20 {
            return None;
        }
        let n = self.n as f64;
        let v: f64 = (0..3).map(|k| (self.q[k] - self.s[k] * self.s[k] / n) / n).sum();
        Some(v.max(0.0).sqrt())
    }
}

/// Classifier with hysteresis: between `still` and `worn` the previous class is kept.
#[derive(Debug, Clone)]
pub struct Classifier {
    pub still_dps: f64,
    pub worn_dps: f64,
    last: Motion,
}

impl Classifier {
    pub fn new(still_dps: f64, worn_dps: f64) -> Self {
        Self { still_dps, worn_dps, last: Motion::Unknown }
    }
    pub fn classify(&mut self, std_dps: Option<f64>) -> Motion {
        let m = match std_dps {
            None => Motion::Unknown,
            Some(s) if s > MOVING_DPS => Motion::Moving,
            Some(s) if s >= self.worn_dps => Motion::Worn,
            Some(s) if s <= self.still_dps => Motion::Still,
            Some(_) => match self.last {
                Motion::Unknown => Motion::Still,
                Motion::Moving => Motion::Worn,
                l => l,
            },
        };
        self.last = m;
        m
    }
}

/// Replay of a EuRoC `imu0/data.csv` (`ts_ns, wx, wy, wz [rad/s], ax, ay, az`): motion class per `window_s`.
/// Returns `(t_s, std_dps, class)` rows.
pub fn replay_euroc(text: &str, window_s: f64, cls: &mut Classifier) -> Vec<(f64, f64, Motion)> {
    let mut out = Vec::new();
    let mut t0: Option<f64> = None;
    let mut w = Window::default();
    let mut w_start = 0.0;
    for line in text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let f: Vec<f64> = line.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        if f.len() < 4 {
            continue;
        }
        let t = f[0] / 1e9;
        let t0 = *t0.get_or_insert(t);
        let rel = t - t0;
        if rel - w_start >= window_s {
            let s = w.std_dps();
            out.push((w_start, s.unwrap_or(f64::NAN), cls.classify(s)));
            w = Window::default();
            w_start += window_s * ((rel - w_start) / window_s).floor();
        }
        let d = 180.0 / std::f64::consts::PI;
        w.push([f[1] * d, f[2] * d, f[3] * d]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(seed: &mut u64, amp: f64) -> f64 {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((*seed >> 11) as f64 / (1u64 << 53) as f64 - 0.5) * 2.0 * amp
    }

    fn window(amp: f64, bias: f64) -> Window {
        let mut s = 7u64;
        let mut w = Window::default();
        for _ in 0..500 {
            w.push([bias + noise(&mut s, amp), noise(&mut s, amp), noise(&mut s, amp)]);
        }
        w
    }

    #[test]
    fn std_ignores_the_gyro_bias() {
        let a = window(0.1, 0.0).std_dps().unwrap();
        let b = window(0.1, 3.0).std_dps().unwrap();
        assert!((a - b).abs() < 1e-6);
        assert!(Window::default().std_dps().is_none());
    }

    #[test]
    fn classes_and_hysteresis() {
        let mut c = Classifier::new(0.2, 0.3);
        assert_eq!(c.classify(None), Motion::Unknown);
        assert_eq!(c.classify(Some(0.1)), Motion::Still);
        assert_eq!(c.classify(Some(0.25)), Motion::Still, "band keeps Still");
        assert_eq!(c.classify(Some(0.5)), Motion::Worn);
        assert_eq!(c.classify(Some(0.25)), Motion::Worn, "band keeps Worn");
        assert_eq!(c.classify(Some(20.0)), Motion::Moving);
        assert_eq!(c.classify(Some(0.25)), Motion::Worn, "after Moving the band means Worn");
        assert_eq!(c.classify(Some(0.15)), Motion::Still);
    }

    #[test]
    fn euroc_replay_finds_a_put_on() {
        // 10 s at rest (0.1 dps noise), then worn (2 dps)
        let mut s = 3u64;
        let mut text = String::from("#timestamp [ns],w_RS_S_x [rad s^-1],...\n");
        let r = std::f64::consts::PI / 180.0;
        for i in 0..20_000u64 {
            let amp = if i < 10_000 { 0.17 } else { 3.5 };
            let g = [noise(&mut s, amp) * r, noise(&mut s, amp) * r, noise(&mut s, amp) * r];
            text.push_str(&format!("{},{},{},{},0,9.8,0\n", 1_000_000_000_000 + i * 1_000_000, g[0], g[1], g[2]));
        }
        let rows = replay_euroc(&text, 0.5, &mut Classifier::new(0.2, 0.3));
        assert_eq!(rows.len(), 39);
        assert!(rows[..19].iter().all(|r| r.2 == Motion::Still), "{rows:?}");
        assert!(rows[21..].iter().all(|r| r.2 == Motion::Worn), "{rows:?}");
    }
}
