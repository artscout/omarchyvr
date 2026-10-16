//! Decode throughput, Rust vs the original C, on the real p83 (IMU) and controller captures.
use pimax_difftest::*;
use pimax_proto::*;
use std::hint::black_box;
use std::io::Read;
use std::time::Instant;

fn load(name: &str) -> Vec<Report> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures")
        .join(name);
    let mut d = Vec::new();
    flate2::read::GzDecoder::new(std::fs::File::open(p).unwrap())
        .read_to_end(&mut d)
        .unwrap();
    d.as_chunks::<72>()
        .0
        .iter()
        .map(|c| c[8..].try_into().unwrap())
        .collect()
}

fn time(label: &str, n: usize, mut f: impl FnMut()) -> f64 {
    for _ in 0..3 {
        f();
    }
    let mut best = f64::MAX;
    for _ in 0..15 {
        let t = Instant::now();
        f();
        best = best.min(t.elapsed().as_secs_f64());
    }
    println!(
        "{label:<28} {:8.1} ns/report  {:8.1} Mreports/s",
        best / n as f64 * 1e9,
        n as f64 / best / 1e6
    );
    best / n as f64 * 1e9
}

fn main() {
    let imu = load("p83.rec.gz");
    let ctl = load("ctrl_buttons_hub2.rec.gz");
    println!(
        "decode throughput, best of 15 passes ({} IMU reports, {} controller reports)",
        imu.len(),
        ctl.len()
    );
    let r1 = time("rust decode_imu", imu.len(), || {
        let mut acc = 0f32;
        for r in &imu {
            if let Some(s) = decode_imu(black_box(r)) {
                acc += s[0].accel_g[0] + s[1].gyro_dps[2];
            }
        }
        black_box(acc);
    });
    let c1 = time("c    pimax_decode_imu", imu.len(), || {
        let mut acc = 0f32;
        for r in &imu {
            if let Some(s) = c_decode_imu(black_box(r)) {
                acc += s[0].accel_g[0] + s[1].gyro_dps[2];
            }
        }
        black_box(acc);
    });
    let r2 = time("rust decode_controller", ctl.len(), || {
        let mut acc = 0u32;
        for r in &ctl {
            if let Some(c) = decode_controller(black_box(r)) {
                acc = acc.wrapping_add(c.buttons + u32::from(c.trigger));
            }
        }
        black_box(acc);
    });
    let c2 = time("c    pimax_decode_controller", ctl.len(), || {
        let mut acc = 0u32;
        for r in &ctl {
            if let Some(c) = c_decode_controller(black_box(r)) {
                acc = acc.wrapping_add(c.buttons + u32::from(c.trigger));
            }
        }
        black_box(acc);
    });
    println!(
        "ratio rust/c: imu {:.2}x, controller {:.2}x (below 1.0 = Rust faster)",
        r1 / c1,
        r2 / c2
    );
    println!("headroom: the headset produces 500 IMU + 2x250 controller reports per second");
}
