//! Speech recognisers behind one trait: a whole segment in, text out.
//!
//! * [`Parakeet`] (default): NVIDIA Parakeet TDT 0.6B v3 int8, multilingual (25 European languages incl. English and
//!   Spanish, no language setting needed), sherpa-onnx C API on the CPU.
//! * [`WhisperServer`]: whisper.cpp's `whisper-server` (large-v3-turbo on the GPU through Vulkan, port 8082) over
//!   HTTP, with Parakeet as the fallback when the server is down (docs/AI.md section 17).
//! * `Whisper` (cargo feature `whisper`): whisper.cpp through whisper-rs with Vulkan, the engine WayVR uses; for
//!   comparisons.
//! * [`FnEngine`]: a closure, for tests.

use omarchyvr_voicebench::sherpa::{bpe_vocab_from_tokens, default_lib_path, Hotwords, Lib, Recognizer, SttConfig};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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
        /// Hotword score (`modified_beam_search` biased to [`HOTWORDS`]); None: greedy search.
        hotwords: Option<f32>,
    },
    /// whisper.cpp's server on 127.0.0.1:`port`; `fallback` (a Parakeet) answers while it is down.
    WhisperServer {
        port: u16,
        timeout_ms: u64,
        fallback: Box<EngineSpec>,
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
            hotwords: None,
        }
    }

    /// Short name for logs and the `hello` event.
    pub fn name(&self) -> String {
        match self {
            EngineSpec::Parakeet {
                model_dir,
                threads,
                hotwords,
                ..
            } => {
                let m = model_dir
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                match hotwords {
                    Some(sc) => format!("{m} (sherpa-onnx cpu, {threads} threads, hotwords {sc})"),
                    None => format!("{m} (sherpa-onnx cpu, {threads} threads)"),
                }
            }
            EngineSpec::WhisperServer { port, fallback, .. } => format!(
                "whisper large-v3-turbo (whisper-server on the GPU, port {port}), {} while the server is down",
                fallback.name()
            ),
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
            EngineSpec::WhisperServer { fallback, .. } => fallback.sherpa_lib(),
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
                hotwords,
            } => Ok(Box::new(Parakeet::load_with(
                lib, model_dir, *threads, *hotwords,
            )?)),
            EngineSpec::WhisperServer {
                port,
                timeout_ms,
                fallback,
            } => Ok(Box::new(WhisperServer::new(
                *port,
                Duration::from_millis(*timeout_ms),
                fallback.build()?,
            ))),
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
        Parakeet::load_with(lib, model_dir, threads, None)
    }

    /// With `hotwords` (a score, 1.5 measured best): beam search biased to [`HOTWORDS`], the words the greedy search
    /// mishears (Max, passthrough, screens, ...). A failure to write the helper files falls back to greedy search.
    pub fn load_with(
        lib: &Path,
        model_dir: &Path,
        threads: i32,
        hotwords: Option<f32>,
    ) -> Result<Parakeet, String> {
        let lib = Lib::load(lib).map_err(|e| format!("sherpa-onnx library: {e}"))?;
        let mut cfg = SttConfig::from_dir(model_dir, threads.max(1));
        if let Some(score) = hotwords {
            match hotword_files(&cfg.tokens) {
                Ok((file, bpe_vocab)) => {
                    cfg.hotwords = Some(Hotwords {
                        file,
                        bpe_vocab,
                        score,
                    })
                }
                Err(e) => eprintln!("omarchyvr-voice: hotwords off: {e}"),
            }
        }
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

/// The words the Parakeet search is biased to (measured 2026-10-16: noisy WER 18.5% -> 13.3%, docs/AI.md section 17).
pub const HOTWORDS: [&str; 10] = [
    "Max",
    "passthrough",
    "screens",
    "workspace",
    "YouTube",
    "Chrome",
    "terminal",
    "foot",
    "cinema",
    "Omarchy",
];
/// Hotword score measured best (1.5; 2 to 8 were worse on the noisy clips).
pub const HOTWORD_SCORE: f32 = 1.5;

/// Write the hotword list and the BPE vocabulary (from `tokens.txt`) under `~/.cache/omarchyvr/voice/`.
fn hotword_files(tokens: &Path) -> Result<(PathBuf, PathBuf), String> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache")
        })
        .join("omarchyvr/voice");
    std::fs::create_dir_all(&base).map_err(|e| format!("{}: {e}", base.display()))?;
    let tok = std::fs::read_to_string(tokens).map_err(|e| format!("{}: {e}", tokens.display()))?;
    let write = |name: &str, text: String| -> Result<PathBuf, String> {
        let p = base.join(name);
        if std::fs::read_to_string(&p).ok().as_deref() != Some(text.as_str()) {
            std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))?;
        }
        Ok(p)
    };
    Ok((
        write("hotwords.txt", HOTWORDS.join("\n") + "\n")?,
        write("bpe.vocab", bpe_vocab_from_tokens(&tok))?,
    ))
}

// ------------------------------------------------------------------------------------------------ whisper-server

/// whisper-server's port (`omarchyvr_config::ai::ASR_PORT`).
pub use omarchyvr_config::ai::ASR_PORT;

/// Brackets whisper puts around sounds ("[BLANK_AUDIO]", "(silence)", "[Music]") are not speech.
fn clean_whisper(text: &str) -> String {
    let t = normalize_text(text);
    let bare = t.trim_matches(|c: char| "[]()*♪ .".contains(c));
    if (t.starts_with('[') && t.ends_with(']')) || (t.starts_with('(') && t.ends_with(')')) || bare.is_empty() {
        return String::new();
    }
    t
}

/// 16 kHz 16-bit mono WAV in memory, padded to 1.1 s (whisper skips shorter input).
pub fn wav_bytes(audio: &[f32]) -> Vec<u8> {
    let n = audio.len().max(17_600);
    let data = (n * 2) as u32;
    let mut w = Vec::with_capacity(44 + n * 2);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&16_000u32.to_le_bytes());
    w.extend_from_slice(&32_000u32.to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data.to_le_bytes());
    for i in 0..n {
        let x = audio.get(i).copied().unwrap_or(0.0).clamp(-1.0, 1.0);
        w.extend_from_slice(&((x * 32767.0).round() as i16).to_le_bytes());
    }
    w
}

/// The `POST /inference` request: multipart `file` (the WAV), `response_format=json`, `temperature=0.0`.
pub fn inference_request(wav: &[u8], boundary: &str) -> Vec<u8> {
    let mut body = Vec::with_capacity(wav.len() + 512);
    let field = |b: &mut Vec<u8>, name: &str, value: &str| {
        b.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes(),
        );
    };
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n").as_bytes(),
    );
    body.extend_from_slice(wav);
    body.extend_from_slice(b"\r\n");
    field(&mut body, "response_format", "json");
    field(&mut body, "temperature", "0.0");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    let mut req = format!(
        "POST /inference HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: multipart/form-data; boundary={boundary}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    req.extend_from_slice(&body);
    req
}

fn decode_chunked(mut b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(i) = b.windows(2).position(|w| w == b"\r\n") {
        let n = usize::from_str_radix(String::from_utf8_lossy(&b[..i]).split(';').next().unwrap_or("").trim(), 16).unwrap_or(0);
        if n == 0 || b.len() < i + 2 + n {
            break;
        }
        out.extend_from_slice(&b[i + 2..i + 2 + n]);
        b = &b[(i + 2 + n + 2).min(b.len())..];
    }
    out
}

/// Parse the server's answer (status line, headers, JSON `{"text": "..."}`) into the transcript text.
pub fn parse_inference_response(raw: &[u8]) -> Result<String, String> {
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or("no HTTP answer")?;
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let code: u16 = head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).ok_or("no HTTP status")?;
    let body = &raw[split + 4..];
    let body = if head.to_ascii_lowercase().contains("transfer-encoding: chunked") { decode_chunked(body) } else { body.to_vec() };
    if code != 200 {
        return Err(format!("HTTP {code}: {}", String::from_utf8_lossy(&body).chars().take(120).collect::<String>()));
    }
    let v: serde_json::Value = serde_json::from_slice(&body).map_err(|e| format!("bad JSON from whisper-server: {e}"))?;
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        return Err(format!("whisper-server: {e}"));
    }
    v.get("text").and_then(|t| t.as_str()).map(clean_whisper).ok_or_else(|| "whisper-server answered without text".into())
}

/// One request to the server.
pub fn server_transcribe(port: u16, timeout: Duration, audio: &[f32]) -> Result<String, String> {
    use std::io::{Read, Write};
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let mut s = std::net::TcpStream::connect_timeout(&addr, timeout.min(Duration::from_millis(250))).map_err(|e| format!("port {port}: {e}"))?;
    s.set_read_timeout(Some(timeout)).ok();
    s.set_write_timeout(Some(timeout)).ok();
    s.set_nodelay(true).ok();
    let req = inference_request(&wav_bytes(audio), "----pimaxvoice7d2f");
    s.write_all(&req).map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).map_err(|e| format!("no answer in {} ms: {e}", timeout.as_millis()))?;
    parse_inference_response(&raw)
}

/// How long a failed server is left alone before the next try (the fallback answers meanwhile).
const RETRY_AFTER: Duration = Duration::from_secs(3);

/// The GPU recogniser: whisper-server over HTTP, the fallback while it is down (never stops voice).
pub struct WhisperServer {
    port: u16,
    timeout: Duration,
    fallback: Box<dyn Engine>,
    down_until: Option<Instant>,
    /// What the last request used (for the log lines on a change).
    on_gpu: Option<bool>,
}

impl WhisperServer {
    pub fn new(port: u16, timeout: Duration, fallback: Box<dyn Engine>) -> WhisperServer {
        WhisperServer {
            port,
            timeout,
            fallback,
            down_until: None,
            on_gpu: None,
        }
    }

    fn note(&mut self, gpu: bool, why: &str) {
        if self.on_gpu != Some(gpu) {
            self.on_gpu = Some(gpu);
            if gpu {
                eprintln!("omarchyvr-voice: asr: the GPU server answers (port {})", self.port);
            } else {
                eprintln!("omarchyvr-voice: asr: GPU server not usable ({why}); Parakeet on the processor answers");
            }
        }
    }
}

impl Engine for WhisperServer {
    fn transcribe(&mut self, audio: &[f32]) -> Result<Transcript, String> {
        if self.down_until.is_none_or(|t| Instant::now() >= t) {
            match server_transcribe(self.port, self.timeout, audio) {
                Ok(text) => {
                    self.down_until = None;
                    self.note(true, "");
                    return Ok(Transcript {
                        text,
                        lang: Some("en".into()),
                    });
                }
                Err(e) => {
                    self.down_until = Some(Instant::now() + RETRY_AFTER);
                    self.note(false, &e);
                }
            }
        }
        self.fallback.transcribe(audio)
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
            hotwords: None,
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
    fn wav_is_16k_mono_16bit_and_padded() {
        let w = wav_bytes(&[0.0, 1.0, -1.0, 2.0]);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(&w[8..16], b"WAVEfmt ");
        assert_eq!(u16::from_le_bytes([w[22], w[23]]), 1, "mono");
        assert_eq!(u32::from_le_bytes([w[24], w[25], w[26], w[27]]), 16_000);
        assert_eq!(u16::from_le_bytes([w[34], w[35]]), 16, "bits");
        // padded to 1.1 s: whisper skips shorter input
        assert_eq!(w.len(), 44 + 17_600 * 2);
        let data = u32::from_le_bytes([w[40], w[41], w[42], w[43]]) as usize;
        assert_eq!(data, 17_600 * 2);
        assert_eq!(i16::from_le_bytes([w[46], w[47]]), 32767);
        assert_eq!(i16::from_le_bytes([w[48], w[49]]), -32767);
        assert_eq!(i16::from_le_bytes([w[50], w[51]]), 32767, "clamped");
        let long = wav_bytes(&vec![0.1; 40_000]);
        assert_eq!(long.len(), 44 + 80_000);
    }

    #[test]
    fn multipart_request_has_file_format_and_temperature() {
        let wav = wav_bytes(&[0.0; 10]);
        let r = inference_request(&wav, "BND");
        let head_end = r.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let head = String::from_utf8_lossy(&r[..head_end]).into_owned();
        assert!(head.starts_with("POST /inference HTTP/1.1\r\n"), "{head}");
        assert!(head.contains("Content-Type: multipart/form-data; boundary=BND"), "{head}");
        let body = &r[head_end + 4..];
        assert!(head.contains(&format!("Content-Length: {}", body.len())), "{head}");
        let text = String::from_utf8_lossy(body).into_owned();
        assert!(text.starts_with("--BND\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.wav\""), "{text:.200}");
        assert!(text.contains("name=\"response_format\"\r\n\r\njson\r\n"));
        assert!(text.contains("name=\"temperature\"\r\n\r\n0.0\r\n"));
        assert!(text.ends_with("--BND--\r\n"));
        assert!(body.windows(wav.len()).any(|w| w == &wav[..]), "the WAV is in the body byte for byte");
    }

    #[test]
    fn response_parse_text_chunked_errors_and_sound_tags() {
        let ok = |body: &str| format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len()).into_bytes();
        assert_eq!(parse_inference_response(&ok(r#"{"text":" Max, can I focus YouTube?\n"}"#)).unwrap(), "Max, can I focus YouTube?");
        assert_eq!(parse_inference_response(&ok(r#"{"text":" [BLANK_AUDIO]"}"#)).unwrap(), "");
        assert_eq!(parse_inference_response(&ok(r#"{"text":" (silence)"}"#)).unwrap(), "");
        assert_eq!(parse_inference_response(&ok(r#"{"text":" ..."}"#)).unwrap(), "");
        let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n7\r\n{\"text\"\r\na\r\n:\" hi yo\"}\r\n0\r\n\r\n".to_vec();
        assert_eq!(parse_inference_response(&chunked).unwrap(), "hi yo");
        let e = parse_inference_response(b"HTTP/1.1 500 Internal Server Error\r\n\r\nboom").unwrap_err();
        assert!(e.contains("HTTP 500") && e.contains("boom"), "{e}");
        assert!(parse_inference_response(&ok(r#"{"error":"failed to read audio"}"#)).unwrap_err().contains("failed to read audio"));
        assert!(parse_inference_response(&ok("not json")).is_err());
        assert!(parse_inference_response(b"").is_err());
        assert!(parse_inference_response(&ok("{}")).is_err());
    }

    /// A one-shot fake whisper-server: answers `n` requests with `text`.
    fn fake_server(n: usize, text: &'static str) -> (u16, std::thread::JoinHandle<Vec<usize>>) {
        use std::io::{Read, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            let mut sizes = Vec::new();
            for _ in 0..n {
                let (mut c, _) = l.accept().unwrap();
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                loop {
                    let k = c.read(&mut chunk).unwrap();
                    buf.extend_from_slice(&chunk[..k]);
                    if let Some(he) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..he]).to_lowercase();
                        let len: usize = head.split("content-length: ").nth(1).and_then(|x| x.split("\r\n").next()).and_then(|x| x.trim().parse().ok()).unwrap_or(0);
                        if buf.len() >= he + 4 + len {
                            break;
                        }
                    }
                    if k == 0 {
                        break;
                    }
                }
                sizes.push(buf.len());
                let body = format!("{{\"text\":\"{text}\"}}");
                write!(c, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
            }
            sizes
        });
        (port, h)
    }

    #[test]
    fn server_round_trip_over_http() {
        let (port, h) = fake_server(1, " Open the browser.");
        let t = server_transcribe(port, Duration::from_secs(5), &[0.1; 16_000]).unwrap();
        assert_eq!(t, "Open the browser.");
        assert!(h.join().unwrap()[0] > 32_000, "the WAV went over the wire");
    }

    #[test]
    fn engine_uses_the_server_and_falls_back_when_it_is_down() {
        // up: the server answers, the fallback is never called
        let (port, h) = fake_server(1, "from the gpu");
        let mut e = WhisperServer::new(port, Duration::from_secs(5), Box::new(FnEngine(|_: &[f32]| panic!("fallback used"))));
        let t = e.transcribe(&[0.0; 16_000]).unwrap();
        assert_eq!((t.text.as_str(), t.lang.as_deref()), ("from the gpu", Some("en")));
        h.join().unwrap();
        // down: nothing listens on the port (the listener is gone), so the fallback answers at once and the dead
        // server is left alone for a few seconds
        let mut calls = 0;
        let mut e = WhisperServer::new(port, Duration::from_millis(300), Box::new(FnEngine(move |_: &[f32]| {
            calls += 1;
            format!("cpu {calls}")
        })));
        let t0 = Instant::now();
        assert_eq!(e.transcribe(&[0.0; 16_000]).unwrap().text, "cpu 1");
        assert!(t0.elapsed() < Duration::from_secs(2), "no waiting on a dead server");
        assert!(e.down_until.is_some());
        assert_eq!(e.transcribe(&[0.0; 16_000]).unwrap().text, "cpu 2");
        // after the pause the server is tried again and, when it is back, used
        let (port2, h2) = fake_server(1, "back");
        e.port = port2;
        e.down_until = Some(Instant::now() - Duration::from_millis(1));
        assert_eq!(e.transcribe(&[0.0; 16_000]).unwrap().text, "back");
        assert!(e.down_until.is_none());
        h2.join().unwrap();
    }

    #[test]
    fn engine_spec_names_the_gpu_and_the_fallback() {
        let cpu = EngineSpec::Parakeet { lib: "/x/l.so".into(), model_dir: "/m/parakeet-tdt-0.6b-v3-int8".into(), threads: 2, hotwords: Some(HOTWORD_SCORE) };
        assert!(cpu.name().contains("hotwords 1.5"), "{}", cpu.name());
        let g = EngineSpec::WhisperServer { port: 8082, timeout_ms: 4000, fallback: Box::new(cpu) };
        let n = g.name();
        assert!(n.contains("whisper-server on the GPU, port 8082") && n.contains("parakeet-tdt-0.6b-v3-int8"), "{n}");
        assert_eq!(g.sherpa_lib(), Some(Path::new("/x/l.so")), "the VAD needs the fallback's library");
        // the fallback must load before the engine can be built: a missing library is an error, not a silent GPU-only engine
        assert!(g.build().err().unwrap().contains("/x/l.so"));
    }

    #[test]
    fn hotword_list_is_the_measured_one() {
        assert_eq!(HOTWORDS.len(), 10);
        assert!(HOTWORDS.contains(&"Omarchy") && HOTWORDS.contains(&"passthrough"));
        assert_eq!(HOTWORD_SCORE, 1.5);
    }

    #[test]
    fn fn_engine() {
        let mut e = FnEngine(|a: &[f32]| format!("{} samples", a.len()));
        assert_eq!(e.transcribe(&[0.0; 3]).unwrap().text, "3 samples");
    }
}
