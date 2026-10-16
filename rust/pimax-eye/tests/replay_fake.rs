//! The replay engine against the in-process chip, driven by the REAL cold-start plan (with the two cut uploads
//! either refused, skipped or filled with fabricated bytes that only the fake ever sees).
mod common;

use common::*;
use pimax_eye::fake::{FakeChip, Sent};
use pimax_eye::replay::{StepKind, EP_BULK_OUT};
use pimax_eye::session::{in_decoder, read_stream, run_plan, ReplayError, ReplayOptions};
use pimax_eye::transport::Transport;
use std::io;
use std::time::Duration;

fn quick() -> ReplayOptions {
    ReplayOptions {
        allow_incomplete: false,
        timeout: Duration::from_millis(300),
    }
}

/// What a faithful replay must send: control transfers and bulk frames of the plan, in order, nothing else.
fn expected_sent(plan: &pimax_eye::replay::Plan, skip: &[usize]) -> Vec<Sent> {
    let mut v = Vec::new();
    for (i, s) in plan.steps.iter().enumerate() {
        if skip.contains(&i) {
            continue;
        }
        match &s.kind {
            StepKind::ControlOut { setup, data } => v.push(Sent::Control {
                setup: *setup,
                data: data.clone(),
            }),
            StepKind::ControlIn { setup } => v.push(Sent::Control {
                setup: *setup,
                data: Vec::new(),
            }),
            StepKind::Request { frames, .. } => {
                v.extend(frames.iter().map(|f| Sent::BulkOut {
                    ep: EP_BULK_OUT,
                    data: f.clone(),
                }));
            }
        }
    }
    v
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn full_replay_sends_exactly_the_plan_then_the_stream_starts() {
    let plan = fabricated_complete_plan();
    assert!(plan.is_complete());
    let mut chip = FakeChip::new();
    let mut dec = in_decoder();
    let mut log = Vec::new();
    let rep = run_plan(&mut chip, &plan, &quick(), &mut dec, |l| {
        log.push(l.to_string())
    })
    .unwrap();
    assert_eq!(rep.sent, 39);
    assert!(rep.skipped.is_empty());
    assert_eq!(rep.exchanges.len(), 21);
    assert!(rep.exchanges.iter().all(|e| e.status == 1));
    assert_eq!(rep.control_in.len(), 18 - 2);
    assert_eq!(
        chip.sent,
        expected_sent(&plan, &[]),
        "nothing but the plan may reach the device"
    );
    assert!(
        chip.streaming,
        "the profile upload (0x456) starts the stream in the fake"
    );

    // 200 ms of stream: ~24 samples at 120.5 Hz
    let mut first = None;
    let s = read_stream(&mut chip, &mut dec, Duration::from_millis(200), |g, at| {
        first.get_or_insert((at, g.timestamp_us));
    })
    .unwrap();
    assert_eq!(s.parse_errors, 0, "{:?}", s.first_parse_error);
    assert_eq!(s.framing_errors, 0);
    assert!((15..=26).contains(&s.samples), "{} samples", s.samples);
    assert!(s.first_sample_after.unwrap() < Duration::from_millis(60));
    let hz = s.host_rate_hz().unwrap();
    assert!((95.0..130.0).contains(&hz), "host rate {hz}");
    let dev = s.stats.device_rate_hz().unwrap();
    assert!((dev - 120.48).abs() < 0.1, "device rate {dev}");
    assert_eq!(s.stats.counter_gaps, 0);
    assert_eq!(s.stats.timestamp_gaps, 0);
    assert_eq!(s.stats.states[0][2], s.samples, "fake eye A is tracked");
    assert!(log.iter().any(|l| l.contains("op 0x456")));
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn incomplete_capture_is_refused_before_anything_is_sent() {
    let plan = cold_start_plan();
    let mut chip = FakeChip::new();
    let err = run_plan(&mut chip, &plan, &quick(), &mut in_decoder(), |_| {}).unwrap_err();
    match err {
        ReplayError::Incomplete(steps) => {
            assert_eq!(
                steps.iter().map(|s| s.step).collect::<Vec<_>>(),
                vec![8, 38]
            );
            assert_eq!(
                steps.iter().map(|s| s.missing).collect::<Vec<_>>(),
                vec![2318, 30343]
            );
            assert!(steps[1].what.contains("op=0x456"));
        }
        e => panic!("{e}"),
    }
    assert!(
        chip.sent.is_empty(),
        "a refused plan must not touch the device"
    );
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn allow_incomplete_skips_exactly_the_cut_steps() {
    let plan = cold_start_plan();
    let mut chip = FakeChip::new();
    let opts = ReplayOptions {
        allow_incomplete: true,
        ..quick()
    };
    let rep = run_plan(&mut chip, &plan, &opts, &mut in_decoder(), |_| {}).unwrap();
    assert_eq!(rep.skipped, vec![8, 38]);
    assert_eq!(rep.sent, 37);
    assert_eq!(chip.sent, expected_sent(&plan, &[8, 38]));
    assert!(
        !chip.streaming,
        "without the profile upload the fake does not stream"
    );
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn an_unexpected_status_stops_the_replay_at_once() {
    let plan = fabricated_complete_plan();
    let mut chip = FakeChip::new();
    chip.status_override.insert(0x777, 7);
    let err = run_plan(&mut chip, &plan, &quick(), &mut in_decoder(), |_| {}).unwrap_err();
    assert!(
        matches!(
            err,
            ReplayError::BadStatus {
                seq: 4,
                opcode: 0x777,
                got: 7,
                expected: 1,
                ..
            }
        ),
        "{err}"
    );
    // control steps 0-3 and requests seq 1-4 were sent, nothing after the failing request
    let bulk = chip
        .sent
        .iter()
        .filter(|s| matches!(s, Sent::BulkOut { .. }))
        .count();
    assert_eq!(bulk, 4);
    assert_eq!(chip.sent.len(), 4 + 4);
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn a_silent_chip_gives_no_response_error_without_retry() {
    let plan = fabricated_complete_plan();
    let mut chip = FakeChip::new();
    chip.silent.push(0x76c);
    let opts = ReplayOptions {
        timeout: Duration::from_millis(120),
        ..quick()
    };
    let t = std::time::Instant::now();
    let err = run_plan(&mut chip, &plan, &opts, &mut in_decoder(), |_| {}).unwrap_err();
    assert!(
        matches!(
            err,
            ReplayError::NoResponse {
                seq: 3,
                opcode: 0x76c,
                ..
            }
        ),
        "{err}"
    );
    assert!(t.elapsed() < Duration::from_secs(2));
    let sends_of_76c = chip
        .sent
        .iter()
        .filter(|s| matches!(s, Sent::BulkOut { data, .. } if data.len() > 15 && data[15] == 3 && data[8..12] == [0, 0, 0, 0x51]))
        .count();
    assert_eq!(
        sends_of_76c, 1,
        "the request must be sent once, never retried"
    );
}

/// Transport failing at the k-th operation.
struct Flaky {
    inner: FakeChip,
    left: usize,
}

impl Transport for Flaky {
    fn control(&mut self, s: [u8; 8], o: &[u8], t: Duration) -> io::Result<Vec<u8>> {
        self.tick()?;
        self.inner.control(s, o, t)
    }
    fn bulk_out(&mut self, ep: u8, d: &[u8], t: Duration) -> io::Result<()> {
        self.tick()?;
        self.inner.bulk_out(ep, d, t)
    }
    fn bulk_in(&mut self, ep: u8, m: usize, t: Duration) -> io::Result<Option<Vec<u8>>> {
        self.inner.bulk_in(ep, m, t)
    }
}

impl Flaky {
    fn tick(&mut self) -> io::Result<()> {
        if self.left == 0 {
            return Err(io::Error::from_raw_os_error(libc_epipe()));
        }
        self.left -= 1;
        Ok(())
    }
}

fn libc_epipe() -> i32 {
    32
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn a_usb_error_stops_the_replay_and_names_the_step() {
    let plan = fabricated_complete_plan();
    let mut chip = Flaky {
        inner: FakeChip::new(),
        left: 6,
    };
    let err = run_plan(&mut chip, &plan, &quick(), &mut in_decoder(), |_| {}).unwrap_err();
    match err {
        ReplayError::Io { step, source, .. } => {
            assert_eq!(
                step, 6,
                "four control steps, then requests 1 and 2 passed, request 3 failed"
            );
            assert_eq!(source.raw_os_error(), Some(32));
        }
        e => panic!("{e}"),
    }
    assert_eq!(chip.inner.sent.len(), 6);
}

#[test]
fn attaching_to_a_running_stream_needs_no_traffic_to_the_device() {
    let mut chip = FakeChip::new();
    chip.streaming = true;
    let s = read_stream(
        &mut chip,
        &mut in_decoder(),
        Duration::from_millis(100),
        |_, _| {},
    )
    .unwrap();
    assert!(s.samples >= 8);
    assert!(chip.sent.is_empty());
}

#[test]
fn a_chip_that_never_streams_is_reported_as_such() {
    let mut chip = FakeChip::new();
    let s = read_stream(
        &mut chip,
        &mut in_decoder(),
        Duration::from_millis(60),
        |_, _| {},
    )
    .unwrap();
    assert_eq!(s.samples, 0);
    assert!(s.timeouts > 0 && s.first_sample_after.is_none() && s.host_rate_hz().is_none());
}
