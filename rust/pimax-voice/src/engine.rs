//! Speech recognisers behind one trait: a whole segment in, text out.
//!
//! * [`Parakeet`] (default): NVIDIA Parakeet TDT 0.6B v3 int8, multilingual (25 European languages incl. English and
//!   Spanish, no language setting needed), sherpa-onnx C API on the CPU.
//! * `Whisper` (cargo feature `whisper`): whisper.cpp through whisper-rs with Vulkan, the engine WayVR uses; for
//!   comparisons.
//! * [`FnEngine`]: a closure, for tests.

use pimax_voicebench::sherpa::{default_lib_path, Lib, Recognizer, SttConfig};
use std::path::{Path, PathBuf};

/// Result of one decode.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Transcript {
    pub text: String,
    /// Language code if the engine reports one (else the pipeline guesses from the text).
    pub lang: Option<String>,
}

/// A recogniser of whole segments (16 kHz mono, -1..1).
pub trait Engine {
    fn transcribe(&mut self, audio: &[f32]) -> Result<Transcript, String>;
}

/// Collapse whitespace and trim.
pub fn normalize_text(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Which engine to build, with its settings (built inside the decoding thread: the handles are not `Send`).
#[derive(Debug, Clone, PartialEq)]
pub enum EngineSpec {
    Parakeet {
        lib: PathBuf,
        model_dir: PathBuf,
        threads: i32,
    },
    #[cfg(feature = "whisper")]
    Whisper {
        model: PathBuf,
        threads: i32,
        gpu: bool,
        /// `None` = auto-detect.
        lang: Option<String>,
    },
}

/// Default Parakeet model directory (`~/models/parakeet-tdt-0.6b-v3-int8`).
pub fn default_model_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join("models/parakeet-tdt-0.6b-v3-int8")
}

impl EngineSpec {
    /// Parakeet v3 with the default library and model locations.
    pub fn parakeet_default(threads: i32) -> EngineSpec {
        EngineSpec::Parakeet {
            lib: default_lib_path(),
            model_dir: default_model_dir(),
            threads,
        }
    }

    /// Short name for logs and the `hello` event.
    pub fn name(&self) -> String {
        match self {
            EngineSpec::Parakeet {
                model_dir, threads, ..
            } => {
                let m = model_dir
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                format!("{m} (sherpa-onnx cpu, {threads} threads)")
            }
            #[cfg(feature = "whisper")]
            EngineSpec::Whisper {
                model,
                threads,
                gpu,
                ..
            } => {
                let m = model
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                format!(
                    "{m} (whisper.cpp {}, {threads} threads)",
                    if *gpu { "vulkan" } else { "cpu" }
                )
            }
        }
    }

    /// The sherpa-onnx library this spec uses (also needed by the Silero VAD), if any.
    pub fn sherpa_lib(&self) -> Option<&Path> {
        match self {
            EngineSpec::Parakeet { lib, .. } => Some(lib),
            #[cfg(feature = "whisper")]
            EngineSpec::Whisper { .. } => None,
        }
    }

    /// Load the models.
    pub fn build(&self) -> Result<Box<dyn Engine>, String> {
        match self {
            EngineSpec::Parakeet {
                lib,
                model_dir,
                threads,
            } => Ok(Box::new(Parakeet::load(lib, model_dir, *threads)?)),
            #[cfg(feature = "whisper")]
            EngineSpec::Whisper {
                model,
                threads,
                gpu,
                lang,
            } => Ok(Box::new(whisper::Whisper::load(
                model,
                *threads,
                *gpu,
                lang.clone(),
            )?)),
        }
    }
}

/// Parakeet TDT through sherpa-onnx.
pub struct Parakeet {
    rec: Recognizer,
}

impl Parakeet {
    pub fn load(lib: &Path, model_dir: &Path, threads: i32) -> Result<Parakeet, String> {
        let lib = Lib::load(lib).map_err(|e| format!("sherpa-onnx library: {e}"))?;
        let cfg = SttConfig::from_dir(model_dir, threads.max(1));
        let rec = Recognizer::new(&lib, &cfg)
            .map_err(|e| format!("Parakeet model {}: {e}", model_dir.display()))?;
        Ok(Parakeet { rec })
    }
}

impl Engine for Parakeet {
    fn transcribe(&mut self, audio: &[f32]) -> Result<Transcript, String> {
        let text = self
            .rec
            .transcribe(audio, 16_000)
            .map_err(|e| e.to_string())?;
        Ok(Transcript {
            text: normalize_text(&text),
            lang: None,
        })
    }
}

/// A closure as an engine (tests, simulations).
pub struct FnEngine<F: FnMut(&[f32]) -> String>(pub F);

impl<F: FnMut(&[f32]) -> String> Engine for FnEngine<F> {
    fn transcribe(&mut self, audio: &[f32]) -> Result<Transcript, String> {
        Ok(Transcript {
            text: (self.0)(audio),
            lang: None,
        })
    }
}

#[cfg(feature = "whisper")]
pub mod whisper {
    //! whisper.cpp (whisper-rs 0.16, Vulkan build), set up like WayVR's `whisper_stt.rs`: greedy, no timestamps.
    use super::{normalize_text, Engine, Transcript};
    use std::path::Path;
    use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

    pub struct Whisper {
        ctx: WhisperContext,
        threads: i32,
        lang: Option<String>,
    }

    impl Whisper {
        pub fn load(
            model: &Path,
            threads: i32,
            gpu: bool,
            lang: Option<String>,
        ) -> Result<Whisper, String> {
            let params = WhisperContextParameters {
                use_gpu: gpu,
                ..Default::default()
            };
            let ctx = WhisperContext::new_with_params(&model.to_string_lossy(), params)
                .map_err(|e| format!("whisper model {}: {e}", model.display()))?;
            Ok(Whisper {
                ctx,
                threads: threads.max(1),
                lang,
            })
        }
    }

    impl Engine for Whisper {
        fn transcribe(&mut self, audio: &[f32]) -> Result<Transcript, String> {
            let mut p = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
            p.set_n_threads(self.threads);
            p.set_language(Some(self.lang.as_deref().unwrap_or("auto")));
            p.set_no_timestamps(true);
            p.set_print_special(false);
            p.set_print_progress(false);
            p.set_print_realtime(false);
            p.set_print_timestamps(false);
            p.set_suppress_blank(true);
            p.set_no_context(true);
            let mut st = self.ctx.create_state().map_err(|e| e.to_string())?;
            // whisper wants at least 1 s of audio
            let mut buf;
            let audio = if audio.len() < 16_000 {
                buf = audio.to_vec();
                buf.resize(16_100, 0.0);
                &buf[..]
            } else {
                audio
            };
            st.full(p, audio).map_err(|e| e.to_string())?;
            let text: String = st.as_iter().map(|s| s.to_string()).collect();
            let lang = st
                .full_lang_id_from_state()
                .ok()
                .and_then(whisper_rs::get_lang_str)
                .map(str::to_string);
            Ok(Transcript {
                text: normalize_text(&text),
                lang,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_is_normalised() {
        assert_eq!(
            normalize_text("  Open   the\tbrowser. \n"),
            "Open the browser."
        );
        assert_eq!(normalize_text(""), "");
    }

    #[test]
    fn spec_names_and_missing_files() {
        let s = EngineSpec::Parakeet {
            lib: "/x/lib.so".into(),
            model_dir: "/m/parakeet-tdt-0.6b-v3-int8".into(),
            threads: 2,
        };
        assert_eq!(
            s.name(),
            "parakeet-tdt-0.6b-v3-int8 (sherpa-onnx cpu, 2 threads)"
        );
        assert_eq!(s.sherpa_lib(), Some(Path::new("/x/lib.so")));
        let e = s.build().err().unwrap();
        assert!(e.contains("/x/lib.so"), "{e}");
        assert!(default_model_dir().ends_with("models/parakeet-tdt-0.6b-v3-int8"));
    }

    #[test]
    fn fn_engine() {
        let mut e = FnEngine(|a: &[f32]| format!("{} samples", a.len()));
        assert_eq!(e.transcribe(&[0.0; 3]).unwrap().text, "3 samples");
    }
}
