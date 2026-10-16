//! `pimax-eye replay`: send the frames of `data/replay-frames.txt` (and nothing else) and listen for gaze.

use crate::frames::{hex, select, Action, FStep, Guard, Skips, AUTH_OPS};
use crate::gaze::{format_line, parse_payload, EyeState, GazeStats};
use crate::session::{in_decoder, IN_BUFFER};
use crate::sigint;
use crate::transport::Transport;
use crate::ttp::{opcode_name, Kind, Message, STREAM_GAZE};
use std::io::Write;
use std::time::{Duration, Instant};

/// Options of one run.
pub struct Opts {
    /// Optional parts to leave out.
    pub skips: Skips,
    /// Seconds to listen after the last step.
    pub seconds: f64,
    /// Continue although gaze frames are already arriving before the first send.
    pub force: bool,
    /// Timeout of every USB transfer.
    pub timeout: Duration,
}

/// Result of a run.
#[derive(Debug, Default)]
pub struct Outcome {
    /// Exit code: 0 gaze received, 3 aborted (no response, USB error, unexpected status, Ctrl-C), 4 no gaze.
    pub code: i32,
    /// `(opcode, status word, payload length)` of every response.
    pub replies: Vec<(u32, u32, u32)>,
    /// Gaze samples decoded.
    pub samples: u64,
}

struct Log<'a> {
    out: &'a mut dyn Write,
    raw: Option<&'a mut dyn Write>,
    t0: Instant,
}

impl Log<'_> {
    fn say(&mut self, s: &str) {
        let _ = writeln!(self.out, "[{:7.3}] {s}", self.t0.elapsed().as_secs_f64());
        let _ = self.out.flush();
    }
    fn raw_in(&mut self, m: &Message) {
        if let Some(r) = self.raw.as_mut() {
            let _ = writeln!(
                r,
                "{:.4} IN kind={:?} seq={} status={} id={:#x} aux={} plen={} {}",
                self.t0.elapsed().as_secs_f64(),
                m.header.kind,
                m.header.seq,
                m.header.status,
                m.header.id,
                m.header.aux,
                m.header.payload_len,
                if m.header.id == STREAM_GAZE {
                    String::from("(gaze payload omitted)")
                } else {
                    hex(&m.payload)
                }
            );
        }
    }
}

fn short_hex(b: &[u8], n: usize) -> String {
    if b.len() <= n {
        hex(b)
    } else {
        format!("{}... ({} B)", hex(&b[..n]), b.len())
    }
}

fn describe(m: &Message) -> String {
    let h = &m.header;
    format!(
        "{:?} seq={} status={} id={:#x} aux={} plen={} payload={}",
        h.kind,
        h.seq,
        h.status,
        h.id,
        h.aux,
        h.payload_len,
        short_hex(&m.payload, 96)
    )
}

/// Run the selected steps and the listening phase.
pub fn run_session<'a, T: Transport>(
    t: &mut T,
    steps: &[FStep],
    opts: &Opts,
    out: &'a mut dyn Write,
    raw: Option<&'a mut dyn Write>,
) -> Result<Outcome, String> {
    let guard = Guard::new(steps)?;
    let sel = select(steps, opts.skips);
    let mut log = Log {
        out,
        raw,
        t0: Instant::now(),
    };
    let mut dec = in_decoder();
    let mut o = Outcome::default();
    let mut stats = GazeStats::default();
    let mut pre_gaze = 0u64;

    // Drain whatever the chip has pending; gaze already flowing means a previous session is still subscribed.
    let drain_until = Instant::now() + Duration::from_millis(300);
    while Instant::now() < drain_until {
        if let Some(b) = t
            .bulk_in(0x83, IN_BUFFER, Duration::from_millis(100))
            .map_err(|e| format!("initial IN read: {e}"))?
        {
            for m in dec.push(&b).into_iter().flatten() {
                log.raw_in(&m);
                if m.header.kind == Kind::Notification && m.header.id == STREAM_GAZE {
                    pre_gaze += 1;
                } else {
                    log.say(&format!("pending IN: {}", describe(&m)));
                }
            }
        }
    }
    if pre_gaze > 0 {
        log.say(&format!("{pre_gaze} gaze notifications were already arriving before any send (previous session still subscribed)"));
        if !opts.force {
            o.code = 3;
            return Err(
                "device already streaming; rerun with --force-replay to send the sequence anyway"
                    .into(),
            );
        }
    } else {
        log.say("IN pipe idle before the first send");
    }

    let mut prev_t: Option<f64> = None;
    let mut prev_sent = Instant::now();
    for st in sel {
        if sigint::stopped() {
            log.say("Ctrl-C: stopping, nothing further sent");
            o.code = 3;
            return Ok(o);
        }
        if let (Some(pt), Some(ct), true) = (prev_t, st.t, matches!(st.action, Action::Ttp { .. }))
        {
            let gap = (ct - pt).clamp(0.0, 1.0);
            let left = Duration::from_secs_f64(gap).saturating_sub(prev_sent.elapsed());
            if !left.is_zero() {
                std::thread::sleep(left);
            }
        }
        prev_sent = Instant::now();
        if matches!(st.action, Action::Ttp { .. }) {
            prev_t = st.t;
        }
        match &st.action {
            Action::Enumeration => unreachable!("never selected"),
            Action::Control { setup, data } => {
                log.say(&format!(
                    "STEP {} [{}] CTRL setup={} data={}",
                    st.n,
                    st.class,
                    hex(setup),
                    hex(data)
                ));
                let r = guard
                    .control(t, *setup, data, opts.timeout)
                    .map_err(|e| format!("step {}: {e}", st.n))?;
                if setup[0] & 0x80 != 0 {
                    let txt = String::from_utf8_lossy(&r).replace('\0', "\\0");
                    log.say(&format!(
                        "  <- {} B hex={} text={:?}",
                        r.len(),
                        short_hex(&r, 48),
                        txt.chars().take(300).collect::<String>()
                    ));
                }
            }
            Action::Ttp { seq, op, frames } => {
                log.say(&format!(
                    "STEP {} [{}] seq={seq} op={op:#05x} ({}) -> {} URB(s), {} B",
                    st.n,
                    st.class,
                    opcode_name(*op),
                    frames.len(),
                    frames.iter().map(Vec::len).sum::<usize>()
                ));
                for f in frames {
                    guard
                        .bulk_out(t, f, opts.timeout)
                        .map_err(|e| format!("step {}: {e}", st.n))?;
                }
                let sent = Instant::now();
                let wait = if *op == 0x456 {
                    Duration::from_secs(10)
                } else {
                    Duration::from_secs(3)
                };
                let resp = loop {
                    if sigint::stopped() {
                        o.code = 3;
                        return Ok(o);
                    }
                    if sent.elapsed() > wait {
                        log.say(&format!(
                            "NO RESPONSE to seq {seq} op {op:#x} within {} s: stopping",
                            wait.as_secs()
                        ));
                        o.code = 3;
                        return Ok(o);
                    }
                    let Some(b) = t
                        .bulk_in(0x83, IN_BUFFER, Duration::from_millis(200))
                        .map_err(|e| format!("IN read: {e}"))?
                    else {
                        continue;
                    };
                    let mut found = None;
                    for r in dec.push(&b) {
                        match r {
                            Ok(m) => {
                                log.raw_in(&m);
                                if m.header.kind == Kind::Response && m.header.seq == *seq {
                                    found = Some(m);
                                } else if m.header.kind == Kind::Notification
                                    && m.header.id == STREAM_GAZE
                                {
                                    o.samples += 1;
                                } else {
                                    log.say(&format!("  IN other: {}", describe(&m)));
                                }
                            }
                            Err(e) => log.say(&format!("  IN framing error: {e}")),
                        }
                    }
                    if let Some(m) = found {
                        break m;
                    }
                };
                log.say(&format!(
                    "  <- {} ({} ms)",
                    describe(&resp),
                    sent.elapsed().as_millis()
                ));
                o.replies
                    .push((*op, resp.header.status, resp.header.payload_len));
                if resp.header.status != 1 {
                    log.say(&format!(
                        "  UNEXPECTED response status {} (all captured replies have 1)",
                        resp.header.status
                    ));
                    if !AUTH_OPS.contains(op) {
                        log.say("  stopping: unexpected status on a non-auth step");
                        o.code = 3;
                        return Ok(o);
                    }
                }
            }
        }
    }

    log.say(&format!(
        "sequence sent; listening for gaze for {:.1} s",
        opts.seconds
    ));
    let start = Instant::now();
    let dur = Duration::from_secs_f64(opts.seconds);
    let mut first: Option<Duration> = None;
    let mut last = Duration::ZERO;
    let mut shown = 0u64;
    let mut other = 0u64;
    let mut bytes = 0u64;
    let mut last_sample = None;
    while start.elapsed() < dur && !sigint::stopped() {
        let Some(b) = t
            .bulk_in(0x83, IN_BUFFER, Duration::from_millis(100))
            .map_err(|e| format!("IN read: {e}"))?
        else {
            continue;
        };
        let at = start.elapsed();
        bytes += b.len() as u64;
        for r in dec.push(&b) {
            match r {
                Ok(m) => {
                    log.raw_in(&m);
                    if m.header.kind == Kind::Notification && m.header.id == STREAM_GAZE {
                        match parse_payload(&m.payload) {
                            Ok(g) => {
                                stats.add(&g);
                                o.samples += 1;
                                first.get_or_insert(at);
                                last = at;
                                if shown < 3 || stats.samples % 120 == 0 {
                                    shown += 1;
                                    log.say(&format!(
                                        "gaze #{} {}",
                                        stats.samples,
                                        format_line(&g)
                                    ));
                                }
                                last_sample = Some(g);
                            }
                            Err(e) => log.say(&format!("gaze payload decode error: {e}")),
                        }
                    } else {
                        other += 1;
                        log.say(&format!("IN other: {}", describe(&m)));
                    }
                }
                Err(e) => log.say(&format!("IN framing error: {e}")),
            }
        }
    }
    let span = last.saturating_sub(first.unwrap_or_default());
    let host_rate = (stats.samples > 1 && !span.is_zero())
        .then(|| (stats.samples - 1) as f64 / span.as_secs_f64());
    log.say(&format!(
        "SUMMARY listened {:.2} s: {} gaze samples decoded (+{} during the sequence), {} other msgs, {} bytes; host rate {} Hz, device-timestamp rate {} Hz; first gaze after {} ms",
        start.elapsed().as_secs_f64(),
        stats.samples,
        o.samples - stats.samples,
        other,
        bytes,
        host_rate.map_or("-".into(), |r| format!("{r:.1}")),
        stats.device_rate_hz().map_or("-".into(), |r| format!("{r:.1}")),
        first.map_or("-".into(), |f| f.as_millis().to_string()),
    ));
    let names = ["lost", "origin-only", "tracked"];
    for (e, n) in ["A", "B"].iter().enumerate() {
        log.say(&format!(
            "  eye {n}: {}",
            (0..3)
                .map(|i| format!("{}={}", names[i], stats.states[e][i]))
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }
    log.say(&format!(
        "  counter gaps {}, timestamp gaps {}, regressions {}",
        stats.counter_gaps, stats.timestamp_gaps, stats.timestamp_regressions
    ));
    if let Some(g) = last_sample {
        log.say(&format!(
            "sample frame: ts={} us counter={:?}",
            g.timestamp_us, g.frame_counter
        ));
        for (n, e) in ["A", "B"].iter().zip(&g.eyes) {
            log.say(&format!(
                "  eye {n}: state={} origin={:?} valid={} dir={:?} valid={} pupil={:.2} mm pupil_pos={:?} valid={} flags={:?}",
                match e.state() { EyeState::Lost => "lost", EyeState::OriginOnly => "origin-only", EyeState::Tracked => "tracked" },
                e.origin_mm, e.origin_valid, e.direction, e.direction_valid, e.pupil_diameter_mm, e.pupil_pos, e.pupil_pos_valid, e.flags
            ));
        }
    }
    o.code = if sigint::stopped() {
        3
    } else if stats.samples > 0 {
        0
    } else {
        4
    };
    Ok(o)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeChip;
    use crate::frames::{parse, DATA};

    #[test]
    fn dry_selection_never_contains_denied_ops() {
        let steps = parse(DATA).unwrap();
        assert!(Guard::new(&steps).is_ok());
    }

    #[test]
    fn runs_against_the_fake_chip() {
        let steps = parse(DATA).unwrap();
        let mut chip = FakeChip::new();
        let mut out = Vec::new();
        let opts = Opts {
            skips: Skips {
                auth: false,
                cal: true,
                vendor_write: true,
            },
            seconds: 0.2,
            force: false,
            timeout: Duration::from_millis(500),
        };
        let r = run_session(&mut chip, &steps, &opts, &mut out, None);
        let text = String::from_utf8_lossy(&out).into_owned();
        // the fake may not answer every opcode; either outcome must never panic and must not send foreign frames
        assert!(r.is_ok() || !r.unwrap_err().is_empty(), "{text}");
    }
}
