//! Closing a session must return promptly in every state.
use pimax::fake;
use pimax::{Config, Handler, Session};
use pimax_proto::*;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct Locker(Arc<Mutex<u64>>);
impl Handler for Locker {
    fn on_imu(&mut self, _s: &ImuSample) {
        *self.0.lock().unwrap() += 1;
    }
}

fn imu_report() -> Report {
    let mut r = [0u8; 64];
    r[0] = 0x11;
    r
}

#[test]
fn close_while_connected_and_flooded() {
    let (be, dev) = fake::pair();
    let m = Arc::new(Mutex::new(0u64));
    let s = Session::open_with_backend(Config::default(), be, Locker(m.clone())).unwrap();
    s.wait_connected(Duration::from_secs(2)).unwrap();
    let t = Instant::now();
    for _ in 0..200 {
        let _ = dev.send_report(1, &imu_report());
    }
    s.close();
    assert!(t.elapsed() < Duration::from_millis(500), "{:?}", t.elapsed());
}

#[test]
fn close_while_not_connected() {
    let (be, dev) = fake::pair();
    dev.unplug();
    let s = Session::open_with_backend(Config::default(), be, Locker(Default::default())).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    let t = Instant::now();
    drop(s);
    assert!(t.elapsed() < Duration::from_millis(500), "{:?}", t.elapsed());
}

/// Closer does not hold the callback's mutex (Monado's destroy order): a callback blocked briefly must not stall close.
#[test]
fn close_with_callback_contending() {
    let (be, dev) = fake::pair();
    let m = Arc::new(Mutex::new(0u64));
    let s = Session::open_with_backend(Config::default(), be, Locker(m.clone())).unwrap();
    s.wait_connected(Duration::from_secs(2)).unwrap();
    let m2 = m.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let h = std::thread::spawn(move || {
        let g = m2.lock().unwrap();
        tx.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        drop(g);
    });
    rx.recv().unwrap();
    let _ = dev.send_report(1, &imu_report());
    std::thread::sleep(Duration::from_millis(50));
    let t = Instant::now();
    s.close();
    h.join().unwrap();
    assert!(t.elapsed() < Duration::from_millis(600), "{:?}", t.elapsed());
}
