//! Controller pairing (approved by the user on 2026-10-14: the StartPair command only).
//!
//! Windows flow (analysis/auto-mode.md section 3): host sends `58 04 04 00 SS` (SS = 2 left, 3 right; opt 0, EP 0x01;
//! captures/20261013-124629-controllers-hub2.pcap frames 59024, 98342, 182552), ack `4c 04 04 00 01 07`. The firmware
//! log (IF3) shows `eSpiCmdStartPair` / `Pairing device = N`, the user holds the controller's pairing buttons, then
//! `pair finished` + `radio_pair_success_process` (success) or `radio_pair_cancel_or_timeout_process` (~30 s).
//!
//! Send path: the same as the IPD motor command (`pimax-eye ipd --apply`, rust/pimax-eye/src/ipd.rs `send_motor`): one
//! write-only whole-report write on hidraw IF0. Monado's pimax driver keeps IF0 open for its keepalive; hidraw allows
//! several openers and the writes coexist, so this never takes the device from a running session (and works without
//! one). The broker does not own command writes. The firmware log is read from IF3 (every opener gets its own queue).
//! Opened BEFORE the send, so the outcome cannot be missed. If IF3 is not readable nothing is sent.
//!
//! This module can build exactly one opcode, 0x04 (StartPair). `58 03 19 00` (DeviceCheckResult) is NOT approved and
//! is never built or sent here. One send per user request, never retried.
//!
//! Attach after success: Monado's pimax_ctrl.c attach thread sends the burst (`58 04 16 SS 1e`, `58 03 50 SS`,
//! `58 07 0a SS ..`, `58 03 03 SS`) at start/reconnect, and again when a controller's 0x37 heartbeat shows it awake
//! without reports for > 1.5 s (at most every 2.5 s). A controller paired during a running session is therefore
//! attached by the driver within about 2 s; no extra command is needed from here.
use std::fs::File;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Pairing UI states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairState {
    Idle,
    /// StartPair sent for this slot; the user should hold the controller's pairing combo.
    Waiting { slot: u8, since_s: u32 },
    Paired { slot: u8 },
    Failed { slot: u8 },
}

/// Firmware log verdicts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogEvent {
    Started,
    Success,
    TimeoutOrCancel,
}

/// Classify one firmware log line (IF3 / bridge log text).
pub fn parse_log(line: &str) -> Option<LogEvent> {
    if line.contains("radio_pair_success_process") || line.contains("pair finished") {
        Some(LogEvent::Success)
    } else if line.contains("radio_pair_cancel_or_timeout_process") {
        Some(LogEvent::TimeoutOrCancel)
    } else if line.contains("eSpiCmdStartPair") || line.contains("Pairing device") {
        Some(LogEvent::Started)
    } else {
        None
    }
}

/// Seconds after which the UI gives up (firmware times out at ~30 s).
pub const TIMEOUT_S: u32 = 35;

/// Advance the UI state by a log event or one elapsed second (`ev = None`).
pub fn step(s: PairState, ev: Option<LogEvent>) -> PairState {
    match (s, ev) {
        (PairState::Waiting { slot, .. }, Some(LogEvent::Success)) => PairState::Paired { slot },
        (PairState::Waiting { slot, .. }, Some(LogEvent::TimeoutOrCancel)) => PairState::Failed { slot },
        (PairState::Waiting { slot, since_s }, None) if since_s + 1 >= TIMEOUT_S => PairState::Failed { slot },
        (PairState::Waiting { slot, since_s }, None) => PairState::Waiting { slot, since_s: since_s + 1 },
        (s, _) => s,
    }
}

/// The slot for a side name.
pub fn slot_of(side: &str) -> Option<u8> {
    match side {
        "left" => Some(2),
        "right" => Some(3),
        _ => None,
    }
}

/// Side name of a slot.
pub fn side_of(slot: u8) -> &'static str {
    if slot == 2 { "left" } else { "right" }
}

/// The StartPair report `58 04 04 00 SS`. The only command this module can produce.
pub fn start_pair_report(slot: u8) -> Result<pimax_proto::Report, String> {
    if !matches!(slot, 2 | 3) {
        return Err("slot must be 2 (left) or 3 (right)".into());
    }
    pimax_proto::build_command(0x04, 0, &[slot]).map_err(|e| format!("{e:?}"))
}

/// What the user should do (the pairing button combo is not in our captures).
pub const INSTRUCTION: &str = "Press and hold the controller's pairing buttons (see Pimax manual) within 30 s";

/// Final result of one pairing attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Paired,
    /// No success within the window (our 35 s timer, or the firmware's own timeout/cancel).
    Timeout,
    /// Could not send, or no way to observe the result (nothing was sent in the latter case).
    Failed(String),
}

/// The side effects of a pairing run (fake in tests).
pub trait PairIo {
    /// Make the firmware log readable. Called before the send; an error means nothing is sent.
    fn open_log(&mut self) -> Result<(), String>;
    /// Send StartPair for `slot`, exactly once.
    fn send(&mut self, slot: u8) -> Result<(), String>;
    /// Wait up to `dt` for firmware log lines; the verdicts found.
    fn poll(&mut self, dt: Duration) -> Vec<LogEvent>;
}

/// One pairing attempt: opens the log, sends once, then waits (one tick per `poll` of 1 s) for the firmware verdict
/// or the [`TIMEOUT_S`] timer. `emit` gets every state (`Waiting` at the start and about every 5 s). Never retries.
pub fn run(slot: u8, io: &mut dyn PairIo, emit: &mut dyn FnMut(PairState)) -> Outcome {
    if let Err(e) = start_pair_report(slot) {
        return Outcome::Failed(e);
    }
    if let Err(e) = io.open_log() {
        return Outcome::Failed(format!("firmware log (IF3) not readable, nothing sent: {e}"));
    }
    if let Err(e) = io.send(slot) {
        return Outcome::Failed(format!("send failed: {e}"));
    }
    let mut s = PairState::Waiting { slot, since_s: 0 };
    emit(s);
    loop {
        let evs = io.poll(Duration::from_secs(1));
        let mut ev = None;
        for e in evs {
            if matches!(e, LogEvent::Success | LogEvent::TimeoutOrCancel) {
                ev = Some(e);
                break;
            }
        }
        s = step(s, ev);
        match s {
            PairState::Paired { .. } => {
                emit(s);
                return Outcome::Paired;
            }
            PairState::Failed { .. } => {
                emit(s);
                return Outcome::Timeout;
            }
            PairState::Waiting { since_s, .. } if since_s % 5 == 0 => emit(s),
            _ => {}
        }
    }
}

/// A VR session that owns the headset (monado-service or vrserver) runs: its name. Pair and unpair refuse then.
pub fn vr_session_owner() -> Option<&'static str> {
    let rd = std::fs::read_dir("/proc").ok()?;
    for e in rd.flatten() {
        if !e.file_name().to_string_lossy().bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        if let Ok(c) = std::fs::read_to_string(e.path().join("comm")) {
            match c.trim() {
                "monado-service" => return Some("Monado"),
                "vrserver" => return Some("SteamVR"),
                _ => {}
            }
        }
    }
    None
}

/// Plain-words refusal while a VR session runs.
pub fn session_refusal(who: &str, what: &str) -> String {
    format!("{who} is running and owns the headset: stop VR first, then {what}.")
}

static BUSY: AtomicBool = AtomicBool::new(false);

/// Held while a pairing attempt runs in this process (one at a time).
pub struct Guard(());

impl Guard {
    pub fn acquire() -> Option<Guard> {
        (!BUSY.swap(true, Ordering::SeqCst)).then_some(Guard(()))
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::SeqCst);
    }
}

/// The real device access: IF0 write-only for the send, IF3 for the log.
pub struct HidIo {
    cmd: Option<std::path::PathBuf>,
    log: Option<File>,
    split: pimax_proto::LineSplitter,
}

impl HidIo {
    pub fn new() -> Self {
        let ifs = pimax::discovery::find_interfaces(&pimax::discovery::Roots::from_env());
        let [cmd, _, _, log] = ifs;
        let log = log.and_then(|p| pimax::hid::open(&p, false).ok());
        Self { cmd, log, split: pimax_proto::LineSplitter::new() }
    }
}

impl Default for HidIo {
    fn default() -> Self {
        Self::new()
    }
}

/// One whole-report write on the command node, like the IPD motor write. Not retried.
pub fn write_start_pair(node: &Path, slot: u8) -> io::Result<()> {
    let r = start_pair_report(slot).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let buf = pimax_proto::to_write_buf(&r);
    let mut f = std::fs::OpenOptions::new().write(true).custom_flags(libc::O_CLOEXEC).open(node)?;
    let n = f.write(&buf)?;
    if n != buf.len() {
        return Err(io::Error::new(io::ErrorKind::WriteZero, format!("short hidraw write: {n} of {}", buf.len())));
    }
    Ok(())
}

impl PairIo for HidIo {
    fn open_log(&mut self) -> Result<(), String> {
        if self.cmd.is_none() {
            return Err("headset IF0 not found".into());
        }
        if self.log.is_none() {
            return Err("headset IF3 not found or not openable".into());
        }
        Ok(())
    }

    fn send(&mut self, slot: u8) -> Result<(), String> {
        let node = self.cmd.as_ref().ok_or("headset IF0 not found")?;
        write_start_pair(node, slot).map_err(|e| e.to_string())
    }

    fn poll(&mut self, dt: Duration) -> Vec<LogEvent> {
        let mut out = Vec::new();
        let Some(f) = self.log.as_ref() else {
            std::thread::sleep(dt);
            return out;
        };
        match pimax::sys::wait_readable(&[f], Some(dt)) {
            Ok(r) if r.first() == Some(&true) => {}
            Ok(_) => return out,
            Err(_) => {
                std::thread::sleep(dt);
                return out;
            }
        }
        let mut d = [0u8; pimax_proto::REPORT_SIZE];
        while let Ok(Some(n)) = pimax::hid::read_report(f, &mut d) {
            self.split.push(&d[..n], |l| {
                if let Some(e) = parse_log(String::from_utf8_lossy(l).trim()) {
                    out.push(e);
                }
            });
        }
        out
    }
}

/// JSON event line for the control socket / `events` stream.
pub fn event_line(side: &str, state: &str, detail: &str) -> String {
    format!(
        "{{\"event\":\"pair\",\"side\":{},\"state\":{},\"detail\":{}}}",
        crate::ipc::q(side),
        crate::ipc::q(state),
        crate::ipc::q(detail)
    )
}

/// Event for a state of a running attempt.
pub fn state_event(s: PairState) -> Option<String> {
    match s {
        PairState::Waiting { slot, since_s } => Some(event_line(side_of(slot), "waiting", &format!("{since_s}/{TIMEOUT_S} s: {INSTRUCTION}"))),
        PairState::Paired { slot } => Some(event_line(side_of(slot), "paired", "controller paired; the driver attaches it within about 2 s")),
        _ => None,
    }
}

/// Event for the final outcome (`paired` / `timeout` / `failed`).
pub fn outcome_event(slot: u8, o: &Outcome) -> String {
    let side = side_of(slot);
    match o {
        Outcome::Paired => event_line(side, "paired", "controller paired; the driver attaches it within about 2 s"),
        Outcome::Timeout => event_line(side, "timeout", "no pairing within 35 s; nothing is retried"),
        Outcome::Failed(e) => event_line(side, "failed", e),
    }
}

/// Run one attempt with the real headset, calling `out` with every event line. Refuses a second concurrent attempt.
pub fn run_real(slot: u8, out: &mut dyn FnMut(&str)) -> Outcome {
    if let Some(who) = vr_session_owner() {
        let o = Outcome::Failed(session_refusal(who, "pair"));
        out(&outcome_event(slot, &o));
        return o;
    }
    let Some(_g) = Guard::acquire() else {
        let o = Outcome::Failed("a pairing attempt is already running".into());
        out(&outcome_event(slot, &o));
        return o;
    };
    let mut io = HidIo::new();
    let o = run(slot, &mut io, &mut |s| {
        if let (Some(l), PairState::Waiting { .. }) = (state_event(s), s) {
            out(&l)
        }
    });
    out(&outcome_event(slot, &o));
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        sent: Vec<u8>,
        log_ok: bool,
        script: Vec<Vec<LogEvent>>,
        polls: u32,
    }

    impl PairIo for Fake {
        fn open_log(&mut self) -> Result<(), String> {
            if self.log_ok { Ok(()) } else { Err("no IF3".into()) }
        }
        fn send(&mut self, slot: u8) -> Result<(), String> {
            self.sent.push(slot);
            Ok(())
        }
        fn poll(&mut self, _: Duration) -> Vec<LogEvent> {
            self.polls += 1;
            if self.script.is_empty() { vec![] } else { self.script.remove(0) }
        }
    }

    fn fake(script: Vec<Vec<LogEvent>>) -> Fake {
        Fake { sent: vec![], log_ok: true, script, polls: 0 }
    }

    #[test]
    fn exact_bytes_for_left_and_right() {
        let l = start_pair_report(2).unwrap();
        let r = start_pair_report(3).unwrap();
        assert_eq!(&l[..6], &[0x58, 0x04, 0x04, 0x00, 0x02, 0x00]);
        assert_eq!(&r[..6], &[0x58, 0x04, 0x04, 0x00, 0x03, 0x00]);
        assert!(l[5..].iter().all(|b| *b == 0) && r[5..].iter().all(|b| *b == 0));
        let w = pimax_proto::to_write_buf(&l);
        assert_eq!(&w[..6], &[0x00, 0x58, 0x04, 0x04, 0x00, 0x02]);
        assert_eq!((slot_of("left"), slot_of("right"), slot_of("x")), (Some(2), Some(3), None));
    }

    #[test]
    fn no_other_opcode_can_be_produced() {
        for slot in 0..=255u8 {
            match start_pair_report(slot) {
                Ok(r) => {
                    assert!(slot == 2 || slot == 3);
                    assert_eq!((r[0], r[1], r[2], r[3]), (0x58, 0x04, 0x04, 0x00));
                    assert_ne!((r[1], r[2], r[3]), (0x03, 0x19, 0x00)); // DeviceCheckResult
                }
                Err(_) => assert!(slot != 2 && slot != 3),
            }
        }
        // the source builds frames through start_pair_report only
        let src = include_str!("pairing.rs");
        let code = src.split("#[cfg(test)]").next().unwrap();
        assert_eq!(code.matches("build_command(").count(), 1);
        assert!(code.contains("build_command(0x04, 0, &[slot])"));
    }

    #[test]
    fn success_sends_once() {
        let mut io = fake(vec![vec![], vec![LogEvent::Started], vec![LogEvent::Success]]);
        let mut states = vec![];
        assert_eq!(run(3, &mut io, &mut |s| states.push(s)), Outcome::Paired);
        assert_eq!(io.sent, [3]);
        assert_eq!(states.first(), Some(&PairState::Waiting { slot: 3, since_s: 0 }));
        assert_eq!(states.last(), Some(&PairState::Paired { slot: 3 }));
    }

    #[test]
    fn timeout_gives_failed_and_never_retries() {
        let mut io = fake(vec![]);
        let mut last = PairState::Idle;
        assert_eq!(run(2, &mut io, &mut |s| last = s), Outcome::Timeout);
        assert_eq!(last, PairState::Failed { slot: 2 });
        assert_eq!(io.sent, [2]);
        assert_eq!(io.polls, TIMEOUT_S);
        // firmware timeout/cancel is also a failure, still one send
        let mut io = fake(vec![vec![LogEvent::TimeoutOrCancel]]);
        assert_eq!(run(2, &mut io, &mut |_| {}), Outcome::Timeout);
        assert_eq!(io.sent, [2]);
    }

    #[test]
    fn nothing_is_sent_without_a_readable_log_or_with_a_bad_slot() {
        let mut io = fake(vec![]);
        io.log_ok = false;
        assert!(matches!(run(2, &mut io, &mut |_| {}), Outcome::Failed(_)));
        assert!(io.sent.is_empty());
        let mut io = fake(vec![]);
        assert!(matches!(run(1, &mut io, &mut |_| {}), Outcome::Failed(_)));
        assert!(io.sent.is_empty());
    }

    #[test]
    fn event_lines_are_flat_json() {
        let l = outcome_event(2, &Outcome::Timeout);
        let m = crate::ipc::parse_flat(&l).unwrap();
        assert_eq!((m["event"].as_str(), m["side"].as_str(), m["state"].as_str()), ("pair", "left", "timeout"));
        assert_eq!(crate::ipc::parse_flat(&outcome_event(3, &Outcome::Paired)).unwrap()["state"], "paired");
        assert_eq!(crate::ipc::parse_flat(&outcome_event(3, &Outcome::Failed("x".into()))).unwrap()["state"], "failed");
    }

    #[test]
    fn guard_is_exclusive() {
        let g = Guard::acquire().unwrap();
        assert!(Guard::acquire().is_none());
        drop(g);
        assert!(Guard::acquire().is_some());
    }

    #[test]
    fn log_lines_and_ui_flow() {
        assert_eq!(parse_log("[123] radio_evt_cb dev 2 pair finished"), Some(LogEvent::Success));
        assert_eq!(parse_log("radio_pair_cancel_or_timeout_process"), Some(LogEvent::TimeoutOrCancel));
        assert_eq!(parse_log("eSpiCmdStartPair"), Some(LogEvent::Started));
        assert_eq!(parse_log("Panel ON"), None);
        let w = PairState::Waiting { slot: 3, since_s: 0 };
        assert_eq!(step(w, Some(LogEvent::Success)), PairState::Paired { slot: 3 });
        assert_eq!(step(w, Some(LogEvent::TimeoutOrCancel)), PairState::Failed { slot: 3 });
        let mut s = w;
        for _ in 0..TIMEOUT_S {
            s = step(s, None);
        }
        assert_eq!(s, PairState::Failed { slot: 3 });
        assert_eq!(step(PairState::Idle, Some(LogEvent::Success)), PairState::Idle);
    }
}
