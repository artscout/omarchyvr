//! The measurements: STT latency per clip, TTS time to first audio, CPU use, and the report formats.

use crate::resample::resample;
use crate::sherpa::{Audio, Recognizer, Tts};
use crate::stats::{summarize, Summary};
use crate::text::wer;
use std::fmt::Write as _;
use std::time::{Duration, Instant};

/// CPU time of the whole process (all threads), seconds.
#[allow(unsafe_code)]
pub fn cpu_seconds() -> f64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `ts` is a valid out pointer.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut ts) };
    if rc != 0 {
        return f64::NAN;
    }
    ts.tv_sec as f64 + ts.tv_nsec as f64 * 1e-9
}

/// `CLOCK_MONOTONIC` in nanoseconds: the clock Monado stamps its metrics with, to line voice bursts up with compositor frames.
#[allow(unsafe_code)]
pub fn monotonic_ns() -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `ts` is a valid out pointer.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    if rc != 0 {
        return 0;
    }
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

/// Peak resident set size of the process, MiB.
#[allow(unsafe_code)]
pub fn max_rss_mib() -> f64 {
    // SAFETY: zeroed `rusage` is a valid value and `getrusage` fills it.
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: valid out pointer.
    let rc = unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    if rc != 0 {
        return f64::NAN;
    }
    ru.ru_maxrss as f64 / 1024.0
}

/// Average number of busy cores while `sleep` elapses right after some work (does the thread pool spin when idle?).
pub fn idle_cores(sleep: Duration) -> f64 {
    let (c0, t0) = (cpu_seconds(), Instant::now());
    std::thread::sleep(sleep);
    (cpu_seconds() - c0) / t0.elapsed().as_secs_f64()
}

/// One utterance to benchmark.
#[derive(Debug, Clone)]
pub struct Clip {
    pub name: String,
    pub lang: String,
    /// What was said (empty for recordings without a known transcript).
    pub reference: String,
    /// Mono, 16 kHz.
    pub samples: Vec<f32>,
}

impl Clip {
    pub fn audio_s(&self) -> f64 {
        self.samples.len() as f64 / 16000.0
    }
}

/// STT result for one clip.
#[derive(Debug, Clone)]
pub struct SttRow {
    pub clip: String,
    pub lang: String,
    pub audio_s: f64,
    pub wall_ms: Summary,
    /// Mean CPU seconds per decode / mean wall seconds per decode.
    pub cores: f64,
    pub text: String,
    /// (word errors, reference words) when a reference exists.
    pub wer: Option<(usize, usize)>,
}

impl SttRow {
    /// Real-time factor: decode time per second of audio (lower is faster).
    pub fn rtf(&self) -> f64 {
        self.wall_ms.p50 / 1000.0 / self.audio_s
    }
}

/// Runs the recognizer over `clips`: `warmup` untimed decodes, then `reps` timed ones per clip.
pub fn bench_stt(rec: &Recognizer, clips: &[Clip], reps: usize, warmup: usize) -> Result<Vec<SttRow>, crate::sherpa::Error> {
    let mut rows = Vec::new();
    for clip in clips {
        let mut text = String::new();
        let mut walls = Vec::new();
        let (mut cpu, mut wall) = (0.0, 0.0);
        for i in 0..warmup + reps {
            let (c0, t0) = (cpu_seconds(), Instant::now());
            text = rec.transcribe(&clip.samples, 16000)?;
            let w = t0.elapsed().as_secs_f64();
            let c = cpu_seconds() - c0;
            if i >= warmup {
                walls.push(w * 1000.0);
                cpu += c;
                wall += w;
            }
        }
        rows.push(SttRow {
            clip: clip.name.clone(),
            lang: clip.lang.clone(),
            audio_s: clip.audio_s(),
            wall_ms: summarize(&walls).ok_or_else(|| crate::sherpa::Error("reps must be >= 1".into()))?,
            cores: if wall > 0.0 { cpu / wall } else { 0.0 },
            wer: (!clip.reference.is_empty()).then(|| wer(&clip.reference, &text)),
            text,
        });
    }
    Ok(rows)
}

/// A sentence to synthesise.
#[derive(Debug, Clone)]
pub struct Item {
    pub id: &'static str,
    pub lang: &'static str,
    pub speaker: i32,
    pub text: &'static str,
}

/// English (af_heart = 3, am_adam = 11) and Spanish (ef_dora = 28, em_alex = 29) command-style test sentences, short to long.
pub fn corpus() -> Vec<Item> {
    vec![
        Item { id: "ack_en", lang: "en", speaker: 3, text: "Okay." },
        Item { id: "ack_es", lang: "es", speaker: 28, text: "Listo." },
        Item { id: "en1", lang: "en", speaker: 3, text: "Open the browser." },
        Item { id: "en2", lang: "en", speaker: 3, text: "Move the terminal to the left panel and recenter the view." },
        Item { id: "en3", lang: "en", speaker: 11, text: "Launch Half-Life Alyx, dim the room, and put my browser on the right panel." },
        Item {
            id: "en4",
            lang: "en",
            speaker: 3,
            text: "Please read the last three messages in my inbox, then summarize the build log and tell me whether the tests passed or failed.",
        },
        Item { id: "es1", lang: "es", speaker: 28, text: "Abre el navegador." },
        Item { id: "es2", lang: "es", speaker: 28, text: "Mueve la terminal al panel de la izquierda y recentra la vista." },
        Item { id: "es3", lang: "es", speaker: 29, text: "Lanza el juego, baja la luz de la sala y pon mi navegador en el panel de la derecha." },
        Item {
            id: "es4",
            lang: "es",
            speaker: 28,
            text: "Por favor lee los últimos tres mensajes de mi bandeja de entrada, luego resume el registro de compilación y dime si las pruebas pasaron o fallaron.",
        },
    ]
}

/// TTS result for one sentence.
#[derive(Debug, Clone)]
pub struct TtsRow {
    pub id: String,
    pub lang: String,
    pub chars: usize,
    pub audio_s: f64,
    /// Time from the call until the first chunk of samples arrives.
    pub first_audio_ms: Summary,
    /// Time until the call returns.
    pub total_ms: Summary,
    pub chunks: usize,
    pub cores: f64,
}

impl TtsRow {
    /// Synthesis time per second of speech.
    pub fn rtf(&self) -> f64 {
        self.total_ms.p50 / 1000.0 / self.audio_s
    }
}

/// Synthesises `item` `warmup + reps` times; returns the row and the audio of the last run.
pub fn bench_tts(tts: &Tts, item: &Item, reps: usize, warmup: usize) -> Result<(TtsRow, Audio), crate::sherpa::Error> {
    let mut first = Vec::new();
    let mut total = Vec::new();
    let mut chunks = 0usize;
    let mut last = None;
    let (mut cpu, mut wall) = (0.0, 0.0);
    for i in 0..warmup + reps {
        let t0 = Instant::now();
        let c0 = cpu_seconds();
        let mut t_first = None;
        let mut n_chunks = 0usize;
        let audio = tts.generate(item.text, item.speaker, 1.0, &mut |s: &[f32]| {
            if !s.is_empty() {
                n_chunks += 1;
                t_first.get_or_insert_with(|| t0.elapsed());
            }
            true
        })?;
        let w = t0.elapsed();
        let c = cpu_seconds() - c0;
        if i >= warmup {
            first.push(t_first.unwrap_or(w).as_secs_f64() * 1000.0);
            total.push(w.as_secs_f64() * 1000.0);
            cpu += c;
            wall += w.as_secs_f64();
        }
        chunks = n_chunks;
        last = Some(audio);
    }
    let audio = last.ok_or_else(|| crate::sherpa::Error("no runs".into()))?;
    let row = TtsRow {
        id: item.id.into(),
        lang: item.lang.into(),
        chars: item.text.chars().count(),
        audio_s: audio.samples.len() as f64 / f64::from(audio.sample_rate),
        first_audio_ms: summarize(&first).ok_or_else(|| crate::sherpa::Error("reps must be >= 1".into()))?,
        total_ms: summarize(&total).unwrap(),
        chunks,
        cores: if wall > 0.0 { cpu / wall } else { 0.0 },
    };
    Ok((row, audio))
}

/// Builds the 16 kHz STT clip for a synthesised sentence: resample (timed) and pad `pad_s` of silence on both sides, like the
/// pre-roll and tail of a push-to-talk utterance.
pub fn clip_from_tts(item: &Item, audio: &Audio, pad_s: f64) -> (Clip, f64) {
    let t0 = Instant::now();
    let mut s = resample(&audio.samples, audio.sample_rate, 16000);
    let resample_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let pad = vec![0.0f32; (pad_s * 16000.0) as usize];
    let mut padded = pad.clone();
    padded.append(&mut s);
    padded.extend_from_slice(&pad);
    (Clip { name: item.id.into(), lang: item.lang.into(), reference: item.text.into(), samples: padded }, resample_ms)
}

/// JSON string escaping.
pub fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(o, "\\u{:04x}", c as u32);
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// Markdown table of STT rows.
pub fn stt_markdown(model: &str, rows: &[SttRow]) -> String {
    let mut s = format!("**STT {model}**\n\n| clip | lang | audio s | decode p50 ms | p95 ms | RTF | cores | WER | transcript |\n|---|---|---|---|---|---|---|---|---|\n");
    for r in rows {
        let wer = r.wer.map_or("-".to_string(), |(e, n)| format!("{e}/{n}"));
        let _ = writeln!(
            s,
            "| {} | {} | {:.2} | {:.0} | {:.0} | {:.3} | {:.1} | {} | {} |",
            r.clip,
            r.lang,
            r.audio_s,
            r.wall_ms.p50,
            r.wall_ms.p95,
            r.rtf(),
            r.cores,
            wer,
            r.text.replace('|', "/")
        );
    }
    s
}

/// Markdown table of TTS rows.
pub fn tts_markdown(rows: &[TtsRow]) -> String {
    let mut s = String::from("**TTS Kokoro-82M**\n\n| id | lang | chars | audio s | first audio p50 ms | p95 ms | total p50 ms | RTF | chunks | cores |\n|---|---|---|---|---|---|---|---|---|---|\n");
    for r in rows {
        let _ = writeln!(
            s,
            "| {} | {} | {} | {:.2} | {:.0} | {:.0} | {:.0} | {:.3} | {} | {:.1} |",
            r.id, r.lang, r.chars, r.audio_s, r.first_audio_ms.p50, r.first_audio_ms.p95, r.total_ms.p50, r.rtf(), r.chunks, r.cores
        );
    }
    s
}

/// JSON object for a summary (ms).
pub fn summary_json(s: &Summary) -> String {
    format!("{{\"n\":{},\"min\":{:.3},\"mean\":{:.3},\"p50\":{:.3},\"p95\":{:.3},\"max\":{:.3}}}", s.n, s.min, s.mean, s.p50, s.p95, s.max)
}

/// JSON for STT rows.
pub fn stt_json(rows: &[SttRow]) -> String {
    let items: Vec<String> = rows
        .iter()
        .map(|r| {
            format!(
                "{{\"clip\":{},\"lang\":{},\"audio_s\":{:.3},\"wall_ms\":{},\"cores\":{:.2},\"rtf\":{:.4},\"text\":{},\"wer\":{}}}",
                json_str(&r.clip),
                json_str(&r.lang),
                r.audio_s,
                summary_json(&r.wall_ms),
                r.cores,
                r.rtf(),
                json_str(&r.text),
                r.wer.map_or("null".to_string(), |(e, n)| format!("[{e},{n}]"))
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}

/// JSON for TTS rows.
pub fn tts_json(rows: &[TtsRow]) -> String {
    let items: Vec<String> = rows
        .iter()
        .map(|r| {
            format!(
                "{{\"id\":{},\"lang\":{},\"chars\":{},\"audio_s\":{:.3},\"first_audio_ms\":{},\"total_ms\":{},\"rtf\":{:.4},\"chunks\":{},\"cores\":{:.2}}}",
                json_str(&r.id),
                json_str(&r.lang),
                r.chars,
                r.audio_s,
                summary_json(&r.first_audio_ms),
                summary_json(&r.total_ms),
                r.rtf(),
                r.chunks,
                r.cores
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sum(v: f64) -> Summary {
        Summary { n: 3, min: v, mean: v, p50: v, p95: v, max: v }
    }

    #[test]
    fn cpu_clock_advances_with_work_and_rss_is_positive() {
        let c0 = cpu_seconds();
        let mut x = 0u64;
        for i in 0..30_000_000u64 {
            x = x.wrapping_add(i.wrapping_mul(i));
        }
        assert!(x != 1);
        assert!(cpu_seconds() > c0);
        assert!(max_rss_mib() > 1.0);
    }

    #[test]
    fn idle_cores_is_a_finite_non_negative_rate() {
        // other tests run in parallel threads of this process, so only the arithmetic is checked here
        let c = idle_cores(Duration::from_millis(50));
        assert!(c.is_finite() && c >= 0.0, "{c}");
    }

    #[test]
    fn corpus_has_both_languages_and_unique_ids() {
        let c = corpus();
        let mut ids: Vec<_> = c.iter().map(|i| i.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), c.len());
        assert!(c.iter().any(|i| i.lang == "en") && c.iter().any(|i| i.lang == "es"));
        assert!(c.iter().all(|i| (0..54).contains(&i.speaker)));
    }

    #[test]
    fn clip_from_tts_resamples_pads_and_keeps_reference() {
        let item = &corpus()[0];
        let audio = Audio { samples: (0..24000).map(|i| ((i as f32) * 0.05).sin() * 0.3).collect(), sample_rate: 24000 };
        let (clip, ms) = clip_from_tts(item, &audio, 0.3);
        assert_eq!(clip.samples.len(), 16000 + 2 * 4800);
        assert!(clip.samples[..4800].iter().all(|v| *v == 0.0));
        assert!(clip.samples[4800..4900].iter().any(|v| v.abs() > 0.01));
        assert_eq!(clip.reference, item.text);
        assert!((clip.audio_s() - 1.6).abs() < 1e-9);
        assert!(ms >= 0.0);
    }

    #[test]
    fn monotonic_clock_is_monotonic() {
        let a = monotonic_ns();
        let b = monotonic_ns();
        assert!(a > 0 && b >= a);
    }

    #[test]
    fn json_escaping() {
        assert_eq!(json_str("a\"b\\c\n\t\u{1}é"), "\"a\\\"b\\\\c\\n\\t\\u0001é\"");
    }

    #[test]
    fn markdown_and_json_render_rows() {
        let r = SttRow {
            clip: "en1".into(),
            lang: "en".into(),
            audio_s: 2.0,
            wall_ms: sum(200.0),
            cores: 3.5,
            text: "open | the browser".into(),
            wer: Some((1, 4)),
        };
        assert!((r.rtf() - 0.1).abs() < 1e-12);
        let md = stt_markdown("v3", std::slice::from_ref(&r));
        assert!(md.contains("| en1 | en | 2.00 | 200 | 200 | 0.100 | 3.5 | 1/4 | open / the browser |"), "{md}");
        let js = stt_json(&[r]);
        assert!(js.contains("\"wer\":[1,4]") && js.contains("\"rtf\":0.1000"), "{js}");
        let t = TtsRow { id: "es1".into(), lang: "es".into(), chars: 18, audio_s: 1.0, first_audio_ms: sum(150.0), total_ms: sum(160.0), chunks: 1, cores: 4.0 };
        assert!((t.rtf() - 0.16).abs() < 1e-12);
        assert!(tts_markdown(std::slice::from_ref(&t)).contains("| es1 | es | 18 | 1.00 | 150 | 150 | 160 | 0.160 | 1 | 4.0 |"));
        assert!(tts_json(std::slice::from_ref(&t)).contains("\"chunks\":1"));
    }
}
