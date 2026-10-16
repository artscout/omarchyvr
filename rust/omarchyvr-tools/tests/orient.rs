use omarchyvr_tools::orient::*;
use std::io::Read;

fn fixtures() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures")
}

fn load_rec(name: &str) -> Vec<[u8; 64]> {
    let mut d = Vec::new();
    flate2::read::GzDecoder::new(std::fs::File::open(fixtures().join(name)).unwrap())
        .read_to_end(&mut d)
        .unwrap();
    d.as_chunks::<72>()
        .0
        .iter()
        .map(|c| c[8..].try_into().unwrap())
        .collect()
}

fn cal() -> Calib {
    let xml =
        std::fs::read_to_string(fixtures().join("../../analysis/headset-calibration.xml")).unwrap();
    parse_calib(&xml).expect("calibration terms present")
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn calibration_terms_parse() {
    let c = cal();
    // the XML's first aBias / wBias / ka / kg attributes (analysis/imu-orientation.md section 3: wBias z is negative)
    assert!(
        (c.a_bias[0] - 0.08).abs() < 0.05 && c.a_bias[1].abs() < 0.1,
        "{:?}",
        c.a_bias
    );
    assert!(c.w_bias[2] < 0.0 && c.w_bias[0] > 0.0);
    assert!(c.ka.iter().chain(&c.kg).all(|k| k.abs() < 0.05));
    assert_eq!(parse_calib("<x/>"), None);
}

/// Replays the real p83 capture through the Rust filter and compares to quaternions produced by
/// `linux/pimax_imu_orient.py` (rust/fixtures/make_fixtures.py), every 1000th sample.
#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn mahony_matches_python_on_real_capture() {
    let gold: Vec<Vec<f64>> = std::fs::read_to_string(fixtures().join("mahony_golden.txt"))
        .unwrap()
        .lines()
        .map(|l| l.split(' ').map(|x| x.parse().unwrap()).collect())
        .collect();
    assert!(gold.len() > 100);
    let c = cal();
    let mut un = Unwrap::default();
    let mut f = Mahony::new(1.0, 0.0);
    let (mut prev, mut init, mut idx) = (0u64, false, 0usize);
    let mut checked = 0;
    let mut worst = 0f64;
    for r in load_rec("p83.rec.gz") {
        let Some(ss) = un.push(&r) else { continue };
        for s in ss {
            let (ac, g) = correct(s.acc, s.gyro, Some(&c));
            if !init {
                f.seed(ac);
                init = true;
                prev = s.t_us;
                continue;
            }
            let dt = (s.t_us - prev) as f64 * 1e-6;
            prev = s.t_us;
            if !(0.0 < dt && dt < 0.05) {
                continue;
            }
            f.update(g, ac, dt);
            idx += 1;
            if idx % 1000 == 0 {
                let gl = &gold[idx / 1000 - 1];
                assert_eq!(gl[0] as usize, idx);
                for k in 0..4 {
                    worst = worst.max((f.q[k] - gl[1 + k]).abs());
                }
                let (y, p, r) = f.euler();
                for (a, b) in [(y, gl[5]), (p, gl[6]), (r, gl[7])] {
                    worst = worst.max((a - b).abs() * 1e-3); // degrees, scaled
                }
                checked += 1;
            }
        }
    }
    assert_eq!(checked, gold.len());
    assert!(worst < 1e-9, "max deviation from python {worst:e}");
}

#[test]
fn timestamp_wrap_is_unwrapped() {
    let mut un = Unwrap::default();
    let mut r = [0u8; 64];
    r[..4].copy_from_slice(&[0x53, 0x31, 0x08, 0x00]);
    r[4..8].copy_from_slice(&(u32::MAX - 500).to_le_bytes());
    let a = un.push(&r).unwrap();
    r[4..8].copy_from_slice(&1500u32.to_le_bytes());
    let b = un.push(&r).unwrap();
    assert!(b[0].t_us > a[1].t_us, "{} {}", b[0].t_us, a[1].t_us);
    assert_eq!(
        b[0].t_us - a[0].t_us,
        1500 + 500 + 1 + (1u64 << 32) - (1u64 << 32)
    );
    assert!(un.push(&[0u8; 64]).is_none());
}

#[test]
fn axis_modes_and_euler_conventions() {
    assert_eq!(axis_map(0, [1.0, 2.0, 3.0]), [1.0, 2.0, 3.0]);
    assert_eq!(axis_map(1, [1.0, 2.0, 3.0]), [-1.0, 2.0, -3.0]);
    // level and still: gravity on +Y -> identity orientation, zero euler
    let mut f = Mahony::new(1.0, 0.0);
    f.seed([0.0, 1.0, 0.0]);
    let (y, p, r) = f.euler();
    assert!(
        y.abs() < 1e-9 && p.abs() < 1e-9 && r.abs() < 1e-9,
        "{y} {p} {r}"
    );
    // a constant yaw rate of 90 dps for 1 s about +Y turns the heading by 90 degrees
    for _ in 0..1000 {
        f.update([0.0, 90.0, 0.0], [0.0, 1.0, 0.0], 0.001);
    }
    let (y, p, _r) = f.euler();
    assert!((y - 90.0).abs() < 0.5 && p.abs() < 0.5, "yaw {y} pitch {p}");
}
