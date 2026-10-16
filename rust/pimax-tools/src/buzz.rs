//! `pimax controllers buzz left|right [--amp 25] [--ms 300]`: one test vibration (cmd 0x07, `58 09 07 SS AMP DUR`,
//! Windows capture 2026-10-15; the defaults are Pimax Play's own test pulse: 25 %, 300 ms).
//!
//! * A VR session runs (monado-service, or SteamVR's vrserver with driver_monado): the session owns the headset HID, so the pulse goes through it. The request
//!   `SLOT AMP MS` is written to `$XDG_RUNTIME_DIR/pimax-haptic-request` (by rename); the Monado Pimax driver
//!   (PIMAX_HAPTICS) takes it within 100 ms and sends it rate limited on its session thread.
//! * No VR session: one whole-report write on IF0 (like `controllers unpair`). The controller must be awake and paired.
//!
//! Also used by the app's "Test vibration" button on each controller card.
use pimax::haptics::{format_request, request_path};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

/// Pimax Play's test pulse.
pub const DEFAULT_AMP: u8 = 25;
/// Pimax Play's test pulse.
pub const DEFAULT_MS: u16 = 300;
/// Longest test pulse the CLI accepts.
pub const MAX_MS: u16 = 5000;
/// How long a running session gets to take the request.
pub const TAKE_WAIT: Duration = Duration::from_millis(1000);

/// Slot of `left` / `right`.
pub fn slot_of(side: &str) -> Option<u8> {
    match side {
        "left" => Some(pimax_proto::SLOT_LEFT),
        "right" => Some(pimax_proto::SLOT_RIGHT),
        _ => None,
    }
}

/// True when a VR session owns the headset HID: `monado-service`, or SteamVR's `vrserver` (driver_monado runs the same
/// Pimax driver inside it, and it takes the request the same way).
pub fn session_running(proc_root: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(proc_root) else { return false };
    rd.flatten().any(|e| {
        e.file_name().to_str().is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()))
            && std::fs::read_to_string(e.path().join("comm")).is_ok_and(|c| matches!(c.trim(), "monado-service" | "vrserver"))
    })
}

/// Hand a request to the running session and wait until it is taken. Err (request removed) when nobody takes it.
pub fn request_through_session(path: &Path, slot: u8, amp: u8, ms: u16, wait: Duration) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, format_request(slot, amp, ms)).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    let t0 = Instant::now();
    while t0.elapsed() < wait {
        if !path.exists() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = std::fs::remove_file(path);
    Err("the running VR session did not take the request within 1 s: its Monado build has no vibration support \
         (PIMAX_HAPTICS). Install the new build, or stop VR and try again."
        .into())
}

/// One whole-report write of the vibration command on the IF0 node (no session needed).
pub fn write_direct(node: &Path, slot: u8, amp: u8, ms: u16) -> io::Result<()> {
    let buf = pimax_proto::to_write_buf(&pimax_proto::haptic_command(slot, amp, ms));
    let mut f = std::fs::OpenOptions::new().write(true).custom_flags(libc::O_CLOEXEC).open(node)?;
    let n = f.write(&buf)?;
    if n != buf.len() {
        return Err(io::Error::new(io::ErrorKind::WriteZero, format!("short hidraw write: {n} of {}", buf.len())));
    }
    Ok(())
}

/// Check the arguments: `(slot, amp, ms)`.
pub fn validate(side: &str, amp: u32, ms: u32) -> Result<(u8, u8, u16), String> {
    let slot = slot_of(side).ok_or("buzz left|right [--amp 1-100] [--ms 1-5000]")?;
    if !(1..=100).contains(&amp) {
        return Err(format!("--amp must be 1-100 (percent), not {amp}"));
    }
    if !(1..=MAX_MS as u32).contains(&ms) {
        return Err(format!("--ms must be 1-{MAX_MS}, not {ms}"));
    }
    Ok((slot, amp as u8, ms as u16))
}

/// The whole command: through the running session if there is one, else directly. Ok(plain-words result).
pub fn buzz_side(side: &str, amp: u32, ms: u32) -> Result<String, String> {
    let (slot, amp, ms) = validate(side, amp, ms)?;
    if session_running(Path::new("/proc")) {
        request_through_session(&request_path(), slot, amp, ms, TAKE_WAIT)?;
        return Ok(format!("The {side} controller should vibrate now ({amp} %, {ms} ms, sent through the running VR session)."));
    }
    let [cmd, ..] = pimax::discovery::find_interfaces(&pimax::discovery::Roots::from_env());
    let node = cmd.ok_or("headset not found (IF0)")?;
    write_direct(&node, slot, amp, ms).map_err(|e| format!("send failed: {e}"))?;
    Ok(format!(
        "Sent a {amp} %, {ms} ms pulse to the {side} controller. If it did not vibrate, it is asleep or not paired: press a \
         button on it and try again."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments() {
        assert_eq!(validate("right", 25, 300), Ok((3, 25, 300)));
        assert_eq!(validate("left", 100, 5000), Ok((2, 100, 5000)));
        assert!(validate("both", 25, 300).is_err());
        assert!(validate("left", 0, 300).is_err() && validate("left", 101, 300).is_err());
        assert!(validate("left", 25, 0).is_err() && validate("left", 25, 5001).is_err());
    }

    #[test]
    fn session_detection_reads_comm() {
        let d = std::env::temp_dir().join(format!("pimax-buzz-proc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("12")).unwrap();
        std::fs::write(d.join("12/comm"), "wayvr\n").unwrap();
        assert!(!session_running(&d));
        std::fs::create_dir_all(d.join("345")).unwrap();
        std::fs::write(d.join("345/comm"), "vrserver\n").unwrap();
        assert!(session_running(&d), "SteamVR runs the Pimax driver in-process");
        std::fs::write(d.join("345/comm"), "monado-service\n").unwrap();
        assert!(session_running(&d));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn request_is_taken_by_the_session_or_removed() {
        let p = std::env::temp_dir().join(format!("pimax-buzz-req-{}", std::process::id()));
        let _ = std::fs::remove_file(&p);
        // nobody takes it: error, and no stale request is left behind
        assert!(request_through_session(&p, 3, 25, 300, Duration::from_millis(60)).is_err());
        assert!(!p.exists());
        // a "driver" takes it (the same parser the Monado driver uses through pimax_haptic_poll_request)
        let q = p.clone();
        let t = std::thread::spawn(move || {
            for _ in 0..100 {
                if let Ok(Some(r)) = pimax::haptics::take_request(&q) {
                    return Some(r);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            None
        });
        assert_eq!(request_through_session(&p, 2, 40, 200, Duration::from_millis(1000)), Ok(()));
        assert_eq!(t.join().unwrap(), Some((2, 40, 200)));
    }

    #[test]
    fn direct_write_is_the_captured_report_with_report_id_0() {
        let p = std::env::temp_dir().join(format!("pimax-buzz-node-{}", std::process::id()));
        std::fs::write(&p, b"").unwrap();
        write_direct(&p, 3, 25, 300).unwrap();
        let b = std::fs::read(&p).unwrap();
        assert_eq!(b.len(), 65);
        assert_eq!(&b[..8], &[0x00, 0x58, 0x09, 0x07, 0x03, 0x19, 0x2c, 0x01]);
        let _ = std::fs::remove_file(&p);
    }
}
