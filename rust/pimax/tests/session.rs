mod common;
use common::*;
use pimax::fake::{self, FakeDevice};
use pimax::{Config, Error, Event, RateSource, Session, State};
use pimax_proto::*;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

const MS: fn(u64) -> Duration = Duration::from_millis;

fn start(cfg: Config) -> (Session, Receiver<Event>, FakeDevice) {
    let (be, dev) = fake::pair();
    let (s, rx) = Session::open_channel_with_backend(cfg, be, 100_000, true).unwrap();
    s.wait_connected(MS(2000)).unwrap();
    assert!(dev.wait_host(MS(1000)));
    (s, rx, dev)
}

/// Device-side thread that answers requests like the headset does.
fn responder(
    dev: FakeDevice,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> std::thread::JoinHandle<Vec<Report>> {
    std::thread::spawn(move || {
        let mut seen = vec![];
        while !stop.load(std::sync::atomic::Ordering::Relaxed) {
            let Some(c) = dev.recv_command(MS(20)) else {
                continue;
            };
            seen.push(c);
            if c[0] != 0x58 {
                continue;
            }
            let mut r = [0u8; 64];
            match c[2] {
                0x16 => {
                    // noise first: a 0x37 and an unrelated reply must not satisfy the request
                    r[..16].copy_from_slice(&[
                        0x4c, 0x0f, 0x37, 0x02, 0x48, 1, 2, 3, 4, 0x49, 0, 2, 1, 0, 8, 0x2d,
                    ]);
                    let _ = dev.send_report(0, &r);
                    r = [0u8; 64];
                    r[..4].copy_from_slice(&[0x4c, 0x04, 0x51, 0x00]);
                    let _ = dev.send_report(0, &r);
                    r = [0u8; 64];
                    r[..4].copy_from_slice(&[0x4c, 0x1a, 0x16, c[3]]);
                    r[4] = 5;
                    r[5..10].copy_from_slice(b"H1234");
                    let _ = dev.send_report(0, &r);
                }
                0x03 => {
                    r[..8].copy_from_slice(&[0x4c, 0x07, 0x03, c[3], 0x00, 0x08, 0x2d, 0x6b]);
                    let _ = dev.send_report(0, &r);
                }
                _ => {}
            }
        }
        seen
    })
}

#[test]
fn connect_sends_wake_then_keepalives_every_213ms() {
    let (s, _rx, dev) = start(Config::default());
    let t0 = Instant::now();
    let mut cmds = vec![];
    while t0.elapsed() < MS(1100) {
        if let Some(c) = dev.recv_command(MS(50)) {
            cmds.push((t0.elapsed(), c));
        }
    }
    assert_eq!(cmds[0].1, wake(), "first command is the wake");
    let ka: Vec<_> = cmds.iter().filter(|(_, c)| c[2] == 0x39).collect();
    assert!(
        ka.len() >= 5 && ka.len() <= 6,
        "keepalives in 1.1 s: {}",
        ka.len()
    );
    for (_, c) in &ka {
        assert_eq!(*c, keepalive(0));
        assert_eq!(c[1], 4, "Windows / python wire form LL=4");
    }
    for w in ka.windows(2) {
        let d = (w[1].0 - w[0].0).as_millis() as i64;
        assert!((d - 213).abs() < 40, "keepalive spacing {d} ms");
    }
    assert!(s.stats().keepalives >= 5);
}

#[test]
fn rate_byte_follows_handle_and_file() {
    let (s, _rx, dev) = start(Config::default());
    s.set_video_rate(72);
    let seen = dev.drain_commands(MS(600));
    assert!(
        seen.iter().any(|c| *c == keepalive(72)),
        "rate 72 reaches the keepalive"
    );
    drop(s);

    // file source (pimax_keepalived.py): re-read on every keepalive, missing = 0, garbage = 0
    let dir = tmpdir("ratefile");
    let f = dir.join("pimax-rate");
    let (s, _rx, dev) = start(Config {
        rate: RateSource::File(f.clone()),
        ..Config::default()
    });
    let a = dev.drain_commands(MS(300));
    assert!(a.iter().filter(|c| c[2] == 0x39).all(|c| c[4] == 0));
    std::fs::write(&f, "90\n").unwrap();
    let b = dev.drain_commands(MS(500));
    assert!(b.iter().any(|c| *c == keepalive(90)));
    std::fs::write(&f, "junk").unwrap();
    let c = dev.drain_commands(MS(500));
    assert!(c.iter().any(|c| *c == keepalive(0)));
    drop(s);
}

#[test]
fn passive_session_sends_nothing() {
    let (s, _rx, dev) = start(Config::passive());
    assert!(dev.drain_commands(MS(600)).is_empty());
    assert!(s.is_connected());
}

#[test]
fn imu_controller_status_log_events_and_drain() {
    let (s, rx, dev) = start(Config::default());
    let imu = load_rec("p83.rec.gz");
    let ctl = load_rec("p82.rec.gz");
    for (_, r) in imu.iter().take(300) {
        dev.send_report(2, r).unwrap();
    }
    for (_, r) in ctl.iter().filter(|(_, b)| b[3] == 3).take(200) {
        dev.send_report(1, r).unwrap();
    }
    let mut st = [0u8; 64];
    st[..16].copy_from_slice(&[
        0x4c, 0x0f, 0x37, 0x03, 0x48, 4, 0x88, 0x91, 2, 0x63, 0, 2, 1, 0, 8, 0x2e,
    ]);
    st[17..26].copy_from_slice(&[0x0f, 0x37, 0x02, 0x48, 0, 0, 0, 0, 0x49]);
    dev.send_report(0, &st).unwrap();
    let mut lg = [0u8; 64];
    lg[..30].copy_from_slice(b"[00:00:01] app: hello\r\npartial");
    dev.send_report(3, &lg).unwrap();
    let mut lg2 = [0u8; 64];
    lg2[..6].copy_from_slice(b" line\n");
    dev.send_report(3, &lg2).unwrap();
    std::thread::sleep(MS(300));
    let evs: Vec<Event> = rx.try_iter().collect();
    let imus: Vec<_> = evs
        .iter()
        .filter_map(|e| {
            if let Event::Imu(i) = e {
                Some(*i)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(imus.len(), 600, "two samples per report");
    let first = decode_imu(&imu[0].1).unwrap();
    assert_eq!(imus[0], first[0]);
    assert_eq!(imus[1], first[1]);
    let ctrls: Vec<_> = evs
        .iter()
        .filter_map(|e| {
            if let Event::Controller(c) = e {
                Some(*c)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(ctrls.len(), 200);
    assert!(ctrls.iter().all(|c| c.slot == 3));
    let sts: Vec<_> = evs
        .iter()
        .filter_map(|e| {
            if let Event::Status(c) = e {
                Some(*c)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        sts.iter().map(|s| (s.slot, s.battery)).collect::<Vec<_>>(),
        [(3, 0x63), (2, 0x49)]
    );
    let logs: Vec<_> = evs
        .iter()
        .filter_map(|e| {
            if let Event::Log(l) = e {
                Some(l.as_str())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(logs, ["[00:00:01] app: hello", "partial line"]);
    assert!(evs
        .iter()
        .any(|e| matches!(e, Event::State(State::Connected))));
    let stt = s.stats();
    assert_eq!(
        (
            stt.imu_reports,
            stt.controller_reports,
            stt.status_messages,
            stt.log_lines
        ),
        (300, 200, 2, 2)
    );
}

#[test]
fn request_reply_matching_and_timeout() {
    let (s, _rx, dev) = start(Config::default());
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let h = responder(dev.clone(), stop.clone());
    assert_eq!(s.read_serial(0, MS(1000)).unwrap(), "H1234");
    assert_eq!(
        s.read_version(2, MS(1000)).unwrap(),
        vec![0x00, 0x08, 0x2d, 0x6b]
    );
    // no responder for 0x2c: times out, and the next request still works
    let t = Instant::now();
    assert!(matches!(
        s.request(0x2c, 0, &[], MS(150)),
        Err(Error::Timeout)
    ));
    assert!(t.elapsed() >= MS(140) && t.elapsed() < MS(400));
    assert_eq!(s.read_serial(0, MS(1000)).unwrap(), "H1234");
    assert!(matches!(s.send(0x16, 0, &[0; 61]), Err(Error::TooLong)));
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let seen = h.join().unwrap();
    assert!(
        seen.iter()
            .any(|c| c[..5] == [0x58, 0x04, 0x16, 0x00, 0x1e]),
        "serial request wire form"
    );
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn calibration_over_the_session_with_a_lost_reply() {
    let xml = std::fs::read(fixtures_dir().join("../../analysis/headset-calibration.xml")).unwrap();
    let blob = calib::build_blob(&xml);
    let (s, _rx, dev) = start(Config::default());
    let d2 = dev.clone();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let st2 = stop.clone();
    let h = std::thread::spawn(move || {
        let mut dropped_once = false;
        while !st2.load(std::sync::atomic::Ordering::Relaxed) {
            let Some(c) = d2.recv_command(MS(20)) else {
                continue;
            };
            if c[2] == 0x0a && c[4] == 1 {
                let addr = u16::from(c[5]) | u16::from(c[6]) << 8;
                if addr == 0x100 && !dropped_once {
                    dropped_once = true; // lose the first reply: the host must retry
                    continue;
                }
                let _ = d2.send_report(0, &calib::make_block_reply(&blob, addr));
            }
        }
    });
    let got = s.read_calibration_blob(MS(300)).unwrap();
    assert_eq!(calib::inflate_blob(&got).unwrap(), xml);
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    h.join().unwrap();
}

#[test]
fn unplug_and_reconnect() {
    let (s, rx, dev) = start(Config::default());
    assert_eq!(s.stats().connects, 1);
    dev.unplug();
    let t = Instant::now();
    while s.is_connected() && t.elapsed() < MS(2000) {
        std::thread::sleep(MS(5));
    }
    assert!(!s.is_connected(), "session notices the hang-up");
    assert!(matches!(s.send(0x03, 0, &[]), Err(Error::NotConnected)));
    assert!(matches!(
        s.request(0x03, 0, &[], MS(100)),
        Err(Error::NotConnected)
    ));
    dev.plug();
    s.wait_connected(MS(3000)).unwrap();
    assert!(dev.wait_host(MS(500)));
    assert_eq!(s.stats().connects, 2);
    let first = dev.recv_command(MS(500)).unwrap();
    assert_eq!(first, wake(), "wake is repeated after a reconnect");
    std::thread::sleep(MS(50));
    let states: Vec<_> = rx
        .try_iter()
        .filter_map(|e| {
            if let Event::State(s) = e {
                Some(s)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        states,
        [State::Connected, State::Disconnected, State::Connected]
    );
}

#[test]
fn pending_request_is_aborted_when_the_device_vanishes() {
    let (s, _rx, dev) = start(Config::default());
    let d = dev.clone();
    std::thread::spawn(move || {
        std::thread::sleep(MS(100));
        d.unplug();
    });
    let t = Instant::now();
    let r = s.request(0x2c, 0, &[], MS(3000));
    assert!(matches!(r, Err(Error::NotConnected)), "{r:?}");
    assert!(t.elapsed() < MS(1500));
}

#[test]
fn auto_attach_initial_burst_and_reattach_on_0x37() {
    let (s, _rx, dev) = start(Config {
        auto_attach: true,
        ..Config::default()
    });
    let t0 = Instant::now();
    let mut seen: Vec<(Duration, Report)> = vec![];
    // announce slot 2 as awake-but-silent shortly after the first burst
    std::thread::sleep(MS(100));
    let mut st = [0u8; 64];
    st[..16].copy_from_slice(&[
        0x4c, 0x0f, 0x37, 0x02, 0x48, 4, 0x88, 0x91, 2, 0x63, 0, 2, 1, 0, 8, 0x2e,
    ]);
    dev.send_report(0, &st).unwrap();
    while t0.elapsed() < MS(3400) {
        if let Some(c) = dev.recv_command(MS(50)) {
            seen.push((t0.elapsed(), c));
        }
    }
    let attach: Vec<_> = seen
        .iter()
        .filter(|(_, c)| matches!(c[2], 0x16 | 0x50 | 0x0a | 0x03))
        .collect();
    // initial: 4 commands x 2 slots; then one re-burst for slot 2 only (>= 2.5 s after the first)
    let by_slot = |slot: u8| attach.iter().filter(|(_, c)| c[3] == slot).count();
    assert_eq!(by_slot(2), 8, "slot 2: initial + one retry");
    assert_eq!(by_slot(3), 4, "slot 3: initial only");
    let first = attach.iter().find(|(_, c)| c[3] == 2).unwrap();
    assert_eq!(first.1[2], 0x16);
    let retry = attach
        .iter()
        .filter(|(_, c)| c[3] == 2 && c[2] == 0x16)
        .nth(1)
        .unwrap();
    assert!(
        retry.0 - first.0 >= MS(2400),
        "retry rate limited to 2.5 s: {:?}",
        retry.0 - first.0
    );
    // spacing inside the (live) retry burst is about 10 ms
    let b: Vec<_> = attach
        .iter()
        .filter(|(_, c)| c[3] == 2)
        .skip(4)
        .take(4)
        .collect();
    assert!(b[3].0 - b[0].0 >= MS(25) && b[3].0 - b[0].0 < MS(120));
    assert_eq!(s.stats().auto_attaches, 1);
}

#[test]
fn no_reattach_while_reports_flow() {
    let (s, _rx, dev) = start(Config {
        auto_attach: true,
        ..Config::default()
    });
    let rep = load_rec("p82.rec.gz")
        .into_iter()
        .find(|(_, b)| b[3] == 2)
        .unwrap()
        .1;
    let mut st = [0u8; 64];
    st[..16].copy_from_slice(&[
        0x4c, 0x0f, 0x37, 0x02, 0x48, 4, 0x88, 0x91, 2, 0x63, 0, 2, 1, 0, 8, 0x2e,
    ]);
    let t0 = Instant::now();
    while t0.elapsed() < MS(3200) {
        dev.send_report(1, &rep).unwrap(); // slot 2 alive
        dev.send_report(0, &st).unwrap(); // stray heartbeat
        std::thread::sleep(MS(20));
    }
    assert_eq!(s.stats().auto_attaches, 0);
}

#[test]
fn video_prepare_wire_and_rate() {
    let (s, _rx, dev) = start(Config::default());
    s.video_prepare(72).unwrap();
    let seen = dev.drain_commands(MS(500));
    let pos = |p: &[u8]| {
        seen.iter()
            .position(|c| c.starts_with(p))
            .unwrap_or_else(|| panic!("missing {p:02x?}"))
    };
    let a = pos(&[0x58, 0x05, 0x5b, 0x05, 0x02, 0x02]);
    let b = pos(&[0x58, 0x04, 0x29, 0x00, 0x01]);
    let c = pos(&[0x58, 0x0d, 0x1a, 0x05, 0x02, 0x48]);
    let d = pos(&[0x58, 0x0d, 0x1a, 0x05, 0x05, 0x00]);
    assert!(a < b && b < c && c < d);
    assert!(seen.iter().any(|c| *c == keepalive(72)));
}

#[test]
fn close_is_prompt_and_drop_joins() {
    let (s, _rx, _dev) = start(Config::default());
    let t = Instant::now();
    s.close();
    assert!(t.elapsed() < MS(100), "close took {:?}", t.elapsed());
    // never connects: still stops promptly
    let (be, dev) = fake::pair();
    dev.unplug();
    let s = Session::open_with_backend(Config::default(), be, pimax_noop()).unwrap();
    assert!(matches!(s.wait_connected(MS(100)), Err(Error::Timeout)));
    let t = Instant::now();
    drop(s);
    assert!(t.elapsed() < MS(100));
}

struct Noop;
impl pimax::Handler for Noop {}
fn pimax_noop() -> Noop {
    Noop
}

#[test]
fn haptic_goes_out_on_the_session_thread_rate_limited() {
    let (s, _rx, dev) = start(Config::default());
    let _ = dev.drain_commands(MS(50));
    // Pimax Play's test pulse (right, 25 %, 300 ms): one command, the captured bytes
    assert!(s.haptic(SLOT_RIGHT, Some((25, 300))));
    let seen = dev.drain_commands(MS(100));
    let h: Vec<_> = seen.iter().filter(|c| c[2] == 0x07).collect();
    assert_eq!(h.len(), 1);
    assert_eq!(
        &h[0][..10],
        &[0x58, 0x09, 0x07, 0x03, 0x19, 0x2c, 0x01, 0, 0, 0]
    );
    // a burst of 20 requests within ~1 ms: far fewer commands, the last one is the newest value
    let t0 = Instant::now();
    for k in 0..20u8 {
        s.haptic(SLOT_LEFT, Some((80 + k, 12)));
    }
    let seen = dev.drain_commands(MS(100));
    let h: Vec<_> = seen.iter().filter(|c| c[2] == 0x07).collect();
    assert!(t0.elapsed() >= MS(8));
    assert!(!h.is_empty() && h.len() <= 3, "{} commands", h.len());
    assert_eq!(
        &h.last().unwrap()[..7],
        &[0x58, 0x09, 0x07, 0x02, 99, 12, 0]
    );
    assert!(!s.haptic(5, Some((1, 1))), "unknown slot");
    let (sent, _replaced) = s.haptic_counts();
    assert!(sent >= 2);
}

fn wakes_in(dev: &FakeDevice, ms: u64) -> Vec<Duration> {
    let t0 = Instant::now();
    let mut v = vec![];
    while t0.elapsed() < MS(ms) {
        if let Some(c) = dev.recv_command(MS(20)) {
            if c == wake() {
                v.push(t0.elapsed());
            }
        }
    }
    v
}

#[test]
fn unacked_wake_is_sent_once_more_after_half_a_second() {
    // after a headset restart the first wake is ignored (Windows capture take 2): wake at 0, again at ~0.5 s, no third
    let (_s, _rx, dev) = start(Config::default());
    let w = wakes_in(&dev, 1600);
    assert_eq!(w.len(), 2, "wake, then exactly one re-send: {w:?}");
    assert!(w[1] >= MS(450) && w[1] <= MS(900), "{w:?}");
}

#[test]
fn acked_wake_is_not_repeated() {
    let (_s, _rx, dev) = start(Config::default());
    let mut r = [0u8; 64];
    r[..6].copy_from_slice(&[0x4c, 0x04, 0x1a, 0x05, 0x01, 0x07]);
    dev.send_report(0, &r).unwrap();
    assert_eq!(wakes_in(&dev, 1200).len(), 1, "only the first wake");
}
