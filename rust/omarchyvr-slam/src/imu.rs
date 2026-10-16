//! IMU noise from a still segment of a recording. The factory file's `IMUNoise` block is all 0.0, so Basalt needs
//! numbers from somewhere: white-noise densities from the sample scatter while the headset rests
//! (`sigma_discrete = density * sqrt(rate)`), inflated for real use. Bias random walk needs minutes of still data
//! (Allan deviation); with shorter segments the defaults stay.

use crate::euroc::ImuRow;
use pimax_cam::slam::ImuNoise;

/// Result of [`estimate`].
#[derive(Clone, Debug, PartialEq)]
pub struct NoiseEstimate {
    /// Window start/end (ns).
    pub window: (i64, i64),
    /// Measured rate (Hz).
    pub rate_hz: f64,
    /// Per-axis sample standard deviation, rad/s and m/s².
    pub gyro_std: [f64; 3],
    /// Per-axis sample standard deviation.
    pub accel_std: [f64; 3],
    /// Mean gyro (bias estimate, rad/s) and accel in the window.
    pub gyro_mean: [f64; 3],
    /// Mean accel.
    pub accel_mean: [f64; 3],
    /// Noise model with densities = std / sqrt(rate) * inflate.
    pub noise: ImuNoise,
}

fn stats(rows: &[ImuRow], f: impl Fn(&ImuRow) -> [f64; 3]) -> ([f64; 3], [f64; 3]) {
    let n = rows.len() as f64;
    let mut m = [0.0; 3];
    for r in rows {
        let v = f(r);
        for k in 0..3 {
            m[k] += v[k] / n;
        }
    }
    let mut s = [0.0; 3];
    for r in rows {
        let v = f(r);
        for k in 0..3 {
            s[k] += (v[k] - m[k]).powi(2) / (n - 1.0).max(1.0);
        }
    }
    (m, [s[0].sqrt(), s[1].sqrt(), s[2].sqrt()])
}

/// Find the stillest `window_s` seconds (smallest summed gyro variance) and derive a noise model. Fails when the
/// stillest window still looks like motion (gyro std above 0.02 rad/s on any axis).
pub fn estimate(imu: &[ImuRow], window_s: f64, inflate: f64) -> Result<NoiseEstimate, String> {
    if imu.len() < 100 {
        return Err("too few IMU samples".into());
    }
    let span = (imu[imu.len() - 1].ts - imu[0].ts) as f64 / 1e9;
    let rate = (imu.len() - 1) as f64 / span;
    let n = ((window_s * rate) as usize).clamp(50, imu.len());
    let step = (n / 8).max(1);
    let mut best: Option<(f64, usize)> = None;
    let mut i = 0;
    while i + n <= imu.len() {
        let (_, s) = stats(&imu[i..i + n], |r| r.gyro);
        let score = s[0] * s[0] + s[1] * s[1] + s[2] * s[2];
        if best.is_none_or(|(b, _)| score < b) {
            best = Some((score, i));
        }
        i += step;
    }
    let (_, i0) = best.ok_or("no window")?;
    let w = &imu[i0..i0 + n];
    let (gm, gs) = stats(w, |r| r.gyro);
    let (am, as_) = stats(w, |r| r.accel);
    if gs.iter().any(|&s| s > 0.02) {
        return Err(format!(
            "no still segment: the quietest {window_s:.1} s still has gyro std {gs:.4?} rad/s (put the headset down for a few seconds)"
        ));
    }
    let d = ImuNoise::default();
    let sq = rate.sqrt();
    let noise = ImuNoise {
        accel_noise_std: [
            as_[0] / sq * inflate,
            as_[1] / sq * inflate,
            as_[2] / sq * inflate,
        ],
        gyro_noise_std: [
            gs[0] / sq * inflate,
            gs[1] / sq * inflate,
            gs[2] / sq * inflate,
        ],
        accel_bias_std: d.accel_bias_std,
        gyro_bias_std: d.gyro_bias_std,
        rate_hz: (rate / 100.0).round() * 100.0,
        source: String::new(),
    };
    Ok(NoiseEstimate {
        window: (w[0].ts, w[w.len() - 1].ts),
        rate_hz: rate,
        gyro_std: gs,
        accel_std: as_,
        gyro_mean: gm,
        accel_mean: am,
        noise,
    })
}

/// Gyro bias from the first still `window_s` of a recording (mean gyro, rad/s, recording = body frame), the way
/// the driver measures it at start-up: the first window whose per-axis standard deviation stays below 0.004 rad/s
/// (gyro) and 0.03 m/s² (accel). Returns (bias, window start ns) or `None` when the headset never lay still.
pub fn still_gyro_bias(imu: &[ImuRow], window_s: f64) -> Option<([f64; 3], i64)> {
    if imu.len() < 100 {
        return None;
    }
    let span = (imu[imu.len() - 1].ts - imu[0].ts) as f64 / 1e9;
    let rate = (imu.len() - 1) as f64 / span;
    let n = ((window_s * rate) as usize).clamp(50, imu.len());
    let step = ((0.1 * rate) as usize).max(1);
    let mut i = 0;
    while i + n <= imu.len() {
        let w = &imu[i..i + n];
        let (gm, gs) = stats(w, |r| r.gyro);
        let (_, as_) = stats(w, |r| r.accel);
        if gs.iter().all(|&s| s < 0.004) && as_.iter().all(|&s| s < 0.03) {
            return Some((gm, w[0].ts));
        }
        i += step;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic pseudo-random normal-ish noise (sum of 12 uniforms).
    fn noise(seed: &mut u64) -> f64 {
        let mut s = 0.0;
        for _ in 0..12 {
            *seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            s += (*seed >> 11) as f64 / (1u64 << 53) as f64;
        }
        s - 6.0
    }

    #[test]
    fn finds_still_window_and_densities() {
        let mut seed = 7u64;
        let mut v = Vec::new();
        for i in 0..10_000i64 {
            let moving = i < 4000; // first 4 s: head motion
            let amp = if moving { 0.5 } else { 0.002 };
            let g = [
                amp * noise(&mut seed),
                amp * noise(&mut seed),
                amp * noise(&mut seed),
            ];
            let a = [
                0.02 * noise(&mut seed),
                9.81 + 0.02 * noise(&mut seed),
                0.02 * noise(&mut seed),
            ];
            v.push(ImuRow {
                ts: i * 1_000_000,
                gyro: g,
                accel: a,
            });
        }
        let e = estimate(&v, 2.0, 1.0).unwrap();
        assert!(e.window.0 >= 4_000_000_000, "{:?}", e.window);
        for k in 0..3 {
            assert!((e.gyro_std[k] - 0.002).abs() < 0.0003, "{:?}", e.gyro_std);
            let dens = 0.002 / 1000f64.sqrt();
            assert!((e.noise.gyro_noise_std[k] - dens).abs() < dens * 0.2);
        }
        assert!((e.accel_mean[1] - 9.81).abs() < 0.01);
        assert_eq!(e.noise.rate_hz, 1000.0);
        // All motion: refused.
        let moving: Vec<ImuRow> = v[..4000].to_vec();
        assert!(estimate(&moving, 2.0, 1.0).is_err());
        assert!(still_gyro_bias(&moving, 1.0).is_none());
        // Still part found after the motion; mean = bias (zero here) within the noise.
        let (b, t) = still_gyro_bias(&v, 1.0).unwrap();
        assert!(t >= 4_000_000_000, "{t}");
        assert!(b.iter().all(|x| x.abs() < 0.0005), "{b:?}");
    }
}
