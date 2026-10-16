//! One-shot probe of command 0x2c `eSpiCmdGetVisorStatus` (a wear/proximity candidate), for the live test only.
//! The daemon never sends it.
//!
//! The request is the exact one Windows sends: `58 04 2c 00 00` (zero padded to 64 bytes) on IF0 / EP 0x01,
//! reply `4c 04 2c 00 1c 54` on EP 0x81 ~2 ms later. It appears twice in every Windows cold start
//! (captures2/20261013-130457-cold-start-hub2.pcap frames 41980/41984 and 42586/42588,
//! pimax-data/windows-20261014/usb-captures/20261014-043157-cold1-hub2.pcap frames 14567/14573 and 14647/14651,
//! 20261014-043545-cold2-hub2.pcap frames 14185/14191 and 14577/14579) and once after a refresh-rate change
//! (captures2/20261013-130710-settings-hub2.pcap frame 232807). All six replies are `1c 54`: whether it changes with
//! wear is unknown, which is what this probe is for (worn vs on the desk, see analysis/auto-mode-live-checklist.md).
use std::time::{Duration, Instant};

/// The request, as in the captures.
pub fn request() -> pimax_proto::Report {
    pimax_proto::build_command(0x2c, 0, &[0]).expect("fits")
}

/// Reply data bytes of a 0x2c reply packet (the two bytes after `4c 04 2c 00`).
pub fn reply_data(r: &pimax_proto::Report) -> Option<[u8; 2]> {
    (r[0] == 0x4c && r[1] == 0x04 && r[2] == 0x2c && r[3] == 0x00).then(|| [r[4], r[5]])
}

/// Send the request `n` times, `every` apart, print each reply. Needs the headset awake (omarchyvr-keepalived).
pub fn probe(n: u32, every: Duration) -> Result<(), String> {
    let ifs = pimax::discovery::find_interfaces(&pimax::discovery::Roots::from_env());
    let p = ifs[0].as_ref().ok_or("headset IF0 not found")?;
    let f = pimax::hid::open(p, true).map_err(|e| format!("{}: {e}", p.display()))?;
    let req = request();
    for i in 0..n {
        pimax::hid::write_report(&f, &req).map_err(|e| format!("write: {e}"))?;
        let t0 = Instant::now();
        let mut got = None;
        let mut buf: pimax_proto::Report = [0; 64];
        while t0.elapsed() < Duration::from_millis(500) && got.is_none() {
            let _ = pimax::sys::wait_readable(&[&f], Some(Duration::from_millis(50)));
            while let Ok(Some(_)) = pimax::hid::read_report(&f, &mut buf) {
                if let Some(d) = reply_data(&buf) {
                    got = Some(d);
                }
            }
        }
        match got {
            Some(d) => println!("{i:3} {} visor reply {:02x} {:02x}", pimax::sys::local_hms(), d[0], d[1]),
            None => println!("{i:3} {} no reply in 500 ms", pimax::sys::local_hms()),
        }
        std::thread::sleep(every);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_is_byte_identical_to_the_windows_capture() {
        let r = request();
        assert_eq!(&r[..5], &[0x58, 0x04, 0x2c, 0x00, 0x00]);
        assert!(r[5..].iter().all(|b| *b == 0));
    }

    #[test]
    fn reply_parsing() {
        let mut r = [0u8; 64];
        r[..6].copy_from_slice(&[0x4c, 0x04, 0x2c, 0x00, 0x1c, 0x54]);
        assert_eq!(reply_data(&r), Some([0x1c, 0x54]));
        r[2] = 0x2d;
        assert_eq!(reply_data(&r), None);
    }
}
