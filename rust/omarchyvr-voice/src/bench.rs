//! `omarchyvr-voice bench`: the whole pipeline on known clips, fed at the real audio rate through the live driver
//! (capture thread -> processing loop -> decoding thread), as a microphone would. Measured per utterance: end of
//! speech -> final transcript (wall clock, from the moment the last speech sample was handed over), decode time,
//! partials, word errors; per run: CPU cores used while listening and while idle (VAD only).

use crate::engine::{Engine, EngineSpec, Transcript};
use crate::intent::Intents;
use crate::pipeline::{run_live, Core, LiveConfig};
use crate::protocol::{Command, Event};
use crate::segment::SegConfig;
use crate::server::{Collect, Sink};
use crate::source::feed;
use crate::vad::{Vad, RATE};
use omarchyvr_voicebench::bench::cpu_seconds;
use omarchyvr_voicebench::text::wer;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

/// One test utterance.
#[derive(Debug, Clone)]
pub struct Clip {
    pub id: String,
    pub lang: String,
    pub reference: String,
    pub samples: Vec<f32>,
}

/// The voicebench sentences (Kokoro TTS, `omarchyvr-voicebench bench --out DIR` writes `DIR/clips/<id>-16k.wav`).
pub const CORPUS: &[(&str, &str, &str)] = &[
    ("ack_en", "en", "Okay."),
    ("ack_es", "es", "Listo."),
    ("en1", "en", "Open the browser."),
    ("en2", "en", "Move the terminal to the left panel and recenter the view."),
    ("en3", "en", "Launch Half-Life Alyx, dim the room, and put my browser on the right panel."),
    ("en4", "en", "Please read the last three messages in my inbox, then summarize the build log and tell me whether the tests passed or failed."),
    ("es1", "es", "Abre el navegador."),
    ("es2", "es", "Mueve la terminal al panel de la izquierda y recentra la vista."),
    ("es3", "es", "Lanza el juego, baja la luz de la sala y pon mi navegador en el panel de la derecha."),
    ("es4", "es", "Por favor lee los últimos tres mensajes de mi bandeja de entrada, luego resume el registro de compilación y dime si las pruebas pasaron o fallaron."),
];

/// Load the corpus clips that exist in `dir`.
pub fn load_corpus(dir: &Path) -> Vec<Clip> {
    CORPUS
        .iter()
        .filter_map(|(id, lang, text)| {
            let p = dir.join(format!("{id}-16k.wav"));
            crate::source::load_wav_16k(&p).ok().map(|samples| Clip {
                id: id.to_string(),
                lang: lang.to_string(),
                reference: text.to_string(),
                samples,
            })
        })
        .collect()
}

/// First and one-past-last sample with |x| > `thr` (speech bounds of a clean clip).
pub fn speech_bounds(s: &[f32], thr: f32) -> Option<(usize, usize)> {
    let a = s.iter().position(|x| x.abs() > thr)?;
    let b = s.iter().rposition(|x| x.abs() > thr)? + 1;
    Some((a, b))
}

/// Per-utterance result.
#[derive(Debug, Clone, Default)]
pub struct Row {
    pub id: String,
    pub lang: String,
    pub speech_s: f64,
    /// End of speech (last speech sample handed to the pipeline) -> final event, wall clock.
    pub latency_ms: f64,
    pub decode_ms: f64,
    /// What the pipeline itself reports (hangover + queue wait + decode).
    pub reported_ms: f64,
    pub partials: usize,
    /// Audio time of the first partial after the speech onset.
    pub first_partial_s: Option<f64>,
    pub errors: usize,
    pub words: usize,
    pub segments: usize,
    pub text: String,
}

/// Whole-run numbers.
#[derive(Debug, Clone, Default)]
pub struct Run {
    pub label: String,
    pub rows: Vec<Row>,
    /// Mean busy cores over the run (speech and gaps).
    pub cores: f64,
    /// Busy cores while only silence comes in (VAD + level meter).
    pub idle_cores: f64,
    pub audio_s: f64,
    pub decode_total_ms: f64,
    /// How far the processing loop ran behind the audio (level events vs. feed time): median and max, ms.
    pub lag_ms: (f64, f64),
}

/// An engine that records its decode times.
struct Timed {
    inner: Box<dyn Engine>,
    log: Arc<Mutex<Vec<f64>>>,
}

impl Engine for Timed {
    fn transcribe(&mut self, audio: &[f32]) -> Result<Transcript, String> {
        let t = Instant::now();
        let r = self.inner.transcribe(audio);
        self.log
            .lock()
            .unwrap()
            .push(t.elapsed().as_secs_f64() * 1e3);
        r
    }
}

/// Everything a run needs.
pub struct BenchSetup<'a> {
    pub label: String,
    pub engine: EngineSpec,
    pub vad: Box<dyn Vad + Send>,
    pub seg: SegConfig,
    pub clips: &'a [Clip],
    /// Silence between clips, seconds.
    pub gap_s: f64,
    /// Level of the background noise added everywhere (keeps the energy VAD honest), RMS.
    pub noise: f32,
}

fn lcg_noise(n: usize, amp: f32, seed: &mut u32) -> Vec<f32> {
    (0..n)
        .map(|_| {
            *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            amp * 1.7 * ((*seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0)
        })
        .collect()
}

/// Run the clips one after the other (with `gap_s` of noise between them) through the live pipeline in real time.
pub fn run(setup: BenchSetup<'_>) -> Result<Run, String> {
    let r = RATE as usize;
    let mut seed = 7u32;
    // 3 s of noise first: idle CPU is measured there
    let idle_n = 3 * r;
    let mut stream = lcg_noise(idle_n, setup.noise, &mut seed);
    let mut spans = Vec::new(); // (clip index, speech start, speech end) in stream samples
    for (i, c) in setup.clips.iter().enumerate() {
        let (a, b) = speech_bounds(&c.samples, 0.02).unwrap_or((0, c.samples.len()));
        let base = stream.len();
        let noise = lcg_noise(c.samples.len(), setup.noise, &mut seed);
        stream.extend(c.samples.iter().zip(noise).map(|(s, n)| s + n));
        spans.push((i, base + a, base + b));
        stream.extend(lcg_noise(
            (setup.gap_s * r as f64) as usize,
            setup.noise,
            &mut seed,
        ));
    }
    let (atx, arx) = mpsc::channel();
    let (_ctx, crx) = mpsc::channel::<Command>();
    let sink = Arc::new(Collect::default());
    let stop = Arc::new(AtomicBool::new(false));
    let log = Arc::new(Mutex::new(Vec::new()));
    let spec = setup.engine.clone();
    let log2 = Arc::clone(&log);
    let cfg = LiveConfig {
        engine: Box::new(move || {
            Ok(Box::new(Timed {
                inner: spec.build()?,
                log: log2,
            }) as Box<dyn Engine>)
        }),
        intents: Intents::none(),
    };
    let core = Core::new(setup.vad, setup.seg);
    let fed: Arc<Mutex<Vec<(usize, Instant)>>> = Arc::new(Mutex::new(Vec::new()));
    let cpu_marks: Arc<Mutex<Vec<(usize, f64, Instant)>>> = Arc::new(Mutex::new(Vec::new()));
    let ready = Arc::new(AtomicBool::new(false));
    let feeder = {
        let (fed, cpu_marks, ready, stop) = (
            Arc::clone(&fed),
            Arc::clone(&cpu_marks),
            Arc::clone(&ready),
            Arc::clone(&stop),
        );
        let sink = Arc::clone(&sink);
        std::thread::spawn(move || {
            // wait until the engine is loaded (the live driver says "listening")
            let t0 = Instant::now();
            while !ready.load(std::sync::atomic::Ordering::Relaxed)
                && t0.elapsed() < Duration::from_secs(60)
            {
                if sink
                    .events()
                    .iter()
                    .any(|e| matches!(e, Event::Status { .. }))
                {
                    ready.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            std::thread::sleep(Duration::from_millis(300)); // let the warm-up decode's thread pools settle
            cpu_marks
                .lock()
                .unwrap()
                .push((0, cpu_seconds(), Instant::now()));
            let mut marked = false;
            feed(&stream, 20, true, &atx, &stop, |pos, at| {
                fed.lock().unwrap().push((pos, at));
                if !marked && pos >= idle_n {
                    marked = true;
                    cpu_marks.lock().unwrap().push((pos, cpu_seconds(), at));
                }
            });
            std::thread::sleep(Duration::from_millis(1500));
            cpu_marks
                .lock()
                .unwrap()
                .push((usize::MAX, cpu_seconds(), Instant::now()));
            drop(atx);
        })
    };
    let sink_dyn: Arc<dyn Sink> = sink.clone();
    let res = run_live(core, cfg, &arx, &crx, sink_dyn, &stop);
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = feeder.join();
    res?;

    // when was sample `s` handed over?
    let fed = fed.lock().unwrap().clone();
    let fed_at = |s: usize| fed.iter().find(|(p, _)| *p >= s).map(|(_, t)| *t);
    let events = sink.0.lock().unwrap().clone();
    if std::env::var_os("PIMAX_VOICE_BENCH_DEBUG").is_some() {
        for &(i, a, b) in &spans {
            eprintln!(
                "span {} {:.3}..{:.3}",
                setup.clips[i].id,
                a as f64 / r as f64,
                b as f64 / r as f64
            );
        }
        for (_, e) in &events {
            if !matches!(e, Event::Level { .. } | Event::Partial { .. }) {
                eprintln!("{}", crate::protocol::encode(e));
            }
        }
    }
    let mut rows = Vec::new();
    for &(i, a, b) in &spans {
        let c = &setup.clips[i];
        let (t_a, t_b) = (a as f64 / r as f64, b as f64 / r as f64);
        let mut row = Row {
            id: c.id.clone(),
            lang: c.lang.clone(),
            speech_s: (b - a) as f64 / r as f64,
            ..Default::default()
        };
        let mut texts = Vec::new();
        let mut last_final: Option<(Instant, u32, u32)> = None;
        let mut segs = std::collections::BTreeSet::new();
        for (at, e) in &events {
            match e {
                Event::Final {
                    seg,
                    text,
                    t0,
                    t1,
                    latency_ms,
                    decode_ms,
                    ..
                } if *t1 > t_a && *t0 < t_b + 0.2 && !text.is_empty() => {
                    texts.push(text.clone());
                    segs.insert(*seg);
                    last_final = Some((*at, *latency_ms, *decode_ms));
                }
                Event::Partial { t, .. } if *t > t_a && *t < t_b + 0.6 => {
                    row.partials += 1;
                    if row.first_partial_s.is_none() {
                        row.first_partial_s = Some(t - t_a);
                    }
                }
                _ => {}
            }
        }
        row.segments = segs.len();
        row.text = texts.join(" ");
        let (e, w) = wer(&c.reference, &row.text);
        row.errors = e;
        row.words = w;
        if let (Some((at, rep, dec)), Some(t_end)) = (last_final, fed_at(b)) {
            row.latency_ms = at.saturating_duration_since(t_end).as_secs_f64() * 1e3;
            row.reported_ms = f64::from(rep);
            row.decode_ms = f64::from(dec);
        }
        rows.push(row);
    }
    let marks = cpu_marks.lock().unwrap().clone();
    let mut run = Run {
        label: setup.label,
        rows,
        audio_s: stream_len_s(&fed),
        ..Default::default()
    };
    if marks.len() == 3 {
        let (_, c0, t0) = marks[0];
        let (_, c1, t1) = marks[1];
        let (_, c2, t2) = marks[2];
        run.idle_cores = (c1 - c0) / (t1 - t0).as_secs_f64().max(1e-6);
        run.cores = (c2 - c1) / (t2 - t1).as_secs_f64().max(1e-6);
    }
    run.decode_total_ms = log.lock().unwrap().iter().sum();
    let mut lags: Vec<f64> = Vec::new();
    let mut k = 0usize;
    for (at, e) in &events {
        if let Event::Level { .. } = e {
            k += 1;
            let pos = k * crate::pipeline::LEVEL_FRAMES as usize * crate::vad::FRAME;
            if let Some(t) = fed_at(pos) {
                lags.push(at.saturating_duration_since(t).as_secs_f64() * 1e3);
            }
        }
    }
    lags.sort_by(f64::total_cmp);
    if !lags.is_empty() {
        run.lag_ms = (lags[lags.len() / 2], lags[lags.len() - 1]);
    }
    Ok(run)
}

fn stream_len_s(fed: &[(usize, Instant)]) -> f64 {
    fed.last().map_or(0.0, |(p, _)| *p as f64 / f64::from(RATE))
}

/// Markdown report of several runs.
pub fn markdown(runs: &[Run]) -> String {
    let mut s = String::new();
    for run in runs {
        let words: usize = run.rows.iter().map(|r| r.words).sum();
        let errs: usize = run.rows.iter().map(|r| r.errors).sum();
        let mut lat: Vec<f64> = run.rows.iter().map(|r| r.latency_ms).collect();
        lat.sort_by(f64::total_cmp);
        let med = if lat.is_empty() {
            0.0
        } else {
            lat[lat.len() / 2]
        };
        let _ = writeln!(
            s,
            "### {}\n\nend of speech -> final: median {:.0} ms, max {:.0} ms; word errors {errs}/{words}; CPU {:.2} cores while listening ({:.0} s of audio, {:.0} ms of decoding), {:.2} cores idle (VAD only); processing lag median {:.0} ms, max {:.0} ms\n",
            run.label,
            med,
            lat.last().copied().unwrap_or(0.0),
            run.cores,
            run.audio_s,
            run.decode_total_ms,
            run.idle_cores,
            run.lag_ms.0,
            run.lag_ms.1
        );
        let _ = writeln!(s, "| clip | lang | speech s | latency ms | decode ms | reported ms | partials | first partial s | errors | segs | transcript |");
        let _ = writeln!(s, "|---|---|---|---|---|---|---|---|---|---|---|");
        for r in &run.rows {
            let fp = r
                .first_partial_s
                .map_or("-".to_string(), |v| format!("{v:.2}"));
            let _ = writeln!(
                s,
                "| {} | {} | {:.2} | {:.0} | {:.0} | {:.0} | {} | {} | {}/{} | {} | {} |",
                r.id,
                r.lang,
                r.speech_s,
                r.latency_ms,
                r.decode_ms,
                r.reported_ms,
                r.partials,
                fp,
                r.errors,
                r.words,
                r.segments,
                r.text
            );
        }
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds() {
        let mut s = vec![0.0; 100];
        s[10] = 0.5;
        s[50] = -0.3;
        assert_eq!(speech_bounds(&s, 0.02), Some((10, 51)));
        assert_eq!(speech_bounds(&[0.0; 10], 0.02), None);
    }

    #[test]
    fn corpus_is_complete() {
        assert_eq!(CORPUS.len(), 10);
        assert!(load_corpus(Path::new("/nonexistent")).is_empty());
    }

    #[test]
    fn markdown_has_rows() {
        let run = Run {
            label: "x".into(),
            rows: vec![Row {
                id: "en1".into(),
                lang: "en".into(),
                latency_ms: 600.0,
                words: 3,
                text: "Open the browser.".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let m = markdown(&[run]);
        assert!(m.contains("median 600 ms"), "{m}");
        assert!(m.contains("| en1 | en |"), "{m}");
    }
}
