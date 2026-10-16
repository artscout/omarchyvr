//! Safe wrappers over the sherpa-onnx C API: offline (non-streaming) recognizer for NeMo transducers such as Parakeet TDT, and
//! offline TTS for Kokoro. The library is `dlopen`ed (`libsherpa-onnx-c-api.so` + its `libonnxruntime.so` next to it), so this
//! crate builds without any native dependency. CPU provider only.

use crate::ffi;
use std::ffi::{c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Error with a human readable message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

type Result<T> = std::result::Result<T, Error>;

/// Default location of the C API library installed by `pip install sherpa-onnx-core` into `~/src/venv-voice`.
pub fn default_lib_path() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join("src/venv-voice/lib/python3.14/site-packages/sherpa_onnx/lib/libsherpa-onnx-c-api.so")
}

/// The loaded C API.
pub struct Lib {
    api: ffi::SherpaOnnxLib,
}

impl Lib {
    /// Loads the library at `path`.
    pub fn load(path: &Path) -> Result<Arc<Lib>> {
        // SAFETY: loading a shared library runs its initialisers; the library is the trusted sherpa-onnx build.
        let api = unsafe { ffi::SherpaOnnxLib::new(path) }.map_err(|e| Error(format!("{}: {e}", path.display())))?;
        Ok(Arc::new(Lib { api }))
    }

    /// Library version string.
    pub fn version(&self) -> String {
        // SAFETY: returns a static NUL-terminated string.
        unsafe { CStr::from_ptr(self.api.SherpaOnnxGetVersionStr()) }.to_string_lossy().into_owned()
    }

    /// Git SHA of the library build.
    pub fn git_sha(&self) -> String {
        // SAFETY: as above.
        unsafe { CStr::from_ptr(self.api.SherpaOnnxGetGitSha1()) }.to_string_lossy().into_owned()
    }
}

fn cstr(s: &str) -> Result<CString> {
    CString::new(s).map_err(|_| Error(format!("string contains a NUL byte: {s:?}")))
}

fn path_cstr(p: &Path) -> Result<CString> {
    if !p.exists() {
        return Err(Error(format!("missing file: {}", p.display())));
    }
    cstr(&p.to_string_lossy())
}

/// Files of a NeMo transducer (Parakeet TDT, sherpa-onnx export).
#[derive(Debug, Clone)]
pub struct SttConfig {
    pub encoder: PathBuf,
    pub decoder: PathBuf,
    pub joiner: PathBuf,
    pub tokens: PathBuf,
    pub threads: i32,
    /// Hotwords (one per line) and their score: switches decoding to `modified_beam_search` with a BPE vocabulary
    /// (`bpe_vocab`: `token<TAB>-id` per line, built from `tokens.txt`). None: greedy search.
    pub hotwords: Option<Hotwords>,
}

/// Contextual biasing for a transducer (docs/AI.md section 17).
#[derive(Debug, Clone, PartialEq)]
pub struct Hotwords {
    pub file: PathBuf,
    pub bpe_vocab: PathBuf,
    pub score: f32,
}

/// The sentencepiece-style vocabulary sherpa-onnx wants for BPE hotwords, from a `tokens.txt` (`token id` per line).
pub fn bpe_vocab_from_tokens(tokens: &str) -> String {
    let mut o = String::new();
    for l in tokens.lines() {
        if let Some((t, i)) = l.rsplit_once(' ') {
            if let Ok(i) = i.trim().parse::<i64>() {
                o += &format!("{t}\t-{i}\n");
            }
        }
    }
    o
}

impl SttConfig {
    /// Standard layout of the `sherpa-onnx-nemo-parakeet-tdt-0.6b-*-int8` directories.
    pub fn from_dir(dir: &Path, threads: i32) -> SttConfig {
        SttConfig {
            encoder: dir.join("encoder.int8.onnx"),
            decoder: dir.join("decoder.int8.onnx"),
            joiner: dir.join("joiner.int8.onnx"),
            tokens: dir.join("tokens.txt"),
            threads,
            hotwords: None,
        }
    }
}

/// Offline recognizer (CPU).
pub struct Recognizer {
    lib: Arc<Lib>,
    ptr: *const ffi::SherpaOnnxOfflineRecognizer,
}

impl Recognizer {
    /// Loads the models.
    pub fn new(lib: &Arc<Lib>, cfg: &SttConfig) -> Result<Recognizer> {
        let enc = path_cstr(&cfg.encoder)?;
        let dec = path_cstr(&cfg.decoder)?;
        let joi = path_cstr(&cfg.joiner)?;
        let tok = path_cstr(&cfg.tokens)?;
        let provider = cstr("cpu")?;
        let model_type = cstr("nemo_transducer")?;
        let method = cstr(if cfg.hotwords.is_some() { "modified_beam_search" } else { "greedy_search" })?;
        let hot = cfg.hotwords.as_ref().map(|h| Ok::<_, Error>((path_cstr(&h.file)?, path_cstr(&h.bpe_vocab)?, cstr("bpe")?))).transpose()?;
        let mut c = ffi::SherpaOnnxOfflineRecognizerConfig::default();
        c.feat_config.sample_rate = 16000;
        c.feat_config.feature_dim = 80;
        c.model_config.transducer.encoder = enc.as_ptr();
        c.model_config.transducer.decoder = dec.as_ptr();
        c.model_config.transducer.joiner = joi.as_ptr();
        c.model_config.tokens = tok.as_ptr();
        c.model_config.num_threads = cfg.threads;
        c.model_config.provider = provider.as_ptr();
        c.model_config.model_type = model_type.as_ptr();
        c.decoding_method = method.as_ptr();
        if let (Some(h), Some((file, vocab, unit))) = (&cfg.hotwords, &hot) {
            c.hotwords_file = file.as_ptr();
            c.hotwords_score = h.score;
            c.model_config.modeling_unit = unit.as_ptr();
            c.model_config.bpe_vocab = vocab.as_ptr();
        }
        // SAFETY: `c` points to NUL-terminated strings that outlive the call; the library copies what it needs.
        let ptr = unsafe { lib.api.SherpaOnnxCreateOfflineRecognizer(&c) };
        if ptr.is_null() {
            return Err(Error("SherpaOnnxCreateOfflineRecognizer returned NULL (see stderr)".into()));
        }
        Ok(Recognizer { lib: Arc::clone(lib), ptr })
    }

    /// Transcribes one utterance of mono samples (-1..1) at `sample_rate`.
    pub fn transcribe(&self, samples: &[f32], sample_rate: u32) -> Result<String> {
        let n = i32::try_from(samples.len()).map_err(|_| Error("clip too long".into()))?;
        let api = &self.lib.api;
        // SAFETY: stream/result handles come from this library and are destroyed before returning; `samples` is valid for `n` floats.
        unsafe {
            let stream = api.SherpaOnnxCreateOfflineStream(self.ptr);
            if stream.is_null() {
                return Err(Error("SherpaOnnxCreateOfflineStream returned NULL".into()));
            }
            api.SherpaOnnxAcceptWaveformOffline(stream, sample_rate as i32, samples.as_ptr(), n);
            api.SherpaOnnxDecodeOfflineStream(self.ptr, stream);
            let res = api.SherpaOnnxGetOfflineStreamResult(stream);
            let text = if res.is_null() || (*res).text.is_null() {
                String::new()
            } else {
                CStr::from_ptr((*res).text).to_string_lossy().into_owned()
            };
            if !res.is_null() {
                api.SherpaOnnxDestroyOfflineRecognizerResult(res);
            }
            api.SherpaOnnxDestroyOfflineStream(stream);
            Ok(text)
        }
    }
}

impl Drop for Recognizer {
    fn drop(&mut self) {
        // SAFETY: `ptr` was created by this library and is destroyed once.
        unsafe { self.lib.api.SherpaOnnxDestroyOfflineRecognizer(self.ptr) }
    }
}

/// Kokoro model files and language handling.
#[derive(Debug, Clone)]
pub struct TtsConfig {
    pub model: PathBuf,
    pub voices: PathBuf,
    pub tokens: PathBuf,
    /// espeak-ng-data directory.
    pub data_dir: PathBuf,
    /// Comma separated lexicon files (`lexicon-us-en.txt,lexicon-zh.txt`), empty to use espeak-ng for everything.
    pub lexicon: String,
    /// espeak-ng voice for the whole text (`es`, `en-us`), empty for the model default.
    pub lang: String,
    pub threads: i32,
    /// Sentences per chunk (and per callback): 1 gives the lowest time to first audio.
    pub max_num_sentences: i32,
}

impl TtsConfig {
    /// Standard layout of `kokoro-multi-lang-v1_0`.
    pub fn from_dir(dir: &Path, lexicon: &str, lang: &str, threads: i32) -> TtsConfig {
        TtsConfig {
            model: dir.join("model.onnx"),
            voices: dir.join("voices.bin"),
            tokens: dir.join("tokens.txt"),
            data_dir: dir.join("espeak-ng-data"),
            lexicon: lexicon
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|f| dir.join(f).to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(","),
            lang: lang.to_string(),
            threads,
            max_num_sentences: 1,
        }
    }
}

/// Generated speech.
#[derive(Debug, Clone, PartialEq)]
pub struct Audio {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

/// Offline TTS (CPU).
pub struct Tts {
    lib: Arc<Lib>,
    ptr: *const ffi::SherpaOnnxOfflineTts,
}

type ChunkCallback<'a> = dyn FnMut(&[f32]) -> bool + 'a;

unsafe extern "C" fn trampoline(samples: *const f32, n: i32, _progress: f32, arg: *mut c_void) -> i32 {
    // SAFETY: `arg` is the address of a `&mut ChunkCallback` that lives for the duration of the generate call.
    let cb = unsafe { &mut *arg.cast::<&mut ChunkCallback<'_>>() };
    let chunk = if samples.is_null() || n <= 0 { &[][..] } else { unsafe { std::slice::from_raw_parts(samples, n as usize) } };
    i32::from(cb(chunk))
}

impl Tts {
    /// Loads the model.
    pub fn new(lib: &Arc<Lib>, cfg: &TtsConfig) -> Result<Tts> {
        let model = path_cstr(&cfg.model)?;
        let voices = path_cstr(&cfg.voices)?;
        let tokens = path_cstr(&cfg.tokens)?;
        let data_dir = path_cstr(&cfg.data_dir)?;
        let lexicon = cstr(&cfg.lexicon)?;
        let lang = cstr(&cfg.lang)?;
        let provider = cstr("cpu")?;
        let mut c = ffi::SherpaOnnxOfflineTtsConfig::default();
        c.model.kokoro.model = model.as_ptr();
        c.model.kokoro.voices = voices.as_ptr();
        c.model.kokoro.tokens = tokens.as_ptr();
        c.model.kokoro.data_dir = data_dir.as_ptr();
        c.model.kokoro.length_scale = 1.0;
        if !cfg.lexicon.is_empty() {
            c.model.kokoro.lexicon = lexicon.as_ptr();
        }
        if !cfg.lang.is_empty() {
            c.model.kokoro.lang = lang.as_ptr();
        }
        c.model.num_threads = cfg.threads;
        c.model.provider = provider.as_ptr();
        c.max_num_sentences = cfg.max_num_sentences;
        c.silence_scale = 0.2;
        // SAFETY: as in `Recognizer::new`.
        let ptr = unsafe { lib.api.SherpaOnnxCreateOfflineTts(&c) };
        if ptr.is_null() {
            return Err(Error("SherpaOnnxCreateOfflineTts returned NULL (see stderr)".into()));
        }
        Ok(Tts { lib: Arc::clone(lib), ptr })
    }

    /// Output sample rate (24000 for Kokoro).
    pub fn sample_rate(&self) -> u32 {
        // SAFETY: valid handle.
        unsafe { self.lib.api.SherpaOnnxOfflineTtsSampleRate(self.ptr) as u32 }
    }

    /// Number of speaker ids.
    pub fn num_speakers(&self) -> i32 {
        // SAFETY: valid handle.
        unsafe { self.lib.api.SherpaOnnxOfflineTtsNumSpeakers(self.ptr) }
    }

    /// Synthesises `text`; `on_chunk` receives each chunk of samples as soon as it is ready and returns `false` to stop.
    pub fn generate(&self, text: &str, speaker: i32, speed: f32, on_chunk: &mut ChunkCallback<'_>) -> Result<Audio> {
        let text = cstr(text)?;
        let gc = ffi::SherpaOnnxGenerationConfig { silence_scale: 0.2, speed, sid: speaker, ..Default::default() };
        let mut cb_ref: &mut ChunkCallback<'_> = on_chunk;
        let arg = (&mut cb_ref as *mut &mut ChunkCallback<'_>).cast::<c_void>();
        // SAFETY: `trampoline` only dereferences `arg`, which points at `cb_ref` on this stack frame for the whole call.
        unsafe {
            let audio = self.lib.api.SherpaOnnxOfflineTtsGenerateWithConfig(self.ptr, text.as_ptr(), &gc, Some(trampoline), arg);
            if audio.is_null() {
                return Err(Error("SherpaOnnxOfflineTtsGenerateWithConfig returned NULL".into()));
            }
            let a = &*audio;
            let samples = if a.samples.is_null() || a.n <= 0 { Vec::new() } else { std::slice::from_raw_parts(a.samples, a.n as usize).to_vec() };
            let rate = a.sample_rate as u32;
            self.lib.api.SherpaOnnxDestroyOfflineTtsGeneratedAudio(audio);
            Ok(Audio { samples, sample_rate: rate })
        }
    }
}

impl Drop for Tts {
    fn drop(&mut self) {
        // SAFETY: `ptr` was created by this library and is destroyed once.
        unsafe { self.lib.api.SherpaOnnxDestroyOfflineTts(self.ptr) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_library_is_a_clean_error() {
        let e = Lib::load(Path::new("/nonexistent/libsherpa-onnx-c-api.so")).err().unwrap();
        assert!(e.0.contains("/nonexistent"), "{e}");
    }

    #[test]
    fn bpe_vocab_is_token_tab_minus_id() {
        let v = bpe_vocab_from_tokens("<unk> 0\n<|nospeech|> 1\n▁the 42\nbad line\n");
        assert_eq!(v, "<unk>\t-0\n<|nospeech|>\t-1\n▁the\t-42\n");
        let c = SttConfig::from_dir(Path::new("/m"), 2);
        assert!(c.hotwords.is_none(), "greedy search by default");
    }

    #[test]
    fn stt_config_paths() {
        let c = SttConfig::from_dir(Path::new("/m/p3"), 4);
        assert_eq!(c.encoder, Path::new("/m/p3/encoder.int8.onnx"));
        assert_eq!(c.tokens, Path::new("/m/p3/tokens.txt"));
        assert_eq!(c.threads, 4);
    }

    #[test]
    fn tts_config_lexicon_paths_are_joined_and_empty_is_empty() {
        let c = TtsConfig::from_dir(Path::new("/m/k"), "lexicon-us-en.txt,lexicon-zh.txt", "", 4);
        assert_eq!(c.lexicon, "/m/k/lexicon-us-en.txt,/m/k/lexicon-zh.txt");
        assert_eq!(c.max_num_sentences, 1);
        assert_eq!(TtsConfig::from_dir(Path::new("/m/k"), "", "es", 4).lexicon, "");
    }

    #[test]
    fn config_rejects_missing_files_and_nul() {
        assert!(path_cstr(Path::new("/definitely/not/here")).is_err());
        assert!(cstr("a\0b").is_err());
        assert!(cstr("ok").is_ok());
    }

    #[test]
    fn default_lib_path_points_into_the_venv() {
        assert!(default_lib_path().to_string_lossy().ends_with("sherpa_onnx/lib/libsherpa-onnx-c-api.so"));
    }

    /// Needs ~/src/venv-voice (sherpa-onnx-core) and ~/models: run with `cargo test -p pimax-voicebench -- --ignored`.
    #[test]
    #[ignore = "needs sherpa-onnx-core in ~/src/venv-voice"]
    fn live_library_loads_and_reports_a_version() {
        let lib = Lib::load(&default_lib_path()).expect("load");
        assert!(lib.version().starts_with("1."), "{}", lib.version());
    }
}
