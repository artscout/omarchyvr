//! Voice activity detection on 32 ms frames (512 samples at 16 kHz): a speech / non-speech decision per frame.
//! The hysteresis (minimum speech, hangover, pre-roll) is the segmenter's job ([`crate::segment`]).
//!
//! * [`SileroVad`]: Silero VAD through the sherpa-onnx C API (the library the recogniser already loads), 1 thread.
//! * [`EnergyVad`]: RMS level above an adaptive noise floor; no model, deterministic (tests, fallback).

use std::ffi::{c_char, CString};
use std::path::{Path, PathBuf};

/// Samples per frame (Silero's window at 16 kHz).
pub const FRAME: usize = 512;
/// Sample rate of everything after capture.
pub const RATE: u32 = 16_000;
/// Frame duration in milliseconds.
pub const FRAME_MS: f64 = FRAME as f64 * 1000.0 / RATE as f64;

/// Per-frame speech detector.
pub trait Vad {
    /// `frame` has exactly [`FRAME`] samples in -1..1.
    fn is_speech(&mut self, frame: &[f32]) -> bool;
    /// Forget the internal state (after a pause or a forced split).
    fn reset(&mut self);
    fn name(&self) -> &'static str;
}

/// RMS level in dBFS (-120 for digital silence).
pub fn rms_db(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return -120.0;
    }
    let e: f64 = samples
        .iter()
        .map(|&s| f64::from(s) * f64::from(s))
        .sum::<f64>()
        / samples.len() as f64;
    if e <= 1e-12 {
        -120.0
    } else {
        (10.0 * e.log10()) as f32
    }
}

/// Speech = level more than `margin_db` above the tracked noise floor and above `min_db`.
#[derive(Debug, Clone)]
pub struct EnergyVad {
    /// Tracked noise floor, dBFS.
    pub floor_db: f32,
    pub margin_db: f32,
    pub min_db: f32,
    /// How fast the floor creeps up per frame while the level is above it (dB).
    pub rise_db: f32,
    /// The floor starts at the first frame's level (the stream usually starts in silence; if not, it falls
    /// quickly at the first pause).
    pub started: bool,
}

impl Default for EnergyVad {
    fn default() -> Self {
        EnergyVad {
            floor_db: -70.0,
            margin_db: 12.0,
            min_db: -50.0,
            rise_db: 0.05,
            started: false,
        }
    }
}

impl Vad for EnergyVad {
    fn is_speech(&mut self, frame: &[f32]) -> bool {
        let db = rms_db(frame);
        if !self.started {
            self.started = true;
            // ... but never above a quiet room's level, so speech right at the start is still speech
            self.floor_db = (db - 3.0).clamp(-100.0, -55.0);
        }
        let speech = db > self.floor_db + self.margin_db && db > self.min_db;
        if db < self.floor_db {
            // follow a falling floor quickly
            self.floor_db = 0.5 * (self.floor_db + db);
        } else if !speech {
            self.floor_db += 0.1 * (db - self.floor_db).min(1.0);
        } else {
            // constant loud noise is not speech forever: creep up slowly
            self.floor_db += self.rise_db;
        }
        self.floor_db = self.floor_db.max(-100.0);
        speech
    }

    fn reset(&mut self) {}

    fn name(&self) -> &'static str {
        "energy"
    }
}

/// A VAD whose speech frames must also be `gate_db` above the tracked noise floor. Silero keeps "speech" while its
/// probability stays above threshold - 0.15, which drags 150-500 ms of room noise onto the end of every utterance;
/// the energy gate cuts that tail, so the end of speech (and the transcript) comes sooner.
pub struct Gated<V: Vad> {
    pub inner: V,
    floor: EnergyVad,
}

impl<V: Vad> Gated<V> {
    pub fn new(inner: V, gate_db: f32) -> Gated<V> {
        Gated {
            inner,
            floor: EnergyVad {
                margin_db: gate_db,
                min_db: -100.0,
                ..EnergyVad::default()
            },
        }
    }
}

impl<V: Vad> Vad for Gated<V> {
    fn is_speech(&mut self, frame: &[f32]) -> bool {
        let loud = self.floor.is_speech(frame);
        // always run the inner model: it keeps its context
        self.inner.is_speech(frame) && loud
    }

    fn reset(&mut self) {
        self.inner.reset();
    }

    fn name(&self) -> &'static str {
        "silero+gate"
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SileroCfg {
    model: *const c_char,
    threshold: f32,
    min_silence_duration: f32,
    min_speech_duration: f32,
    window_size: i32,
    max_speech_duration: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VadCfg {
    silero_vad: SileroCfg,
    sample_rate: i32,
    num_threads: i32,
    provider: *const c_char,
    debug: i32,
    ten_vad: SileroCfg, // same layout as SherpaOnnxTenVadModelConfig
}

// Layouts checked against sherpa-onnx 1.13.8 c-api.h with a C program (sizeof/offsetof).
const _: () = {
    assert!(std::mem::size_of::<SileroCfg>() == 32);
    assert!(std::mem::offset_of!(SileroCfg, max_speech_duration) == 24);
    assert!(std::mem::size_of::<VadCfg>() == 88);
    assert!(std::mem::offset_of!(VadCfg, provider) == 40);
    assert!(std::mem::offset_of!(VadCfg, ten_vad) == 56);
};

type Opaque = std::ffi::c_void;

struct VadApi {
    create: unsafe extern "C" fn(*const VadCfg, f32) -> *const Opaque,
    destroy: unsafe extern "C" fn(*const Opaque),
    accept: unsafe extern "C" fn(*const Opaque, *const f32, i32),
    detected: unsafe extern "C" fn(*const Opaque) -> i32,
    empty: unsafe extern "C" fn(*const Opaque) -> i32,
    clear: unsafe extern "C" fn(*const Opaque),
    reset: unsafe extern "C" fn(*const Opaque),
    _lib: libloading::Library,
}

/// Silero VAD (sherpa-onnx `SherpaOnnxVoiceActivityDetector`), used frame by frame through `Detected()`.
pub struct SileroVad {
    api: VadApi,
    ptr: *const Opaque,
    _model: CString,
    _provider: CString,
}

/// Default Silero model location (`~/models/vad/silero_vad.onnx`).
pub fn default_silero_model() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join("models/vad/silero_vad.onnx")
}

impl SileroVad {
    /// Load the detector from the sherpa-onnx C API library `lib` and the Silero model `model`.
    /// `threshold`: speech probability threshold (0.5 is Silero's default).
    pub fn new(lib: &Path, model: &Path, threshold: f32) -> Result<SileroVad, String> {
        if !model.exists() {
            return Err(format!("missing VAD model {}", model.display()));
        }
        // SAFETY: loading the trusted sherpa-onnx library (already loaded by the recogniser); the symbol types
        // match c-api.h of 1.13.8.
        let api = unsafe {
            let l = libloading::Library::new(lib).map_err(|e| format!("{}: {e}", lib.display()))?;
            fn sym<T: Copy>(l: &libloading::Library, name: &[u8]) -> Result<T, String> {
                // SAFETY: the caller names the C type of the symbol.
                unsafe {
                    l.get::<T>(name)
                        .map(|s| *s)
                        .map_err(|e| format!("{}: {e}", String::from_utf8_lossy(name)))
                }
            }
            VadApi {
                create: sym(&l, b"SherpaOnnxCreateVoiceActivityDetector")?,
                destroy: sym(&l, b"SherpaOnnxDestroyVoiceActivityDetector")?,
                accept: sym(&l, b"SherpaOnnxVoiceActivityDetectorAcceptWaveform")?,
                detected: sym(&l, b"SherpaOnnxVoiceActivityDetectorDetected")?,
                empty: sym(&l, b"SherpaOnnxVoiceActivityDetectorEmpty")?,
                clear: sym(&l, b"SherpaOnnxVoiceActivityDetectorClear")?,
                reset: sym(&l, b"SherpaOnnxVoiceActivityDetectorReset")?,
                _lib: l,
            }
        };
        let model_c =
            CString::new(model.to_string_lossy().as_bytes()).map_err(|_| "NUL in model path")?;
        let provider = CString::new("cpu").expect("no NUL");
        let silero = SileroCfg {
            model: model_c.as_ptr(),
            threshold,
            // Short internal hysteresis: Detected() then follows the frames closely; ours is in the segmenter.
            min_silence_duration: 0.064,
            min_speech_duration: 0.064,
            window_size: FRAME as i32,
            max_speech_duration: 60.0,
        };
        let cfg = VadCfg {
            silero_vad: silero,
            sample_rate: RATE as i32,
            num_threads: 1,
            provider: provider.as_ptr(),
            debug: 0,
            ten_vad: SileroCfg {
                model: std::ptr::null(),
                ..silero
            },
        };
        // SAFETY: cfg and its strings outlive the call; the library copies the configuration.
        let ptr = unsafe { (api.create)(&cfg, 30.0) };
        if ptr.is_null() {
            return Err("SherpaOnnxCreateVoiceActivityDetector returned NULL (see stderr)".into());
        }
        Ok(SileroVad {
            api,
            ptr,
            _model: model_c,
            _provider: provider,
        })
    }
}

// SAFETY: the detector is a heap object of the C++ library without thread affinity; `&mut self` methods mean it is
// used by one thread at a time, so moving it to another thread is fine.
unsafe impl Send for SileroVad {}

impl Vad for SileroVad {
    fn is_speech(&mut self, frame: &[f32]) -> bool {
        debug_assert_eq!(frame.len(), FRAME);
        // SAFETY: valid handle, `frame` is valid for its length.
        unsafe {
            (self.api.accept)(self.ptr, frame.as_ptr(), frame.len() as i32);
            // the detector queues closed segments; we only use the per-window decision
            if (self.api.empty)(self.ptr) == 0 {
                (self.api.clear)(self.ptr);
            }
            (self.api.detected)(self.ptr) != 0
        }
    }

    fn reset(&mut self) {
        // SAFETY: valid handle.
        unsafe { (self.api.reset)(self.ptr) }
    }

    fn name(&self) -> &'static str {
        "silero"
    }
}

impl Drop for SileroVad {
    fn drop(&mut self) {
        // SAFETY: created by this library, destroyed once.
        unsafe { (self.api.destroy)(self.ptr) }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Deterministic test signal: `n` samples of a vowel-like harmonic tone at amplitude `amp`.
    pub fn tone(n: usize, amp: f32, seed: u32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let t = i as f32 / RATE as f32;
                let f0 = 140.0 + (seed % 7) as f32 * 10.0;
                amp * ((2.0 * std::f32::consts::PI * f0 * t).sin()
                    + 0.5 * (2.0 * std::f32::consts::PI * 2.0 * f0 * t).sin()
                    + 0.25 * (2.0 * std::f32::consts::PI * 3.0 * f0 * t).sin())
                    / 1.75
            })
            .collect()
    }

    /// Low white-ish noise from an LCG.
    pub fn noise(n: usize, amp: f32, seed: u32) -> Vec<f32> {
        let mut s = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
        (0..n)
            .map(|_| {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                amp * ((s >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0)
            })
            .collect()
    }

    fn frames(v: &[f32]) -> impl Iterator<Item = &[f32]> {
        v.as_chunks::<FRAME>().0.iter().map(|f| &f[..])
    }

    #[test]
    fn rms_db_values() {
        assert_eq!(rms_db(&[]), -120.0);
        assert_eq!(rms_db(&[0.0; 512]), -120.0);
        assert!((rms_db(&[1.0; 512]) - 0.0).abs() < 1e-4);
        assert!((rms_db(&[0.1; 512]) - -20.0).abs() < 1e-3);
    }

    #[test]
    fn energy_vad_separates_tone_from_noise_and_adapts() {
        let mut v = EnergyVad::default();
        let quiet = noise(FRAME * 60, 0.002, 1);
        let speech: usize = frames(&quiet).filter(|f| v.is_speech(f)).count();
        assert_eq!(speech, 0, "noise is not speech");
        let loud = tone(FRAME * 30, 0.2, 2);
        let hits = frames(&loud).filter(|f| v.is_speech(f)).count();
        assert!(hits >= 29, "{hits}");
        // back to noise: silence again at once
        let after = noise(FRAME * 10, 0.002, 3);
        assert_eq!(frames(&after).filter(|f| v.is_speech(f)).count(), 0);
        // a much louder but constant noise floor is learned within a few seconds
        let hum = noise(FRAME * 600, 0.05, 4);
        let flags: Vec<bool> = frames(&hum).map(|f| v.is_speech(f)).collect();
        assert!(flags[0], "a sudden loud noise is speech at first");
        let late = flags[450..].iter().filter(|&&s| s).count();
        assert_eq!(late, 0, "constant noise is learned as floor");
    }

    #[test]
    fn energy_vad_ignores_quiet_signals_below_min_db() {
        let mut v = EnergyVad::default();
        for f in frames(&vec![0.0; FRAME * 20]) {
            assert!(!v.is_speech(f));
        }
        // -60 dBFS tone above a digital-silence floor is still below min_db
        let faint = tone(FRAME * 10, 0.001, 5);
        assert_eq!(frames(&faint).filter(|f| v.is_speech(f)).count(), 0);
    }

    struct Always;
    impl Vad for Always {
        fn is_speech(&mut self, _: &[f32]) -> bool {
            true
        }
        fn reset(&mut self) {}
        fn name(&self) -> &'static str {
            "always"
        }
    }

    #[test]
    fn gate_cuts_frames_near_the_noise_floor() {
        let mut g = Gated::new(Always, 6.0);
        let quiet = noise(FRAME * 40, 0.002, 1);
        let n = frames(&quiet).filter(|f| g.is_speech(f)).count();
        assert!(n <= 2, "{n}");
        let loud = tone(FRAME * 20, 0.05, 2);
        assert_eq!(frames(&loud).filter(|f| g.is_speech(f)).count(), 20);
        let tail = noise(FRAME * 10, 0.002, 3);
        assert_eq!(frames(&tail).filter(|f| g.is_speech(f)).count(), 0);
        assert_eq!(g.name(), "silero+gate");
    }

    #[test]
    fn silero_missing_files_are_errors() {
        let e = SileroVad::new(
            Path::new("/nonexistent/lib.so"),
            Path::new("/nonexistent/m.onnx"),
            0.5,
        )
        .err()
        .unwrap();
        assert!(e.contains("missing VAD model"), "{e}");
    }
}
