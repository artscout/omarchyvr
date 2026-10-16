//! Rust == C, on real captures, on mutations of real reports and on random bytes.
use pimax_difftest::*;
use pimax_proto::*;
use proptest::prelude::*;
use std::io::Read;

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

fn check(r: &Report) -> Result<(), String> {
    if let Some(d) = imu_diff(&decode_imu(r), &c_decode_imu(r)) {
        return Err(format!("IMU {d}\n{r:02x?}"));
    }
    if let Some(d) = ctrl_diff(&decode_controller(r), &c_decode_controller(r)) {
        return Err(format!("CTRL {d}\n{r:02x?}"));
    }
    Ok(())
}

#[test]
fn struct_layout_of_the_c_library_matches_the_mirrors() {
    // SAFETY: shim functions return constants.
    unsafe {
        assert_eq!(shim_sizeof_imu(), std::mem::size_of::<c_imu_sample>());
        assert_eq!(shim_sizeof_ctrl(), std::mem::size_of::<c_ctrl_report>());
        assert_eq!(
            shim_off_ctrl_battery(),
            std::mem::offset_of!(c_ctrl_report, battery)
        );
        assert_eq!(
            shim_off_ctrl_raw(),
            std::mem::offset_of!(c_ctrl_report, raw)
        );
        assert_eq!(
            shim_off_imu_ipd(),
            std::mem::offset_of!(c_imu_sample, ipd_mm)
        );
    }
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn identical_on_every_captured_report() {
    let mut n = 0usize;
    let (mut imu, mut ctl, mut none) = (0, 0, 0);
    for f in [
        "p83.rec.gz",
        "p82.rec.gz",
        "ctrl_buttons_hub2.rec.gz",
        "if0_replies_coldstart.rec.gz",
        "if3_log_coldstart.rec.gz",
    ] {
        for r in load(f) {
            check(&r).unwrap_or_else(|e| panic!("{f}: {e}"));
            match (decode_imu(&r).is_some(), decode_controller(&r).is_some()) {
                (true, _) => imu += 1,
                (_, true) => ctl += 1,
                _ => none += 1,
            }
            n += 1;
        }
    }
    println!("compared {n} reports: {imu} IMU, {ctl} controller, {none} other");
    assert!(n > 160_000 && imu > 60_000 && ctl > 100_000 && none > 500);
}

#[test]
fn identical_under_single_byte_mutation_of_real_reports() {
    let base_imu = load("p83.rec.gz")[1000];
    let base_ctl = load("ctrl_buttons_hub2.rec.gz")[5000];
    let base_ctl3 = load("p82.rec.gz").into_iter().find(|r| r[3] == 3).unwrap();
    let mut n = 0;
    for base in [base_imu, base_ctl, base_ctl3] {
        for i in 0..64 {
            for v in 0..=255u8 {
                let mut r = base;
                r[i] = v;
                check(&r).unwrap();
                n += 1;
            }
        }
    }
    assert_eq!(n, 3 * 64 * 256);
}

#[test]
fn identical_for_the_controller_slot_and_sample_selection_corners() {
    let mut r = [0u8; 64];
    r[..5].copy_from_slice(&[0x53, 0x32, 0x08, 0x02, 0x43]);
    for slot in 0..=255u8 {
        r[3] = slot;
        check(&r).unwrap();
    }
    r[3] = 2;
    // first sample all zero except one byte at each position of the 12 byte window, second sample set
    for i in 0..12 {
        let mut q = r;
        q[16 + i] = 1;
        q[30..42].fill(0x7f);
        check(&q).unwrap();
        let mut q = r;
        q[30 + i] = 1;
        check(&q).unwrap();
    }
}

#[test]
fn find_interfaces_matches_the_c_implementation_on_this_machine() {
    let roots = pimax::discovery::Roots {
        hidraw_class: "/sys/class/hidraw".into(),
        dev: "/dev".into(),
    };
    let rust = pimax::discovery::find_interfaces(&roots);
    let (mask, cpaths) = c_find_interfaces();
    assert_eq!(
        i32::from(pimax::discovery::mask(&rust)),
        mask,
        "interface mask"
    );
    for i in 0..4 {
        if let Some(p) = &rust[i] {
            assert_eq!(p.display().to_string(), cpaths[i], "IF{i}");
        } else {
            assert_eq!(cpaths[i], "");
        }
    }
    println!("find_interfaces mask {mask:#06b} {cpaths:?}");
}

fn any_report() -> impl Strategy<Value = Report> {
    (proptest::collection::vec(any::<u8>(), 64), 0usize..8).prop_map(|(v, k)| {
        let mut r = [0u8; 64];
        r.copy_from_slice(&v);
        match k {
            0 | 1 => r[..4].copy_from_slice(&[0x53, 0x31, 0x08, 0x00]),
            2 | 3 => {
                r[..3].copy_from_slice(&[0x53, 0x32, 0x08]);
                r[3] = 2 + (r[3] & 1);
            }
            4 => {
                // zero first controller sample
                r[..3].copy_from_slice(&[0x53, 0x32, 0x08]);
                r[3] = 3;
                r[16..28].fill(0);
            }
            _ => {}
        }
        r
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200_000))]
    #[test]
    fn random_reports_decode_identically(r in any_report()) {
        prop_assert!(check(&r).is_ok(), "{}", check(&r).unwrap_err());
    }
}
