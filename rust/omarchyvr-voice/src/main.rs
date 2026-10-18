//! `omarchyvr-voice`: local voice-to-text for the Dream Air. See `omarchyvr-voice help`.

use omarchyvr_voice::act::{self, Actor, DryRun, Real, Runner, State as ActState};
use omarchyvr_voice::captions::{Captions, Wayvr};
use omarchyvr_voice::catalogue::{self, Catalogue};
use omarchyvr_voice::engine::{default_model_dir, EngineSpec};
use omarchyvr_voice::intent::Intents;
use omarchyvr_voice::pipeline::{run_live, run_offline, Core, LiveConfig};
use omarchyvr_voice::protocol::{self, Command, Event, State};
use omarchyvr_voice::segment::{frames_of_ms, SegConfig};
use omarchyvr_voice::server::{self, Server, Sink, Stdout, Tee};
use omarchyvr_voice::vad::{default_silero_model, EnergyVad, Gated, SileroVad, Vad};
use omarchyvr_voice::{bench, source};
use omarchyvr_voicebench::sherpa::default_lib_path;
use omarchyvr_config::ai::{AiConfig, VoiceListen};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

const HELP: &str = "omarchyvr-voice <command> [options]   local voice-to-text (nothing leaves this machine)

  listen [--device NAME|default] [--input FILE.wav|-] [--fast] [--socket PATH|none] [--json] [--levels]
      Capture the headset mic (PipeWire; NAME = node name or a substring of name/description, default \"Pimax\",
      falls back to the default source), detect speech, print partial and final transcripts and publish them as
      JSON lines on the socket (default $XDG_RUNTIME_DIR/omarchyvr-voice.sock). --input FILE.wav replays a file at the
      real audio rate (--fast: as fast as possible); --input - reads s16le 16 kHz mono from stdin
      (pw-record --rate 16000 --channels 1 --format s16 - | omarchyvr-voice listen --input -). Ctrl-C stops.
  captions [--wayvrctl BIN] [--overlay captions] [--clear-s 6] [--lines 2] [--width 56] [--socket PATH]
      live captions: the transcript of a running listener on the WayVR panel `captions` (desk mode puts it under the
      main screen) through `wayvrctl batch`; partials show while you speak, the text clears after --clear-s of silence.
      Reconnects when the listener or WayVR restarts. Nothing is typed.
  act [--dry-run] [--no-toast] [--wayvrctl BIN] [--socket PATH]
      voice commands: final transcripts that start with \"Pimax\" -> an intent of the catalogue (built-in
      voice-intents.toml + ~/.config/omarchyvr/voice-intents.toml) -> its action, with a toast in the headset.
      Lock asks for a spoken \"yes\"; \"Pimax undo\" reverts presets and backgrounds. --dry-run prints instead.
  do \"TEXT\" [\"TEXT\"...] [--dry-run] [--no-wake]
      the same for typed utterances, in order (e.g. do \"pimax lock the screen\" \"yes\" --dry-run).
      --no-wake: \"Pimax\" is implied.
  gaze [--wayvrctl BIN] [--at UNIX_MS] [--input GAZE.json] [--json]
      what the eyes are on: `wayvrctl gaze` (silent eye tracking in WayVR) -> the fixation (the last stable one,
      or the one at --at, e.g. when an utterance started) -> desktop pixel -> the Hyprland window under it
      (address, class, title) and a best-effort tab. --input resolves a saved `wayvrctl gaze` answer instead.
      Read-only.
  intents [--all]                        the catalogue: enabled intents (--all: also disabled) and loader notes
  transcribe FILE.wav... [--json]        offline: segments and transcripts of each file (decoded inline)
  devices                                PipeWire capture sources; * = the one `listen` picks
  tail [--json]                          print the events of a running listener (socket client)
  pause | resume | ping                  send a command to a running listener
  aibench [--fixture FILE.tsv] [--runs 3] [--out FILE.md] [--label TEXT] [--url URL] [--style compact|tools]
          [--timeout-s S] [--wayvrctl BIN]
      the voice AI against the running model (ai.toml's voice provider): every phrase of the fixture (default:
      the built-in tests/fixtures/aibench.tsv, real and mangled transcripts) through the full route (matcher,
      fuzzy, model), nothing executed (hyprctl/gaze are read live). Prints accuracy, asks, wrong actions and
      p50/p95 latency (end of transcript -> decision). --url: another local server (.../v1/chat/completions).
  audit --state [--variant NAME|FILE] [--subset N --seed S] [--split dev|held|all] [--tier medium|hard] [--only TEXT] [--list] [--snapshot]
      THE QUALITY MEASURE: each case starts from a desk state (screens, apps, tabs, gaze), runs the words through the
      whole pipeline against a simulator of that desk and is scored on the end state reached, nothing else changed
      (docs/VOICE-STATE-TESTS.md). Writes analysis/voice-state-audit.html, appends analysis/voice-experiments.csv.
  audit [--out FILE.html] [--live] [--json-out FILE] [--label TEXT] [--url URL] [--pause-ms 1000] [--remote [--max 40] [--action-model M]] [--hard [--max 40]] [--log]
      A fast SMOKE TEST of intent names (not the quality measure: see audit --state).
      every intent: its first catalogue phrase, an ASR-mangled copy and the natural phrasings of
      tests/fixtures/audit.tsv through the full route (exact, fuzzy, model) in dry run; pass / ask / fail, slots,
      latency, as an HTML page; --pause-ms between model requests (keeps the CPU free). --live also types for real
      into a scratch terminal it opens and closes itself (foot running cat): typing, Enter, Backspace.
      --remote: ai.toml's remote voice provider (two stages): only the phrases the matcher leaves to the model, at
      most --max (40) spread over the intents, paced by --pause-ms; prints the pass rate, p50/p95 latency and the
      total cost (usage.cost). Dry run: nothing is executed, typed or clicked; --live is refused.
      --hard: the same selection of phrases for any provider (the local two-stage benchmark, docs/AI.md 16).
  routerbench [--fixture FILE.tsv] [--hard] [--max N] [--label held-out] [--out TRAIN.jsonl]
      stage 1 only: ai.toml's two-stage router (Lara, port 8081, or PIMAX_AI_ROUTER_BASE) classifies every
      audit phrase; prints top-1 accuracy, how often the right category is among the ones the act request gets,
      the matcher's own guess for comparison, and p50/p95 latency. Nothing runs. --label held-out: only the
      natural phrasings (not in the training rows); --out: write Laya training rows from the catalogue instead.
  ai-state [WORDS...]          (read-only: the exact state text the AI gets for these words from the live desktop, with sizes)
  ai-size                      (prints what each AI request kind sends for a typical state: characters, ~tokens)
  ai-mode [local-toggle|local-on|local-off|preset-next|low|medium|high|next|simple|two-stage|online|show]
      AI runs on (the Work desk watch's AI button): saves it in ai.toml ([use] voice and voice_local only),
      starts the local AI in that shape (omarchyvr-ai up) and prints the mode; a running act switches by itself.
  aiprompt WORDS...                      print the compact model request for these words (no live context)
  bench [--clips DIR] [--threads 1,2,4] [--partial-ms 640,0] [--out FILE.md] [--gap 1.5] [--noise 0.002]
      the pipeline on the voicebench clips, in real time through the live driver: end of speech -> final, CPU
  vad FILE.wav [--vad silero|energy]     per-frame VAD decisions (# speech, . silence; one char = 32 ms)
  info                                   library / model / VAD / socket check

engine and VAD options (listen, transcribe, bench):
  --threads N (2)  --model DIR (~/models/parakeet-tdt-0.6b-v3-int8)  --lib PATH (sherpa-onnx C API library)
  --vad silero|silero-gate|energy (silero if ~/models/vad/silero_vad.onnx exists)  --vad-model PATH  --vad-threshold 0.5
  silero-gate (experimental): Silero AND --gate-db 6 above the noise floor; ends utterances ~0.5 s sooner but lost
  one long utterance in the bench
  --endpoint-ms N (silence that ends an utterance; default from the app's Listening setting: 480/750/1100)  --hangover-ms N (same after a dangling word: 1120/1500/2000)  --partial-ms 640 (0 = no partials)  --max-s 20
  --intents FILE.json|none (extra command rules; built-in: hud.clear, vr.recenter)
  --engine auto|parakeet|whisper-server|whisper   (default auto: ai.toml's `asr`, set on the app's Local AI page)
      auto = Parakeet on the processor (with hotwords; --no-hotwords turns them off) or, when asr is auto/gpu, the
      large Whisper on the GPU through whisper-server (--asr-port 8082, --asr-timeout-ms 4000) with Parakeet as the
      fallback while the server is down. whisper = in-process whisper.cpp (needs a build with --features whisper:
      --whisper-model FILE [--gpu] [--lang en])
environment: PIMAX_VOICE_SOCKET overrides the socket path.";

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

fn install_signals() {
    // SAFETY: the handler only stores to an atomic.
    unsafe {
        libc::signal(
            libc::SIGINT,
            on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGTERM,
            on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t,
        );
    }
}

/// Parsed options (all commands share one set; each command uses what it needs).
#[derive(Debug, Clone)]
struct Opts {
    positional: Vec<String>,
    device: String,
    input: Option<String>,
    fast: bool,
    socket: Option<PathBuf>,
    json: bool,
    levels: bool,
    threads: Vec<i32>,
    model: PathBuf,
    lib: PathBuf,
    vad: Option<String>,
    vad_model: PathBuf,
    vad_threshold: f32,
    gate_db: f32,
    endpoint_ms: u32,
    /// silence that ends an utterance after a dangling word, ms (`--hangover-ms`)
    hangover_ms: u32,
    /// explicit flags win over the app's `voice_listen` setting
    endpoint_flag: Option<u32>,
    hangover_flag: Option<u32>,
    partial_ms: Vec<u32>,
    max_s: u32,
    intents: Option<String>,
    engine: String,
    no_hotwords: bool,
    asr_port: u16,
    asr_timeout_ms: u64,
    whisper_model: Option<PathBuf>,
    gpu: bool,
    lang: Option<String>,
    clips: PathBuf,
    out: Option<PathBuf>,
    gap: f64,
    noise: f32,
    wayvrctl: PathBuf,
    overlay: String,
    clear_s: f64,
    lines: usize,
    width: usize,
    wayvrctl_set: bool,
    dry_run: bool,
    no_wake: bool,
    no_toast: bool,
    all: bool,
    at_ms: Option<u64>,
    fixture: Option<PathBuf>,
    live: bool,
    /// audit: only phrases the remote provider gets (at most `max`), dry run
    remote: bool,
    /// audit --hard: only the phrases the matcher leaves to the model (the remote benchmark's 40), any provider.
    hard: bool,
    max: Option<usize>,
    /// audit: keep ai.log (requests, answers, costs; keys redacted)
    log: bool,
    /// audit --remote: the stage-2 model instead of ai.toml's action_model
    action_model: Option<String>,
    json_out: Option<PathBuf>,
    /// audit: pause after each model request (ms)
    pause_ms: u64,
    url: Option<String>,
    style: Option<String>,
    timeout_s: Option<f64>,
    runs: usize,
    label: Option<String>,
}

fn list<T: std::str::FromStr>(v: &str, name: &str) -> Result<Vec<T>, String> {
    v.split(',')
        .map(|x| {
            x.trim()
                .parse()
                .map_err(|_| format!("bad value for {name}: {x:?}"))
        })
        .collect()
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let mut o = Opts {
        positional: Vec::new(),
        device: "Pimax".into(),
        input: None,
        fast: false,
        socket: Some(protocol::socket_path()),
        json: false,
        levels: false,
        threads: vec![2],
        model: default_model_dir(),
        lib: default_lib_path(),
        vad: None,
        vad_model: default_silero_model(),
        vad_threshold: 0.5,
        gate_db: 6.0,
        endpoint_ms: 480,
        hangover_ms: 1120,
        endpoint_flag: None,
        hangover_flag: None,
        partial_ms: vec![640],
        max_s: 20,
        intents: None,
        engine: "auto".into(),
        no_hotwords: false,
        asr_port: omarchyvr_config::ai::ASR_PORT,
        asr_timeout_ms: 4000,
        whisper_model: None,
        gpu: false,
        lang: None,
        clips: omarchyvr_config::paths::src_dir(&home).join("desk-out/vbfinal/q4-unpinned/clips"),
        out: None,
        gap: 1.5,
        noise: 0.002,
        wayvrctl: omarchyvr_config::paths::wayvrctl_bin(&home),
        overlay: "captions".into(),
        clear_s: 6.0,
        lines: 2,
        width: 56,
        wayvrctl_set: false,
        dry_run: false,
        no_wake: false,
        no_toast: false,
        all: false,
        at_ms: None,
        fixture: None,
        live: false,
        remote: false,
        hard: false,
        max: None,
        log: false,
        action_model: None,
        json_out: None,
        pause_ms: 1000,
        url: None,
        style: None,
        timeout_s: None,
        runs: 3,
        label: None,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = |name: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match a.as_str() {
            "--device" => o.device = val(a)?,
            "--input" => o.input = Some(val(a)?),
            "--fast" => o.fast = true,
            "--socket" => {
                let v = val(a)?;
                o.socket = if v == "none" {
                    None
                } else {
                    Some(PathBuf::from(v))
                };
            }
            "--json" => o.json = true,
            "--levels" => o.levels = true,
            "--threads" => o.threads = list(&val(a)?, a)?,
            "--model" => o.model = PathBuf::from(val(a)?),
            "--lib" => o.lib = PathBuf::from(val(a)?),
            "--vad" => o.vad = Some(val(a)?),
            "--vad-model" => o.vad_model = PathBuf::from(val(a)?),
            "--vad-threshold" => {
                o.vad_threshold = val(a)?.parse().map_err(|_| "bad --vad-threshold")?
            }
            "--gate-db" => o.gate_db = val(a)?.parse().map_err(|_| "bad --gate-db")?,
            "--endpoint-ms" => o.endpoint_flag = Some(val(a)?.parse().map_err(|_| "bad --endpoint-ms")?),
            "--hangover-ms" => o.hangover_flag = Some(val(a)?.parse().map_err(|_| "bad --hangover-ms")?),
            "--partial-ms" => o.partial_ms = list(&val(a)?, a)?,
            "--max-s" => o.max_s = val(a)?.parse().map_err(|_| "bad --max-s")?,
            "--intents" => o.intents = Some(val(a)?),
            "--engine" => o.engine = val(a)?,
            "--no-hotwords" => o.no_hotwords = true,
            "--asr-port" => o.asr_port = val(a)?.parse().map_err(|_| "bad --asr-port")?,
            "--asr-timeout-ms" => o.asr_timeout_ms = val(a)?.parse().map_err(|_| "bad --asr-timeout-ms")?,
            "--whisper-model" => o.whisper_model = Some(PathBuf::from(val(a)?)),
            "--gpu" => o.gpu = true,
            "--lang" => o.lang = Some(val(a)?),
            "--clips" => o.clips = PathBuf::from(val(a)?),
            "--out" => o.out = Some(PathBuf::from(val(a)?)),
            "--gap" => o.gap = val(a)?.parse().map_err(|_| "bad --gap")?,
            "--wayvrctl" => {
                o.wayvrctl = PathBuf::from(val(a)?);
                o.wayvrctl_set = true;
            }
            "--dry-run" => o.dry_run = true,
            "--no-wake" => o.no_wake = true,
            "--no-toast" => o.no_toast = true,
            "--all" => o.all = true,
            "--at" => o.at_ms = Some(val(a)?.parse().map_err(|_| "bad --at (Unix milliseconds)")?),
            "--overlay" => o.overlay = val(a)?,
            "--clear-s" => o.clear_s = val(a)?.parse().map_err(|_| "bad --clear-s")?,
            "--lines" => o.lines = val(a)?.parse().map_err(|_| "bad --lines")?,
            "--width" => o.width = val(a)?.parse().map_err(|_| "bad --width")?,
            "--noise" => o.noise = val(a)?.parse().map_err(|_| "bad --noise")?,
            "--fixture" => o.fixture = Some(PathBuf::from(val(a)?)),
            "--live" => o.live = true,
            "--remote" => o.remote = true,
            "--hard" => o.hard = true,
            "--log" => o.log = true,
            "--action-model" => o.action_model = Some(val(a)?),
            "--max" => o.max = Some(val(a)?.parse().map_err(|_| "--max N")?),
            "--json-out" => o.json_out = Some(PathBuf::from(val(a)?)),
            "--pause-ms" => o.pause_ms = val(a)?.parse().map_err(|_| "--pause-ms N")?,
            "--runs" => o.runs = val(a)?.parse().map_err(|_| "bad --runs")?,
            "--url" => o.url = Some(val(a)?),
            "--style" => o.style = Some(val(a)?),
            "--timeout-s" => o.timeout_s = Some(val(a)?.parse().map_err(|_| "bad --timeout-s")?),
            "--label" => o.label = Some(val(a)?),
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            _ => o.positional.push(a.clone()),
        }
    }
    if o.threads.is_empty() || o.threads.iter().any(|&t| !(1..=32).contains(&t)) {
        return Err("--threads must be 1..32".into());
    }
    // flag > the app's voice_listen setting (ai.toml) > the default (long)
    let listen = saved_listen();
    (o.endpoint_ms, o.hangover_ms) = resolve_listen(o.endpoint_flag, o.hangover_flag, listen);
    Ok(o)
}

fn parakeet_spec(o: &Opts, threads: i32, hotwords: bool) -> EngineSpec {
    EngineSpec::Parakeet {
        lib: o.lib.clone(),
        model_dir: o.model.clone(),
        threads,
        hotwords: (hotwords && !o.no_hotwords).then_some(omarchyvr_voice::engine::HOTWORD_SCORE),
    }
}

/// What `--engine auto` means: ai.toml's `asr` (the "Speech on graphics card" switch, default off = Parakeet on the
/// processor). On: the whisper-server on the card, with Parakeet as the fallback.
fn auto_engine(o: &Opts, threads: i32) -> EngineSpec {
    let cfg = omarchyvr_config::ai::AiConfig::load().unwrap_or_default();
    // the card also needs the local models (the server belongs to them; Work only)
    let large = cfg.asr_on_gpu() && cfg.local_models_on();
    let cpu = parakeet_spec(o, threads, true);
    if large {
        EngineSpec::WhisperServer {
            port: o.asr_port,
            timeout_ms: o.asr_timeout_ms,
            fallback: Box::new(cpu),
        }
    } else {
        cpu
    }
}

fn engine_spec(o: &Opts, threads: i32) -> Result<EngineSpec, String> {
    match o.engine.as_str() {
        "auto" => Ok(auto_engine(o, threads)),
        "parakeet" => Ok(parakeet_spec(o, threads, true)),
        "whisper-server" => Ok(EngineSpec::WhisperServer {
            port: o.asr_port,
            timeout_ms: o.asr_timeout_ms,
            fallback: Box::new(parakeet_spec(o, threads, true)),
        }),
        #[cfg(feature = "whisper")]
        "whisper" => {
            let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
            let model = o
                .whisper_model
                .clone()
                .unwrap_or_else(|| home.join("models/whisper/ggml-small-q8_0.bin"));
            Ok(EngineSpec::Whisper {
                model,
                threads,
                gpu: o.gpu,
                lang: o.lang.clone(),
            })
        }
        #[cfg(not(feature = "whisper"))]
        "whisper" => {
            let _ = (&o.whisper_model, o.gpu, &o.lang);
            Err("this build has no whisper engine (cargo build --features whisper)".into())
        }
        other => Err(format!("unknown engine {other:?} (auto, parakeet, whisper-server, whisper)")),
    }
}

/// The app's `voice_listen` (ai.toml); the default when the file or key is missing.
fn saved_listen() -> VoiceListen {
    let path = omarchyvr_config::ai::default_path().unwrap_or_else(omarchyvr_voice::llm::ai_path);
    std::fs::read_to_string(path).ok().and_then(|t| AiConfig::parse(&t).ok()).map(|c| c.voice_listen).unwrap_or_default()
}

/// (endpoint_ms, long_hangover_ms): an explicit flag wins, else the setting.
fn resolve_listen(endpoint_flag: Option<u32>, hangover_flag: Option<u32>, setting: VoiceListen) -> (u32, u32) {
    (endpoint_flag.unwrap_or_else(|| setting.endpoint_ms()), hangover_flag.unwrap_or_else(|| setting.long_hangover_ms()))
}

fn seg_config(o: &Opts, partial_ms: u32) -> SegConfig {
    SegConfig {
        hangover: frames_of_ms(o.endpoint_ms),
        long_hangover: frames_of_ms(o.hangover_ms),
        partial_every: frames_of_ms(partial_ms),
        min_partial: frames_of_ms(partial_ms.max(320)),
        max_len: frames_of_ms(o.max_s.max(2) * 1000),
        ..SegConfig::default()
    }
}

fn make_vad(o: &Opts) -> Result<Box<dyn Vad + Send>, String> {
    let want = o.vad.clone().unwrap_or_else(|| {
        if o.vad_model.exists() {
            "silero".into()
        } else {
            "energy".into()
        }
    });
    match want.as_str() {
        "silero" => Ok(Box::new(SileroVad::new(
            &o.lib,
            &o.vad_model,
            o.vad_threshold,
        )?)),
        "silero-gate" => Ok(Box::new(Gated::new(
            SileroVad::new(&o.lib, &o.vad_model, o.vad_threshold)?,
            o.gate_db,
        ))),
        "energy" => {
            if o.vad.is_none() {
                eprintln!(
                    "omarchyvr-voice: {} missing, using the energy VAD",
                    o.vad_model.display()
                );
            }
            Ok(Box::new(EnergyVad::default()))
        }
        other => Err(format!(
            "unknown --vad {other:?} (silero, silero-gate, energy)"
        )),
    }
}

fn load_intents(o: &Opts) -> Result<Intents, String> {
    match o.intents.as_deref() {
        Some("none") => Ok(Intents::none()),
        Some(path) => {
            let mut i = Intents::builtin();
            let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
            i.extend_from_json(&text)?;
            Ok(i)
        }
        None => Ok(Intents::builtin()),
    }
}

fn cmd_listen(o: &Opts) -> Result<(), String> {
    install_signals();
    let spec = engine_spec(o, o.threads[0])?;
    eprintln!("omarchyvr-voice: engine: {} (--engine {})", spec.name(), o.engine);
    let vad = make_vad(o)?;
    let core = Core::new(vad, seg_config(o, o.partial_ms[0]));
    let vad_name = core.vad_name();
    let intents = load_intents(o)?;
    let (atx, arx) = mpsc::channel::<Vec<f32>>();
    let (ctx, crx) = mpsc::channel::<Command>();
    let stop = Arc::new(AtomicBool::new(false));
    let source_label = match &o.input {
        Some(p) if p == "-" => "stdin (s16le 16 kHz mono)".to_string(),
        Some(p) => format!("file {p}"),
        None => format!("PipeWire source {:?}", o.device),
    };
    let hello = Event::Hello {
        engine: format!("{} + {vad_name} VAD", spec.name()),
        source: source_label,
        rate: 16_000,
    };
    let stdout: Arc<dyn Sink> = Arc::new(Stdout::new(o.json, o.levels));
    let server = match &o.socket {
        Some(p) => Some(Arc::new(Server::bind(p, hello.clone(), ctx.clone())?)),
        None => None,
    };
    let mut sinks = vec![Arc::clone(&stdout)];
    if let Some(s) = &server {
        sinks.push(Arc::clone(s) as Arc<dyn Sink>);
        eprintln!("omarchyvr-voice: socket {}", s.path().display());
    }
    let sink: Arc<dyn Sink> = Arc::new(Tee(sinks));
    sink.emit(&hello);
    let feeder = match o.input.as_deref() {
        Some("-") => source::spawn_stdin(atx, Arc::clone(&stop))?,
        Some(path) => {
            source::spawn_wav(std::path::Path::new(path), !o.fast, atx, Arc::clone(&stop))?
        }
        None => spawn_capture(o, atx, &stop, &sink, server.clone())?,
    };
    eprintln!("omarchyvr-voice: loading {} ...", spec.name());
    let stop2 = Arc::clone(&stop);
    std::thread::spawn(move || {
        while !STOP.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        stop2.store(true, Ordering::Relaxed);
    });
    let res = run_live(
        core,
        LiveConfig {
            engine: Box::new(move || spec.build()),
            intents,
        },
        &arx,
        &crx,
        Arc::clone(&sink),
        &stop,
    );
    stop.store(true, Ordering::Relaxed);
    let _ = feeder.join();
    drop(server);
    res
}

#[cfg(feature = "pipewire")]
fn spawn_capture(
    o: &Opts,
    atx: mpsc::Sender<Vec<f32>>,
    stop: &Arc<AtomicBool>,
    sink: &Arc<dyn Sink>,
    server: Option<Arc<Server>>,
) -> Result<std::thread::JoinHandle<()>, String> {
    use omarchyvr_voice::capture::{self, CaptureNote};
    let (ntx, nrx) = mpsc::channel();
    let h = capture::spawn(o.device.clone(), atx, ntx, Arc::clone(stop));
    let sink = Arc::clone(sink);
    let engine_name = engine_spec(o, o.threads[0])
        .map(|s| s.name())
        .unwrap_or_default();
    std::thread::spawn(move || {
        for n in nrx {
            match n {
                CaptureNote::Started(label) => {
                    eprintln!("omarchyvr-voice: capturing from {label}");
                    if let Some(s) = &server {
                        s.set_hello(Event::Hello {
                            engine: engine_name.clone(),
                            source: label,
                            rate: 16_000,
                        });
                    }
                }
                CaptureNote::Fallback(msg) => eprintln!("omarchyvr-voice: {msg}"),
                CaptureNote::Error(e) => {
                    eprintln!("omarchyvr-voice: capture: {e}");
                    sink.emit(&Event::Status {
                        state: State::NoDevice,
                        detail: e,
                    });
                }
            }
        }
    });
    Ok(h)
}

#[cfg(not(feature = "pipewire"))]
fn spawn_capture(
    _o: &Opts,
    _atx: mpsc::Sender<Vec<f32>>,
    _stop: &Arc<AtomicBool>,
    _sink: &Arc<dyn Sink>,
    _server: Option<Arc<Server>>,
) -> Result<std::thread::JoinHandle<()>, String> {
    Err("built without PipeWire: use --input FILE.wav or --input - (pw-record ... -)".into())
}

fn cmd_transcribe(o: &Opts) -> Result<(), String> {
    if o.positional.is_empty() {
        return Err("transcribe needs WAV files".into());
    }
    let spec = engine_spec(o, o.threads[0])?;
    let mut engine = spec.build()?;
    let intents = load_intents(o)?;
    let out = Stdout::new(o.json, false);
    for f in &o.positional {
        let audio = source::load_wav_16k(std::path::Path::new(f))?;
        if !o.json {
            println!("== {f} ({:.2} s)", audio.len() as f64 / 16_000.0);
        }
        let mut core = Core::new(make_vad(o)?, seg_config(o, o.partial_ms[0]));
        core.levels = false;
        run_offline(&mut core, engine.as_mut(), &intents, &audio, 320, &out);
    }
    Ok(())
}

#[cfg(feature = "pipewire")]
fn cmd_devices(o: &Opts) -> Result<(), String> {
    let s = omarchyvr_voice::capture::list_sources()?;
    let pick = omarchyvr_voice::capture::select(&s, &o.device).map(|x| x.id);
    for x in &s {
        println!(
            "{} {:4} {}  ({})",
            if Some(x.id) == pick { "*" } else { " " },
            x.id,
            x.description,
            x.name
        );
    }
    if pick.is_none() {
        println!(
            "(no source matches {:?}: `listen` uses the default source)",
            o.device
        );
    }
    Ok(())
}

#[cfg(not(feature = "pipewire"))]
fn cmd_devices(_o: &Opts) -> Result<(), String> {
    Err("built without PipeWire".into())
}

fn socket(o: &Opts) -> PathBuf {
    o.socket.clone().unwrap_or_else(protocol::socket_path)
}

/// Subscribe to the listener (reconnecting) and draw the captions on the WayVR panel.
fn cmd_captions(o: &Opts) -> Result<(), String> {
    use std::time::{Duration, Instant};
    let path = socket(o);
    let (tx, rx) = mpsc::channel::<Event>();
    std::thread::spawn(move || loop {
        let tx2 = tx.clone();
        let _ = server::subscribe(&path, move |e| tx2.send(e).is_ok());
        if tx.send(Event::Pong).is_err() {
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
    });
    let mut caps = Captions::new(o.width, o.lines, Duration::from_secs_f64(o.clear_s));
    let mut wv = Wayvr::new(o.wayvrctl.clone(), &o.overlay);
    let mut pending = None;
    let mut fresh = true;
    let mut hidden = false;
    let mut last_flag_check = Instant::now();
    // the voice-AI debug line of `omarchyvr-voice act` (only lines written after this start)
    let dbg_path = act::debug_line_path();
    let mtime = |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let mut dbg_seen = mtime(&dbg_path);
    let mut last_dbg_check = Instant::now();
    eprintln!("captions: socket {} -> WayVR panel {:?}", socket(o).display(), o.overlay);
    while !STOP.load(Ordering::Relaxed) {
        let now = Instant::now();
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(e) => {
                if let Some(v) = caps.on_event(&e, now) {
                    pending = Some(v);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if last_dbg_check.elapsed() >= Duration::from_millis(150) {
            last_dbg_check = now;
            let m = mtime(&dbg_path);
            if m.is_some() && m != dbg_seen {
                dbg_seen = m;
                let line = std::fs::read_to_string(&dbg_path).unwrap_or_default();
                if let Some(v) = caps.set_debug(line.lines().next().unwrap_or(""), now) {
                    pending = Some(v);
                }
            }
        }
        if let Some(v) = caps.tick(now) {
            pending = Some(v);
        }
        // "Pimax, captions off": a flag file (omarchyvr-voice act); checked twice a second
        if last_flag_check.elapsed() >= Duration::from_millis(500) {
            last_flag_check = now;
            let off = act::captions_off_path().exists();
            if off != hidden {
                hidden = off;
                pending = Some(if off { Default::default() } else { caps.resync() });
            }
        }
        if hidden && pending.as_ref().is_some_and(|v| v.visible) {
            pending = None;
        }
        if fresh {
            pending = Some(if hidden { Default::default() } else { caps.resync() });
        }
        if let Some(v) = pending.take() {
            let mut f = false;
            if wv.show(&v, &mut f) {
                fresh = false;
            } else {
                fresh = true;
                pending = None; // resynced from the state when the link is back
            }
            if f {
                fresh = false;
            }
        }
    }
    wv.stop();
    Ok(())
}

fn actor(o: &Opts) -> Result<Actor, String> {
    let cat = Catalogue::load_default()?;
    for n in &cat.notes {
        eprintln!("voice: catalogue: {n}");
    }
    let state = if o.dry_run {
        ActState::default()
    } else {
        ActState::from_text(&std::fs::read_to_string(act::state_path()).unwrap_or_default())
    };
    let wayvrctl = o.wayvrctl_set.then(|| o.wayvrctl.to_string_lossy().into_owned());
    let mut a = Actor::new(cat, wayvrctl, state);
    match omarchyvr_voice::llm::Backend::from_config(&a.cat) {
        Ok(Some(b)) => {
            eprintln!("voice: LLM fallback: {}", b.describe());
            let cat = a.cat.clone();
            a.llm = Some(Box::new(move |t: &str| b.classify(&cat, t)));
        }
        Ok(None) => eprintln!("voice: LLM fallback off"),
        Err(e) => eprintln!("voice: LLM fallback off: {e}"),
    }
    // the AI agent with tools: ~/.config/omarchyvr/ai.toml (voice_ai = off | fallback | always; no file = off)
    let ai_toml = omarchyvr_config::ai::default_path().unwrap_or_else(omarchyvr_voice::llm::ai_path);
    let debug_cfg = load_agent(&mut a, &ai_toml, o.dry_run);
    // the debug line under the captions: "Max, debug on/off" wins over voice_ai_debug
    a.debug = act::debug_enabled(debug_cfg);
    // voice_confirm (none by default: every command runs at once; everything is logged)
    a.confirm = std::fs::read_to_string(&ai_toml).ok().and_then(|t| omarchyvr_config::ai::AiConfig::parse(&t).ok()).map(|c| c.voice_confirm).unwrap_or_default();
    eprintln!("voice: confirm: {}", a.confirm.id());
    // voice_listen: how long a held cut-off sentence waits for the next utterance (the catalogue value is the "quick" default)
    let listen = saved_listen();
    if a.cat.wake.followup_s == VoiceListen::Normal.followup_s() {
        a.cat.wake.followup_s = listen.followup_s();
    }
    eprintln!("voice: listening: {} (endpoint {} ms, after a dangling word {} ms, follow-up {} s)", listen.id(), o.endpoint_ms, o.hangover_ms, a.cat.wake.followup_s);
    // the window AI switch: ~/.config/omarchyvr/ai.toml (no file = off)
    match omarchyvr_voice::llm::ChatClient::from_ai_toml(&omarchyvr_voice::llm::ai_path()) {
        Ok(Some(c)) => {
            eprintln!("voice: AI switch: {}", omarchyvr_voice::llm::LlmClient::describe(&c));
            a.ai = Some(Box::new(c));
        }
        Ok(None) => eprintln!("voice: AI switch off (no provider in {})", omarchyvr_voice::llm::ai_path().display()),
        Err(e) => eprintln!("voice: AI switch off: {e}"),
    }
    Ok(a)
}

fn runner(o: &Opts, a: &Actor) -> Box<dyn Runner> {
    if o.dry_run {
        let home = catalogue::expand(&a.cat.backdrop.wayvr_home);
        Box::new(DryRun { skyboxes: act::list_skyboxes(&home, &a.cat.backdrop.skybox_dir), ..Default::default() })
    } else {
        Box::new(Real::new(catalogue::expand(&a.cat.backdrop.wayvr_home), a.cat.backdrop.skybox_dir.clone(), !o.no_toast, true))
    }
}

/// (Re)load the AI agent from ai.toml into the actor; returns `voice_ai_debug`. Used at start and whenever the file
/// changes (the app's or the watch's "AI runs on" switch): between two commands, so nothing in flight is cut.
fn load_agent(a: &mut Actor, ai_toml: &std::path::Path, dry_run: bool) -> bool {
    let wv = a.wayvrctl.clone();
    let mut debug_cfg = true;
    a.agent = None;
    match omarchyvr_voice::agent::Agent::from_ai_toml(ai_toml, &a.cat, &wv) {
        Ok(Some(ag)) => {
            debug_cfg = ag.settings.debug;
            eprintln!(
                "voice: AI agent: {} [{}] ({}, {}, {} tools, ask below {:.0}% confidence, debug {})",
                ag.describe(),
                ag.mode_label(),
                ag.settings.mode.id(),
                match ag.style() {
                    omarchyvr_voice::agent::Style::Compact => "compact".to_string(),
                    omarchyvr_voice::agent::Style::Tools => "tool calling".to_string(),
                    omarchyvr_voice::agent::Style::State => "claude cli: request + state, calls, done/expect loop".to_string(),
                    omarchyvr_voice::agent::Style::TwoStage => format!(
                        "two stages: classify, then act with the category's tools; fallback {}",
                        ag.fallback.as_ref().map(|f| f.describe()).unwrap_or_else(|| "the matcher".into())
                    ),
                },
                ag.tools.len(),
                ag.settings.confidence * 100.0,
                if ag.settings.debug { "on" } else { "off" }
            );
            if !dry_run {
                ag.warm_up();
            }
            a.agent = Some(ag);
        }
        Ok(None) => eprintln!("voice: AI agent off ({})", ai_toml.display()),
        Err(e) => eprintln!("voice: AI agent off: {}", omarchyvr_voice::agent::redact(&e)),
    }
    debug_cfg
}

/// The watch's "AI runs on" row (WayVR, Work desk): shown, with the current mode and, with two stages, the router
/// and action model buttons. In the background (wayvrctl).
fn watch_ai_label(wayvrctl: Option<&str>, text: &str) {
    let Some(wv) = wayvrctl.map(str::to_string) else { return };
    let cfg = omarchyvr_config::ai::AiConfig::load().unwrap_or_default();
    let cmds = watch_commands(text, &cfg);
    std::thread::spawn(move || {
        for args in cmds {
            let _ = std::process::Command::new(&wv).args(&args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
        }
    });
}

/// The wayvrctl commands of [`watch_ai_label`].
fn watch_commands(_text: &str, cfg: &omarchyvr_config::ai::AiConfig) -> Vec<Vec<String>> {
    use omarchyvr_config::ai::RunsOn;
    let online = cfg.runs_on() == RunsOn::Online;
    let on = cfg.local_models_on() && !online && cfg.voice_ai != omarchyvr_config::ai::VoiceAi::Off;
    let mode = if online { "AI: Online".to_string() } else { format!("Local AI: {}", if on { "On" } else { "Off" }) };
    let preset = cfg.preset().map(|p| p.name()).unwrap_or("Custom");
    let m = |el: &str, rest: &[&str]| {
        let mut v: Vec<String> = ["panel-modify", "watch", el].iter().map(|s| s.to_string()).collect();
        v.extend(rest.iter().map(|s| s.to_string()));
        v
    };
    vec![
        m("btn_ai_mode", &["set-text", "--", &mode]),
        m("btn_ai_preset", &["set-text", "--", preset]),
        m("pimax_ai_models", &["set-visible", if on { "1" } else { "0" }]),
        m("pimax_ai_root", &["set-visible", "1"]),
    ]
}

fn mtime(p: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// `omarchyvr-voice ai-mode [next|simple|two-stage|online|show]`: the watch button's command. Saves "AI runs on" in
/// ai.toml (`[use] voice` and `voice_local` only; keys untouched), brings the local AI to that shape
/// (`omarchyvr-ai up`, detached), and prints the new mode. The running `act` sees the file change by itself.
fn cmd_ai_mode(o: &Opts) -> Result<(), String> {
    use omarchyvr_config::ai::{AiConfig, RunsOn};
    let path = omarchyvr_config::ai::default_path().ok_or("no HOME")?;
    let mut cfg = AiConfig::load_from(&path)?;
    let arg = o.positional.first().map(String::as_str).unwrap_or("show");
    if arg == "show" {
        println!("AI: {}", cfg.runs_on().short());
        return Ok(());
    }
    let (want, note) = ai_mode_apply(&mut cfg, arg)?;
    if o.dry_run {
        println!("dry run: ai.toml not written; would be AI: {}", want.short());
        return Ok(());
    }
    cfg.save_to(&path)?;
    if want != RunsOn::Online && cfg.local_models_on() {
        // the local AI follows (detached: the watch button returns at once)
        let _ = std::process::Command::new("omarchyvr-ai").arg("up").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
    }
    if let Some(n) = note {
        println!("{n}");
    }
    println!("AI: {}", want.short());
    Ok(())
}

/// `ai-mode ARG` on the config: next | simple | two-stage | online | router-next | action-next. Returns the mode and
/// a line about a changed model. Only `[use] voice`, `voice_local` and `[local_two_stage]` change.
fn ai_mode_apply(cfg: &mut omarchyvr_config::ai::AiConfig, arg: &str) -> Result<(omarchyvr_config::ai::RunsOn, Option<String>), String> {
    use omarchyvr_config::ai::RunsOn;
    let mut note = None;
    let online = cfg.runs_on() == RunsOn::Online;
    match arg {
        // the watch (user decision 2026-10-16): Local AI On / Off and Low / Medium / High
        "local-toggle" | "local-on" | "local-off" => {
            let on = match arg {
                "local-on" => true,
                "local-off" => false,
                _ => !(cfg.local_models_on() && !online),
            };
            cfg.set_local_models(on, false)?;
            return Ok((cfg.runs_on(), Some(format!("Local AI: {}", if on { "On" } else { "Off" }))));
        }
        "preset-next" | "low" | "medium" | "high" => {
            let p = match omarchyvr_config::ai::Preset::parse(arg) {
                Some(p) => p,
                None => cfg.preset().unwrap_or(omarchyvr_config::ai::Preset::Medium).next(),
            };
            cfg.set_preset(p);
            if !cfg.local_models_on() || online {
                cfg.set_local_models(true, false)?;
            }
            return Ok((cfg.runs_on(), Some(format!("Local AI: {}", p.name()))));
        }
        _ => {}
    }
    let want = match arg {
        "next" => cfg.next_runs_on(),
        "router-next" | "action-next" => {
            // the next router / action model of the two-stage lists (the watch's model buttons)
            let router = arg == "router-next";
            let list: &[omarchyvr_config::ai::LocalModel] = if router { &omarchyvr_config::ai::ROUTER_MODELS } else { &omarchyvr_config::ai::ACTION_MODELS };
            let t = cfg.local_two_stage.get_or_insert_with(Default::default);
            let cur = if router { &mut t.router } else { &mut t.action };
            let i = list.iter().position(|m| m.id == cur.as_str()).unwrap_or(0);
            *cur = list[(i + 1) % list.len()].id.to_string();
            let label = omarchyvr_config::ai::local_model(cur).map(|m| m.label).unwrap_or("");
            note = Some(format!("{}: {label}", if router { "router" } else { "action model" }));
            cfg.runs_on()
        }
        x => RunsOn::parse(x).ok_or_else(|| format!("ai-mode {x}: local-toggle, local-on, local-off, preset-next, low, medium, high, next, simple, two-stage, online, router-next, action-next or show"))?,
    };
    cfg.set_runs_on(want)?;
    Ok((want, note))
}

/// Voice commands from the listener's final transcripts.
fn cmd_act(o: &Opts) -> Result<(), String> {
    use std::time::{Duration, Instant};
    let mut a = actor(o)?;
    let mut r = runner(o, &a);
    let path = socket(o);
    let (tx, rx) = mpsc::channel::<Event>();
    // "Max, cancel" / "stop" while a task of several steps runs: the task stops before its next step, and the words
    // never reach the command loop (they are no new goal)
    let (busy, cancel, wake) = (a.task_busy.clone(), a.task_cancel.clone(), a.cat.wake.words.clone());
    // the first connection asks for the finals said before act was there (it starts ~12 s after the listener; the
    // first command of the live session 2026-10-16 was lost so); a reconnect never replays
    let mut first = true;
    std::thread::spawn(move || loop {
        let tx2 = tx.clone();
        let (busy, cancel, wake) = (busy.clone(), cancel.clone(), wake.clone());
        let ask = if first { Some(Command::Backlog) } else { None };
        let r = server::subscribe_with(&path, ask, move |e| {
            if let Event::Final { text, .. } = &e {
                if busy.load(Ordering::SeqCst) && omarchyvr_voice::command::is_cancel(text, &wake) {
                    cancel.store(true, Ordering::SeqCst);
                    eprintln!("voice: {text:?} -> cancel the running task");
                    return true;
                }
            }
            tx2.send(e).is_ok()
        });
        if r.is_ok() {
            first = false;
        }
        if tx.send(Event::Pong).is_err() {
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
    });
    eprintln!(
        "voice: act on {} ({} intents enabled{}); say \"Max, help\"",
        socket(o).display(),
        a.cat.enabled().count(),
        if o.dry_run { ", DRY RUN" } else { "" }
    );
    let ai_toml = omarchyvr_config::ai::default_path().unwrap_or_else(omarchyvr_voice::llm::ai_path);
    let mut seen = mtime(&ai_toml);
    let mut checked = Instant::now();
    let label = |a: &Actor| format!("AI: {}", a.agent.as_ref().map(|g| g.mode_label()).unwrap_or("off"));
    if !o.dry_run {
        watch_ai_label(Some(a.wayvrctl.as_str()), &label(&a));
    }
    while !STOP.load(Ordering::Relaxed) {
        // "AI runs on" switched (app or watch): a new agent between two commands
        if checked.elapsed() >= Duration::from_secs(2) {
            checked = Instant::now();
            let m = mtime(&ai_toml);
            if m != seen {
                seen = m;
                let before = a.agent.as_ref().map(|g| g.mode_label());
                a.debug = act::debug_enabled(load_agent(&mut a, &ai_toml, o.dry_run));
                let now = label(&a);
                eprintln!("voice: ai.toml changed: {now}");
                if before != a.agent.as_ref().map(|g| g.mode_label()) {
                    r.toast(&now);
                }
                if !o.dry_run {
                    watch_ai_label(Some(a.wayvrctl.as_str()), &now);
                }
            }
        }
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(Event::Final { text, .. }) => {
                if let Some(id) = a.hear(&text, Instant::now(), r.as_mut()) {
                    eprintln!("voice: {text:?} -> {id}");
                }
            }
            Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        a.tick(Instant::now(), r.as_mut());
    }
    Ok(())
}

/// Typed utterances through the same path.
fn cmd_do(o: &Opts) -> Result<(), String> {
    if o.positional.is_empty() {
        return Err("do needs the text, e.g. omarchyvr-voice do \"pimax cinema mode\" --dry-run".into());
    }
    let mut a = actor(o)?;
    let mut r = runner(o, &a);
    // a fresh server has a cold prompt cache: load it first so the commands below show real latencies
    if let Some(ag) = a.agent.as_mut() {
        match ag.prime_now() {
            Ok(ms) if ms > 0 => eprintln!("voice: AI warm-up {ms} ms"),
            Ok(_) => {}
            Err(e) => eprintln!("voice: AI warm-up failed: {e}"),
        }
    }
    let now = std::time::Instant::now();
    for t in &o.positional {
        let text = if o.no_wake { format!("omarchyvr {t}") } else { t.clone() };
        match a.hear(&text, now, r.as_mut()) {
            Some(id) => println!("{t:?} -> {id}"),
            None => println!("{t:?} -> (no command)"),
        }
    }
    Ok(())
}

/// `hyprctl` / `wayvrctl` for `gaze`: read-only tools, stdout captured, nothing else.
struct ToolRunner;

impl Runner for ToolRunner {
    fn live(&self) -> bool {
        true
    }
    fn run(&mut self, argv: &[String], _capture: bool) -> Result<String, String> {
        let (prog, args) = argv.split_first().ok_or("empty command")?;
        let out = std::process::Command::new(prog)
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| format!("{prog}: {e}"))?;
        if !out.status.success() {
            return Err(format!("{prog} {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
    fn passthrough(&mut self, _line: &str) -> Result<String, String> {
        Err("not used by gaze".into())
    }
    fn toast(&mut self, _text: &str) {}
    fn captions(&mut self, _on: bool) -> Result<(), String> {
        Ok(())
    }
    fn skyboxes(&mut self) -> Vec<String> {
        Vec::new()
    }
}

fn cmd_gaze(o: &Opts) -> Result<(), String> {
    use omarchyvr_voice::gaze;
    let mut r = ToolRunner;
    let wayvrctl = o.wayvrctl.to_string_lossy().into_owned();
    let report = match &o.input {
        Some(f) => gaze::parse_report(&std::fs::read_to_string(f).map_err(|e| format!("{f}: {e}"))?)?,
        None => gaze::read_report(&mut r, &wayvrctl)?,
    };
    let monitors = gaze::parse_monitors(&r.run(&["hyprctl".into(), "-j".into(), "monitors".into()], true)?)?;
    let windows = gaze::parse_windows(&r.run(&["hyprctl".into(), "-j".into(), "clients".into()], true)?)?;
    let t = gaze::resolve_from(&report, &monitors, &windows, o.at_ms);
    if o.json {
        let v = serde_json::json!({ "report": report, "target": t });
        println!("{v}");
        return Ok(());
    }
    if !report.tracking {
        println!("gaze tracking is off in WayVR (eye tracking not started, or gaze_target off)");
    }
    match &report.current {
        Some(c) => println!("now: {} ms on {} uv {:.3},{:.3} (conf {:.2})", c.duration_ms, c.overlay, c.uv[0], c.uv[1], c.confidence),
        None => println!("now: {}", if report.valid { "not on an overlay" } else { "no valid gaze (blink / lost)" }),
    }
    match t {
        Some(t) => println!("target: {}", gaze::describe(&t)),
        None => println!("target: none (no fixation of {} ms or more recently)", report.min_fixation_ms),
    }
    Ok(())
}

fn cmd_intents(o: &Opts) -> Result<(), String> {
    let cat = Catalogue::load_default()?;
    let a = Actor::new(cat, None, ActState::default());
    for l in a.list(o.all) {
        println!("{l}");
    }
    println!("wake words: {}", a.cat.wake.words.join(", "));
    println!("user override: {}", catalogue::user_path().display());
    for n in &a.cat.notes {
        println!("note: {n}");
    }
    Ok(())
}

fn cmd_tail(o: &Opts) -> Result<(), String> {
    let levels = o.levels;
    server::subscribe(&socket(o), |e| {
        if o.json {
            println!("{}", protocol::encode(&e));
        } else if let Some(l) = Stdout::format(&e, levels) {
            println!("{l}");
        }
        true
    })
}

fn cmd_bench(o: &Opts) -> Result<(), String> {
    let clips = bench::load_corpus(&o.clips);
    if clips.is_empty() {
        return Err(format!(
            "no clips in {} (omarchyvr-voicebench bench --out DIR writes DIR/clips/*-16k.wav)",
            o.clips.display()
        ));
    }
    let mut runs = Vec::new();
    for &t in &o.threads {
        for &p in &o.partial_ms {
            let label = format!(
                "{} threads={t} partials={} endpoint={} ms vad={}",
                o.engine,
                if p == 0 {
                    "off".to_string()
                } else {
                    format!("{p} ms")
                },
                o.endpoint_ms,
                o.vad.clone().unwrap_or_else(|| "auto".into())
            );
            eprintln!(
                "bench: {label} ({} clips, about {:.0} s)",
                clips.len(),
                clips
                    .iter()
                    .map(|c| c.samples.len() as f64 / 16e3 + o.gap)
                    .sum::<f64>()
                    + 5.0
            );
            let run = bench::run(bench::BenchSetup {
                label,
                engine: engine_spec(o, t)?,
                vad: make_vad(o)?,
                seg: seg_config(o, p),
                clips: &clips,
                gap_s: o.gap,
                noise: o.noise,
            })?;
            eprint!("{}", bench::markdown(std::slice::from_ref(&run)));
            runs.push(run);
        }
    }
    let md = bench::markdown(&runs);
    if let Some(p) = &o.out {
        std::fs::write(p, &md).map_err(|e| format!("{}: {e}", p.display()))?;
    } else {
        print!("{md}");
    }
    Ok(())
}

/// The built-in AI bench phrases.
const AIBENCH: &str = include_str!("../tests/fixtures/aibench.tsv");

/// Read-only host access for the AI bench: hyprctl -j, wayvrctl gaze and the passthrough status are real;
/// everything else is only recorded.
struct BenchRunner {
    ran: Vec<String>,
}

impl Runner for BenchRunner {
    fn run(&mut self, argv: &[String], capture: bool) -> Result<String, String> {
        let read_only = (argv.first().is_some_and(|p| p == "hyprctl") && argv.get(1).is_some_and(|a| a == "-j"))
            || argv.get(1).is_some_and(|a| a == "gaze");
        if read_only {
            return ToolRunner.run(argv, capture);
        }
        self.ran.push(argv.join(" "));
        Ok(String::new())
    }
    fn passthrough(&mut self, line: &str) -> Result<String, String> {
        if line == "status" {
            Real::new(PathBuf::new(), String::new(), false, false).passthrough(line)
        } else {
            Ok(format!("ok {line}"))
        }
    }
    fn toast(&mut self, _text: &str) {}
    fn captions(&mut self, _on: bool) -> Result<(), String> {
        Ok(())
    }
    fn skyboxes(&mut self) -> Vec<String> {
        Vec::new()
    }
}

fn percentile(v: &[u64], p: f64) -> u64 {
    if v.is_empty() {
        return 0;
    }
    let mut s = v.to_vec();
    s.sort();
    s[((s.len() - 1) as f64 * p).round() as usize]
}

/// The voice AI against the running model: accuracy and latency over the fixture phrases.
fn cmd_aibench(o: &Opts) -> Result<(), String> {
    use omarchyvr_voice::agent::{tool_name, Agent};
    use std::time::Instant;
    let text = match &o.fixture {
        Some(f) => std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?,
        None => AIBENCH.to_string(),
    };
    // (expected, heard, slots)
    let mut rows: Vec<(String, String, Vec<(String, String)>)> = Vec::new();
    for l in text.lines() {
        let l = l.split("\t#").next().unwrap_or("").trim_end();
        if l.trim().is_empty() || l.trim_start().starts_with('#') {
            continue;
        }
        let mut f = l.split('\t');
        let (Some(exp), Some(heard)) = (f.next(), f.next()) else { continue };
        let slots = f.next().unwrap_or("").split(',').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect();
        rows.push((exp.trim().to_string(), heard.trim().to_string(), slots));
    }
    let cat = Catalogue::load_default()?;
    let wv = o.wayvrctl.to_string_lossy().into_owned();
    let ai_toml = omarchyvr_config::ai::default_path().unwrap_or_else(omarchyvr_voice::llm::ai_path);
    let mut ag = Agent::from_ai_toml(&ai_toml, &cat, &wv)?.ok_or("the voice AI is off in ai.toml")?;
    ag.log = None;
    match o.style.as_deref() {
        Some("tools") => ag.settings.style = Some(omarchyvr_voice::agent::Style::Tools),
        Some("compact") => ag.settings.style = Some(omarchyvr_voice::agent::Style::Compact),
        Some(x) => return Err(format!("--style {x}: compact or tools")),
        None => {}
    }
    if let Some(t) = o.timeout_s {
        ag.settings.timeout = Some(std::time::Duration::from_secs_f64(t));
    }
    if let Some(u) = &o.url {
        // another local server (a model / thread / priority comparison)
        ag.provider = omarchyvr_voice::agent::Provider::new("bench", omarchyvr_voice::agent::Wire::OpenAi, u, omarchyvr_voice::agent::LOCAL_MODEL, None, Box::new(omarchyvr_voice::agent::Curl));
    }

    let label = o.label.clone().unwrap_or_else(|| ag.provider.describe());
    eprintln!("aibench: {label}: {} phrases x {} runs, {} style, timeout {:?}", rows.len(), o.runs, if ag.compact() { "compact" } else { "tools" }, ag.timeout());
    eprintln!("aibench: system prompt {} chars (~{} tokens)", ag.nlu_system().len(), ag.nlu_system().len() / 4);
    // warm the prompt cache first (not measured)
    {
        let t0 = Instant::now();
        let r = ag.prime_now();
        eprintln!("aibench: warm-up {} ms {:?}", t0.elapsed().as_millis(), r.err());
    }
    let mut a = Actor::new(cat, None, ActState::default());
    a.agent = Some(ag);
    let mut md = format!("### aibench: {label}\n\n| expected | heard | route | decision | ran | ok | ms |\n|---|---|---|---|---|---|---|\n");
    let (mut n, mut good, mut asked, mut wrong, mut missed) = (0, 0, 0, 0, 0);
    let mut all_ms = Vec::new();
    let mut ai_ms = Vec::new();
    let mut llm_ms = Vec::new();
    for run in 0..o.runs.max(1) {
        for (exp, heard, slots) in &rows {
            a.reset();
            let mut r = BenchRunner { ran: vec![] };
            let out = a.hear(&format!("Max {heard}"), Instant::now(), &mut r);
            let t = a.trace.clone().unwrap_or_default();
            let ran = out.as_deref().map(|x| x.trim_start_matches("ai:").trim_end_matches('?').to_string());
            let hit_id = ran.as_deref().is_some_and(|x| tool_name(x) == tool_name(exp));
            let slots_ok = slots.iter().all(|(k, v)| {
                // ask-back / matcher slots are in the pending match; the decision string carries model slots
                t.decision.contains(&format!("{k}={v}")) || r.ran.iter().any(|x| x.contains(v.as_str())) || t.route != "ai"
            });
            let ok = if exp == "none" { out.is_none() } else { hit_id && slots_ok };
            n += 1;
            if ok {
                good += 1;
                if t.asked && exp != "none" {
                    asked += 1;
                }
            } else if out.is_some() && !t.asked {
                wrong += 1;
            } else {
                missed += 1;
            }
            all_ms.push(t.ms);
            if t.route.starts_with("ai") {
                ai_ms.push(t.ms);
                if let Some(l) = t.llm_ms {
                    llm_ms.push(l);
                }
            }
            if run == 0 {
                md += &format!(
                    "| {exp} | {heard} | {} | {} | {}{} | {} | {} |\n",
                    t.route,
                    t.decision,
                    ran.as_deref().unwrap_or("-"),
                    if t.asked { " (asked)" } else { "" },
                    if ok { "yes" } else { "**no**" },
                    t.ms
                );
            }
            eprintln!("aibench: [{run}] {heard:?} -> {} {} {:?}{} {} ms", t.route, t.decision, ran, if t.asked { " (asked)" } else { "" }, t.ms);
        }
    }
    md += &format!(
        "\n{n} requests: **{:.0}% right** ({good}; {asked} of them as a \"Did you mean\" / confirm question), {wrong} wrong actions, {missed} missed.\n\
         Latency, all requests: p50 {} ms, p95 {} ms. Requests that went to the model ({}): p50 {} ms, p95 {} ms (model round trip p50 {} ms, p95 {} ms).\n",
        100.0 * good as f64 / n.max(1) as f64,
        percentile(&all_ms, 0.5),
        percentile(&all_ms, 0.95),
        ai_ms.len(),
        percentile(&ai_ms, 0.5),
        percentile(&ai_ms, 0.95),
        percentile(&llm_ms, 0.5),
        percentile(&llm_ms, 0.95),
    );
    match &o.out {
        Some(p) => {
            let mut prev = std::fs::read_to_string(p).unwrap_or_default();
            prev += &md;
            prev.push('\n');
            std::fs::write(p, prev).map_err(|e| format!("{}: {e}", p.display()))?;
            print!("{}", md.lines().rev().take(2).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n"));
            println!();
        }
        None => print!("{md}"),
    }
    Ok(())
}

/// The audit over every intent (dry run), optionally live checks, as an HTML page.
/// The phrases the matcher leaves to the model (exact, near-exact and noise never reach it), at most `max` spread
/// over the intents.
fn hard_cases(cat: &Catalogue, cases: &[omarchyvr_voice::audit::Case], max: usize) -> Vec<omarchyvr_voice::audit::Case> {
    use omarchyvr_voice::audit;
    use std::time::Instant;
    let mut probe = Actor::new(cat.clone(), None, ActState::default());
    probe.confirm = omarchyvr_config::ai::VoiceConfirm::None;
    let left: Vec<audit::Case> = cases
        .iter()
        .filter(|c| {
            probe.reset();
            let mut r = BenchRunner { ran: vec![] };
            let out = probe.hear(&format!("Max {}", c.heard), Instant::now(), &mut r);
            let t = probe.trace.clone().unwrap_or_default();
            t.route != "noise" && (out.is_none() || t.asked)
        })
        .cloned()
        .collect();
    audit::spread(&left, max)
}

/// `routerbench`: stage 1 alone over the audit phrases.
fn cmd_routerbench(o: &Opts) -> Result<(), String> {
    use omarchyvr_voice::audit;
    use omarchyvr_voice::remote::{self, category_of_intent, Category};
    use std::time::{Duration, Instant};
    let cat = Catalogue::load_default()?;
    let fixture = match &o.fixture {
        Some(f) => std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?,
        None => audit::FIXTURE.to_string(),
    };
    let mut cases = audit::cases(&cat, &fixture);
    let truth = |want: &str| if want == "none" || want.is_empty() { Category::Other } else { category_of_intent(want.trim_start_matches("ai:")) };
    if let Some(out) = &o.out {
        // --out FILE.jsonl: Laya training rows from the catalogue (the audit's natural phrasings stay out)
        let mut text = String::new();
        let q = serde_json::json!({"category": {"type": "choice", "instructions": omarchyvr_voice::router::LAYA_INSTRUCTIONS, "criteria": omarchyvr_voice::router::laya_criteria()}});
        let train = audit::train_cases(&cat);
        for c in &train {
            text += &serde_json::json!({"state": omarchyvr_voice::router::laya_body(&c.heard, "lara")["state"], "questions": q, "expected": {"category": truth(&c.want).id()}}).to_string();
            text.push('\n');
        }
        std::fs::write(out, text).map_err(|e| format!("{}: {e}", out.display()))?;
        println!("{} training rows -> {}", train.len(), out.display());
        return Ok(());
    }
    if o.hard {
        cases = hard_cases(&cat, &cases, o.max.unwrap_or(40));
    } else if o.label.as_deref() == Some("held-out") {
        cases.retain(|c| c.kind == "natural");
    }
    let cfg = omarchyvr_config::ai::AiConfig::load()?;
    let mut rt = omarchyvr_voice::router::from_config(&cfg).ok_or("ai.toml: voice_local is not \"two-stage\"")?;
    let sys = remote::classify_system();
    let (mut top, mut inset, mut guess_ok, mut n, mut errs, mut union) = (0, 0, 0, 0, 0, 0);
    let mut ms: Vec<u64> = Vec::new();
    let mut ntools: Vec<usize> = Vec::new();
    let tools = omarchyvr_voice::agent::tools(&cat);
    for c in &cases {
        let want = truth(&c.want);
        let words = omarchyvr_voice::command::words(&c.heard);
        let cands = omarchyvr_voice::fuzzy::candidates(&cat, &words, 3);
        let g: Vec<Category> = remote::local_guess(&words, &cands).into_iter().map(|g| g.0).collect();
        let t0 = Instant::now();
        let res = rt.classify(&c.heard, "", &sys, Duration::from_secs(10));
        let t = t0.elapsed().as_millis() as u64;
        n += 1;
        guess_ok += usize::from(g.first() == Some(&want));
        match res {
            Ok(cl) => {
                ms.push(t);
                let cats = cl.cats();
                let both = remote::with_guess(&cats, &g);
                union += usize::from(both.contains(&want));
                top += usize::from(cl.category == want);
                inset += usize::from(cats.contains(&want));
                ntools.push(remote::subset(&tools, &cats).len());
                let mark = if cl.category == want { "ok " } else if cats.contains(&want) { "in " } else { "BAD" };
                println!("{mark} {t:4}ms {:12} -> {:28} {}", want.id(), cats.iter().map(|c| c.id()).collect::<Vec<_>>().join(",") + &format!(" {:.2}", cl.confidence), c.heard);
            }
            Err(e) => {
                errs += 1;
                println!("ERR {t:4}ms {e}");
            }
        }
    }
    ms.sort();
    ntools.sort();
    let pct = |v: &[u64], q: f64| v.get(((v.len().max(1) - 1) as f64 * q).round() as usize).copied().unwrap_or(0);
    let pc = |x: usize| 100.0 * x as f64 / n.max(1) as f64;
    println!(
        "routerbench {} ({} phrases{}): top-1 {top} ({:.0}%), right category offered {inset} ({:.0}%), with the matcher's guess {union} ({:.0}%), matcher guess top-1 {guess_ok} ({:.0}%), errors {errs}; p50 {} ms, p95 {} ms; tools offered median {}",
        rt.describe(),
        n,
        if o.hard { ", hard" } else { "" },
        pc(top),
        pc(inset),
        pc(union),
        pc(guess_ok),
        pct(&ms, 0.5),
        pct(&ms, 0.95),
        ntools.get(ntools.len() / 2).copied().unwrap_or(0)
    );
    Ok(())
}

fn cmd_audit(o: &Opts) -> Result<(), String> {
    use omarchyvr_voice::agent::Agent;
    use omarchyvr_voice::audit;
    use std::time::Instant;
    let cat = Catalogue::load_default()?;
    let fixture = match &o.fixture {
        Some(f) => std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?,
        None => audit::FIXTURE.to_string(),
    };
    let mut cases = audit::cases(&cat, &fixture);
    let wv = o.wayvrctl.to_string_lossy().into_owned();
    if o.remote || o.hard {
        if o.live {
            return Err("--remote / --hard are dry runs only: no --live".into());
        }
        // the phrases the matcher leaves to the model (exact, near-exact and noise never reach a provider)
        if o.fixture.is_some() {
            // a given fixture: only its phrases
            cases.retain(|c| c.kind == "natural");
        }
        let all = cases.len();
        cases = hard_cases(&cat, &cases, o.max.unwrap_or(40));
        eprintln!("audit: {} of {all} phrases picked among those the matcher leaves to the model", cases.len());
    }
    let ai_toml = omarchyvr_config::ai::default_path().unwrap_or_else(omarchyvr_voice::llm::ai_path);
    let mut ag = Agent::from_ai_toml(&ai_toml, &cat, &wv)?;
    if let (Some(a), Some(u)) = (ag.as_mut(), &o.url) {
        a.provider = omarchyvr_voice::agent::Provider::new("audit", omarchyvr_voice::agent::Wire::OpenAi, u, omarchyvr_voice::agent::LOCAL_MODEL, None, Box::new(omarchyvr_voice::agent::Curl));
    }
    if let Some(a) = ag.as_mut() {
        a.dry_run = true;
        if !o.log {
            a.log = None;
        }
        if let Some(m) = &o.action_model {
            a.provider.action_model = Some(m.clone());
        }
        if o.remote && a.provider.local {
            return Err(format!("--remote: the voice provider {} is on this machine (ai.toml [use] voice)", a.provider.describe()));
        }
        let r = a.prime_now();
        eprintln!("audit: model {} warm {:?}", a.provider.describe(), r);
    } else if o.remote {
        return Err("--remote: no voice AI (ai.toml voice_ai is off or missing)".into());
    }
    let label = o.label.clone().unwrap_or_else(|| ag.as_ref().map(|a| a.describe()).unwrap_or_else(|| "no AI".into()));
    let mut a = Actor::new(cat, None, ActState::default());
    a.agent = ag;
    a.confirm = omarchyvr_config::ai::VoiceConfirm::None; // the user's default: no questions
    eprintln!("audit: {} phrases ({label})", cases.len());
    let mut out = Vec::new();
    let mut json = String::new();
    let mut costs: Vec<f64> = Vec::new();
    let mut model_ms: Vec<u64> = Vec::new();
    for c in &cases {
        a.reset();
        let mut r = BenchRunner { ran: vec![] };
        let t0 = Instant::now();
        let ran = a.hear(&format!("Max {}", c.heard), Instant::now(), &mut r);
        let ms = t0.elapsed().as_millis() as u64;
        let t = a.trace.clone().unwrap_or_default();
        let (verdict, why) = audit::judge_full(c, ran.as_deref(), t.asked, &t.slots, &t.decision, &t.result);
        eprintln!("audit: {verdict:4} {:28} {:44} -> {:6} {} {} {ms} ms {why}", c.want, c.heard, t.route, ran.as_deref().unwrap_or("-"), t.decision);
        json += &serde_json::json!({"want": c.want, "heard": c.heard, "kind": c.kind, "route": t.route, "ran": ran, "asked": t.asked, "decision": t.decision, "slots": t.slots, "ms": ms, "verdict": verdict, "why": why, "cost": t.cost, "prompt": t.prompt}).to_string();
        json.push('\n');
        if let Some(x) = t.cost {
            costs.push(x);
        }
        if t.route.starts_with("ai") {
            model_ms.push(ms);
        }
        if t.route.starts_with("ai") && o.pause_ms > 0 {
            // space the model requests out: a burst keeps llama-server on every CPU thread and the desktop lags
            std::thread::sleep(std::time::Duration::from_millis(o.pause_ms));
        }
        out.push(audit::Outcome { case: c.clone(), route: t.route, ran: ran.unwrap_or_else(|| "-".into()), asked: t.asked, decision: t.decision, slots: t.slots, result: t.result, ms, verdict, why, prompt: t.prompt });
    }
    let live = if o.live { audit_live()? } else { Vec::new() };
    let (p, ak, f, n) = audit::tally(&out);
    println!("audit: {p}/{n} pass ({:.0}%), {ak} asked, {f} fail", 100.0 * p as f64 / n.max(1) as f64);
    let pct = |v: &[u64], q: f64| -> u64 {
        let mut s = v.to_vec();
        s.sort();
        s.get(((s.len().max(1) - 1) as f64 * q).round() as usize).copied().unwrap_or(0)
    };
    let total: f64 = costs.iter().sum::<f64>() + 0.0;
    let cost_line = format!(
        "{} model requests: p50 {} ms, p95 {} ms; cost ${total:.5} total{}",
        model_ms.len(),
        pct(&model_ms, 0.5),
        pct(&model_ms, 0.95),
        if costs.is_empty() { " (the provider reported none)".to_string() } else { format!(", ${:.6} per command", total / costs.len() as f64) }
    );
    println!("audit: {cost_line}");
    for (name, ok, d) in &live {
        println!("live: {} {name} {d}", if *ok { "pass" } else { "FAIL" });
    }
    if let Some(j) = &o.json_out {
        std::fs::write(j, json).map_err(|e| format!("{}: {e}", j.display()))?;
    }
    if let Some(h) = &o.out {
        let sub = if o.remote {
            format!("{n} phrases the matcher leaves to the model (natural and ASR-mangled), spread over the intents, dry run through the remote two-stage route. voice_confirm = none. Model: {label}. {cost_line}.")
        } else {
            format!("{} phrases: each intent's first catalogue phrase, an ASR-mangled copy and natural phrasings, dry run through exact / fuzzy / model. voice_confirm = none. Model: {label}.", n)
        };
        std::fs::write(h, audit::html("Voice command audit", &sub, &[("All intents", out)], &live)).map_err(|e| format!("{}: {e}", h.display()))?;
        println!("wrote {}", h.display());
    }
    Ok(())
}

/// Live checks on the desktop against a scratch window the audit opens itself: a `foot` terminal running
/// `cat > FILE` (canonical tty mode: a line reaches FILE at Enter, Backspace edits the line), closed after by its
/// PID. Only typing and keys are sent live, and only while the scratch window has the focus (checked before every
/// step; else the step is skipped). Browser, window and workspace intents stay in the dry run.
fn audit_live() -> Result<Vec<(String, bool, String)>, String> {
    use std::time::{Duration, Instant};
    let sh = |argv: &[&str]| -> String {
        std::process::Command::new(argv[0]).args(&argv[1..]).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
    };
    let active = || -> serde_json::Value { serde_json::from_str(&sh(&["hyprctl", "-j", "activewindow"])).unwrap_or_default() };
    let orig_addr = active()["address"].as_str().unwrap_or("").to_string();
    let dir = std::env::temp_dir().join(format!("pimax-audit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let file = dir.join("typed.txt");
    let _ = std::fs::remove_file(&file);
    let title = "pimax-audit-scratch";
    let mut child = std::process::Command::new("foot")
        .args(["--title", title, "--app-id", "pimax-audit-scratch", "sh", "-c", &format!("cat > '{}'", file.display())])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("foot: {e}"))?;
    let pid = child.id() as i64;
    let t0 = Instant::now();
    let scratch = loop {
        let cs: serde_json::Value = serde_json::from_str(&sh(&["hyprctl", "-j", "clients"])).unwrap_or_default();
        if let Some(c) = cs.as_array().into_iter().flatten().find(|c| c["pid"].as_i64() == Some(pid)) {
            break c["address"].as_str().unwrap_or("").to_string();
        }
        if t0.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("the scratch window did not open".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    std::thread::sleep(Duration::from_millis(500));
    let mut a = Actor::new(Catalogue::load_default()?, None, ActState::default());
    a.confirm = omarchyvr_config::ai::VoiceConfirm::None;
    let mut r = Real::new(PathBuf::new(), String::new(), false, false);
    let mut res: Vec<(String, bool, String)> = Vec::new();
    let mut sent: Vec<String> = Vec::new();
    // one phrase, only while our window is focused (focus it first; never type anywhere else)
    let say = |a: &mut Actor, r: &mut Real, words: &str, sent: &mut Vec<String>| -> Option<String> {
        let _ = std::process::Command::new("hyprctl").args(["dispatch", &format!("hl.dsp.focus({{ window = \"address:{scratch}\" }})")]).output();
        std::thread::sleep(Duration::from_millis(150));
        if active()["address"].as_str() != Some(scratch.as_str()) {
            return None;
        }
        a.reset();
        let id = a.hear(&format!("Max {words}"), Instant::now(), r).unwrap_or_else(|| "-".into());
        sent.push(format!("{words} -> {id}"));
        std::thread::sleep(Duration::from_millis(250));
        Some(id)
    };
    let mut ok_steps = true;
    for w in ["type hello pimax", "press enter", "write line two", "press enter", "type abcd", "press backspace", "press enter"] {
        if say(&mut a, &mut r, w, &mut sent).is_none() {
            ok_steps = false;
            res.push((format!("\"{w}\""), false, "skipped: the scratch window lost the focus".into()));
            break;
        }
    }
    std::thread::sleep(Duration::from_millis(300));
    let got = std::fs::read_to_string(&file).unwrap_or_default();
    if ok_steps {
        res.push(("type.text (first character kept) + key.enter".into(), got.starts_with("hello pimax\nline two\n"), format!("cat got {got:?}")));
        res.push(("key.press backspace".into(), got.ends_with("abc\n"), format!("cat got {got:?}")));
    }
    // clean up: our process (by PID), the file, the focus
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
    std::thread::sleep(Duration::from_millis(200));
    if !orig_addr.is_empty() {
        let _ = std::process::Command::new("hyprctl").args(["dispatch", &format!("hl.dsp.focus({{ window = \"address:{orig_addr}\" }})")]).output();
    }
    res.push(("sent live (to the scratch terminal only)".into(), true, sent.join(" | ")));
    Ok(res)
}

fn cmd_vad(o: &Opts) -> Result<(), String> {
    let Some(f) = o.positional.first() else {
        return Err("vad needs a WAV file".into());
    };
    let audio = source::load_wav_16k(std::path::Path::new(f))?;
    let mut vad = make_vad(o)?;
    let line: String = audio
        .as_chunks::<{ omarchyvr_voice::vad::FRAME }>()
        .0
        .iter()
        .map(|fr| if vad.is_speech(fr) { '#' } else { '.' })
        .collect();
    println!(
        "{} VAD, {} frames of 32 ms ({:.2} s); 10 chars = 320 ms",
        vad.name(),
        line.len(),
        audio.len() as f64 / 16e3
    );
    for (i, c) in line.as_bytes().chunks(100).enumerate() {
        println!("{:7.2}s {}", i as f64 * 3.2, String::from_utf8_lossy(c));
    }
    Ok(())
}

fn cmd_info(o: &Opts) -> Result<(), String> {
    let lib = omarchyvr_voicebench::sherpa::Lib::load(&o.lib).map_err(|e| e.to_string());
    match &lib {
        Ok(l) => println!(
            "sherpa-onnx {} ({}) at {}",
            l.version(),
            l.git_sha(),
            o.lib.display()
        ),
        Err(e) => println!("sherpa-onnx library: {e}"),
    }
    for f in [
        "encoder.int8.onnx",
        "decoder.int8.onnx",
        "joiner.int8.onnx",
        "tokens.txt",
    ] {
        let p = o.model.join(f);
        println!(
            "{} {}",
            if p.exists() { "ok     " } else { "MISSING" },
            p.display()
        );
    }
    println!(
        "{} {} (Silero VAD)",
        if o.vad_model.exists() {
            "ok     "
        } else {
            "MISSING"
        },
        o.vad_model.display()
    );
    println!("socket {}", socket(o).display());
    println!(
        "features: pipewire={} whisper={}",
        cfg!(feature = "pipewire"),
        cfg!(feature = "whisper")
    );
    lib.map(|_| ())
}

fn run(args: &[String]) -> Result<(), String> {
    let Some(cmd) = args.first() else {
        return Err(HELP.into());
    };
    // the state audit has its own options (src/statetest.rs)
    if cmd == "audit" && args.iter().any(|a| a == "--state") {
        return omarchyvr_voice::statetest::cli(&args[1..]);
    }
    let mut o = parse(&args[1..])?;
    if o.engine == "auto" && !matches!(cmd.as_str(), "listen" | "transcribe") {
        // benchmarks and tools measure one fixed engine
        o.engine = "parakeet".into();
    }
    match cmd.as_str() {
        "listen" => cmd_listen(&o),
        "transcribe" => cmd_transcribe(&o),
        "devices" => cmd_devices(&o),
        "tail" => cmd_tail(&o),
        "captions" => {
            install_signals();
            cmd_captions(&o)
        }
        "pause" => server::send_command(&socket(&o), Command::Pause),
        "resume" => server::send_command(&socket(&o), Command::Resume),
        "ping" => {
            server::send_command(&socket(&o), Command::Ping)?;
            println!("ok");
            Ok(())
        }
        "act" => {
            install_signals();
            cmd_act(&o)
        }
        "do" => cmd_do(&o),
        "intents" => cmd_intents(&o),
        "gaze" => cmd_gaze(&o),
        "bench" => cmd_bench(&o),
        "aibench" => cmd_aibench(&o),
        "audit" => cmd_audit(&o),
        "routerbench" => cmd_routerbench(&o),
        "ai-mode" => cmd_ai_mode(&o),
        "claude-live" => {
            // the real `claude` CLI through the Claude CLI code path over a simulated desktop (nothing real is touched)
            // CLAUDE_LIVE_MODEL (default haiku), CLAUDE_LIVE_NO_PREWARM=1
            let model = std::env::var("CLAUDE_LIVE_MODEL").unwrap_or_else(|_| "haiku".to_string());
            print!("{}", omarchyvr_voice::claudelive::run(&o.positional, &model, std::env::var_os("CLAUDE_LIVE_NO_PREWARM").is_none())?);
            Ok(())
        }
        "ai-state" => {
            let cat = Catalogue::load_default()?;
            println!("{}", omarchyvr_voice::statetest::ai_state(&o.positional.join(" "), &cat));
            Ok(())
        }
        "ai-size" => {
            // what each AI request kind sends for a typical state (characters, approximate tokens)
            let cat = Catalogue::load_default()?;
            print!("{}", omarchyvr_voice::agent::size_report(&cat));
            Ok(())
        }
        "aiprompt" => {
            // the compact request body for some words (no live context): for debugging and the docs
            let cat = Catalogue::load_default()?;
            let words = omarchyvr_voice::command::words(&o.positional.join(" "));
            let cands = omarchyvr_voice::fuzzy::candidates(&cat, &words, 3);
            let ctx = omarchyvr_voice::agent::Context { request: words.join(" "), ..Default::default() };
            let (user, _) = omarchyvr_voice::nlu::user_message(&ctx, &cands, &ctx.request, false);
            let p = omarchyvr_voice::agent::Provider::local_default(Box::new(omarchyvr_voice::agent::Curl));
            println!("{}", p.nlu_body(&omarchyvr_voice::nlu::system_prompt(&cat), &user, &omarchyvr_voice::nlu::grammar(&cat)));
            Ok(())
        }
        "info" => cmd_info(&o),
        "vad" => cmd_vad(&o),
        "help" | "--help" | "-h" => {
            println!("{HELP}");
            Ok(())
        }
        other => Err(format!("unknown command {other:?}\n\n{HELP}")),
    }
}

fn main() -> ExitCode {
    // run from a WayVR watch button (`ai-mode`): the user's own XDG directories, not WayVR's isolated ones (else
    // ai.toml would be read and written in the WayVR home, not ~/.config/omarchyvr)
    omarchyvr_config::paths::restore_user_xdg();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("omarchyvr-voice: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod listen_tests {
    use super::*;

    #[test]
    fn listen_precedence_flag_then_setting_then_default() {
        assert_eq!(resolve_listen(None, None, VoiceListen::default()), (750, 1500), "default is long");
        assert_eq!(resolve_listen(None, None, VoiceListen::Normal), (480, 1120));
        assert_eq!(resolve_listen(None, None, VoiceListen::VeryLong), (1100, 2000));
        assert_eq!(resolve_listen(Some(300), None, VoiceListen::VeryLong), (300, 2000), "flag wins per value");
        assert_eq!(resolve_listen(Some(300), Some(900), VoiceListen::Long), (300, 900));
    }
}

#[cfg(test)]
mod watch_tests {
    #[test]
    fn the_watch_buttons_cycle_and_keep_the_keys() {
        use omarchyvr_config::ai::{AiConfig, RunsOn};
        let mut c = AiConfig::parse("[use]\nvoice = \"local\"\n[providers.local]\nkind = \"openai-compatible\"\nbase_url = \"http://127.0.0.1:8080/v1\"\n[providers.or]\nkind = \"openai-compatible\"\nbase_url = \"https://openrouter.ai/api/v1\"\napi_key = \"sk-or-v1-TEST\"\n").unwrap();
        assert_eq!(super::ai_mode_apply(&mut c, "next").unwrap().0, RunsOn::TwoStage);
        assert_eq!(super::ai_mode_apply(&mut c, "next").unwrap().0, RunsOn::Online);
        assert_eq!(super::ai_mode_apply(&mut c, "next").unwrap().0, RunsOn::Simple);
        assert_eq!(super::ai_mode_apply(&mut c, "two-stage").unwrap().0, RunsOn::TwoStage);
        let (m, n) = super::ai_mode_apply(&mut c, "action-next").unwrap();
        assert_eq!((m, c.two_stage().action.as_str()), (RunsOn::TwoStage, "qwen3.5-9b"));
        assert!(n.unwrap().starts_with("action model: Qwen3.5-9B"));
        super::ai_mode_apply(&mut c, "router-next").unwrap();
        assert_eq!(c.two_stage().router, "laya");
        assert!(super::ai_mode_apply(&mut c, "sideways").is_err());
        // the watch: Local AI On / Off and the presets
        let (_, n) = super::ai_mode_apply(&mut c, "local-toggle").unwrap();
        assert_eq!((n.as_deref(), c.local_models_on()), (Some("Local AI: Off"), false));
        super::ai_mode_apply(&mut c, "local-toggle").unwrap();
        assert!(c.local_models_on());
        super::ai_mode_apply(&mut c, "low").unwrap();
        assert_eq!((c.preset(), c.two_stage().gpu_layers), (Some(omarchyvr_config::ai::Preset::Low), Some(0)));
        super::ai_mode_apply(&mut c, "preset-next").unwrap();
        assert_eq!(c.preset(), Some(omarchyvr_config::ai::Preset::Medium));
        assert!(c.render().contains("sk-or-v1-TEST"), "the key stays");
    }

    #[test]
    fn the_watch_row() {
        let mut cfg = omarchyvr_config::ai::AiConfig::parse("voice_local = \"two-stage\"\nvoice_local_models = true\n[providers.local]\nkind = \"openai-compatible\"\nbase_url = \"http://127.0.0.1:8080/v1\"\nmodel = \"pimax-local\"\n[local_two_stage]\npreset = \"high\"\nrouter = \"lara\"\naction = \"qwen3.5-9b\"\n").unwrap();
        let c = super::watch_commands("", &cfg);
        let j: Vec<String> = c.iter().map(|v| v.join(" ")).collect();
        assert_eq!(j[0], "panel-modify watch btn_ai_mode set-text -- Local AI: On");
        assert_eq!(j[1], "panel-modify watch btn_ai_preset set-text -- High");
        assert_eq!(j[2], "panel-modify watch pimax_ai_models set-visible 1");
        cfg.voice_local_models = Some(false);
        let j: Vec<String> = super::watch_commands("", &cfg).iter().map(|v| v.join(" ")).collect();
        assert!(j[0].ends_with("Local AI: Off") && j[2].ends_with("set-visible 0"), "the preset button only when on");
    }
}
