//! Controller unpair: the one command `58 03 05 SS` (cmd 0x05, opt = slot, no data), from the Windows captures:
//! 2026-10-15 take 13 (Pimax Play "unpair right": `58 03 05 03`, reply `4c 04 05 03 01 07`) and the redo take 3
//! (`20261015-134547-ctrl-pair-unpair-hub2.pcap`: left t=40.871 `58 03 05 02`, reply `4c 04 05 02 01 07`; right t=52.533
//! `58 03 05 03`). The controller stream stops 20 ms later. Slot 2 = left, slot 3 = right. One write per user request,
//! never retried; write-only on IF0, no session needed.
use crate::pairing::slot_of;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// The Unpair report `58 03 05 SS`.
pub fn unpair_report(slot: u8) -> Result<pimax_proto::Report, String> {
    // both hands are captured (redo take 3): 58 03 05 02 (left) and 58 03 05 03 (right)
    if slot != 2 && slot != 3 {
        return Err("slot must be 2 (left) or 3 (right)".into());
    }
    pimax_proto::build_command(0x05, slot, &[]).map_err(|e| format!("{e:?}"))
}

/// One whole-report write on the command node.
pub fn write_unpair(node: &Path, slot: u8) -> io::Result<()> {
    let r = unpair_report(slot).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let buf = pimax_proto::to_write_buf(&r);
    let mut f = std::fs::OpenOptions::new().write(true).custom_flags(libc::O_CLOEXEC).open(node)?;
    let n = f.write(&buf)?;
    if n != buf.len() {
        return Err(io::Error::new(io::ErrorKind::WriteZero, format!("short hidraw write: {n} of {}", buf.len())));
    }
    Ok(())
}

/// `pimax controllers unpair left|right`: refuses while a pairing runs in this process; Ok(plain-words result).
pub fn unpair_side(side: &str) -> Result<String, String> {
    let slot = slot_of(side).ok_or("unpair left|right")?;
    if let Some(who) = crate::pairing::vr_session_owner() {
        return Err(crate::pairing::session_refusal(who, "unpair"));
    }
    let Some(_g) = crate::pairing::Guard::acquire() else {
        return Err("a pairing attempt is running; wait until it ends".into());
    };
    let [cmd, ..] = pimax::discovery::find_interfaces(&pimax::discovery::Roots::from_env());
    let node = cmd.ok_or("headset not found (IF0)")?;
    write_unpair(&node, slot).map_err(|e| format!("send failed: {e}"))?;
    Ok(format!("The {side} controller was unpaired. Put it in pairing mode and press Pair to connect it again."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_match_the_windows_capture() {
        // 20261015-124923-track-controllers-hub2.pcap t=4.36: 58 03 05 03 00 ...
        let r = unpair_report(3).unwrap();
        assert_eq!(&r[..6], &[0x58, 0x03, 0x05, 0x03, 0x00, 0x00]);
        assert!(r[4..].iter().all(|b| *b == 0));
        // 20261015-134547-ctrl-pair-unpair-hub2.pcap t=40.871 (left, reply 4c 04 05 02 01 07) and t=52.533 (right)
        let l = unpair_report(2).unwrap();
        assert_eq!(&l[..6], &[0x58, 0x03, 0x05, 0x02, 0x00, 0x00]);
        assert!(l[4..].iter().all(|b| *b == 0));
        assert_eq!(&pimax_proto::to_write_buf(&l)[..5], &[0x00, 0x58, 0x03, 0x05, 0x02]);
        assert_eq!(&pimax_proto::to_write_buf(&r)[..5], &[0x00, 0x58, 0x03, 0x05, 0x03]);
        assert!(unpair_report(0).is_err() && unpair_report(4).is_err());
        assert_eq!((slot_of("left"), slot_of("right")), (Some(2), Some(3)));
    }

    #[test]
    fn session_refusal_is_plain() {
        let m = crate::pairing::session_refusal("Monado", "unpair");
        assert!(m.contains("Monado") && m.contains("stop VR first"));
    }
}
