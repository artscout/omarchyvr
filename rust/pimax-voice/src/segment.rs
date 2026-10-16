//! Speech segmentation on per-frame VAD decisions: pre-roll, minimum speech, hangover (end of speech), forced split
//! of very long segments, and when to run a partial decode.
//!
//! Pure state machine, no clocks: times are sample counts at 16 kHz since the stream started.

use crate::vad::{FRAME, FRAME_MS};
use std::collections::VecDeque;

/// Segmenter settings, in frames of [`FRAME`] samples (32 ms).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegConfig {
    /// Consecutive speech frames that open a segment.
    pub min_speech: u32,
    /// Non-speech frames after the last speech frame that close it (the end-of-speech delay).
    pub hangover: u32,
    /// Frames before the onset that go into the segment.
    pub preroll: u32,
    /// Non-speech frames after the last speech frame that stay in the segment audio.
    pub tail: u32,
    /// Longest segment; a longer one is closed (forced) and a new one opened.
    pub max_len: u32,
    /// Speech frames between partial decodes; 0 disables partials.
    pub partial_every: u32,
    /// Shortest segment that gets a partial decode.
    pub min_partial: u32,
}

/// Milliseconds to whole frames (rounded up).
pub fn frames_of_ms(ms: u32) -> u32 {
    (f64::from(ms) / FRAME_MS).ceil() as u32
}

impl Default for SegConfig {
    /// 128 ms to open, 480 ms of silence to close, 320 ms pre-roll, 160 ms tail, 20 s max, a partial every 640 ms
    /// once the segment is 640 ms long.
    fn default() -> Self {
        SegConfig {
            min_speech: frames_of_ms(128),
            hangover: frames_of_ms(480),
            preroll: frames_of_ms(320),
            tail: frames_of_ms(160),
            max_len: frames_of_ms(20_000),
            partial_every: frames_of_ms(640),
            min_partial: frames_of_ms(640),
        }
    }
}

/// What the segmenter decided after a frame.
#[derive(Debug, Clone, PartialEq)]
pub enum SegEvent {
    /// A segment opened; `start` is the first sample of its audio (onset minus pre-roll).
    Start { seg: u64, start: u64 },
    /// Time for a partial decode of the open segment: its audio so far.
    Partial { seg: u64, audio: Vec<f32> },
    /// The segment closed. `audio` covers `start..end`; `speech_end` is the end of its last speech frame;
    /// `decided` is the sample at which the end was decided (`speech_end` + hangover, or the split point).
    End {
        seg: u64,
        start: u64,
        end: u64,
        speech_end: u64,
        decided: u64,
        forced: bool,
        audio: Vec<f32>,
    },
}

#[derive(Debug)]
struct Open {
    seg: u64,
    start: u64,
    audio: Vec<f32>,
    frames: u32,
    /// Length of `audio` at the end of the last speech frame.
    speech_len: usize,
    silence_run: u32,
    since_partial: u32,
}

/// The state machine. Feed every frame with its VAD decision through [`Segmenter::push`].
#[derive(Debug)]
pub struct Segmenter {
    cfg: SegConfig,
    /// Recent frames while idle (pre-roll + onset candidates).
    ring: VecDeque<Vec<f32>>,
    run: u32,
    open: Option<Open>,
    next_seg: u64,
    /// Samples consumed so far.
    pos: u64,
}

impl Segmenter {
    pub fn new(cfg: SegConfig) -> Segmenter {
        let cfg = SegConfig {
            min_speech: cfg.min_speech.max(1),
            hangover: cfg.hangover.max(1),
            max_len: cfg.max_len.max(2),
            ..cfg
        };
        Segmenter {
            cfg,
            ring: VecDeque::new(),
            run: 0,
            open: None,
            next_seg: 1,
            pos: 0,
        }
    }

    pub fn config(&self) -> &SegConfig {
        &self.cfg
    }

    /// Samples consumed so far.
    pub fn position(&self) -> u64 {
        self.pos
    }

    /// Is a segment open?
    pub fn in_speech(&self) -> bool {
        self.open.is_some()
    }

    /// Id of the open segment.
    pub fn open_seg(&self) -> Option<u64> {
        self.open.as_ref().map(|o| o.seg)
    }

    /// Drop the open segment and the pre-roll (pause); returns its id if one was open.
    pub fn abort(&mut self) -> Option<u64> {
        self.ring.clear();
        self.run = 0;
        self.open.take().map(|o| o.seg)
    }

    /// Close the open segment now (end of input), as if the hangover had passed.
    pub fn flush(&mut self) -> Option<SegEvent> {
        let o = self.open.take()?;
        Some(self.close(o, false))
    }

    fn close(&mut self, mut o: Open, forced: bool) -> SegEvent {
        let keep = if forced {
            o.audio.len()
        } else {
            (o.speech_len + self.cfg.tail as usize * FRAME).min(o.audio.len())
        };
        o.audio.truncate(keep);
        SegEvent::End {
            seg: o.seg,
            start: o.start,
            end: o.start + keep as u64,
            speech_end: o.start + o.speech_len as u64,
            decided: self.pos,
            forced,
            audio: o.audio,
        }
    }

    /// One frame ([`FRAME`] samples) and its VAD decision. Events are appended to `out`.
    pub fn push(&mut self, frame: &[f32], speech: bool, out: &mut Vec<SegEvent>) {
        debug_assert_eq!(frame.len(), FRAME);
        self.pos += frame.len() as u64;
        if let Some(mut o) = self.open.take() {
            o.audio.extend_from_slice(frame);
            o.frames += 1;
            if speech {
                o.silence_run = 0;
                o.speech_len = o.audio.len();
                o.since_partial += 1;
            } else {
                o.silence_run += 1;
            }
            if o.silence_run >= self.cfg.hangover {
                let ev = self.close(o, false);
                out.push(ev);
                return;
            }
            if o.frames >= self.cfg.max_len {
                let ev = self.close(o, true);
                out.push(ev);
                if speech {
                    // keep going: the speech continues in a new segment (no pre-roll: it is in the closed one)
                    self.open_segment(Vec::new(), self.pos, out);
                }
                return;
            }
            if speech
                && self.cfg.partial_every > 0
                && o.since_partial >= self.cfg.partial_every
                && o.frames >= self.cfg.min_partial
            {
                o.since_partial = 0;
                out.push(SegEvent::Partial {
                    seg: o.seg,
                    audio: o.audio.clone(),
                });
            }
            self.open = Some(o);
            return;
        }
        // idle
        self.ring.push_back(frame.to_vec());
        let cap = (self.cfg.preroll + self.cfg.min_speech) as usize;
        while self.ring.len() > cap {
            self.ring.pop_front();
        }
        self.run = if speech { self.run + 1 } else { 0 };
        if self.run >= self.cfg.min_speech {
            self.run = 0;
            let n = self.ring.len();
            let mut audio = Vec::with_capacity((n + 64) * FRAME);
            for f in self.ring.drain(..) {
                audio.extend_from_slice(&f);
            }
            let start = self.pos - audio.len() as u64;
            self.open_segment(audio, start, out);
        }
    }

    fn open_segment(&mut self, audio: Vec<f32>, start: u64, out: &mut Vec<SegEvent>) {
        let seg = self.next_seg;
        self.next_seg += 1;
        let frames = (audio.len() / FRAME) as u32;
        let speech_len = audio.len();
        self.open = Some(Open {
            seg,
            start,
            audio,
            frames,
            speech_len,
            silence_run: 0,
            since_partial: frames.min(self.cfg.min_speech),
        });
        out.push(SegEvent::Start { seg, start });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed a pattern of speech flags ('#' speech, '.' silence); frame k is filled with the value k.
    fn run(cfg: SegConfig, pattern: &str) -> (Vec<SegEvent>, Segmenter) {
        let mut s = Segmenter::new(cfg);
        let mut out = Vec::new();
        for (k, c) in pattern.chars().filter(|c| !c.is_whitespace()).enumerate() {
            let frame = vec![k as f32; FRAME];
            s.push(&frame, c == '#', &mut out);
        }
        (out, s)
    }

    fn cfg() -> SegConfig {
        SegConfig {
            min_speech: 2,
            hangover: 3,
            preroll: 2,
            tail: 1,
            max_len: 100,
            partial_every: 0,
            min_partial: 0,
        }
    }

    fn frame_ids(audio: &[f32]) -> Vec<u32> {
        audio.chunks(FRAME).map(|f| f[0] as u32).collect()
    }

    #[test]
    fn default_config_in_frames() {
        let c = SegConfig::default();
        assert_eq!(
            (c.min_speech, c.hangover, c.preroll, c.tail),
            (4, 15, 10, 5)
        );
        assert_eq!(c.max_len, 625);
        assert_eq!(frames_of_ms(1), 1);
        assert_eq!(frames_of_ms(0), 0);
    }

    #[test]
    fn single_utterance_with_preroll_tail_and_hangover() {
        // frames: 0-3 silence, 4-9 speech, 10.. silence
        let (ev, s) = run(cfg(), "....###### ......");
        assert_eq!(ev.len(), 2, "{ev:?}");
        // opens after 2 speech frames (4, 5) with 2 frames of pre-roll (2, 3)
        assert_eq!(
            ev[0],
            SegEvent::Start {
                seg: 1,
                start: 2 * FRAME as u64
            }
        );
        let SegEvent::End {
            seg,
            start,
            end,
            speech_end,
            decided,
            forced,
            audio,
        } = &ev[1]
        else {
            panic!("{ev:?}")
        };
        assert_eq!((*seg, *forced), (1, false));
        assert_eq!(frame_ids(audio), vec![2, 3, 4, 5, 6, 7, 8, 9, 10]);
        assert_eq!(*start, 2 * FRAME as u64);
        assert_eq!(*speech_end, 10 * FRAME as u64);
        assert_eq!(*end, 11 * FRAME as u64);
        // decided after 3 silent frames (10, 11, 12)
        assert_eq!(*decided, 13 * FRAME as u64);
        assert!(!s.in_speech());
        assert_eq!(s.position(), 16 * FRAME as u64);
    }

    #[test]
    fn short_blips_and_short_pauses() {
        // a single speech frame never opens a segment
        let (ev, _) = run(cfg(), "..#..#..#..");
        assert!(ev.is_empty(), "{ev:?}");
        // a 2-frame pause inside speech (shorter than the hangover) keeps one segment
        let (ev, _) = run(cfg(), "..###..###.....");
        assert_eq!(
            ev.iter()
                .filter(|e| matches!(e, SegEvent::End { .. }))
                .count(),
            1
        );
        // a 3-frame pause splits
        let (ev, _) = run(cfg(), "..###...###.....");
        let ends: Vec<u64> = ev
            .iter()
            .filter_map(|e| {
                if let SegEvent::End { seg, .. } = e {
                    Some(*seg)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(ends, vec![1, 2]);
    }

    #[test]
    fn forced_split_keeps_all_audio_and_continues() {
        let c = SegConfig {
            max_len: 5,
            ..cfg()
        };
        let (ev, s) = run(c, "..############");
        let kinds: Vec<String> = ev
            .iter()
            .map(|e| match e {
                SegEvent::Start { seg, .. } => format!("S{seg}"),
                SegEvent::End { seg, forced, .. } => {
                    format!("E{seg}{}", if *forced { "f" } else { "" })
                }
                SegEvent::Partial { .. } => "P".into(),
            })
            .collect();
        assert_eq!(kinds, vec!["S1", "E1f", "S2", "E2f", "S3"]);
        // every sample from the onset is in exactly one segment
        let mut ids = Vec::new();
        for e in &ev {
            if let SegEvent::End { audio, .. } = e {
                ids.extend(frame_ids(audio));
            }
        }
        assert_eq!(ids, (0..10).collect::<Vec<u32>>());
        assert!(s.in_speech());
    }

    #[test]
    fn partials_follow_speech_and_stop_in_silence() {
        let c = SegConfig {
            partial_every: 3,
            min_partial: 4,
            ..cfg()
        };
        let (ev, _) = run(c, "..##########....");
        let partial_lens: Vec<usize> = ev
            .iter()
            .filter_map(|e| {
                if let SegEvent::Partial { audio, .. } = e {
                    Some(audio.len() / FRAME)
                } else {
                    None
                }
            })
            .collect();
        // segment audio grows by one frame per speech frame from 4 frames (2 pre-roll + 2 onset)
        assert_eq!(partial_lens, vec![5, 8, 11]);
        // partials disabled
        let (ev, _) = run(cfg(), "..##########....");
        assert!(!ev.iter().any(|e| matches!(e, SegEvent::Partial { .. })));
    }

    #[test]
    fn flush_and_abort() {
        let mut s = Segmenter::new(cfg());
        let mut out = Vec::new();
        for k in 0..6 {
            s.push(&vec![k as f32; FRAME], k >= 2, &mut out);
        }
        assert!(s.in_speech());
        let Some(SegEvent::End { audio, forced, .. }) = s.flush() else {
            panic!()
        };
        assert!(!forced);
        assert_eq!(frame_ids(&audio), vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(s.flush(), None);
        for k in 6..10 {
            s.push(&vec![k as f32; FRAME], true, &mut out);
        }
        assert_eq!(s.abort(), Some(2));
        assert!(!s.in_speech());
        assert_eq!(s.abort(), None);
    }
}
