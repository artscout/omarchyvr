//! `--demo`: a scripted conversation (English, then Spanish with accents, then a long wrapped sentence, then
//! noise) replayed forever through the same display path as the socket.

use crate::client::Notice;
use crate::protocol::{Event, State};
use std::time::Instant;

struct Seg {
    seg: u64,
    speech: (f32, f32),
    partials: &'static [(f32, &'static str)],
    fin: (f32, &'static str, &'static str),
}

const SEGS: [Seg; 4] = [
    Seg {
        seg: 1,
        speech: (1.0, 3.0),
        partials: &[
            (1.4, "open the"),
            (1.9, "open the bro"),
            (2.5, "open the browser"),
        ],
        fin: (3.4, "Open the browser.", "en"),
    },
    Seg {
        seg: 2,
        speech: (5.0, 8.0),
        partials: &[
            (5.5, "abre el"),
            (6.2, "abre el navegador"),
            (6.9, "abre el navegador y busca"),
            (7.6, "abre el navegador y busca el tiempo en"),
        ],
        fin: (8.5, "Abre el navegador y busca el tiempo en Bogotá.", "es"),
    },
    Seg {
        seg: 3,
        speech: (10.5, 15.0),
        partials: &[
            (11.0, "¿qué hora"),
            (11.8, "¿qué hora es? vamos"),
            (12.8, "¿qué hora es? ¡vamos a probar el año"),
            (13.9, "¿qué hora es? ¡vamos a probar el año nuevo con un texto"),
            (
                14.6,
                "¿qué hora es? ¡vamos a probar el año nuevo con un texto más largo para ver cómo se ajusta",
            ),
        ],
        fin: (
            15.5,
            "¿Qué hora es? ¡Vamos a probar el año nuevo con un texto más largo para ver cómo se ajusta en varias líneas!",
            "es",
        ),
    },
    Seg {
        seg: 4,
        speech: (18.0, 18.5),
        partials: &[(18.3, "uh")],
        fin: (19.0, "", "en"),
    },
];

/// The events of one cycle with their times in seconds, in time order.
pub fn script() -> Vec<(f32, Event)> {
    let mut v: Vec<(f32, Event)> = vec![
        (
            0.0,
            Event::Hello {
                engine: "demo".into(),
                source: "demo script".into(),
                rate: 16000,
            },
        ),
        (
            0.0,
            Event::Status {
                state: State::Listening,
                detail: String::new(),
            },
        ),
    ];
    for s in &SEGS {
        v.push((
            s.speech.0,
            Event::SpeechStart {
                seg: s.seg,
                t: f64::from(s.speech.0),
            },
        ));
        for &(t, text) in s.partials {
            v.push((
                t,
                Event::Partial {
                    seg: s.seg,
                    text: text.into(),
                    t: f64::from(t),
                },
            ));
        }
        let (t, text, lang) = s.fin;
        v.push((
            t,
            Event::Final {
                seg: s.seg,
                text: text.into(),
                lang: Some(lang.into()),
                t0: f64::from(s.speech.0),
                t1: f64::from(s.speech.1),
                latency_ms: 400,
                decode_ms: 70,
            },
        ));
    }
    // about four level events per second, loud while a segment is open
    let end = SEGS.last().map_or(0.0, |s| s.fin.0);
    let mut t = 0.25f32;
    while t <= end {
        let speaking = SEGS.iter().any(|s| t >= s.speech.0 && t < s.speech.1);
        let wobble = (t * 7.3).sin();
        let db = if speaking {
            -24.0 + 8.0 * wobble
        } else {
            -55.0 + 2.0 * wobble
        };
        v.push((
            t,
            Event::Level {
                db,
                speech: speaking,
            },
        ));
        t += 0.25;
    }
    // stable by time; events at equal times keep their order (levels after the other events of that instant)
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    v
}

pub struct Demo {
    start: Option<Instant>,
    script: Vec<(f32, Event)>,
    next: usize,
    cycle: f32,
}

impl Demo {
    /// `timeout`: the HUD's hide timeout in seconds; the cycle waits that long (plus 3 s) after the last event so
    /// that the hiding is part of the demo.
    pub fn new(timeout: f32) -> Demo {
        let script = script();
        let last = script.last().map_or(0.0, |e| e.0);
        Demo {
            start: None,
            script,
            next: 0,
            cycle: last + timeout.max(0.0) + 3.0,
        }
    }

    pub fn cycle_secs(&self) -> f32 {
        self.cycle
    }

    /// Events due at `now`; a restart of the cycle looks like a reconnection (`Connected` first).
    pub fn poll(&mut self, now: Instant, out: &mut Vec<Notice>) {
        let start = match self.start {
            Some(s) => s,
            None => {
                self.start = Some(now);
                self.next = 0;
                out.push(Notice::Connected);
                now
            }
        };
        let elapsed = now.saturating_duration_since(start).as_secs_f32();
        while self.next < self.script.len() && self.script[self.next].0 <= elapsed {
            out.push(Notice::Event(self.script[self.next].1.clone()));
            self.next += 1;
        }
        if elapsed >= self.cycle {
            self.start = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::{Config, Display, FixedMeasure, Kind};
    use std::time::Duration;

    #[test]
    fn script_is_sorted_and_complete() {
        let s = script();
        assert!(s.windows(2).all(|w| w[0].0 <= w[1].0));
        assert!(matches!(s[0].1, Event::Hello { .. }));
        let finals = s
            .iter()
            .filter(|(_, e)| matches!(e, Event::Final { .. }))
            .count();
        assert_eq!(finals, 4);
        // every partial belongs to a segment that gets its final later
        for (t, e) in &s {
            if let Event::Partial { seg, .. } = e {
                assert!(s.iter().any(
                    |(t2, e2)| matches!(e2, Event::Final { seg: s2, .. } if s2 == seg) && t2 > t
                ));
            }
        }
        // about 4 level events per second
        let levels = s
            .iter()
            .filter(|(_, e)| matches!(e, Event::Level { .. }))
            .count();
        assert!((70..=80).contains(&levels), "{levels}");
        // Spanish with accents and ñ, and an empty final
        assert!(s.iter().any(|(_, e)| matches!(e,
            Event::Final { text, .. } if text.contains('ñ') && text.contains('¿') && text.contains('á'))));
        assert!(s
            .iter()
            .any(|(_, e)| matches!(e, Event::Final { text, .. } if text.is_empty())));
    }

    #[test]
    fn replayed_through_the_display_it_shows_hides_and_repeats() {
        let mut demo = Demo::new(12.0);
        let mut d = Display::new(Config::default());
        let m = FixedMeasure(10.0);
        let t0 = Instant::now();
        let mut shown_ntilde = false;
        let mut max_lines = 0;
        let mut hidden_after_text = false;
        let mut saw_text = false;
        let mut connects = 0;
        let steps = (2.2 * demo.cycle_secs() / 0.05) as usize;
        for i in 0..steps {
            let now = t0 + Duration::from_secs_f32(i as f32 * 0.05);
            let mut n = Vec::new();
            demo.poll(now, &mut n);
            for notice in n {
                match notice {
                    Notice::Connected => {
                        connects += 1;
                        d.on_connected();
                    }
                    Notice::Event(e) => d.on_event(&e, now),
                    other => panic!("{other:?}"),
                }
            }
            d.expire(now);
            let v = d.view(&m, 400.0);
            max_lines = max_lines.max(v.lines.len());
            if v.visible && !v.lines.is_empty() {
                saw_text = true;
            }
            if saw_text && !v.visible {
                hidden_after_text = true;
            }
            if v.lines
                .iter()
                .any(|l| l.text.contains('ñ') && l.kind == Kind::Final)
            {
                shown_ntilde = true;
            }
        }
        assert!(shown_ntilde, "the Spanish final with ñ is shown");
        assert_eq!(max_lines, 3);
        assert!(hidden_after_text, "the HUD hides between cycles");
        assert_eq!(connects, 3, "two full cycles and the start of the third");
    }
}
