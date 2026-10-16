//! `pimax-voicebench`: CPU STT/TTS latency harness. See `--help`.

use pimax_voicebench::bench::{self, Clip};
use pimax_voicebench::resample::resample;
use pimax_voicebench::sherpa::{default_lib_path, Lib, Recognizer, SttConfig, Tts, TtsConfig};
use pimax_voicebench::wav;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

const HELP: &str = "pimax-voicebench <command>

  bench [--lib PATH] [--models DIR] [--threads 4] [--reps 5] [--warmup 1] [--out DIR]
        [--stt v3,v2] [--pad 0.3] [--no-tts-es] [--skip-stt]
      Kokoro-82M synthesises the English and Spanish test sentences (time to first audio, CPU use), the audio is resampled
      24 -> 16 kHz and padded, and each Parakeet TDT 0.6B model transcribes it (latency, RTF, WER) next to the recordings in
      the model's test_wavs. Writes results.json, results.md and the generated WAVs into --out.
  idle [--lib PATH] [--models DIR] [--threads 4]
      Does the ONNX Runtime thread pool keep burning CPU after a decode? Prints the busy cores per 250 ms for 8 s after
      one transcription and one synthesis.
  resample IN.wav OUT.wav RATE      convert a WAV (any channels -> mono) to RATE Hz, 16-bit
  info [--lib PATH] [--models DIR]  library version and model file check
";

fn opt<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
}

fn num<T: std::str::FromStr>(args: &[String], name: &str, default: T) -> Result<T, String> {
    opt(args, name).map_or(Ok(default), |v| v.parse().map_err(|_| format!("bad value for {name}")))
}

fn models_dir(args: &[String]) -> PathBuf {
    opt(args, "--models").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("models"))
}

fn lib_path(args: &[String]) -> PathBuf {
    opt(args, "--lib").map(PathBuf::from).unwrap_or_else(default_lib_path)
}

fn model_dir_name(tag: &str) -> Result<&'static str, String> {
    match tag {
        "v3" => Ok("parakeet-tdt-0.6b-v3-int8"),
        "v2" => Ok("parakeet-tdt-0.6b-v2-int8"),
        other => Err(format!("unknown --stt model {other} (v3, v2)")),
    }
}

fn run(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("resample") => {
            let (i, o, r) = (args.get(1), args.get(2), args.get(3));
            let (Some(i), Some(o), Some(r)) = (i, o, r) else { return Err(HELP.into()) };
            let rate: u32 = r.parse().map_err(|_| "bad RATE")?;
            let w = wav::read(Path::new(i)).map_err(|e| e.to_string())?;
            let out = resample(&w.to_mono(), w.sample_rate, rate);
            wav::write_pcm16(Path::new(o), &out, rate, 1).map_err(|e| e.to_string())?;
            println!("{} Hz x{} -> {} Hz mono, {} samples", w.sample_rate, w.channels, rate, out.len());
            Ok(())
        }
        Some("info") => {
            let lib = Lib::load(&lib_path(args)).map_err(|e| e.to_string())?;
            println!("sherpa-onnx {} ({})", lib.version(), lib.git_sha());
            let m = models_dir(args);
            for f in [
                "parakeet-tdt-0.6b-v3-int8/encoder.int8.onnx",
                "parakeet-tdt-0.6b-v2-int8/encoder.int8.onnx",
                "kokoro-multi-lang-v1_0/model.onnx",
                "kokoro-multi-lang-v1_0/voices.bin",
                "kokoro-multi-lang-v1_0/espeak-ng-data",
            ] {
                println!("{:60} {}", m.join(f).display(), if m.join(f).exists() { "ok" } else { "MISSING" });
            }
            Ok(())
        }
        Some("bench") => bench_cmd(args),
        Some("idle") => idle_cmd(args),
        _ => Err(HELP.to_string()),
    }
}

fn idle_cmd(args: &[String]) -> Result<(), String> {
    let threads: i32 = num(args, "--threads", 4)?;
    let models = models_dir(args);
    let lib = Lib::load(&lib_path(args)).map_err(|e| e.to_string())?;
    let rec = Recognizer::new(&lib, &SttConfig::from_dir(&models.join("parakeet-tdt-0.6b-v3-int8"), threads)).map_err(|e| e.to_string())?;
    let clip: Vec<f32> = (0..32000).map(|i| ((i as f32) * 0.02).sin() * 0.1).collect();
    let probe = |label: &str| {
        print!("{label}: busy cores per 250 ms:");
        for _ in 0..32 {
            let (c0, t0) = (bench::cpu_seconds(), Instant::now());
            std::thread::sleep(std::time::Duration::from_millis(250));
            print!(" {:.2}", (bench::cpu_seconds() - c0) / t0.elapsed().as_secs_f64());
        }
        println!();
    };
    probe("after load only");
    let _ = rec.transcribe(&clip, 16000).map_err(|e| e.to_string())?;
    probe("after one transcription");
    for _ in 0..5 {
        let _ = rec.transcribe(&clip, 16000).map_err(|e| e.to_string())?;
    }
    probe("after five more transcriptions");
    drop(rec);
    probe("after dropping the recognizer");
    Ok(())
}

fn bench_cmd(args: &[String]) -> Result<(), String> {
    let bench_t0 = bench::monotonic_ns();
    let threads: i32 = num(args, "--threads", 4)?;
    let reps: usize = num(args, "--reps", 5)?;
    let warmup: usize = num(args, "--warmup", 1)?;
    let pad: f64 = num(args, "--pad", 0.3)?;
    let out_dir = PathBuf::from(opt(args, "--out").unwrap_or("voicebench-out"));
    let models = models_dir(args);
    std::fs::create_dir_all(out_dir.join("clips")).map_err(|e| e.to_string())?;
    let lib = Lib::load(&lib_path(args)).map_err(|e| e.to_string())?;
    println!("sherpa-onnx {} ({}), {threads} threads, {reps} timed reps after {warmup} warm-up, CPU provider only", lib.version(), lib.git_sha());
    let mut md = String::new();
    let mut json = vec![format!("\"sherpa_onnx\":{}", bench::json_str(&lib.version())), format!("\"threads\":{threads}"), format!("\"reps\":{reps}")];

    // ---- TTS (also produces the clips) ----
    let kokoro = models.join("kokoro-multi-lang-v1_0");
    let mut clips: Vec<Clip> = Vec::new();
    let mut tts_rows = Vec::new();
    let cfg_en = TtsConfig::from_dir(&kokoro, "lexicon-us-en.txt,lexicon-zh.txt", "", threads);
    let t0 = Instant::now();
    let tts_en = Tts::new(&lib, &cfg_en).map_err(|e| e.to_string())?;
    let load_en_ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("Kokoro (en lexicon) loaded in {load_en_ms:.0} ms, {} speakers, {} Hz", tts_en.num_speakers(), tts_en.sample_rate());
    let cold_tts = |t: &Tts, sid: i32| {
        let t0 = Instant::now();
        let _ = t.generate("Okay.", sid, 1.0, &mut |_: &[f32]| true);
        t0.elapsed().as_secs_f64() * 1000.0
    };
    let cold_en_ms = cold_tts(&tts_en, 3);
    println!("Kokoro en: first (cold) synthesis of \"Okay.\": {cold_en_ms:.0} ms");
    json.push(format!("\"tts_en_cold_first_synthesis_ms\":{cold_en_ms:.0}"));
    let tts_es = if args.iter().any(|a| a == "--no-tts-es") {
        None
    } else {
        let cfg_es = TtsConfig::from_dir(&kokoro, "", "es", threads);
        let t0 = Instant::now();
        match Tts::new(&lib, &cfg_es) {
            Ok(t) => {
                println!("Kokoro (es via espeak-ng) loaded in {:.0} ms", t0.elapsed().as_secs_f64() * 1000.0);
                let c = cold_tts(&t, 28);
                println!("Kokoro es: first (cold) synthesis: {c:.0} ms");
                json.push(format!("\"tts_es_cold_first_synthesis_ms\":{c:.0}"));
                Some(t)
            }
            Err(e) => {
                eprintln!("Spanish TTS unavailable: {e}");
                None
            }
        }
    };
    let mut resample_ms = Vec::new();
    for item in bench::corpus() {
        let tts = if item.lang == "es" { tts_es.as_ref() } else { Some(&tts_en) };
        let Some(tts) = tts else { continue };
        let (row, audio) = bench::bench_tts(tts, &item, reps, warmup).map_err(|e| e.to_string())?;
        println!(
            "TTS {:4} {:2} {:3} chars -> {:5.2} s audio: first audio {:6.0} ms (p95 {:.0}), total {:6.0} ms, RTF {:.3}, {:.1} cores",
            row.id, row.lang, row.chars, row.audio_s, row.first_audio_ms.p50, row.first_audio_ms.p95, row.total_ms.p50, row.rtf(), row.cores
        );
        wav::write_pcm16(&out_dir.join("clips").join(format!("{}-24k.wav", item.id)), &audio.samples, audio.sample_rate, 1).map_err(|e| e.to_string())?;
        let (clip, ms) = bench::clip_from_tts(&item, &audio, pad);
        wav::write_pcm16(&out_dir.join("clips").join(format!("{}-16k.wav", item.id)), &clip.samples, 16000, 1).map_err(|e| e.to_string())?;
        resample_ms.push(ms);
        clips.push(clip);
        tts_rows.push(row);
    }
    println!("TTS idle CPU after synthesis: {:.2} cores", bench::idle_cores(std::time::Duration::from_secs(2)));
    md.push_str(&bench::tts_markdown(&tts_rows));
    json.push(format!("\"tts_load_ms\":{load_en_ms:.0}"));
    json.push(format!("\"tts\":{}", bench::tts_json(&tts_rows)));
    if !resample_ms.is_empty() {
        let s = pimax_voicebench::stats::summarize(&resample_ms).unwrap();
        println!("resample 24 -> 16 kHz: mean {:.2} ms per clip (max {:.2})", s.mean, s.max);
        json.push(format!("\"resample_24k_16k_ms\":{}", bench::summary_json(&s)));
    }
    drop(tts_en);
    drop(tts_es);

    // ---- STT ----
    if !args.iter().any(|a| a == "--skip-stt") {
        for tag in opt(args, "--stt").unwrap_or("v3,v2").split(',') {
            let dir = models.join(model_dir_name(tag)?);
            let t0 = Instant::now();
            let rec = Recognizer::new(&lib, &SttConfig::from_dir(&dir, threads)).map_err(|e| e.to_string())?;
            let load_ms = t0.elapsed().as_secs_f64() * 1000.0;
            println!("Parakeet {tag} loaded in {load_ms:.0} ms");
            let mut all = clips.clone();
            if let Ok(rd) = std::fs::read_dir(dir.join("test_wavs")) {
                let mut files: Vec<_> = rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "wav")).collect();
                files.sort();
                for f in files {
                    let w = wav::read(&f).map_err(|e| e.to_string())?;
                    let name = format!("rec-{}", f.file_stem().unwrap_or_default().to_string_lossy());
                    let lang = if name.contains("es") { "es" } else { "en" }.to_string();
                    all.push(Clip { name, lang, reference: String::new(), samples: resample(&w.to_mono(), w.sample_rate, 16000) });
                }
            }
            let cold = clips.first().map(|c| {
                let t = Instant::now();
                let _ = rec.transcribe(&c.samples, 16000);
                t.elapsed().as_secs_f64() * 1000.0
            });
            if let Some(c) = cold {
                println!("STT {tag}: first (cold) decode of {}: {c:.0} ms", clips[0].name);
                json.push(format!("\"stt_{tag}_cold_first_decode_ms\":{c:.0}"));
            }
            let cpu0 = bench::cpu_seconds();
            let rows = bench::bench_stt(&rec, &all, reps, warmup).map_err(|e| e.to_string())?;
            for r in &rows {
                println!(
                    "STT {tag} {:8} {:2} {:5.2} s audio: p50 {:6.0} ms (p95 {:.0}) RTF {:.3} {:.1} cores WER {}  | {}",
                    r.clip, r.lang, r.audio_s, r.wall_ms.p50, r.wall_ms.p95, r.rtf(), r.cores,
                    r.wer.map_or("-".into(), |(e, n)| format!("{e}/{n}")), r.text
                );
            }
            println!("STT {tag}: total CPU {:.1} s, idle after: {:.2} cores, peak RSS {:.0} MiB", bench::cpu_seconds() - cpu0, bench::idle_cores(std::time::Duration::from_secs(2)), bench::max_rss_mib());
            md.push_str(&format!("\nload time {load_ms:.0} ms\n\n{}", bench::stt_markdown(&format!("Parakeet TDT 0.6B {tag} int8"), &rows)));
            json.push(format!("\"stt_{tag}\":{{\"load_ms\":{load_ms:.0},\"rows\":{}}}", bench::stt_json(&rows)));
        }
    }
    let bench_t1 = bench::monotonic_ns();
    println!("VOICEBENCH_WINDOW_MONOTONIC_NS {bench_t0} {bench_t1}");
    json.push(format!("\"window_monotonic_ns\":[{bench_t0},{bench_t1}]"));
    json.push(format!("\"peak_rss_mib\":{:.0}", bench::max_rss_mib()));
    std::fs::write(out_dir.join("results.json"), format!("{{{}}}\n", json.join(","))).map_err(|e| e.to_string())?;
    std::fs::write(out_dir.join("results.md"), md).map_err(|e| e.to_string())?;
    println!("wrote {}", out_dir.display());
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
