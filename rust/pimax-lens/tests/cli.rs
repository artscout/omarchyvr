//! The `pimax-lens` command line tool, black box.
use std::path::PathBuf;
use std::process::Command;

fn bin() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_pimax-lens"));
    c.env_remove("PIMAX_LENS_CONF")
        .env_remove("PIMAX_LENS_OFF")
        .env_remove("PIMAX_LENS_DEBUG_GRID");
    c
}

fn example() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../analysis/tools/lens.conf.example")
}

fn tmp(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn no_arguments_prints_usage_and_exits_2() {
    let o = bin().output().unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("usage: pimax-lens show"));
    let o = bin().arg("bogus").output().unwrap();
    assert_eq!(o.status.code(), Some(2));
}

#[test]
fn show_reports_file_hash_rings_and_corners() {
    let o = bin()
        .arg("show")
        .arg("--conf")
        .arg(example())
        .output()
        .unwrap();
    assert!(o.status.success());
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(out.contains("(found)"), "{out}");
    let crc = pimax_lens::png::crc32(&std::fs::read(example()).unwrap());
    assert!(out.contains(&format!("config crc32: {crc:08x}")), "{out}");
    assert!(out.contains("30 deg: r = 0.7318 (1300 px)"), "{out}");
    assert!(out.contains("corner TL"), "{out}");
    assert!(!out.contains("WARNING"), "{out}");
    // a missing file falls back to the defaults and says so
    let o = bin()
        .arg("show")
        .arg("--conf")
        .arg("/nonexistent/x.conf")
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(
        out.contains("not found, built-in defaults") && out.contains("30 deg: r = 0.7318"),
        "{out}"
    );
}

#[test]
fn show_warns_about_bad_lines() {
    let d = tmp("cli-bad");
    let f = d.join("lens.conf");
    std::fs::write(&f, "fov = 1 2\nk = 0 -3 0\n").unwrap();
    let o = bin().arg("show").arg("--conf").arg(&f).output().unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(
        out.contains("WARNING pimax lens:") && out.contains("bad key/value 'fov = 1 2'"),
        "{out}"
    );
    assert!(out.contains("WARNING: radial curve folds back"), "{out}");
}

#[test]
fn grid_writes_pictures() {
    let d = tmp("cli-grid");
    let o = bin()
        .args(["grid", "--out"])
        .arg(&d)
        .args(["--div", "16", "--conf"])
        .arg(example())
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    for n in [
        "pimax_lens_grid_left.png",
        "pimax_lens_grid_right.png",
        "pimax_lens_summary.txt",
    ] {
        assert!(d.join(n).metadata().unwrap().len() > 100, "{n}");
    }
    let img = pimax_lens::png::decode(&std::fs::read(d.join("pimax_lens_grid_left.png")).unwrap())
        .unwrap();
    assert_eq!((img.width, img.height), (240, 222));
}

#[test]
fn fit_reproduces_the_default_coefficients_and_extends_to_the_corners() {
    let o = bin()
        .args(["fit", "--terms", "3", "--rmax", "0"])
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(
        out.contains("base = 0.65307") && out.contains("k = 0.60779 -0.72489 0.58858"),
        "{out}"
    );
    assert!(!out.contains("fit_range"));
    let o = bin()
        .args(["fit", "--terms", "4", "--rmax", "1.5"])
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(
        out.contains("fit_range = 1.5") && out.contains("continuation samples"),
        "{out}"
    );
    // the output parses as a lens.conf
    let frag: String = out
        .lines()
        .filter(|l| !l.starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    let (_, w) = pimax_lens::config::parse(&frag, 1.0);
    assert!(w.is_empty(), "{w:?}\n{frag}");
}

#[test]
fn fit_reads_a_table_file_and_rejects_garbage() {
    let d = tmp("cli-table");
    let f = d.join("t.csv");
    let rows: Vec<String> = (0..20)
        .map(|i| {
            format!(
                "{},{}",
                f64::from(i) * 2.5,
                (f64::from(i) * 2.5).to_radians().tan() / 0.66
            )
        })
        .collect();
    std::fs::write(&f, format!("# angle,radius\n{}\n", rows.join("\n"))).unwrap();
    let o = bin()
        .args(["fit", "--rmax", "0", "--terms", "3", "--table"])
        .arg(&f)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(String::from_utf8_lossy(&o.stdout).contains("base = "));
    std::fs::write(&f, "1 2\nnonsense\n").unwrap();
    let o = bin().args(["fit", "--table"]).arg(&f).output().unwrap();
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("expected 'angle_deg radius'"));
}

#[test]
fn make_lut_writes_a_file_that_show_then_uses() {
    let dir = tmp("make-lut");
    let dump = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/pvr-dump-sub.json");
    let lut = dir.join("sub.bin");
    let o = bin()
        .arg("make-lut")
        .arg(&dump)
        .arg("--out")
        .arg(&lut)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(
        out.contains("serial P511X0X000000000002") && out.contains("bicubic vs"),
        "{out}"
    );
    // relative `lut =` resolves next to the conf file
    let conf = dir.join("lut.conf");
    std::fs::write(&conf, "cant_deg = 2\nlut = sub.bin\n").unwrap();
    let o = bin().arg("show").arg("--conf").arg(&conf).output().unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(
        out.contains("eye 1 LUT") && out.contains("fov L47.54 R52.58 U44.55 D44.55"),
        "{out}"
    );
    // missing input / missing --out are errors
    assert!(!bin()
        .arg("make-lut")
        .arg(&dump)
        .output()
        .unwrap()
        .status
        .success());
    assert!(!bin()
        .args(["make-lut", "/nonexistent.json", "--out", "x"])
        .output()
        .unwrap()
        .status
        .success());
}
