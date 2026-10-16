//! Non-PipeWire audio sources: WAV files (optionally paced in real time) and raw s16le from stdin.

use crate::vad::RATE;
use pimax_voicebench::resample::resample;
use pimax_voicebench::wav;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A WAV file as 16 kHz mono.
pub fn load_wav_16k(path: &Path) -> Result<Vec<f32>, String> {
    let w = wav::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mono = w.to_mono();
    Ok(if w.sample_rate == RATE {
        mono
    } else {
        resample(&mono, w.sample_rate, RATE)
    })
}

/// Write 16 kHz mono as a 16-bit WAV.
pub fn save_wav_16k(path: &Path, samples: &[f32]) -> Result<(), String> {
    wav::write_pcm16(path, samples, RATE, 1).map_err(|e| format!("{}: {e}", path.display()))
}

/// Feed `samples` in `chunk_ms` chunks; with `realtime` at the audio rate (as a microphone would). Calls `fed` with
/// the sample index after each chunk and the time it was sent. Stops early when `stop` is set or the receiver is gone.
pub fn feed(
    samples: &[f32],
    chunk_ms: u32,
    realtime: bool,
    tx: &Sender<Vec<f32>>,
    stop: &AtomicBool,
    mut fed: impl FnMut(usize, Instant),
) {
    let n = (RATE * chunk_ms / 1000).max(1) as usize;
    let t0 = Instant::now();
    let mut pos = 0;
    for c in samples.chunks(n) {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        if realtime {
            let due = t0 + Duration::from_secs_f64((pos + c.len()) as f64 / f64::from(RATE));
            let now = Instant::now();
            if due > now {
                std::thread::sleep(due - now);
            }
        }
        if tx.send(c.to_vec()).is_err() {
            return;
        }
        pos += c.len();
        fed(pos, Instant::now());
    }
}

/// Spawn a thread feeding a WAV file.
pub fn spawn_wav(
    path: &Path,
    realtime: bool,
    tx: Sender<Vec<f32>>,
    stop: Arc<AtomicBool>,
) -> Result<std::thread::JoinHandle<()>, String> {
    let samples = load_wav_16k(path)?;
    std::thread::Builder::new()
        .name("voice-wav".into())
        .spawn(move || feed(&samples, 20, realtime, &tx, &stop, |_, _| {}))
        .map_err(|e| e.to_string())
}

/// Convert little-endian s16 bytes to samples; returns the number of bytes used (even).
pub fn s16le_to_f32(bytes: &[u8], out: &mut Vec<f32>) -> usize {
    let n = bytes.len() / 2 * 2;
    out.extend(
        bytes[..n]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| f32::from(i16::from_le_bytes(*b)) / 32768.0),
    );
    n
}

/// Spawn a thread reading raw s16le 16 kHz mono from stdin (e.g. `pw-record --rate 16000 --channels 1 --format s16 -`).
pub fn spawn_stdin(
    tx: Sender<Vec<f32>>,
    stop: Arc<AtomicBool>,
) -> Result<std::thread::JoinHandle<()>, String> {
    std::thread::Builder::new()
        .name("voice-stdin".into())
        .spawn(move || {
            let mut stdin = std::io::stdin().lock();
            let mut buf = vec![0u8; 640];
            let mut carry: Vec<u8> = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                let Ok(k) = stdin.read(&mut buf) else { return };
                if k == 0 {
                    return;
                }
                carry.extend_from_slice(&buf[..k]);
                let mut out = Vec::with_capacity(carry.len() / 2);
                let used = s16le_to_f32(&carry, &mut out);
                carry.drain(..used);
                if tx.send(out).is_err() {
                    return;
                }
            }
        })
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn s16_conversion_keeps_odd_bytes() {
        let mut out = Vec::new();
        let used = s16le_to_f32(&[0x00, 0x80, 0xff, 0x7f, 0x01], &mut out);
        assert_eq!(used, 4);
        assert_eq!(out, vec![-1.0, 32767.0 / 32768.0]);
    }

    #[test]
    fn feed_paces_and_reports() {
        let (tx, rx) = mpsc::channel();
        let stop = AtomicBool::new(false);
        let s = vec![0.0f32; 16_000 / 5]; // 200 ms
        let t0 = Instant::now();
        let mut marks = Vec::new();
        feed(&s, 50, true, &tx, &stop, |p, _| marks.push(p));
        assert!(
            t0.elapsed() >= Duration::from_millis(190),
            "{:?}",
            t0.elapsed()
        );
        assert_eq!(marks, vec![800, 1600, 2400, 3200]);
        assert_eq!(rx.try_iter().map(|c| c.len()).sum::<usize>(), 3200);
        // not paced: immediate
        let t0 = Instant::now();
        feed(&vec![0.0; 16_000 * 10], 20, false, &tx, &stop, |_, _| {});
        assert!(t0.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn wav_round_trip_at_16k_and_resampled() {
        let dir = std::env::temp_dir().join(format!("pimax-voice-wav-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.wav");
        let tone: Vec<f32> = (0..16_000).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        save_wav_16k(&p, &tone).unwrap();
        let back = load_wav_16k(&p).unwrap();
        assert_eq!(back.len(), tone.len());
        assert!(back.iter().zip(&tone).all(|(a, b)| (a - b).abs() < 1e-3));
        // 48 kHz stereo file -> 16 kHz mono
        let st: Vec<f32> = (0..48_000 * 2)
            .map(|i| ((i / 2) as f32 * 0.01).sin() * 0.3)
            .collect();
        pimax_voicebench::wav::write_pcm16(&dir.join("b.wav"), &st, 48_000, 2).unwrap();
        let m = load_wav_16k(&dir.join("b.wav")).unwrap();
        assert!((m.len() as i64 - 16_000).abs() <= 2, "{}", m.len());
        assert!(load_wav_16k(&dir.join("missing.wav")).is_err());
    }
}
