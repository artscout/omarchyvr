//! The Rust lens model against the original C one (`pimax_lens.c` of Monado commit 1da3626, byte for byte):
//! same parser results, same log lines, bit-identical UV output for the current lens.conf and for random configs of
//! the old file format; the new optional keys at their neutral values change nothing.
use pimax_lens::{load_file, Lens, LensConfig, Level, Loaded};
use pimax_lens_difftest::*;
use proptest::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

fn repo_example() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../analysis/tools/lens.conf.example")
}

fn temp_dir() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("lens-difftest");
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn unique_file(stem: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    temp_dir().join(format!(
        "{stem}-{}-{}.conf",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Load `path` with the C loader (through the environment) and the Rust one.
fn load_both(path: &Path, off: Option<&str>) -> (COldLens, Loaded) {
    let c = with_env(
        &[("PIMAX_LENS_CONF", path.to_str()), ("PIMAX_LENS_OFF", off)],
        || COldLens::load(ASPECT),
    );
    let off = off.map(str::to_string);
    let env = move |k: &str| {
        if k == "PIMAX_LENS_OFF" {
            off.clone()
        } else {
            None
        }
    };
    (c, load_file(ASPECT, &env, path.to_path_buf()))
}

fn models_equal(c: &COldLens, r: &Lens, pts: &[(f32, f32)]) -> Result<(), String> {
    if c.enabled() != r.config().enabled {
        return Err(format!(
            "enabled: C {} Rust {}",
            c.enabled(),
            r.config().enabled
        ));
    }
    for view in 0..2u32 {
        same_bits(
            &format!("view {view} fov"),
            &c.fov(view),
            &rust_fov_rad(r, view),
        )?;
        for &(u, v) in pts {
            same_bits(
                &format!("view {view} uv({u:?},{v:?})"),
                &c.compute(view, u, v),
                &rust_compute(r, view, u, v),
            )?;
        }
    }
    Ok(())
}

fn logs_equal(c: &COldLens, r: &Loaded) -> Result<(), String> {
    let rl: Vec<LogLine> = r
        .messages
        .iter()
        .map(|m| (m.level, m.text.clone()))
        .collect();
    if c.log != rl {
        return Err(format!(
            "log lines differ\nC:\n{:#?}\nRust:\n{:#?}",
            c.log, rl
        ));
    }
    Ok(())
}

#[test]
fn the_original_c_file_is_unmodified() {
    // FNV of `git show 1da3626:src/xrt/drivers/pimax/pimax_lens.{c,h}` (copyright line set to the public identity); a cheap checksum is enough to catch edits
    fn sum(p: &Path) -> u64 {
        std::fs::read(p)
            .unwrap()
            .iter()
            .fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
                (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
            })
    }
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("csrc/old");
    assert_eq!(
        std::fs::metadata(d.join("pimax_lens.c")).unwrap().len(),
        7689
    );
    assert_eq!(
        std::fs::metadata(d.join("pimax_lens.h")).unwrap().len(),
        1693
    );
    assert_eq!(
        sum(&d.join("pimax_lens.c")),
        0xf759_89a9_4d8c_db19,
        "pimax_lens.c changed (fnv {:#x})",
        sum(&d.join("pimax_lens.c"))
    );
    assert_eq!(
        sum(&d.join("pimax_lens.h")),
        0x80ab_4800_b93a_52cd,
        "pimax_lens.h changed (fnv {:#x})",
        sum(&d.join("pimax_lens.h"))
    );
}

#[test]
fn current_lens_conf_matches_the_c_model_bit_for_bit_on_the_compositor_grid() {
    let mut files = vec![repo_example()];
    // the user's installed file, if there is one (it is a copy of the example today, but may be tuned later)
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(home).join(".config/omarchyvr/lens.conf");
        if p.exists() {
            files.push(p);
        }
    }
    let pts = compositor_points();
    assert_eq!(pts.len(), 4 * 128 * 128);
    for f in files {
        let (c, r) = load_both(&f, None);
        assert!(r.file_found, "{}", f.display());
        assert!(
            r.messages.iter().all(|m| m.level == Level::Info),
            "{:?}",
            r.messages
        );
        models_equal(&c, &r.lens, &pts).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        logs_equal(&c, &r).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
    }
}

#[test]
fn the_comparison_is_sensitive_to_one_ulp() {
    // negative control: if the test could not fail it would prove nothing
    let (c, r) = load_both(&repo_example(), None);
    let pts = compositor_points();
    assert!(models_equal(&c, &r.lens, &pts[..500]).is_ok());
    let mut cfg = r.lens.config().clone();
    cfg.eye[0].k[1][2] = f32::from_bits(cfg.eye[0].k[1][2].to_bits() + 1);
    let err = models_equal(&c, &Lens::new(cfg), &pts).unwrap_err();
    assert!(err.contains("view 0"), "{err}");
    let mut cfg = r.lens.config().clone();
    cfg.eye[1].cy += 1e-6;
    assert!(models_equal(&c, &Lens::new(cfg), &pts).is_err());
    // and the C output is not trivial: green at the optical axis is near the image centre
    let g = c.compute(0, 0.5 + 0.0271 / (2.0 * ASPECT), 0.5 - 0.009 / 2.0);
    assert!(
        (g[2] - 0.5).abs() < 1e-3 && (g[3] - 0.5).abs() < 1e-3,
        "{g:?}"
    );
}

#[test]
fn the_example_file_is_exactly_the_built_in_default() {
    let (c, r) = load_both(&repo_example(), None);
    assert_eq!(r.lens.config(), &LensConfig::defaults(ASPECT));
    // and no file at all gives the same numbers in the C model too
    let none = unique_file("missing");
    let (c0, r0) = load_both(&none, None);
    assert_eq!(r0.lens.config(), &LensConfig::defaults(ASPECT));
    let pts = compositor_points();
    models_equal(&c, &r0.lens, &pts[..4000]).unwrap();
    models_equal(&c0, &r0.lens, &pts[..4000]).unwrap();
    logs_equal(&c0, &r0).unwrap();
}

#[test]
fn distortion_off_and_the_off_variable_match_c() {
    let pts = compositor_points();
    let f = unique_file("off");
    std::fs::write(&f, "distortion = off\nfov = 45 46 47 48\n").unwrap();
    let (c, r) = load_both(&f, None);
    assert!(!r.lens.config().enabled);
    models_equal(&c, &r.lens, &pts[..2000]).unwrap();
    logs_equal(&c, &r).unwrap();
    assert!(r.messages.iter().any(|m| m.text.contains("distortion OFF")));

    let (c, r) = load_both(&repo_example(), Some("1"));
    assert!(!r.lens.config().enabled);
    models_equal(&c, &r.lens, &pts[..2000]).unwrap();
    logs_equal(&c, &r).unwrap();
    // PIMAX_LENS_OFF=0 or empty does not switch it off, in both
    for v in ["0", ""] {
        let (c, r) = load_both(&repo_example(), Some(v));
        assert!(r.lens.config().enabled && c.enabled(), "{v:?}");
    }
}

#[test]
fn config_path_resolution_matches_the_c_loader() {
    let d = temp_dir();
    let xdg = d.join("xdg");
    let home = d.join("home");
    let cases: Vec<(Option<String>, Option<String>, Option<String>)> = vec![
        (
            Some("/nonexistent/explicit.conf".into()),
            Some(xdg.display().to_string()),
            Some(home.display().to_string()),
        ),
        (
            None,
            Some(xdg.display().to_string()),
            Some(home.display().to_string()),
        ),
        (None, None, Some(home.display().to_string())),
        (None, None, None),
        (
            Some(String::new()),
            Some(String::new()),
            Some(home.display().to_string()),
        ),
    ];
    for (conf, xdgv, homev) in cases {
        let c = with_env(
            &[
                ("PIMAX_LENS_CONF", conf.as_deref()),
                ("XDG_CONFIG_HOME", xdgv.as_deref()),
                ("HOME", homev.as_deref()),
                ("PIMAX_LENS_OFF", None),
            ],
            || COldLens::load(ASPECT),
        );
        let (conf2, xdg2, home2) = (conf.clone(), xdgv.clone(), homev.clone());
        let env = move |k: &str| match k {
            "PIMAX_LENS_CONF" => conf2.clone(),
            "XDG_CONFIG_HOME" => xdg2.clone(),
            "HOME" => home2.clone(),
            _ => None,
        };
        let path = pimax_lens::config::config_path(&env);
        let r = load_file(ASPECT, &env, path.clone());
        logs_equal(&c, &r).unwrap_or_else(|e| panic!("{conf:?} {xdgv:?} {homev:?}: {e}"));
        assert!(
            c.log[0].1.contains(&path.display().to_string()),
            "{:?} vs {}",
            c.log[0],
            path.display()
        );
    }
}

/// Values of the OLD file format only (3 numbers per k, every key one of the C parser's).
#[derive(Debug, Clone)]
enum Item {
    Fov([f32; 4]),
    Center([f32; 2]),
    Base(f32),
    Scale(f32),
    K(&'static str, [f32; 3]),
    Ca(&'static str, f32),
    Distortion(&'static str),
    Section(&'static str),
    Junk(&'static str),
    Blank,
    Comment(&'static str),
}

fn render(items: &[Item], commas: bool, trailing: bool) -> String {
    let sep = if commas { ", " } else { " " };
    let mut s = String::new();
    for it in items {
        let line = match it {
            Item::Fov(f) => format!("fov = {}", f.map(|x| x.to_string()).join(sep)),
            Item::Center(c) => format!("center = {}", c.map(|x| x.to_string()).join(sep)),
            Item::Base(b) => format!("base = {b}"),
            Item::Scale(b) => format!("scale = {b}"),
            Item::K(key, k) => format!("{key} = {}", k.map(|x| x.to_string()).join(sep)),
            Item::Ca(key, v) => format!("{key} = {v}"),
            Item::Distortion(v) => format!("distortion = {v}"),
            Item::Section(n) => (*n).to_string(),
            Item::Junk(j) => (*j).to_string(),
            Item::Blank => String::new(),
            Item::Comment(c) => (*c).to_string(),
        };
        s.push_str(&line);
        if trailing && !line.is_empty() && !matches!(it, Item::Comment(_)) {
            s.push_str("   # note ; more");
        }
        s.push('\n');
    }
    s
}

fn item() -> impl Strategy<Value = Item> {
    let f = |lo: f32, hi: f32| lo..hi;
    prop_oneof![
        3 => (f(25.0, 80.0), f(25.0, 80.0), f(25.0, 80.0), f(25.0, 80.0)).prop_map(|(a, b, c, d)| Item::Fov([a, b, c, d])),
        3 => (f(-0.2, 0.2), f(-0.2, 0.2)).prop_map(|(a, b)| Item::Center([a, b])),
        3 => f(0.3, 1.2).prop_map(Item::Base),
        2 => f(0.6, 1.5).prop_map(Item::Scale),
        6 => (prop::sample::select(vec!["k", "k_r", "k_g", "k_b", "K_R"]), f(-1.5, 1.5), f(-1.5, 1.5), f(-1.5, 1.5))
            .prop_map(|(k, a, b, c)| Item::K(k, [a, b, c])),
        5 => (prop::sample::select(vec!["ca_r", "ca_g", "ca_b"]), f(0.96, 1.04)).prop_map(|(k, v)| Item::Ca(k, v)),
        1 => prop::sample::select(vec!["off", "on", "0", "1", "false", "true", "no", "yes"]).prop_map(Item::Distortion),
        3 => prop::sample::select(vec!["[left]", "[right]", "[common]", "[both]", "[left] # eye", "[right]x"]).prop_map(Item::Section),
        2 => prop::sample::select(vec![
            "garbage", "[weird]", "fov = 1 2", "k = 1 2", "unknown = 5", "base =", "= 3", "scale = abc", "center = 1",
            "ca_r = x", "fov = 50 50 44.5", "k_g = 0.1,0.2", "[", "  ", "key", "fov",
        ])
        .prop_map(Item::Junk),
        1 => Just(Item::Blank),
        1 => prop::sample::select(vec!["# comment", "; other comment", "#fov = 1 2 3 4"]).prop_map(Item::Comment),
    ]
}

fn points() -> impl Strategy<Value = Vec<(f32, f32)>> {
    prop::collection::vec((-0.05f32..1.05, -0.05f32..1.05), 24)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 3000, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn random_old_format_configs_match_c_in_logs_and_bits(
        items in prop::collection::vec(item(), 0..30),
        commas in any::<bool>(),
        trailing in any::<bool>(),
        pts in points(),
    ) {
        let text = render(&items, commas, trailing);
        let f = unique_file("rand");
        std::fs::write(&f, &text).unwrap();
        let (c, r) = load_both(&f, None);
        let _ = std::fs::remove_file(&f);
        // the one deliberate deviation of the old grammar is the case of the `distortion` value (see below)
        let case_sensitive_off = text.lines().any(|l| l.to_lowercase().contains("distortion") && l.contains("OFF"));
        prop_assume!(!case_sensitive_off);
        logs_equal(&c, &r).map_err(|e| TestCaseError::fail(format!("{e}\n--- config ---\n{text}")))?;
        models_equal(&c, &r.lens, &pts).map_err(|e| TestCaseError::fail(format!("{e}\n--- config ---\n{text}")))?;
    }

    #[test]
    fn neutral_values_of_the_new_keys_change_nothing(
        items in prop::collection::vec(item(), 0..20),
        pts in points(),
    ) {
        let text = render(&items, false, false);
        let f = unique_file("neutral");
        std::fs::write(&f, &text).unwrap();
        let (c, r) = load_both(&f, None);
        let _ = std::fs::remove_file(&f);
        // restate the effective old-format values per eye and spell out every new key at its neutral value
        let ext = format!("{text}{}", neutral_ext(r.lens.config()));
        let (cfg2, _) = pimax_lens::config::parse(&ext, ASPECT);
        prop_assert_eq!(&cfg2, r.lens.config(), "\n{}", ext);
        models_equal(&c, &Lens::new(cfg2), &pts).map_err(|e| TestCaseError::fail(format!("{e}\n--- config ---\n{ext}")))?;
    }
}

/// `[left]`/`[right]` sections restating an effective configuration with k4 = k5 = 0, per-channel base equal to
/// `base`, zero shifts, no fit range, no black-out, no mask.
fn neutral_ext(cfg: &LensConfig) -> String {
    let mut s = String::new();
    for (sect, eye) in [("[left]", 0usize), ("[right]", 1)] {
        let e = &cfg.eye[eye];
        s += &format!(
            "{sect}\nfov = {} {} {} {}\ncenter = {} {}\nscale = {}\n",
            e.fov[0], e.fov[1], e.fov[2], e.fov[3], e.cx, e.cy, e.scale
        );
        for (i, ch) in ["r", "g", "b"].iter().enumerate() {
            s += &format!(
                "k_{ch} = {} {} {} 0 0\nbase_{ch} = {}\nca_{ch} = {}\nshift_{ch} = 0 0\n",
                e.k[i][0], e.k[i][1], e.k[i][2], e.base[i], e.ca[i]
            );
        }
        s += "fit_range = 0\nblackout = 0\nmask = off\n";
    }
    s
}

#[test]
fn deliberate_differences_from_the_c_parser() {
    // 1. `distortion = OFF` (upper case): the C parser compared case sensitively and left distortion ON.
    let f = unique_file("upper");
    std::fs::write(&f, "distortion = OFF\n").unwrap();
    let (c, r) = load_both(&f, None);
    assert!(c.enabled(), "C ignores the upper case OFF");
    assert!(!r.lens.config().enabled, "Rust honours it");

    // 2. five numbers for k: C read three and ignored the rest, Rust keeps k4 and k5.
    std::fs::write(&f, "k = 0.1 0.2 0.3 0.4 0.5\n").unwrap();
    let (c, r) = load_both(&f, None);
    assert!(c.log.iter().all(|l| l.0 == Level::Info));
    assert_eq!(r.lens.eye(0).k[1], [0.1, 0.2, 0.3, 0.4, 0.5]);

    // 3. non-finite numbers: C accepted "nan"/"inf", Rust reports a bad key/value.
    std::fs::write(&f, "scale = nan\n").unwrap();
    let (c, r) = load_both(&f, None);
    assert!(c.log.iter().all(|l| l.0 == Level::Info));
    assert!(r
        .messages
        .iter()
        .any(|m| m.level == Level::Warn && m.text.contains("bad key/value 'scale = nan'")));

    // 4. implausible numbers (the C parser took anything): rejected with a warning, the setting stays at its default.
    std::fs::write(&f, "fov = 120 120 100 100\nca_r = 0\n").unwrap();
    let (c, r) = load_both(&f, None);
    assert!(
        c.log.iter().all(|l| l.0 == Level::Info),
        "the C parser accepted them silently"
    );
    assert_eq!(
        r.messages.iter().filter(|m| m.level == Level::Warn).count(),
        2
    );
    assert_eq!(r.lens.fov_deg(0), [50.0, 50.0, 44.5, 44.5]);
    let _ = std::fs::remove_file(&f);
}
