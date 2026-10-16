//! Rational-ratio windowed-sinc (Kaiser) polyphase resampler.
//!
//! Used for: Kokoro 24 kHz -> 16 kHz (STT test clips), headset microphone 48/44.1 kHz -> 16 kHz (STT input) and
//! TTS 24 kHz -> 48 kHz (DisplayPort audio, which wants 48 kHz). One-shot ([`resample`]) and streaming
//! ([`Resampler::process`] / [`Resampler::finish`]) use the same code, so chunked and whole-buffer results are identical.

const ZERO_CROSSINGS: f64 = 32.0;
const KAISER_BETA: f64 = 9.0;
/// Cut-off as a fraction of the lower of the two Nyquist frequencies.
const CUTOFF: f64 = 0.96;

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

/// Modified Bessel function of the first kind, order 0 (power series).
pub fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let q = x * x / 4.0;
    for k in 1..60 {
        term *= q / (f64::from(k) * f64::from(k));
        sum += term;
        if term < sum * 1e-18 {
            break;
        }
    }
    sum
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 {
        1.0
    } else {
        (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
    }
}

/// Streaming rational resampler for mono `f32`.
pub struct Resampler {
    /// Output samples per `m` input samples, after reduction.
    l: usize,
    m: usize,
    half: usize,
    /// `l` phases of `2 * half` taps; taps for phase p cover inputs `n0 - half + 1 ..= n0 + half`.
    table: Vec<f32>,
    /// Input samples not yet fully consumed; `buf[0]` has absolute index `buf_start`.
    buf: Vec<f32>,
    buf_start: u64,
    /// Index of the next output sample to produce.
    next_out: u64,
    /// Total input samples pushed.
    pushed: u64,
}

impl Resampler {
    /// Creates a resampler converting `from` Hz to `to` Hz.
    ///
    /// # Panics
    /// Panics if either rate is zero.
    pub fn new(from: u32, to: u32) -> Resampler {
        assert!(from > 0 && to > 0, "sample rates must be non-zero");
        let g = gcd(from, to);
        let (l, m) = ((to / g) as usize, (from / g) as usize);
        // cut-off relative to the input Nyquist: 1.0 = pass everything up to it
        let c = (l as f64 / m as f64).min(1.0) * CUTOFF;
        let half = (ZERO_CROSSINGS / c).ceil() as usize;
        let mut table = vec![0.0f32; l * 2 * half];
        let i0b = bessel_i0(KAISER_BETA);
        for p in 0..l {
            let frac = p as f64 / l as f64;
            let mut taps = vec![0.0f64; 2 * half];
            for (k, tap) in taps.iter_mut().enumerate() {
                // input offset relative to n0: -half+1 ..= half; output time is n0 + frac
                let dt = (k as f64 - (half as f64 - 1.0)) - frac;
                let r = dt / half as f64;
                let w = if r.abs() >= 1.0 { 0.0 } else { bessel_i0(KAISER_BETA * (1.0 - r * r).sqrt()) / i0b };
                *tap = c * sinc(c * dt) * w;
            }
            let sum: f64 = taps.iter().sum();
            for (k, tap) in taps.iter().enumerate() {
                table[p * 2 * half + k] = (tap / sum) as f32;
            }
        }
        Resampler { l, m, half, table, buf: Vec::new(), buf_start: 0, next_out: 0, pushed: 0 }
    }

    /// Group delay of the streaming path in input samples (the first outputs correspond to this much earlier input).
    pub fn latency_input_samples(&self) -> usize {
        self.half
    }

    fn produce(&mut self, flush: bool, out: &mut Vec<f32>) {
        loop {
            let t = self.next_out * self.m as u64; // numerator over l
            let n0 = t / self.l as u64;
            let phase = (t % self.l as u64) as usize;
            let need_last = n0 + self.half as u64; // newest input index used
            if !flush && need_last >= self.pushed {
                break;
            }
            if flush && n0 >= self.pushed {
                break;
            }
            let taps = &self.table[phase * 2 * self.half..(phase + 1) * 2 * self.half];
            let first = n0 as i64 - (self.half as i64 - 1);
            let mut acc = 0.0f32;
            for (k, &tap) in taps.iter().enumerate() {
                let idx = first + k as i64;
                if idx >= self.buf_start as i64 {
                    let rel = (idx - self.buf_start as i64) as usize;
                    if rel < self.buf.len() {
                        acc += tap * self.buf[rel];
                    }
                }
            }
            out.push(acc);
            self.next_out += 1;
        }
        // drop input that no later output can use
        let t = self.next_out * self.m as u64;
        let keep_from = (t / self.l as u64).saturating_sub(self.half as u64);
        if keep_from > self.buf_start {
            let drop = ((keep_from - self.buf_start) as usize).min(self.buf.len());
            self.buf.drain(..drop);
            self.buf_start += drop as u64;
        }
    }

    /// Feeds input samples; returns the output samples that are complete.
    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        self.buf.extend_from_slice(input);
        self.pushed += input.len() as u64;
        let mut out = Vec::with_capacity(input.len() * self.l / self.m + 2);
        self.produce(false, &mut out);
        out
    }

    /// Ends the stream (treating the future as silence) and returns the remaining outputs, so that the total output
    /// length is `ceil(input_len * to / from)`.
    pub fn finish(&mut self) -> Vec<f32> {
        let mut out = Vec::new();
        self.produce(true, &mut out);
        out
    }
}

/// Resamples a whole buffer from `from` Hz to `to` Hz (linear phase, zero delay, zero padded at the edges).
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to {
        return input.to_vec();
    }
    let mut r = Resampler::new(from, to);
    let mut out = r.process(input);
    out.extend(r.finish());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn tone(freq: f64, rate: u32, n: usize, amp: f32) -> Vec<f32> {
        (0..n).map(|i| amp * (2.0 * PI * freq * i as f64 / f64::from(rate)).sin() as f32).collect()
    }

    /// Amplitude of the sinusoid at `freq` (Goertzel-like single bin over the central part of the signal).
    fn amp_at(x: &[f32], freq: f64, rate: u32) -> f64 {
        let n0 = x.len() / 8;
        let seg = &x[n0..x.len() - n0];
        let (mut re, mut im) = (0.0, 0.0);
        for (i, v) in seg.iter().enumerate() {
            let ph = 2.0 * PI * freq * (i + n0) as f64 / f64::from(rate);
            re += f64::from(*v) * ph.cos();
            im += f64::from(*v) * ph.sin();
        }
        2.0 * re.hypot(im) / seg.len() as f64
    }

    #[test]
    fn identity_ratio_copies() {
        let x = tone(440.0, 16000, 500, 0.5);
        assert_eq!(resample(&x, 16000, 16000), x);
    }

    #[test]
    fn output_length_is_ceil_of_ratio() {
        for (from, to, n) in [(24000, 16000, 24000), (48000, 16000, 4800), (24000, 48000, 1001), (44100, 48000, 44100), (16000, 24000, 7)] {
            let out = resample(&vec![0.1; n], from, to);
            let want = (n as u64 * u64::from(to)).div_ceil(u64::from(from)) as usize;
            assert_eq!(out.len(), want, "{from}->{to} n={n}");
        }
        assert!(resample(&[], 24000, 16000).is_empty());
    }

    #[test]
    fn dc_gain_is_unity() {
        for (from, to) in [(24000, 16000), (16000, 48000), (44100, 16000), (48000, 44100)] {
            let out = resample(&vec![0.7; from as usize / 2], from, to);
            let mid = out[out.len() / 2];
            assert!((mid - 0.7).abs() < 1e-4, "{from}->{to}: {mid}");
        }
    }

    #[test]
    fn in_band_tones_keep_amplitude_when_downsampling() {
        for (from, to, f) in [(24000, 16000, 1000.0), (24000, 16000, 6000.0), (48000, 16000, 3000.0), (44100, 16000, 6500.0)] {
            let x = tone(f, from, from as usize, 0.8);
            let y = resample(&x, from, to);
            let a = amp_at(&y, f, to);
            assert!((a - 0.8).abs() < 0.008, "{from}->{to} {f} Hz: amplitude {a}");
        }
    }

    #[test]
    fn out_of_band_tones_are_rejected_not_aliased() {
        // 9 kHz at 48k -> 16k would alias to 7 kHz without a proper low-pass
        let x = tone(9000.0, 48000, 48000, 0.8);
        let y = resample(&x, 48000, 16000);
        let alias = amp_at(&y, 7000.0, 16000);
        assert!(alias < 0.8 * 0.003, "alias amplitude {alias} (want < -50 dB)");
        // 10 kHz at 24k->16k aliases to 6 kHz
        let x = tone(10000.0, 24000, 24000, 0.8);
        let y = resample(&x, 24000, 16000);
        assert!(amp_at(&y, 6000.0, 16000) < 0.8 * 0.003);
    }

    #[test]
    fn upsampling_reproduces_the_sine() {
        let x = tone(2000.0, 24000, 4800, 0.5);
        let y = resample(&x, 24000, 48000);
        assert_eq!(y.len(), 9600);
        let mut max_err = 0.0f64;
        for (i, v) in y.iter().enumerate().skip(400).take(y.len() - 800) {
            let want = 0.5 * (2.0 * PI * 2000.0 * i as f64 / 48000.0).sin();
            max_err = max_err.max((f64::from(*v) - want).abs());
        }
        assert!(max_err < 1e-3, "max error {max_err}");
    }

    #[test]
    fn non_integer_ratio_44100_to_48000() {
        let x = tone(1234.0, 44100, 44100, 0.6);
        let y = resample(&x, 44100, 48000);
        assert!((amp_at(&y, 1234.0, 48000) - 0.6).abs() < 0.006);
    }

    #[test]
    fn streaming_chunks_equal_one_shot() {
        let x: Vec<f32> = (0..9000).map(|i| ((i * 37 % 101) as f32 / 50.0) - 1.0).collect();
        for (from, to) in [(24000, 16000), (16000, 48000), (44100, 48000)] {
            let whole = resample(&x, from, to);
            let mut r = Resampler::new(from, to);
            let mut chunked = Vec::new();
            let mut pos = 0;
            let mut size = 1;
            while pos < x.len() {
                let end = (pos + size).min(x.len());
                chunked.extend(r.process(&x[pos..end]));
                pos = end;
                size = size * 3 % 977 + 1;
            }
            chunked.extend(r.finish());
            assert_eq!(chunked.len(), whole.len(), "{from}->{to}");
            for (a, b) in whole.iter().zip(&chunked) {
                assert!((a - b).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn round_trip_24k_16k_24k_is_close_inside_the_band() {
        let x = tone(1500.0, 24000, 24000, 0.5);
        let back = resample(&resample(&x, 24000, 16000), 16000, 24000);
        assert_eq!(back.len(), x.len());
        let err: f64 = x.iter().zip(&back).skip(500).take(x.len() - 1000).map(|(a, b)| f64::from((a - b).abs())).fold(0.0, f64::max);
        assert!(err < 2e-3, "{err}");
    }

    #[test]
    fn bessel_i0_known_values() {
        assert!((bessel_i0(0.0) - 1.0).abs() < 1e-15);
        assert!((bessel_i0(1.0) - 1.266_065_877_752_008).abs() < 1e-12);
        assert!((bessel_i0(9.0) - 1_093.588_354_511_373).abs() < 1e-6);
    }

    #[test]
    #[should_panic(expected = "non-zero")]
    fn zero_rate_panics() {
        let _ = Resampler::new(0, 16000);
    }

    #[test]
    fn latency_is_reported() {
        let r = Resampler::new(48000, 16000);
        assert!(r.latency_input_samples() > 16);
        assert!(r.latency_input_samples() < 300);
    }
}
