//! Audio in, events out: framing, VAD, segmentation, partial/final decode jobs, intents.
//!
//! [`Core`] is the clock-free part (samples in, events and decode jobs out). Two drivers run it:
//! * [`run_offline`]: decodes inline (files, tests, benchmarks): deterministic.
//! * [`run_live`]: a processing loop fed by a capture channel plus a decoding thread with a job queue. Finals are
//!   never dropped; only the newest partial is kept, and a partial whose segment already ended is dropped, so
//!   decoding never delays the end-of-speech decision.

use crate::engine::{Engine, Transcript};
use crate::intent::Intents;
use crate::lang;
use crate::protocol::{ms, Command, Event, State};
use crate::segment::{SegConfig, SegEvent, Segmenter};
use crate::server::Sink;
use crate::vad::{Vad, FRAME, RATE};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Frames per `level` event (8 x 32 ms = 256 ms).
pub const LEVEL_FRAMES: u32 = 8;
/// All-zero input for this many frames (3 s) is reported as a `no_device` status.
pub const ZERO_FRAMES_WARN: u32 = 94;
/// The detail of that status. The Dream Air's USB mic sends exact zeros while the headset panel is off (the
/// firmware powers the audio codec with the panel).
pub const SILENT_INPUT: &str = "input is digital silence: mic muted, or the headset panel is off (the Dream Air mic works while the display runs)";

/// A decode request.
#[derive(Debug, Clone)]
pub struct Job {
    pub seg: u64,
    pub fin: bool,
    pub audio: Vec<f32>,
    /// Audio time of the segment start / end, seconds.
    pub t0: f64,
    pub t1: f64,
    /// Samples between the end of speech and the end-of-speech decision (the hangover).
    pub endpoint_samples: u64,
    /// When the job was made (the decision time for finals).
    pub made: Instant,
}

/// Output of [`Core::push`].
#[derive(Debug, Clone)]
pub enum Out {
    Event(Event),
    Job(Job),
}

fn secs(samples: u64) -> f64 {
    ms(samples as f64 / f64::from(RATE))
}

/// VAD + segmenter + level meter.
pub struct Core {
    vad: Box<dyn Vad + Send>,
    seg: Segmenter,
    pending: Vec<f32>,
    level_sum: f64,
    level_n: usize,
    level_frames: u32,
    paused: bool,
    seg_events: Vec<SegEvent>,
    /// Consecutive all-zero frames (a gated / powered-off mic delivers exact zeros).
    zero_frames: u32,
    /// Emit `level` events.
    pub levels: bool,
}

impl Core {
    pub fn new(vad: Box<dyn Vad + Send>, cfg: SegConfig) -> Core {
        Core {
            vad,
            seg: Segmenter::new(cfg),
            pending: Vec::with_capacity(4 * FRAME),
            level_sum: 0.0,
            level_n: 0,
            level_frames: 0,
            paused: false,
            seg_events: Vec::new(),
            levels: true,
            zero_frames: 0,
        }
    }

    pub fn vad_name(&self) -> &'static str {
        self.vad.name()
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    /// Audio time consumed so far, seconds.
    pub fn time(&self) -> f64 {
        secs(self.seg.position())
    }

    /// Pause (drop audio; the open segment is discarded) or resume.
    pub fn set_paused(&mut self, paused: bool, out: &mut Vec<Out>) {
        if paused == self.paused {
            return;
        }
        self.paused = paused;
        if paused {
            if let Some(seg) = self.seg.abort() {
                // close the subscriber's partial for that segment
                let t = self.time();
                out.push(Out::Event(Event::Final {
                    seg,
                    text: String::new(),
                    lang: None,
                    t0: t,
                    t1: t,
                    latency_ms: 0,
                    decode_ms: 0,
                }));
            }
            self.pending.clear();
        } else {
            self.vad.reset();
        }
        let state = if paused {
            State::Paused
        } else {
            State::Listening
        };
        out.push(Out::Event(Event::Status {
            state,
            detail: String::new(),
        }));
    }

    /// 16 kHz mono samples in any chunk size.
    pub fn push(&mut self, samples: &[f32], out: &mut Vec<Out>) {
        if self.paused {
            return;
        }
        self.pending.extend_from_slice(samples);
        let mut off = 0;
        while self.pending.len() - off >= FRAME {
            let frame: Vec<f32> = self.pending[off..off + FRAME].to_vec();
            off += FRAME;
            self.frame(&frame, out);
        }
        self.pending.drain(..off);
    }

    fn frame(&mut self, frame: &[f32], out: &mut Vec<Out>) {
        if frame.iter().all(|&s| s == 0.0) {
            self.zero_frames += 1;
            if self.zero_frames == ZERO_FRAMES_WARN {
                out.push(Out::Event(Event::Status {
                    state: State::NoDevice,
                    detail: SILENT_INPUT.into(),
                }));
            }
        } else {
            if self.zero_frames >= ZERO_FRAMES_WARN {
                out.push(Out::Event(Event::Status {
                    state: State::Listening,
                    detail: String::new(),
                }));
            }
            self.zero_frames = 0;
        }
        let speech = self.vad.is_speech(frame);
        self.level_sum += frame
            .iter()
            .map(|&s| f64::from(s) * f64::from(s))
            .sum::<f64>();
        self.level_n += frame.len();
        self.level_frames += 1;
        let mut evs = std::mem::take(&mut self.seg_events);
        self.seg.push(frame, speech, &mut evs);
        for e in evs.drain(..) {
            self.seg_event(e, out);
        }
        self.seg_events = evs;
        if self.level_frames >= LEVEL_FRAMES {
            let rms = (self.level_sum / self.level_n.max(1) as f64) as f32;
            let db = if rms <= 1e-12 {
                -120.0
            } else {
                10.0 * rms.log10()
            };
            if self.levels {
                out.push(Out::Event(Event::Level {
                    db: (db * 10.0).round() / 10.0,
                    speech: self.seg.in_speech(),
                }));
            }
            self.level_sum = 0.0;
            self.level_n = 0;
            self.level_frames = 0;
        }
    }

    fn seg_event(&mut self, e: SegEvent, out: &mut Vec<Out>) {
        let now = Instant::now();
        match e {
            SegEvent::Start { seg, start } => out.push(Out::Event(Event::SpeechStart {
                seg,
                t: secs(start),
            })),
            SegEvent::Partial { seg, audio } => {
                let t1 = self.time();
                let t0 = ms(t1 - audio.len() as f64 / f64::from(RATE));
                out.push(Out::Job(Job {
                    seg,
                    fin: false,
                    audio,
                    t0,
                    t1,
                    endpoint_samples: 0,
                    made: now,
                }));
            }
            SegEvent::End {
                seg,
                start,
                end,
                speech_end,
                decided,
                audio,
                ..
            } => {
                if forced_vad_reset(&audio) {
                    self.vad.reset();
                }
                out.push(Out::Job(Job {
                    seg,
                    fin: true,
                    audio,
                    t0: secs(start),
                    t1: secs(end),
                    endpoint_samples: decided.saturating_sub(speech_end),
                    made: now,
                }));
            }
        }
    }

    /// End of input: close the open segment (its final job is appended).
    pub fn finish(&mut self, out: &mut Vec<Out>) {
        if let Some(e) = self.seg.flush() {
            self.seg_event(e, out);
        }
    }
}

/// Never reset the VAD on normal segment ends (Silero keeps context across them); hook kept for forced splits.
fn forced_vad_reset(_audio: &[f32]) -> bool {
    false
}

/// Turn a decode result into events: the partial / final, and an intent for finals.
pub fn job_events(
    job: &Job,
    r: Result<Transcript, String>,
    decode: Duration,
    waited: Duration,
    intents: &Intents,
    out: &mut Vec<Event>,
) {
    let tr = match r {
        Ok(t) => t,
        Err(e) => {
            out.push(Event::Status {
                state: State::Error,
                detail: format!("decode failed: {e}"),
            });
            Transcript::default()
        }
    };
    let decode_ms = decode.as_secs_f64() * 1e3;
    if !job.fin {
        if !tr.text.is_empty() {
            out.push(Event::Partial {
                seg: job.seg,
                text: tr.text,
                t: job.t1,
            });
        }
        return;
    }
    let lang = tr
        .lang
        .or_else(|| lang::guess(&tr.text).map(str::to_string));
    let endpoint_ms = job.endpoint_samples as f64 * 1e3 / f64::from(RATE);
    let latency_ms = endpoint_ms + waited.as_secs_f64() * 1e3 + decode_ms;
    let intent = intents.match_text(&tr.text);
    out.push(Event::Final {
        seg: job.seg,
        text: tr.text.clone(),
        lang,
        t0: job.t0,
        t1: job.t1,
        latency_ms: latency_ms.round() as u32,
        decode_ms: decode_ms.round() as u32,
    });
    if let Some(i) = intent {
        out.push(Event::Intent {
            seg: job.seg,
            name: i.name,
            text: tr.text,
            slots: i.slots,
        });
    }
}

/// Run everything inline over a whole recording (`chunk` samples per push). Returns the events in order.
pub fn run_offline(
    core: &mut Core,
    engine: &mut dyn Engine,
    intents: &Intents,
    audio: &[f32],
    chunk: usize,
    sink: &dyn Sink,
) {
    let mut outs = Vec::new();
    let mut evs = Vec::new();
    let mut handle = |outs: &mut Vec<Out>, evs: &mut Vec<Event>| {
        for o in outs.drain(..) {
            match o {
                Out::Event(e) => sink.emit(&e),
                Out::Job(job) => {
                    let t = Instant::now();
                    let r = engine.transcribe(&job.audio);
                    job_events(&job, r, t.elapsed(), Duration::ZERO, intents, evs);
                    for e in evs.drain(..) {
                        sink.emit(&e);
                    }
                }
            }
        }
    };
    for c in audio.chunks(chunk.max(1)) {
        core.push(c, &mut outs);
        handle(&mut outs, &mut evs);
    }
    core.finish(&mut outs);
    handle(&mut outs, &mut evs);
}

#[derive(Default)]
struct QueueState {
    finals: VecDeque<Job>,
    partial: Option<Job>,
    closed: bool,
}

/// Decode jobs: finals in order, at most one (the newest) partial.
#[derive(Default)]
pub struct JobQueue {
    m: Mutex<QueueState>,
    cv: Condvar,
}

impl JobQueue {
    pub fn push(&self, job: Job) {
        let mut q = self.m.lock().unwrap();
        if job.fin {
            if q.partial.as_ref().is_some_and(|p| p.seg <= job.seg) {
                q.partial = None;
            }
            q.finals.push_back(job);
        } else {
            q.partial = Some(job);
        }
        self.cv.notify_one();
    }

    /// Drop a pending partial (pause).
    pub fn drop_partial(&self) {
        self.m.lock().unwrap().partial = None;
    }

    /// No more jobs: `pop` returns `None` once the finals are drained.
    pub fn close(&self) {
        self.m.lock().unwrap().closed = true;
        self.cv.notify_all();
    }

    /// Next job (finals first), blocking.
    pub fn pop(&self) -> Option<Job> {
        let mut q = self.m.lock().unwrap();
        loop {
            if let Some(j) = q.finals.pop_front() {
                return Some(j);
            }
            if q.closed {
                return None;
            }
            if let Some(j) = q.partial.take() {
                return Some(j);
            }
            q = self.cv.wait(q).unwrap();
        }
    }
}

/// Builds the engine inside the decoding thread (sherpa-onnx handles are not `Send`).
pub type EngineFactory = Box<dyn FnOnce() -> Result<Box<dyn Engine>, String> + Send>;

/// Settings of [`run_live`].
pub struct LiveConfig {
    pub engine: EngineFactory,
    pub intents: Intents,
}

/// Run until `stop` is set or the audio channel closes. `audio` delivers 16 kHz mono chunks; `commands` comes from
/// the socket. Blocks; the engine is loaded in the decoding thread first (errors are returned before listening).
pub fn run_live(
    mut core: Core,
    cfg: LiveConfig,
    audio: &Receiver<Vec<f32>>,
    commands: &Receiver<Command>,
    sink: Arc<dyn Sink>,
    stop: &AtomicBool,
) -> Result<(), String> {
    let queue = Arc::new(JobQueue::default());
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
    let q2 = Arc::clone(&queue);
    let sink2 = Arc::clone(&sink);
    let LiveConfig { engine, intents } = cfg;
    let decoder = std::thread::Builder::new()
        .name("voice-decode".into())
        .spawn(move || {
            let mut eng = match engine() {
                Ok(e) => {
                    let _ = ready_tx.send(Ok(()));
                    e
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            // warm up (first decode allocates)
            let _ = eng.transcribe(&vec![0.0; RATE as usize / 2]);
            let mut evs = Vec::new();
            let mut last_final = 0u64;
            while let Some(job) = q2.pop() {
                if !job.fin && job.seg <= last_final {
                    continue;
                }
                let waited = job.made.elapsed();
                let t = Instant::now();
                let r = eng.transcribe(&job.audio);
                let dt = t.elapsed();
                if job.fin {
                    last_final = job.seg;
                }
                job_events(&job, r, dt, waited, &intents, &mut evs);
                for e in evs.drain(..) {
                    sink2.emit(&e);
                }
            }
        })
        .map_err(|e| e.to_string())?;
    match ready_rx.recv() {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            let _ = decoder.join();
            return Err(e);
        }
        Err(_) => {
            let _ = decoder.join();
            return Err("decoder thread died while loading".into());
        }
    }
    sink.emit(&Event::Status {
        state: State::Listening,
        detail: String::new(),
    });
    let mut outs = Vec::new();
    let dispatch = |outs: &mut Vec<Out>| {
        for o in outs.drain(..) {
            match o {
                Out::Event(e) => sink.emit(&e),
                Out::Job(j) => queue.push(j),
            }
        }
    };
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        while let Ok(c) = commands.try_recv() {
            match c {
                Command::Pause => {
                    core.set_paused(true, &mut outs);
                    queue.drop_partial();
                }
                Command::Resume => core.set_paused(false, &mut outs),
                Command::Ping => {}
            }
            dispatch(&mut outs);
        }
        match audio.recv_timeout(Duration::from_millis(100)) {
            Ok(chunk) => {
                core.push(&chunk, &mut outs);
                dispatch(&mut outs);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    core.finish(&mut outs);
    dispatch(&mut outs);
    queue.close();
    let _ = decoder.join();
    sink.emit(&Event::Status {
        state: State::Stopped,
        detail: String::new(),
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::FnEngine;
    use crate::server::Collect;
    use crate::vad::tests::{noise, tone};
    use crate::vad::EnergyVad;

    /// silence 1 s, "word" 0.8 s, silence 1 s, "word" 1.2 s, silence 1 s
    fn two_utterances() -> Vec<f32> {
        let r = RATE as usize;
        let mut a = noise(r, 0.001, 1);
        a.extend(tone(r * 8 / 10, 0.2, 2));
        a.extend(noise(r, 0.001, 3));
        a.extend(tone(r * 12 / 10, 0.2, 4));
        a.extend(noise(r, 0.001, 5));
        a
    }

    fn core() -> Core {
        Core::new(Box::new(EnergyVad::default()), SegConfig::default())
    }

    /// The fake engine "hears" the length of the audio in tenths of a second.
    fn fake() -> FnEngine<impl FnMut(&[f32]) -> String> {
        FnEngine(|a: &[f32]| format!("len {}", (a.len() as f32 / 1600.0).round()))
    }

    #[test]
    fn offline_two_segments_with_partials_finals_and_levels() {
        let sink = Collect::default();
        let mut c = core();
        run_offline(
            &mut c,
            &mut fake(),
            &Intents::builtin(),
            &two_utterances(),
            320,
            &sink,
        );
        let ev = sink.events();
        let starts: Vec<f64> = ev
            .iter()
            .filter_map(|e| {
                if let Event::SpeechStart { t, .. } = e {
                    Some(*t)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(starts.len(), 2, "{ev:?}");
        // onset at 1.0 s and 2.8 s, minus up to 320 ms pre-roll and plus the 128 ms onset window
        assert!(starts[0] > 0.6 && starts[0] < 1.0, "{starts:?}");
        assert!(starts[1] > 2.4 && starts[1] < 2.8, "{starts:?}");
        let finals: Vec<(&String, f64, f64, u32)> = ev
            .iter()
            .filter_map(|e| {
                if let Event::Final {
                    text,
                    t0,
                    t1,
                    latency_ms,
                    ..
                } = e
                {
                    Some((text, *t0, *t1, *latency_ms))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(finals.len(), 2);
        // segment = pre-roll + speech + 160 ms tail: about 0.32 + 0.8 + 0.16 s
        assert_eq!(finals[0].0, "len 13");
        assert_eq!(finals[1].0, "len 17");
        // offline latency = hangover (480-512 ms) + decode (~0)
        assert!(finals[0].3 >= 480 && finals[0].3 < 560, "{finals:?}");
        let partials = ev
            .iter()
            .filter(|e| matches!(e, Event::Partial { .. }))
            .count();
        assert!(partials >= 1, "{ev:?}");
        // order per segment: start < partials < final
        let mut seen_final = std::collections::HashSet::new();
        for e in &ev {
            match e {
                Event::Partial { seg, .. } => assert!(!seen_final.contains(seg)),
                Event::Final { seg, .. } => {
                    seen_final.insert(*seg);
                }
                _ => {}
            }
        }
        let levels = ev
            .iter()
            .filter(|e| matches!(e, Event::Level { .. }))
            .count();
        // 5 s of audio = 156 frames = 19 level events of 8 frames
        assert_eq!(levels, 19);
    }

    #[test]
    fn input_ending_in_speech_is_flushed() {
        let sink = Collect::default();
        let mut a = noise(RATE as usize, 0.001, 1);
        a.extend(tone(RATE as usize, 0.2, 2));
        run_offline(&mut core(), &mut fake(), &Intents::none(), &a, 1000, &sink);
        let n = sink
            .events()
            .iter()
            .filter(|e| matches!(e, Event::Final { .. }))
            .count();
        assert_eq!(n, 1);
    }

    #[test]
    fn intents_follow_finals() {
        let sink = Collect::default();
        let mut eng = FnEngine(|_: &[f32]| "Borra la pantalla.".to_string());
        run_offline(
            &mut core(),
            &mut eng,
            &Intents::builtin(),
            &two_utterances(),
            512,
            &sink,
        );
        let ev = sink.events();
        let intents: Vec<&Event> = ev
            .iter()
            .filter(|e| matches!(e, Event::Intent { .. }))
            .collect();
        assert_eq!(intents.len(), 2);
        assert!(matches!(intents[0], Event::Intent { name, .. } if name == "hud.clear"));
        assert!(ev
            .iter()
            .any(|e| matches!(e, Event::Final { lang: Some(l), .. } if l == "es")));
    }

    #[test]
    fn pause_drops_audio_and_closes_the_open_segment() {
        let mut c = core();
        let mut out = Vec::new();
        c.push(&tone(RATE as usize / 2, 0.2, 1), &mut out);
        assert!(out
            .iter()
            .any(|o| matches!(o, Out::Event(Event::SpeechStart { .. }))));
        out.clear();
        c.set_paused(true, &mut out);
        assert!(matches!(&out[0], Out::Event(Event::Final { text, .. }) if text.is_empty()));
        assert!(matches!(
            &out[1],
            Out::Event(Event::Status {
                state: State::Paused,
                ..
            })
        ));
        out.clear();
        c.push(&tone(RATE as usize, 0.2, 1), &mut out);
        assert!(out.is_empty());
        c.set_paused(false, &mut out);
        assert!(matches!(
            &out[0],
            Out::Event(Event::Status {
                state: State::Listening,
                ..
            })
        ));
    }

    #[test]
    fn all_zero_input_is_reported_and_cleared() {
        let mut c = core();
        let mut out = Vec::new();
        c.push(
            &vec![0.0; FRAME * (ZERO_FRAMES_WARN as usize + 5)],
            &mut out,
        );
        let st: Vec<&Event> = out
            .iter()
            .filter_map(|o| match o {
                Out::Event(e @ Event::Status { .. }) => Some(e),
                _ => None,
            })
            .collect();
        assert_eq!(
            st,
            vec![&Event::Status {
                state: State::NoDevice,
                detail: SILENT_INPUT.into()
            }]
        );
        out.clear();
        c.push(&noise(FRAME * 2, 0.001, 9), &mut out);
        assert!(out.iter().any(|o| matches!(
            o,
            Out::Event(Event::Status {
                state: State::Listening,
                ..
            })
        )));
        out.clear();
        c.push(&noise(FRAME * 2, 0.001, 9), &mut out);
        assert!(!out
            .iter()
            .any(|o| matches!(o, Out::Event(Event::Status { .. }))));
    }

    fn job(seg: u64, fin: bool) -> Job {
        Job {
            seg,
            fin,
            audio: vec![],
            t0: 0.0,
            t1: 0.0,
            endpoint_samples: 0,
            made: Instant::now(),
        }
    }

    #[test]
    fn queue_prefers_finals_keeps_newest_partial_and_drops_stale_ones() {
        let q = JobQueue::default();
        q.push(job(1, false));
        q.push(job(1, false));
        q.push(job(2, false));
        assert_eq!(q.pop().map(|j| (j.seg, j.fin)), Some((2, false)));
        q.push(job(3, false));
        q.push(job(3, true)); // its final makes the partial useless
        q.push(job(4, false));
        q.push(job(4, true));
        assert_eq!(q.pop().map(|j| (j.seg, j.fin)), Some((3, true)));
        assert_eq!(q.pop().map(|j| (j.seg, j.fin)), Some((4, true)));
        q.push(job(5, false));
        q.close();
        // closed: remaining finals only
        assert!(q.pop().is_none());
    }

    #[test]
    fn live_driver_end_to_end_with_a_fake_engine() {
        use std::sync::mpsc;
        // a failing engine is reported before listening
        let (_a, arx) = mpsc::channel::<Vec<f32>>();
        let (_c, crx) = mpsc::channel();
        let bad = LiveConfig {
            engine: Box::new(|| Err("no model".to_string())),
            intents: Intents::none(),
        };
        let sink = Arc::new(Collect::default());
        let stop = AtomicBool::new(false);
        assert_eq!(
            run_live(core(), bad, &arx, &crx, sink.clone(), &stop)
                .err()
                .as_deref(),
            Some("no model")
        );

        let (atx, arx) = mpsc::channel();
        let (ctx, crx) = mpsc::channel();
        let audio = two_utterances();
        let feeder = std::thread::spawn(move || {
            // faster than real time, 100 ms chunks; a pause/resume in the first silence
            for (i, c) in audio.chunks(1600).enumerate() {
                if i == 3 {
                    ctx.send(Command::Pause).unwrap();
                    ctx.send(Command::Resume).unwrap();
                    std::thread::sleep(Duration::from_millis(150));
                }
                atx.send(c.to_vec()).unwrap();
                std::thread::sleep(Duration::from_millis(2));
            }
        });
        let cfg = LiveConfig {
            engine: Box::new(|| {
                Ok(
                    Box::new(FnEngine(|a: &[f32]| format!("n{}", a.len() / 1600)))
                        as Box<dyn Engine>,
                )
            }),
            intents: Intents::builtin(),
        };
        run_live(core(), cfg, &arx, &crx, sink.clone(), &stop).unwrap();
        feeder.join().unwrap();
        let ev = sink.events();
        let finals: Vec<&Event> = ev
            .iter()
            .filter(|e| matches!(e, Event::Final { text, .. } if !text.is_empty()))
            .collect();
        assert_eq!(finals.len(), 2, "{ev:?}");
        assert!(
            matches!(
                ev.first(),
                Some(Event::Status {
                    state: State::Listening,
                    ..
                })
            ),
            "{ev:?}"
        );
        assert!(matches!(
            ev.last(),
            Some(Event::Status {
                state: State::Stopped,
                ..
            })
        ));
        assert!(ev.iter().any(|e| matches!(
            e,
            Event::Status {
                state: State::Paused,
                ..
            }
        )));
        for f in finals {
            let Event::Final { latency_ms, .. } = f else {
                unreachable!()
            };
            assert!(*latency_ms >= 480, "{latency_ms}");
        }
    }
}
