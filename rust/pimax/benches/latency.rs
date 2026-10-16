//! IMU latency report -> callback, and session CPU cost, against the in-process fake headset (no hardware).
//!   cargo bench -p pimax --bench latency
//! A sender thread writes IMU reports at 500 Hz (the real rate) into a seqpacket socket; the time from just before `write`
//! to the start of `Handler::on_imu` on the session thread is the latency (it includes the kernel wake-up of the poll).
use pimax::fake;
use pimax::{Config, Handler, Session};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct H {
    t0: Instant,
    sent: Arc<Mutex<Vec<u64>>>,
    lat: Arc<Mutex<Vec<u64>>>,
}

impl Handler for H {
    fn on_imu(&mut self, s: &pimax::proto::ImuSample) {
        if s.sample_in_report != 0 {
            return;
        }
        let now = self.t0.elapsed().as_nanos() as u64;
        let idx = (s.ts_us / 1000) as usize / 2; // report index encoded in the device clock
        let sent = self.sent.lock().unwrap().get(idx).copied();
        if let Some(t) = sent {
            self.lat.lock().unwrap().push(now.saturating_sub(t));
        }
    }
}

fn report(i: u32) -> [u8; 64] {
    let mut r = [0u8; 64];
    r[..4].copy_from_slice(&[0x53, 0x31, 0x08, 0x00]);
    r[4..8].copy_from_slice(&(i * 2000).to_le_bytes());
    r
}

/// CPU seconds (user+sys) of the `pimax-session` thread only (the sender thread busy-waits and is excluded).
fn session_thread_cpu() -> f64 {
    for e in std::fs::read_dir("/proc/self/task").unwrap().flatten() {
        let p = e.path();
        if std::fs::read_to_string(p.join("comm"))
            .map(|c| c.trim() == "pimax-session")
            .unwrap_or(false)
        {
            return pimax::sys::proc_cpu_seconds(p.join("stat").to_str().unwrap()).unwrap_or(0.0);
        }
    }
    0.0
}

fn pct(v: &[u64], p: f64) -> f64 {
    v[((v.len() as f64 * p) as usize).min(v.len() - 1)] as f64 / 1e3
}

fn run(label: &str, n: u32, period: Option<Duration>) {
    let (be, dev) = fake::pair();
    let t0 = Instant::now();
    let sent = Arc::new(Mutex::new(Vec::with_capacity(n as usize)));
    let lat = Arc::new(Mutex::new(Vec::with_capacity(n as usize)));
    let s = Session::open_with_backend(
        Config::passive(),
        be,
        H {
            t0,
            sent: sent.clone(),
            lat: lat.clone(),
        },
    )
    .unwrap();
    s.wait_connected(Duration::from_secs(2)).unwrap();
    assert!(dev.wait_host(Duration::from_secs(1)));
    let cpu0 = session_thread_cpu();
    let start = Instant::now();
    let mut next = start;
    for i in 0..n {
        if let Some(p) = period {
            next += p;
            while Instant::now() < next {
                let left = next.saturating_duration_since(Instant::now());
                if left > Duration::from_micros(300) {
                    std::thread::sleep(left - Duration::from_micros(200));
                } else {
                    std::hint::spin_loop();
                }
            }
        }
        sent.lock().unwrap().push(t0.elapsed().as_nanos() as u64);
        // the sender and the receiver both burn CPU in this process; the sender's busy-wait is excluded below
        while let Err(e) = dev.send_report(2, &report(i)) {
            assert_eq!(e.kind(), std::io::ErrorKind::WouldBlock);
            std::thread::yield_now(); // burst mode: socket buffer full, the reader catches up
        }
    }
    let wall = start.elapsed().as_secs_f64();
    let t = Instant::now();
    while lat.lock().unwrap().len() < n as usize && t.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut l = lat.lock().unwrap().clone();
    l.sort_unstable();
    let cpu = session_thread_cpu() - cpu0;
    println!(
        "{label:<22} n={:<6} latency us: p50 {:7.1}  p90 {:7.1}  p99 {:7.1}  max {:8.1} | {:8.0} reports/s | session thread cpu {:.3}s",
        l.len(), pct(&l, 0.5), pct(&l, 0.9), pct(&l, 0.99), l[l.len() - 1] as f64 / 1e3, n as f64 / wall, cpu
    );
    assert_eq!(l.len(), n as usize, "no report lost");
}

fn main() {
    println!("IMU report -> Handler::on_imu latency (fake device, socketpair; real headset delivers 500 reports/s)");
    run(
        "paced 500 Hz, 10 s",
        5000,
        Some(Duration::from_micros(2000)),
    );
    run("paced 1 kHz, 5 s", 5000, Some(Duration::from_micros(1000)));
    run("burst (unpaced)", 100_000, None);
}
