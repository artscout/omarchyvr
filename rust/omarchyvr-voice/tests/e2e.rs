//! End to end on a recorded fixture: `tests/data/en_es.wav` (Kokoro-82M TTS, generated locally by
//! `tests/data/gen_en_es.rs.txt`): "Recenter the view." (en), "Hola, esto es una prueba del micrófono." (es),
//! "Borra la pantalla." (es), with ~-62 dBFS noise and 1.2 s gaps.
//!
//! These need the local models (`~/models/parakeet-tdt-0.6b-v3-int8`, `~/models/vad/silero_vad.onnx`) and the
//! sherpa-onnx library (`~/src/venv-voice`); without them they print why and pass.

use omarchyvr_voice::engine::{default_model_dir, EngineSpec};
use omarchyvr_voice::intent::{normalize, Intents};
use omarchyvr_voice::pipeline::{run_offline, Core};
use omarchyvr_voice::protocol::{self, Event};
use omarchyvr_voice::segment::SegConfig;
use omarchyvr_voice::server::Collect;
use omarchyvr_voice::source::load_wav_16k;
use omarchyvr_voice::vad::{default_silero_model, SileroVad, Vad};
use omarchyvr_voicebench::sherpa::default_lib_path;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const EXPECTED: [(&str, &str, Option<&str>, f64); 3] = [
    ("Recenter the view.", "en", Some("vr.recenter"), 0.80),
    ("Hola, esto es una prueba del micrófono.", "es", None, 3.19),
    ("Borra la pantalla.", "es", Some("hud.clear"), 6.77),
];

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/en_es.wav")
}

fn models_present() -> bool {
    let ok = default_lib_path().exists()
        && default_model_dir().join("encoder.int8.onnx").exists()
        && default_silero_model().exists();
    if !ok {
        eprintln!(
            "skipped: needs {}, {} and {}",
            default_lib_path().display(),
            default_model_dir().display(),
            default_silero_model().display()
        );
    }
    ok
}

fn check_finals(events: &[Event]) {
    let finals: Vec<(&String, &Option<String>, f64)> = events
        .iter()
        .filter_map(|e| match e {
            Event::Final { text, lang, t0, .. } if !text.is_empty() => Some((text, lang, *t0)),
            _ => None,
        })
        .collect();
    assert_eq!(finals.len(), 3, "{events:?}");
    for ((text, lang, t0), (want, want_lang, _, start)) in finals.iter().zip(EXPECTED) {
        assert_eq!(normalize(text), normalize(want), "{text:?}");
        assert_eq!(lang.as_deref(), Some(want_lang), "{text:?}");
        // the segment starts up to 450 ms (pre-roll + onset) before the speech
        assert!(
            *t0 <= start && *t0 > start - 0.45,
            "{text:?} at {t0}, speech at {start}"
        );
    }
    let intents: Vec<&str> = events
        .iter()
        .filter_map(|e| {
            if let Event::Intent { name, .. } = e {
                Some(name.as_str())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(intents, vec!["vr.recenter", "hud.clear"]);
}

#[test]
fn fixture_through_the_offline_pipeline() {
    if !models_present() {
        return;
    }
    let audio = load_wav_16k(&fixture()).unwrap();
    let vad: Box<dyn Vad + Send> =
        Box::new(SileroVad::new(&default_lib_path(), &default_silero_model(), 0.5).unwrap());
    let mut core = Core::new(vad, SegConfig::default());
    let mut engine = EngineSpec::parakeet_default(2).build().unwrap();
    let sink = Collect::default();
    run_offline(
        &mut core,
        engine.as_mut(),
        &Intents::builtin(),
        &audio,
        320,
        &sink,
    );
    let ev = sink.events();
    check_finals(&ev);
    assert!(
        ev.iter().any(|e| matches!(e, Event::Partial { .. })),
        "partials for the long sentence"
    );
    for e in &ev {
        if let Event::Final {
            latency_ms,
            decode_ms,
            ..
        } = e
        {
            // offline: hangover (480-512 ms) + decode
            assert!(
                *latency_ms >= 480 && *latency_ms < 480 + 32 + decode_ms + 1,
                "{e:?}"
            );
        }
    }
}

#[test]
fn fixture_through_the_cli_live_driver_and_socket() {
    if !models_present() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("omarchyvr-voice-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("voice.sock");
    let mut child = Command::new(env!("CARGO_BIN_EXE_omarchyvr-voice"))
        .args(["listen", "--input"])
        .arg(fixture())
        .args(["--fast", "--json", "--socket"])
        .arg(&sock)
        .args(["--engine", "parakeet", "--no-hotwords"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // subscribe while the engine loads
    let t0 = Instant::now();
    let sub = loop {
        if let Ok(s) = std::os::unix::net::UnixStream::connect(&sock) {
            break s;
        }
        assert!(
            t0.elapsed() < Duration::from_secs(20),
            "socket never appeared"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    let reader = std::thread::spawn(move || {
        let mut got = Vec::new();
        for line in BufReader::new(sub).lines() {
            let Ok(line) = line else { break };
            if let Some(e) = protocol::decode(&line) {
                let stop = matches!(
                    e,
                    Event::Status {
                        state: protocol::State::Stopped,
                        ..
                    }
                );
                got.push(e);
                if stop {
                    break;
                }
            }
        }
        got
    });
    let stdout = child.stdout.take().unwrap();
    let printed: Vec<Event> = BufReader::new(stdout)
        .lines()
        .map_while(Result::ok)
        .filter_map(|l| protocol::decode(&l))
        .collect();
    assert!(child.wait().unwrap().success());
    check_finals(&printed);
    let via_socket = reader.join().unwrap();
    assert!(
        matches!(via_socket.first(), Some(Event::Hello { .. })),
        "{via_socket:?}"
    );
    check_finals(&via_socket);
    assert!(!sock.exists(), "socket removed on exit");
}
