//! Pimax Dream Air lens model for the Monado `pimax` driver, in Rust.
//!
//! * [`config`]: `lens.conf` parser (backward compatible with the C parser it replaces, plus per-channel base,
//!   k4/k5, per-channel shift, `fit_range`, `blackout`, `mask`).
//! * [`model`]: [`Lens::compute`], the distortion callback (bit-identical to the old C model for old configs).
//! * [`mask`]: visibility mask meshes for `XR_KHR_visibility_mask`.
//! * [`fit`]: fit the polynomial to an angle -> radius table, with a continuation out to the panel corners.
//! * [`grid`]: debug rendering of the UV grid (no compositor needed). Env `PIMAX_LENS_DEBUG_GRID=<dir>` makes the
//!   driver write it at start-up.
//! * [`capi`]: the `extern "C"` interface the thin C driver (`pimax_lens.c`) calls; header `include/pimax_lens_rs.h`.
//! * [`pvrfit`]: convert a Pimax PVR distortion dump (`windows/pimax-dump-distortion.ps1`) into this model
//!   (`pimax-lens fit-pvr`).
//! * [`font`], [`png`]: tiny helpers shared with the `pimax-lenstest` crate.

pub mod capi;
pub mod config;
pub mod fit;
pub mod font;
pub mod grid;
pub mod lut;
pub mod mask;
pub mod model;
pub mod png;
pub mod pvrfit;

pub use config::{EyeConfig, LensConfig, Mask, MaskShape, Warning, CHANNELS, K_TERMS};
pub use model::Lens;

use std::fmt::Write as _;
use std::path::PathBuf;

/// Severity of a [`Message`] (maps to Monado's `U_LOG_I` / `U_LOG_W`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
}

/// A log line produced while loading.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub level: Level,
    pub text: String,
}

fn info(text: String) -> Message {
    Message {
        level: Level::Info,
        text,
    }
}

fn warn(text: String) -> Message {
    Message {
        level: Level::Warn,
        text,
    }
}

/// Result of [`load`].
#[derive(Clone, Debug)]
pub struct Loaded {
    pub lens: Lens,
    pub path: PathBuf,
    /// The config file existed and was read.
    pub file_found: bool,
    pub messages: Vec<Message>,
}

/// The log lines describing the active configuration of one eye (the first two have the format of the C driver).
pub fn eye_log_lines(cfg: &LensConfig, e: usize) -> Vec<String> {
    let l = &cfg.eye[e];
    let mut v = vec![
        format!(
            "pimax lens: eye {e} {} fov L{:.1} R{:.1} U{:.1} D{:.1} centre {:.4},{:.4} base {:.4} scale {:.3}",
            if cfg.enabled { "distortion on" } else { "distortion OFF" },
            l.fov[0], l.fov[1], l.fov[2], l.fov[3], l.cx, l.cy, l.base[1], l.scale
        ),
        format!(
            "pimax lens: eye {e} k_r {:.4} {:.4} {:.4}  k_g {:.4} {:.4} {:.4}  k_b {:.4} {:.4} {:.4}  ca {:.4} {:.4} {:.4}",
            l.k[0][0], l.k[0][1], l.k[0][2], l.k[1][0], l.k[1][1], l.k[1][2], l.k[2][0], l.k[2][1], l.k[2][2],
            l.ca[0], l.ca[1], l.ca[2]
        ),
    ];
    if l.k.iter().any(|k| k[3] != 0.0 || k[4] != 0.0) {
        v.push(format!(
            "pimax lens: eye {e} k4 k5: r {:.4} {:.4}  g {:.4} {:.4}  b {:.4} {:.4}",
            l.k[0][3], l.k[0][4], l.k[1][3], l.k[1][4], l.k[2][3], l.k[2][4]
        ));
    }
    if l.base[0] != l.base[1] || l.base[2] != l.base[1] {
        v.push(format!(
            "pimax lens: eye {e} base r {:.5} g {:.5} b {:.5}",
            l.base[0], l.base[1], l.base[2]
        ));
    }
    if l.shift.iter().any(|s| s[0] != 0.0 || s[1] != 0.0) {
        v.push(format!(
            "pimax lens: eye {e} shift r {:+.4},{:+.4} g {:+.4},{:+.4} b {:+.4},{:+.4}",
            l.shift[0][0],
            l.shift[0][1],
            l.shift[1][0],
            l.shift[1][1],
            l.shift[2][0],
            l.shift[2][1]
        ));
    }
    if l.fit_range > 0.0 {
        v.push(format!(
            "pimax lens: eye {e} fit_range {:.3} (curve continues as a line beyond it)",
            l.fit_range
        ));
    }
    if l.has_mask() {
        let mut s = format!("pimax lens: eye {e} visibility mask:");
        if l.blackout.iter().any(|&b| b > 0.0) {
            let _ = write!(
                s,
                " blackout L{} R{} T{} B{}",
                l.blackout[0], l.blackout[1], l.blackout[2], l.blackout[3]
            );
        }
        match l.mask.shape {
            MaskShape::Off => {}
            MaskShape::Ellipse => {
                let _ = write!(s, " ellipse {} {}", l.mask.a, l.mask.b);
            }
            MaskShape::Rect => {
                let _ = write!(s, " rect {} {}", l.mask.a, l.mask.b);
            }
            MaskShape::Superellipse => {
                let _ = write!(s, " superellipse {} {} {}", l.mask.a, l.mask.b, l.mask.n);
            }
        }
        let _ = write!(s, " feather {}", l.mask_feather);
        v.push(s);
    }
    v
}

/// Load the configuration like the driver does: defaults, then the file from `PIMAX_LENS_CONF` /
/// `$XDG_CONFIG_HOME/pimax/lens.conf` / `~/.config/pimax/lens.conf`, then `PIMAX_LENS_OFF`.
pub fn load(aspect: f32, env: &dyn Fn(&str) -> Option<String>) -> Loaded {
    let path = config::config_path(env);
    load_file(aspect, env, path)
}

/// Load the LUT files named by the config (`lut = FILE`). On success the eye's FOV becomes the LUT's FOV port plus
/// `lut_margin` and its centre is reset to the panel middle (the LUT carries the real optical axis). A missing or
/// invalid file is a warning and the eye keeps the radial model.
fn attach_luts(
    cfg: &mut LensConfig,
    conf_dir: Option<&std::path::Path>,
    messages: &mut Vec<Message>,
) -> [Option<std::sync::Arc<lut::Lut>>; 2] {
    let mut out: [Option<std::sync::Arc<lut::Lut>>; 2] = [None, None];
    let mut cache: Vec<(PathBuf, Result<std::sync::Arc<lut::Lut>, String>)> = Vec::new();
    for (e, slot) in out.iter_mut().enumerate() {
        let Some(name) = cfg.eye[e].lut.clone() else {
            continue;
        };
        let mut p = if let Some(rest) = name.strip_prefix("~/") {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(rest)
        } else {
            PathBuf::from(&name)
        };
        if p.is_relative() {
            if let Some(d) = conf_dir {
                p = d.join(p);
            }
        }
        let idx = match cache.iter().position(|(q, _)| *q == p) {
            Some(i) => i,
            None => {
                cache.push((p.clone(), lut::Lut::load(&p).map(std::sync::Arc::new)));
                cache.len() - 1
            }
        };
        match &cache[idx].1 {
            Ok(l) => {
                let ec = &mut cfg.eye[e];
                let m = ec.lut_margin;
                ec.fov = l.fov_deg(e).map(|d| (d + m).clamp(0.5, 89.5));
                ec.cx = 0.0;
                ec.cy = 0.0;
                messages.push(info(format!(
                    "pimax lens: eye {e} LUT {} (serial {}, grid {}x{}, {:?}) fov L{:.2} R{:.2} U{:.2} D{:.2} (margin {m})",
                    p.display(), l.serial, l.n, l.n, ec.lut_interp,
                    ec.fov[0], ec.fov[1], ec.fov[2], ec.fov[3]
                )));
                *slot = Some(l.clone());
            }
            Err(err) => messages.push(warn(format!(
                "pimax lens: eye {e}: cannot use LUT, falling back to the radial model: {err}"
            ))),
        }
    }
    out
}

/// Like [`load`] with an explicit path.
pub fn load_file(aspect: f32, env: &dyn Fn(&str) -> Option<String>, path: PathBuf) -> Loaded {
    let mut cfg = LensConfig::defaults(aspect);
    let mut messages = Vec::new();
    let mut file_found = false;
    match std::fs::read(&path) {
        Ok(bytes) => {
            file_found = true;
            messages.push(info(format!("pimax lens: reading {}", path.display())));
            let text = String::from_utf8_lossy(&bytes);
            for w in config::parse_into(&mut cfg, &text, &path.display().to_string()) {
                messages.push(warn(format!("pimax lens: {}", w.message)));
            }
        }
        Err(_) => {
            messages.push(info(format!(
                "pimax lens: no config at {}, using built-in defaults",
                path.display()
            )));
        }
    }
    if config::forced_off(env) {
        cfg.enabled = false;
    }
    match config::cant_override(env) {
        Some(Ok(a)) => cfg.cant_deg = a,
        Some(Err(v)) => messages.push(warn(format!(
            "pimax lens: bad PIMAX_CANT_DEG '{v}' (number within +-{} expected), ignored",
            config::MAX_CANT_DEG
        ))),
        None => {}
    }
    let luts = if cfg.enabled {
        attach_luts(&mut cfg, path.parent(), &mut messages)
    } else {
        [None, None]
    };
    for (e, lut) in luts.iter().enumerate() {
        if lut.is_none() {
            for line in eye_log_lines(&cfg, e) {
                messages.push(info(line));
            }
        }
    }
    messages.push(info(format!(
        "pimax lens: cant {:.3} deg per eye (left view yawed +{:.3}, right view -{:.3})",
        cfg.cant_deg, cfg.cant_deg, cfg.cant_deg
    )));
    Loaded {
        lens: Lens::new(cfg).with_luts(luts),
        path,
        file_found,
        messages,
    }
}

/// `PIMAX_LENS_DEBUG_GRID=<dir>`: write the UV grid pictures and the text summary of both eyes into `dir`
/// (`PIMAX_LENS_DEBUG_GRID_DIV` divides the 3840x3552 panel size, default 4). Returns the log lines.
pub fn write_debug_grid(lens: &Lens, dir: &std::path::Path, div: u32) -> Vec<Message> {
    let mut out = Vec::new();
    if let Err(e) = std::fs::create_dir_all(dir) {
        out.push(warn(format!(
            "pimax lens: debug grid: cannot create {}: {e}",
            dir.display()
        )));
        return out;
    }
    let opt = grid::GridOptions::for_panel(div);
    let mut summary = String::new();
    for view in 0..2 {
        let img = grid::render(lens, view, &opt);
        let path = dir.join(format!(
            "pimax_lens_grid_{}.png",
            if view == 0 { "left" } else { "right" }
        ));
        match png::write_rgb8(&path, opt.width, opt.height, &img) {
            Ok(()) => out.push(info(format!(
                "pimax lens: debug grid written to {}",
                path.display()
            ))),
            Err(e) => out.push(warn(format!(
                "pimax lens: debug grid: cannot write {}: {e}",
                path.display()
            ))),
        }
        summary.push_str(&grid::summary(lens, view));
    }
    let path = dir.join("pimax_lens_summary.txt");
    match std::fs::write(&path, &summary) {
        Ok(()) => out.push(info(format!(
            "pimax lens: debug grid summary written to {}",
            path.display()
        ))),
        Err(e) => out.push(warn(format!(
            "pimax lens: debug grid: cannot write {}: {e}",
            path.display()
        ))),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ASPECT: f32 = 3840.0 / 3552.0;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn missing_file_uses_defaults_and_logs_like_the_c_driver() {
        let loaded = load_file(ASPECT, &no_env, PathBuf::from("/nonexistent/lens.conf"));
        assert!(!loaded.file_found);
        assert_eq!(loaded.lens.config(), &LensConfig::defaults(ASPECT));
        let t: Vec<&str> = loaded.messages.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(
            t[0],
            "pimax lens: no config at /nonexistent/lens.conf, using built-in defaults"
        );
        assert_eq!(
            t[1],
            "pimax lens: eye 0 distortion on fov L50.0 R50.0 U44.5 D44.5 centre 0.0271,0.0090 base 0.6531 scale 1.000"
        );
        assert_eq!(
            t[2],
            "pimax lens: eye 0 k_r 0.6078 -0.7249 0.5886  k_g 0.6078 -0.7249 0.5886  k_b 0.6078 -0.7249 0.5886  ca 1.0045 1.0000 0.9950"
        );
        assert_eq!(t.len(), 6);
        assert_eq!(
            t[5],
            "pimax lens: cant 0.000 deg per eye (left view yawed +0.000, right view -0.000)"
        );
    }

    #[test]
    fn file_warnings_and_forced_off() {
        let dir = std::env::temp_dir().join(format!("pimax-lens-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("lens.conf");
        std::fs::write(&p, "fov = 40 40 40 40\nbogus line\n").unwrap();
        let env = |k: &str| (k == "PIMAX_LENS_OFF").then(|| "1".to_string());
        let loaded = load_file(ASPECT, &env, p.clone());
        assert!(loaded.file_found);
        assert!(!loaded.lens.config().enabled);
        assert_eq!(loaded.lens.fov_deg(0), [40.0; 4]);
        assert_eq!(
            loaded.messages[0].text,
            format!("pimax lens: reading {}", p.display())
        );
        assert_eq!(loaded.messages[1].level, Level::Warn);
        assert!(
            loaded.messages[1]
                .text
                .ends_with("lens.conf:2 expected key = value"),
            "{:?}",
            loaded.messages[1]
        );
        assert!(loaded.messages[2].text.contains("distortion OFF"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extended_features_are_logged_only_when_used() {
        let (cfg, _) = config::parse("k_r = 1 2 3 4 5\nbase_b = 0.6\nshift_g = 0.01 0\nfit_range = 1.4\nmask = ellipse 1 1\nblackout = 0.01\n", ASPECT);
        let lines = eye_log_lines(&cfg, 0);
        assert!(lines.iter().any(|l| l.contains("k4 k5")));
        assert!(lines
            .iter()
            .any(|l| l.contains("base r 0.65307 g 0.65307 b 0.60000")));
        assert!(lines.iter().any(|l| l.contains("shift r")));
        assert!(lines.iter().any(|l| l.contains("fit_range 1.400")));
        assert!(lines
            .iter()
            .any(|l| l.contains("visibility mask: blackout L0.01") && l.contains("ellipse 1 1")));
        assert_eq!(eye_log_lines(&LensConfig::defaults(ASPECT), 1).len(), 2);
    }

    fn sub_lut_file(dir: &std::path::Path) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let dump = pvrfit::parse_dump(include_str!("../tests/data/pvr-dump-sub.json")).unwrap();
        let p = dir.join("sub.bin");
        std::fs::write(&p, lut::Lut::from_dump(&dump).unwrap().to_bytes()).unwrap();
        p
    }

    #[test]
    fn lut_key_replaces_the_radial_model_and_reproduces_the_dump_at_the_nodes() {
        let dir = std::env::temp_dir().join(format!("pimax-lens-lut-{}", std::process::id()));
        sub_lut_file(&dir);
        let conf = dir.join("lut.conf");
        std::fs::write(
            &conf,
            "cant_deg = 2\nfov = 40 40 40 40\ncenter = 0.3 0.3\nlut = sub.bin\n",
        )
        .unwrap();
        let loaded = load_file(ASPECT, &no_env, conf);
        assert!(
            loaded.messages.iter().all(|m| m.level == Level::Info),
            "{:?}",
            loaded.messages
        );
        assert!(loaded
            .messages
            .iter()
            .any(|m| m.text.contains("eye 0 LUT") && m.text.contains("grid 17x17")));
        let lens = &loaded.lens;
        assert!(lens.lut(0).is_some() && lens.lut(1).is_some());
        // the LUT's FOV port wins over `fov`, the centre is the panel middle
        let f = lens.fov_deg(0);
        assert!(
            (f[0] - 52.577).abs() < 0.01
                && (f[1] - 47.540).abs() < 0.01
                && (f[2] - 44.546).abs() < 0.01
        );
        assert_eq!((lens.eye(0).cx, lens.eye(0).cy), (0.0, 0.0));
        assert_eq!(lens.config().cant_deg, 2.0);
        // compute() = the dump's output UV (image UV y runs down: 1 - V), through each eye's panel orientation
        let dump = pvrfit::parse_dump(include_str!("../tests/data/pvr-dump-sub.json")).unwrap();
        let n = dump.eyes[0].grid_n;
        for (e, ed) in dump.eyes.iter().enumerate() {
            let d = &crate::pvrfit::ORIENTS[lens.lut(e).unwrap().meta(e).orient as usize].0;
            for (j, i) in [(4, 4), (8, 8), (10, 6), (7, 12), (9, 3)] {
                let (pu, pv) = (i as f64 / (n - 1) as f64, j as f64 / (n - 1) as f64);
                let (u, v) = lut::panel_to_display(d, pu, pv);
                let uv = lens.compute(e, u as f32, v as f32);
                for c in 0..3 {
                    let k = (j * n + i) * 2;
                    let (ou, ov) = (ed.grid[c][k] as f32, ed.grid[c][k + 1] as f32);
                    assert!(
                        (uv[c][0] - ou).abs() < 2e-5,
                        "eye {e} c {c}: {uv:?} vs {ou} {ov}"
                    );
                    assert!(
                        (uv[c][1] - (1.0 - ov)).abs() < 2e-5,
                        "eye {e} c {c}: {uv:?} vs {ou} {ov}"
                    );
                }
            }
        }
        // visibility: the display centre is visible, a far corner is outside the image
        let c = lens.compute(0, 0.5, 0.5)[1];
        assert!((0.0..=1.0).contains(&c[0]) && (0.0..=1.0).contains(&c[1]));
        let corner = lens.compute(0, 0.0, 0.0)[1];
        assert!(
            !(0.0..=1.0).contains(&corner[0]) || !(0.0..=1.0).contains(&corner[1]),
            "{corner:?}"
        );
        // custom visibility mask traced from the visible area
        assert!(lens.has_mask(0));
        let m = mask::visibility_mesh(lens, 0, mask::MaskKind::LineLoop, 32).unwrap();
        assert_eq!(m.vertices.len(), 32);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lut_margin_widens_the_fov_and_per_eye_lut_none_keeps_one_eye_radial() {
        let dir = std::env::temp_dir().join(format!("pimax-lens-lutm-{}", std::process::id()));
        let p = sub_lut_file(&dir);
        let conf = dir.join("m.conf");
        std::fs::write(
            &conf,
            format!(
                "lut = {}\nlut_margin = 0.5\nlut_interp = bilinear\n[right]\nlut = none\n",
                p.display()
            ),
        )
        .unwrap();
        let loaded = load_file(ASPECT, &no_env, conf);
        let lens = &loaded.lens;
        assert!(lens.lut(0).is_some() && lens.lut(1).is_none());
        let f = lens.fov_deg(0);
        assert!(
            (f[0] - 53.077).abs() < 0.01 && (f[2] - 45.046).abs() < 0.01,
            "{f:?}"
        );
        assert_eq!(lens.eye(0).lut_interp, lut::Interp::Bilinear);
        assert_eq!(lens.fov_deg(1), [50.0, 50.0, 44.5, 44.5]);
        assert!(!lens.has_mask(1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_or_invalid_lut_warns_and_falls_back_to_the_radial_model() {
        let dir = std::env::temp_dir().join(format!("pimax-lens-lutf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bad = dir.join("bad.bin");
        std::fs::write(&bad, b"garbage").unwrap();
        let reference = load_file(ASPECT, &no_env, PathBuf::from("/nonexistent/lens.conf")).lens;
        for (name, target) in [("missing", dir.join("nope.bin")), ("invalid", bad)] {
            let conf = dir.join(format!("{name}.conf"));
            std::fs::write(&conf, format!("lut = {}\n", target.display())).unwrap();
            let loaded = load_file(ASPECT, &no_env, conf);
            let warns: Vec<_> = loaded
                .messages
                .iter()
                .filter(|m| m.level == Level::Warn)
                .collect();
            assert_eq!(warns.len(), 2, "{name}: {:?}", loaded.messages); // one per eye
            assert!(
                warns[0].text.contains("falling back to the radial model"),
                "{}",
                warns[0].text
            );
            assert!(loaded.lens.lut(0).is_none());
            assert_eq!(loaded.lens.fov_deg(0), [50.0, 50.0, 44.5, 44.5]);
            assert_eq!(
                loaded.lens.compute(0, 0.3, 0.6),
                reference.compute(0, 0.3, 0.6)
            );
            // the radial model's configuration is still logged
            assert!(loaded
                .messages
                .iter()
                .any(|m| m.text.contains("eye 0 distortion on")));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn debug_grid_writes_two_pngs_and_a_summary() {
        let dir = std::env::temp_dir().join(format!("pimax-lens-grid-{}", std::process::id()));
        let lens = Lens::new(LensConfig::defaults(ASPECT));
        let msgs = write_debug_grid(&lens, &dir, 16);
        assert_eq!(msgs.len(), 3, "{msgs:?}");
        assert!(msgs.iter().all(|m| m.level == Level::Info), "{msgs:?}");
        for name in ["pimax_lens_grid_left.png", "pimax_lens_grid_right.png"] {
            let d = png::decode(&std::fs::read(dir.join(name)).unwrap()).unwrap();
            assert_eq!((d.width, d.height, d.channels), (240, 222, 3));
        }
        let s = std::fs::read_to_string(dir.join("pimax_lens_summary.txt")).unwrap();
        assert!(s.contains("eye 0 (left)") && s.contains("eye 1 (right)"));
        let _ = std::fs::remove_dir_all(&dir);
        // unwritable location -> a warning, no panic
        let bad = write_debug_grid(&lens, std::path::Path::new("/proc/definitely/not/here"), 16);
        assert_eq!(bad[0].level, Level::Warn);
    }
}
