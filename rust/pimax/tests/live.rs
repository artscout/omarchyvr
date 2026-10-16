//! Live tests against the real headset. `#[ignore]` by default; run explicitly:
//!     cargo test -p pimax --test live -- --ignored --test-threads=1 --nocapture
//! They run IN PARALLEL with the omarchyvr-keepalived service and are strictly read-only: passive sessions (no wake, no keepalive),
//! identity reads 0x16/0x03/0x50, calibration 0x0a reads and the controller attach burst. No video-prepare, IPD,
//! brightness, eye power or pairing is ever sent. `PIMAX_LIVE_NO_CONTROLLERS=1` skips the attach burst.
mod common;
use pimax::{discovery, Config, Event, Session};
use pimax_proto::*;
use std::time::{Duration, Instant};

const S: fn(u64) -> Duration = Duration::from_secs;

fn service_active() -> bool {
    std::process::Command::new("systemctl")
        .args(["--user", "is-active", "omarchyvr-keepalived"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "active")
        .unwrap_or(false)
}

fn passive() -> (Session, std::sync::mpsc::Receiver<Event>) {
    let (s, rx) = Session::open_channel(Config::passive(), 200_000, true).expect("open");
    s.wait_connected(S(5))
        .expect("headset connected (is it plugged in and is omarchyvr-keepalived running?)");
    (s, rx)
}

#[test]
#[ignore]
fn live_00_interfaces_present() {
    let f = discovery::find_interfaces(&discovery::Roots::default());
    println!(
        "interfaces: {f:?} bridge: {:?} usb devnum {:?}",
        discovery::find_bridge_log(&discovery::Roots::default()),
        discovery::usb_devnum()
    );
    assert_eq!(discovery::mask(&f), 0b1111, "all four interfaces");
}

#[test]
#[ignore]
fn live_identity_reads() {
    let (s, _rx) = passive();
    let serial = s.read_serial(0, S(1)).expect("serial");
    println!("serial {serial}");
    assert!(serial.len() >= 8 && serial.is_ascii(), "{serial:?}");
    let ver = s.read_version(0, S(1)).expect("version");
    println!("version {ver:02x?}");
    assert!(!ver.is_empty());
    let info = s.request(cmd::INFO, 0, &[], S(1)).expect("0x50");
    println!("info50 {info:02x?}");
    assert!(!info.is_empty());
}

#[test]
#[ignore]
fn live_calibration_equals_windows_capture() {
    let (s, _rx) = passive();
    let t = Instant::now();
    let blob = s
        .read_calibration_blob(Duration::from_millis(1000))
        .expect("84 block reads");
    println!("calibration read in {:?}", t.elapsed());
    let xml = calib::inflate_blob(&blob).expect("inflate");
    let want = std::fs::read(common::fixtures_dir().join("../../analysis/headset-calibration.xml"))
        .unwrap();
    assert_eq!(
        xml, want,
        "device calibration == Windows capture (analysis/imu-orientation.md section 1)"
    );
}

#[test]
#[ignore]
fn live_imu_stream_is_sane() {
    let (s, rx) = passive();
    let t0 = Instant::now();
    let mut reports = 0u64;
    let (mut first, mut last) = (None, 0u32);
    let mut sum = 0f64;
    let mut lat_prev = Instant::now();
    let mut gaps = vec![];
    while t0.elapsed() < S(3) {
        if let Ok(Event::Imu(i)) = rx.recv_timeout(Duration::from_millis(100)) {
            if i.sample_in_report == 0 {
                reports += 1;
                first.get_or_insert(i.ts_us);
                last = i.ts_us;
                let a = i.accel_g;
                sum += f64::from(a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
                let now = Instant::now();
                gaps.push(now.duration_since(lat_prev).as_secs_f64() * 1e3);
                lat_prev = now;
            }
        }
    }
    let rate = reports as f64 / 3.0;
    let span = f64::from(last.wrapping_sub(first.unwrap())) / 1e6;
    gaps.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!("IMU {rate:.0} reports/s, device span {span:.2} s, |a| mean {:.3} g, inter-arrival p50 {:.2} ms p99 {:.2} ms max {:.2} ms", sum / reports as f64, gaps[gaps.len() / 2], gaps[gaps.len() * 99 / 100], gaps[gaps.len() - 1]);
    assert!(
        (450.0..550.0).contains(&rate),
        "about 500 reports/s, got {rate}"
    );
    assert!((span - 3.0).abs() < 0.2, "device clock tracks wall clock");
    let mean_g = sum / reports as f64;
    assert!(
        (0.8..1.2).contains(&mean_g),
        "gravity about 1 g (headset held still or moving gently)"
    );
    assert!(s.stats().imu_reports >= reports);
}

#[test]
#[ignore]
fn live_controller_attach_burst_and_reports() {
    if std::env::var_os("PIMAX_LIVE_NO_CONTROLLERS").is_some() {
        return;
    }
    // passive session + the same attach logic the python tool has (initial burst, re-attach on 0x37)
    let (s, rx) = Session::open_channel(
        Config {
            wake: false,
            keepalive: false,
            auto_attach: true,
            ..Config::default()
        },
        200_000,
        true,
    )
    .unwrap();
    s.wait_connected(S(5)).unwrap();
    let t0 = Instant::now();
    let (mut n, mut replies, mut status) = ([0u64; 4], std::collections::BTreeSet::new(), 0);
    let mut last = [None; 4];
    while t0.elapsed() < S(5) {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Event::Controller(c)) => {
                n[c.slot as usize] += 1;
                assert!(
                    c.battery <= 100 || c.battery == 127,
                    "battery {}",
                    c.battery
                );
                last[c.slot as usize] = Some(c);
            }
            Ok(Event::Reply(r)) => {
                if let Some(p) = parse_reply(&r) {
                    if matches!(p.cmd, 0x16 | 0x50 | 0x0a | 0x03) && (p.opt == 2 || p.opt == 3) {
                        replies.insert((p.opt, p.cmd));
                    }
                }
            }
            Ok(Event::Status(_)) => status += 1,
            _ => {}
        }
    }
    println!("controller reports L={} R={} (about 250 Hz each when awake), 0x37 messages {status}, attach replies {replies:?}", n[2], n[3]);
    for sl in [2usize, 3] {
        if let Some(c) = last[sl] {
            println!(
                "slot {sl}: battery {}% charging {} stick ({},{}) trig {} grip {} buttons {:#x}",
                c.battery, c.charging, c.stick_x, c.stick_y, c.trigger, c.grip, c.buttons
            );
            assert!(
                n[sl] > 700,
                "slot {sl} streams at about 250 Hz once attached, got {}",
                n[sl]
            );
        } else {
            println!("slot {sl}: controller asleep or off (no reports): not a failure");
        }
    }
}

#[test]
#[ignore]
fn live_firmware_log_passive() {
    let (s, rx) = passive();
    let t0 = Instant::now();
    let mut lines = vec![];
    while t0.elapsed() < S(2) {
        if let Ok(Event::Log(l)) = rx.recv_timeout(Duration::from_millis(100)) {
            lines.push(l);
        }
    }
    println!(
        "firmware log lines in 2 s: {} {:?}",
        lines.len(),
        lines.iter().take(3).collect::<Vec<_>>()
    );
    assert!(lines.iter().all(|l| !l.contains('\n') && l.len() < 512));
    drop(s);
}

/// Last test (alphabetical): everything above ran next to the service without disturbing it.
#[test]
#[ignore]
fn live_zz_service_unharmed() {
    let before = discovery::usb_devnum();
    let (s, _rx) = passive();
    std::thread::sleep(S(7)); // longer than the firmware's 5.4 s no-keepalive window: the service alone keeps it alive
    drop(s);
    assert!(service_active(), "omarchyvr-keepalived still active");
    assert_eq!(
        discovery::usb_devnum(),
        before,
        "headset did not re-enumerate"
    );
    let (s2, _rx2) = passive();
    assert!(s2.read_serial(0, S(1)).is_ok());
}
