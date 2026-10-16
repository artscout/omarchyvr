//! Controller vibration rate limiter (`58 09 07 SS AMP DUR`, [`pimax_proto::haptic_command`]).
//!
//! Games stream an amplitude envelope: a new value every few ms per hand. The headset forwards each command over the
//! controller radio, and on Windows a new one goes out every 8-10 ms with an 11-12 ms duration (capture 2026-10-15,
//! take 15). The limiter keeps at most one command per [`MIN_GAP`] per hand: a request that comes too soon replaces
//! the waiting one (newest wins) and goes out as soon as the gap has passed. Pure logic, the session thread owns the
//! clock and the writes.
use std::time::{Duration, Instant};

/// Smallest spacing of two commands for the same hand.
pub const MIN_GAP: Duration = Duration::from_millis(8);

#[derive(Debug, Default, Clone, Copy)]
struct Slot {
    last_sent: Option<Instant>,
    pending: Option<(u8, u16)>,
}

/// Per-hand limiter for slots 2 (left) and 3 (right); other slots are ignored.
#[derive(Debug, Default, Clone)]
pub struct Limiter {
    slots: [Slot; 2],
    /// Requests replaced by a newer one before they went out.
    pub replaced: u64,
    /// Commands handed out by [`Limiter::take_due`].
    pub sent: u64,
}

fn idx(slot: u8) -> Option<usize> {
    match slot {
        pimax_proto::SLOT_LEFT => Some(0),
        pimax_proto::SLOT_RIGHT => Some(1),
        _ => None,
    }
}

impl Limiter {
    /// Queue a pulse (`amp` percent, `ms` duration) for `slot`; `None` (stop) drops a waiting one. Returns false for an
    /// unknown slot.
    pub fn offer(&mut self, slot: u8, pulse: Option<(u8, u16)>) -> bool {
        let Some(i) = idx(slot) else { return false };
        let s = &mut self.slots[i];
        if s.pending.is_some() && pulse.is_some() {
            self.replaced += 1;
        }
        s.pending = pulse;
        true
    }

    /// The commands that may go out at `now` as `(slot, amp, ms)`; marks them sent.
    pub fn take_due(&mut self, now: Instant) -> Vec<(u8, u8, u16)> {
        let mut out = Vec::new();
        for (i, slot) in [pimax_proto::SLOT_LEFT, pimax_proto::SLOT_RIGHT]
            .into_iter()
            .enumerate()
        {
            let s = &mut self.slots[i];
            let ready = s
                .last_sent
                .is_none_or(|t| now.saturating_duration_since(t) >= MIN_GAP);
            if let (true, Some((amp, ms))) = (ready, s.pending) {
                s.pending = None;
                s.last_sent = Some(now);
                self.sent += 1;
                out.push((slot, amp, ms));
            }
        }
        out
    }

    /// When the next waiting command may go out (`None` = nothing waits).
    pub fn next_due(&self) -> Option<Instant> {
        self.slots
            .iter()
            .filter(|s| s.pending.is_some())
            .map(|s| s.last_sent.map_or_else(Instant::now, |t| t + MIN_GAP))
            .min()
    }

    /// Forget everything (link closed).
    pub fn clear(&mut self) {
        self.slots = Default::default();
    }
}

/// File name (in `$XDG_RUNTIME_DIR`) of a test pulse request for the running VR session: `omarchyvr controllers buzz`
/// and the app's "Test vibration" button write `SLOT AMP MS\n` (atomically, by rename); the Monado Pimax driver
/// polls it every 100 ms, removes it and sends the pulse on its own session (the one that owns the headset HID).
pub const REQUEST_FILE: &str = "pimax-haptic-request";

/// `$XDG_RUNTIME_DIR/pimax-haptic-request` (`/tmp` when the variable is missing).
pub fn request_path() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap_or_else(|| "/tmp".into()))
        .join(REQUEST_FILE)
}

/// The request line for `slot`, `amp` percent, `ms` milliseconds.
pub fn format_request(slot: u8, amp: u8, ms: u16) -> String {
    format!("{slot} {amp} {ms}\n")
}

/// Parse a request line: slot 2|3, amp 1..=100, ms 1..=65535.
pub fn parse_request(s: &str) -> Option<(u8, u8, u16)> {
    let mut it = s.split_whitespace();
    let slot: u8 = it.next()?.parse().ok()?;
    let amp: u8 = it.next()?.parse().ok()?;
    let ms: u16 = it.next()?.parse().ok()?;
    (idx(slot).is_some() && (1..=100).contains(&amp) && ms >= 1 && it.next().is_none())
        .then_some((slot, amp, ms))
}

/// Take (read and remove) a waiting request; `Ok(None)` when there is none, `Err` for a malformed one (removed too).
pub fn take_request(path: &std::path::Path) -> Result<Option<(u8, u8, u16)>, String> {
    let s = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let _ = std::fs::remove_file(path);
    parse_request(&s)
        .map(Some)
        .ok_or_else(|| format!("bad haptic request {:?}", s.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_pulse_goes_out_at_once() {
        let mut l = Limiter::default();
        let t = Instant::now();
        assert!(l.offer(3, Some((25, 300))));
        assert_eq!(l.take_due(t), vec![(3, 25, 300)]);
        assert_eq!(l.take_due(t), vec![], "sent once");
        assert_eq!(l.next_due(), None);
    }

    #[test]
    fn at_most_one_per_8_ms_per_hand_newest_wins() {
        let mut l = Limiter::default();
        let t0 = Instant::now();
        l.offer(2, Some((99, 12)));
        assert_eq!(l.take_due(t0), vec![(2, 99, 12)]);
        // three more within 3 ms: only the newest survives, and only after the 8 ms gap
        l.offer(2, Some((97, 12)));
        l.offer(2, Some((94, 12)));
        l.offer(2, Some((92, 11)));
        assert_eq!(l.replaced, 2);
        assert_eq!(l.take_due(t0 + Duration::from_millis(3)), vec![]);
        assert_eq!(l.take_due(t0 + Duration::from_millis(7)), vec![]);
        assert_eq!(l.next_due(), Some(t0 + MIN_GAP));
        assert_eq!(l.take_due(t0 + MIN_GAP), vec![(2, 92, 11)]);
        assert_eq!(l.sent, 2);
    }

    #[test]
    fn hands_are_independent() {
        let mut l = Limiter::default();
        let t0 = Instant::now();
        l.offer(2, Some((50, 12)));
        assert_eq!(l.take_due(t0), vec![(2, 50, 12)]);
        l.offer(3, Some((60, 12)));
        assert_eq!(
            l.take_due(t0 + Duration::from_millis(1)),
            vec![(3, 60, 12)],
            "right is not held by the left"
        );
        l.offer(2, Some((51, 12)));
        l.offer(3, Some((61, 12)));
        assert_eq!(l.take_due(t0 + Duration::from_millis(8)), vec![(2, 51, 12)]);
        assert_eq!(l.take_due(t0 + Duration::from_millis(9)), vec![(3, 61, 12)]);
    }

    #[test]
    fn stop_drops_a_waiting_pulse_and_unknown_slots_are_refused() {
        let mut l = Limiter::default();
        let t0 = Instant::now();
        l.offer(3, Some((40, 12)));
        l.take_due(t0);
        l.offer(3, Some((41, 12)));
        l.offer(3, None);
        assert_eq!(l.take_due(t0 + Duration::from_millis(20)), vec![]);
        assert!(!l.offer(0, Some((1, 1))) && !l.offer(4, Some((1, 1))));
        l.offer(2, Some((10, 10)));
        l.clear();
        assert_eq!(l.take_due(t0), vec![]);
    }

    #[test]
    fn game_stream_every_2_ms_comes_out_every_8_ms() {
        // a runtime that calls every 2 ms for 100 ms: 13 commands (t = 0, 8, ..., 96), each the newest value
        let mut l = Limiter::default();
        let t0 = Instant::now();
        let mut out = Vec::new();
        for k in 0..50u64 {
            let t = t0 + Duration::from_millis(2 * k);
            l.offer(3, Some((100 - k as u8, 12)));
            out.extend(l.take_due(t).into_iter().map(|c| (2 * k, c.1)));
        }
        assert_eq!(out.len(), 13);
        assert!(out.windows(2).all(|w| w[1].0 - w[0].0 >= 8));
        assert_eq!(
            out[1],
            (8, 96),
            "the newest amplitude at the time it goes out"
        );
    }

    #[test]
    fn request_round_trip_and_take() {
        assert_eq!(format_request(3, 25, 300), "3 25 300\n");
        assert_eq!(parse_request("3 25 300\n"), Some((3, 25, 300)));
        assert_eq!(parse_request("2 100 65535"), Some((2, 100, 65535)));
        for bad in [
            "",
            "3 25",
            "1 25 300",
            "3 0 300",
            "3 101 300",
            "3 25 0",
            "3 25 300 x",
            "a b c",
        ] {
            assert_eq!(parse_request(bad), None, "{bad:?}");
        }
        let d = std::env::temp_dir().join(format!("omarchyvr-haptic-test-{}", std::process::id()));
        let _ = std::fs::remove_file(&d);
        assert_eq!(take_request(&d), Ok(None));
        std::fs::write(&d, format_request(2, 40, 200)).unwrap();
        assert_eq!(take_request(&d), Ok(Some((2, 40, 200))));
        assert!(!d.exists(), "consumed");
        std::fs::write(&d, "junk").unwrap();
        assert!(take_request(&d).is_err());
        assert!(!d.exists(), "a bad request is removed too");
    }
}
