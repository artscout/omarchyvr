//! Text output: the pattern list, the lens.conf report and the "active config" block printed at start-up, and the
//! banner printed whenever the pattern changes. Pure functions returning strings.

use crate::geometry::Fov;
use crate::patterns::{Params, Pattern};
use crate::state::{State, CONTROLS_HELP};
use pimax_lens::{config, grid, Lens};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// The aspect the Monado driver uses (display width / height as the client sees it).
pub const ASPECT: f32 = 3840.0 / 3552.0;

/// Text of `pimax-lenstest list`.
pub fn list_text(params: &Params) -> String {
    let mut s = String::new();
    for (i, p) in Pattern::ALL.iter().enumerate() {
        let steps = p.steps(params);
        let _ = writeln!(s, "{}. {}  ({})", i + 1, p.name(), p.title());
        let _ = writeln!(s, "   {}", p.purpose());
        if steps > 1 {
            let names: Vec<String> = (0..steps).map(|k| p.step_name(params, k)).collect();
            let _ = writeln!(
                s,
                "   {steps} steps (stick left/right): {}",
                names.join(" | ")
            );
        }
        for l in p.instructions() {
            let _ = writeln!(s, "   - {l}");
        }
        let _ = writeln!(s);
    }
    let _ = writeln!(s, "{CONTROLS_HELP}");
    s
}

/// The lens.conf that Monado will read, found the way the driver finds it.
pub struct LensInfo {
    pub path: PathBuf,
    pub found: bool,
    /// No lens.conf: the shipped Dream Air profile is in use.
    pub reference: bool,
    pub crc32: Option<u32>,
    pub bytes: usize,
    pub lens: Lens,
    pub log: Vec<String>,
}

pub fn load_lens(path: Option<&Path>) -> LensInfo {
    let env = config::process_env;
    let loaded = match path {
        Some(p) => pimax_lens::load_file(ASPECT, &env, p.to_path_buf()),
        None => pimax_lens::load(ASPECT, &env),
    };
    let (crc32, bytes) = std::fs::read(&loaded.path)
        .map(|b| (Some(pimax_lens::png::crc32(&b)), b.len()))
        .unwrap_or((None, 0));
    LensInfo {
        path: loaded.path.clone(),
        found: loaded.file_found,
        reference: loaded.reference,
        crc32,
        bytes,
        log: loaded.messages.iter().map(|m| m.text.clone()).collect(),
        lens: loaded.lens,
    }
}

/// Ring angles of the RINGS pattern and the predicted panel radius for each eye.
pub fn lens_report(info: &LensInfo, full: bool) -> String {
    let mut s = String::new();
    match (info.found, info.crc32) {
        (true, Some(c)) => {
            let _ = writeln!(
                s,
                "lens.conf: {} ({} bytes, crc32 {c:08x})",
                info.path.display(),
                info.bytes
            );
        }
        _ if info.reference => {
            let _ = writeln!(
                s,
                "lens.conf: {} (not found, the shipped Pimax Dream Air profile)",
                info.path.display()
            );
        }
        _ => {
            let _ = writeln!(
                s,
                "lens.conf: {} (not found, built-in defaults)",
                info.path.display()
            );
        }
    }
    for l in &info.log {
        let _ = writeln!(s, "  {l}");
    }
    if full {
        for view in 0..2 {
            s.push_str(&grid::summary(&info.lens, view));
        }
    } else {
        let _ = writeln!(s, "model: panel radius of the rings (green channel, half-heights from the optical centre; 1.0 = {} px):", grid::HALF_HEIGHT_PX);
        for view in 0..2 {
            let mut parts = Vec::new();
            for deg in crate::patterns::rings::MAJOR_DEG {
                match info.lens.radius_for_angle(view, 1, deg) {
                    Some(r) => parts.push(format!(
                        "{deg:.0} deg {r:.3} ({:.0} px)",
                        r * grid::HALF_HEIGHT_PX
                    )),
                    None => parts.push(format!("{deg:.0} deg n/a")),
                }
            }
            let _ = writeln!(
                s,
                "  {}: {}",
                if view == 0 { "left eye " } else { "right eye" },
                parts.join(" | ")
            );
        }
    }
    s
}

/// What the OpenXR session reported, for the start-up block.
#[derive(Clone, Debug, Default)]
pub struct RuntimeInfo {
    pub runtime: String,
    pub system: String,
    pub views: usize,
    pub recommended: (u32, u32),
    pub image: (u32, u32),
    pub fov: [Option<Fov>; 2],
    pub format: String,
    pub encoding: String,
    pub controllers: String,
    /// One line per view from XR_KHR_visibility_mask (empty if the runtime does not offer it).
    pub visibility: Vec<String>,
}

pub fn runtime_report(
    rt: &RuntimeInfo,
    lens: &LensInfo,
    state: &State,
    opts: &crate::cli::RunOpts,
) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "=== pimax-lenstest: active configuration ===");
    let _ = writeln!(s, "OpenXR runtime: {} | system: {}", rt.runtime, rt.system);
    let _ = writeln!(
        s,
        "views: {} | recommended {}x{} | rendering {}x{} per eye (res-div {}) | swapchain {} ({})",
        rt.views,
        rt.recommended.0,
        rt.recommended.1,
        rt.image.0,
        rt.image.1,
        opts.res_div,
        rt.format,
        rt.encoding
    );
    for (i, f) in rt.fov.iter().enumerate() {
        match f {
            Some(f) => {
                let d = f.half_angles_deg();
                let _ = writeln!(s, "view {i} ({}) FOV from the runtime: left {:.2} right {:.2} up {:.2} down {:.2} deg", if i == 0 { "left" } else { "right" }, d[0], d[1], d[2], d[3]);
            }
            None => {
                let _ = writeln!(s, "view {i} FOV: not located yet");
            }
        }
    }
    let _ = writeln!(s, "controllers: {}", rt.controllers);
    for v in &rt.visibility {
        let _ = writeln!(s, "visibility mask {v}");
    }
    let names: Vec<&str> = state.patterns.iter().map(|p| p.name()).collect();
    let _ = writeln!(
        s,
        "patterns: {} | start {} | eye {} | auto {} | text {} | square {} deg {} | format preference {}",
        names.join(","),
        state.pattern().name(),
        state.eye.name(),
        if state.auto_on { format!("every {} s", state.auto_secs) } else { "off".to_string() },
        if state.hud { "on" } else { "off" },
        opts.square_deg,
        state.dir.name(),
        if opts.unorm { "unorm" } else { "srgb" }
    );
    if let Some(dir) = &opts.capture {
        let _ = writeln!(s, "capture: PNGs into {} (compositor output downscaled by {}), settle {} frames per picture", dir.display(), opts.down, opts.settle_frames);
    }
    s.push_str(&lens_report(lens, false));
    let _ = writeln!(s, "{CONTROLS_HELP}");
    s
}

/// Printed when the pattern or step changes.
pub fn banner(state: &State, p: &Params) -> String {
    let pat = state.pattern();
    let steps = pat.steps(p);
    let mut s = String::new();
    let _ = writeln!(
        s,
        "PATTERN {}",
        state.status_line(&pat.step_name(p, state.step), steps)
    );
    let _ = writeln!(s, "  {}", pat.purpose());
    for l in pat.instructions() {
        let _ = writeln!(s, "  - {l}");
    }
    s
}

/// The lines drawn in the headset.
pub fn hud_lines(state: &State, p: &Params) -> Vec<String> {
    let pat = state.pattern();
    let steps = pat.steps(p);
    let mut lines = crate::patterns::wrap(
        &state
            .status_line(&pat.step_name(p, state.step), steps)
            .to_ascii_uppercase(),
        64,
    );
    for l in pat.hud_lines() {
        lines.extend(crate::patterns::wrap(l, 64));
    }
    if pat == Pattern::Align {
        lines.extend(crate::patterns::align::hud_lines(
            &state.align,
            &state.align_cal,
        ));
        return lines;
    }
    if matches!(pat, Pattern::Grid | Pattern::Swim) {
        lines.push(state.tune.short());
        lines.push("KEYS ; ' DK1  O L DK2  - = SCALE  : \" _ + COARSE  R RESET".to_string());
        if pat == Pattern::Swim {
            lines.push("C RE-CENTRES THE ROOM  Q PRINTS THE LENS.CONF VALUES".to_string());
        }
    }
    lines.extend(HUD_CONTROLS.iter().map(|l| (*l).to_string()));
    lines
}

/// The controller layout as the last lines of the on-screen text.
pub const HUD_CONTROLS: [&str; 2] = [
    "A/X NEXT PATTERN   B/Y BACK   STICK L/R STEP   STICK U/D SIDE",
    "STICK CLICK EYE   TRIGGER AUTO   GRIP TEXT",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::RunOpts;
    use crate::patterns::Params;

    fn params() -> Params {
        Params::new(
            1000,
            900,
            Fov::from_half_angles_deg(50.0, 50.0, 44.5, 44.5),
            0,
        )
    }

    fn example() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../analysis/tools/lens.conf.example")
    }

    #[test]
    fn list_has_every_pattern_with_its_instructions_and_steps() {
        let t = list_text(&params());
        for p in Pattern::ALL {
            assert!(
                t.contains(p.name()) && t.contains(p.title()),
                "{}",
                p.name()
            );
            for l in p.instructions() {
                assert!(t.contains(l));
            }
        }
        assert!(
            t.contains(
                "4 steps (stick left/right): ALL (WHITE) | RED ONLY | GREEN ONLY | BLUE ONLY"
            ),
            "{t}"
        );
        assert!(t.contains("EDGE 47.5 DEG RIGHT"), "{t}");
        assert!(t.contains("controls:"));
    }

    #[test]
    fn lens_report_has_the_file_hash_and_ring_radii() {
        let info = load_lens(Some(&example()));
        assert!(info.found);
        let r = lens_report(&info, false);
        let crc = pimax_lens::png::crc32(&std::fs::read(example()).unwrap());
        assert!(
            r.contains("lens.conf:") && r.contains(&format!("crc32 {crc:08x}")),
            "{r}"
        );
        assert!(r.contains("30 deg 0.732 (1300 px)"), "{r}");
        assert!(r.contains("left eye ") && r.contains("right eye"));
        assert!(r.contains("pimax lens: eye 0 distortion on"));
        let full = lens_report(&info, true);
        assert!(full.contains("corner TL") && full.len() > r.len());
        let missing = load_lens(Some(Path::new("/nonexistent/x.conf")));
        assert!(!missing.found);
        assert!(lens_report(&missing, false).contains("not found, built-in defaults"));
    }

    #[test]
    fn runtime_report_lists_fov_format_and_the_lens() {
        let rt = RuntimeInfo {
            runtime: "Monado(XRT) 25.1.0".into(),
            system: "Pimax Dream Air".into(),
            views: 2,
            recommended: (3552, 3840),
            image: (3552, 3840),
            fov: [
                Some(Fov::from_half_angles_deg(50.0, 50.0, 44.5, 44.5)),
                None,
            ],
            format: "R8G8B8A8_SRGB".into(),
            encoding: "bytes reach the panel unchanged".into(),
            controllers: "Oculus Touch profile".into(),
            visibility: vec![
                "view 0: hidden 12 vertices / 12 indices, visible 9 / 24, line loop 8 / 8".into(),
            ],
        };
        let state = State::new(Pattern::ALL.to_vec());
        let r = runtime_report(
            &rt,
            &load_lens(Some(&example())),
            &state,
            &RunOpts::default(),
        );
        for want in [
            "Monado(XRT) 25.1.0",
            "recommended 3552x3840",
            "view 0 (left) FOV from the runtime: left 50.00 right 50.00 up 44.50 down 44.50 deg",
            "view 1 FOV: not located yet",
            "visibility mask view 0: hidden 12 vertices",
            "R8G8B8A8_SRGB",
            "patterns: grid,rings,channels,fov-edge,square100,square25,corners",
            "lens.conf:",
            "controls:",
        ] {
            assert!(r.contains(want), "missing {want:?} in\n{r}");
        }
    }

    #[test]
    fn banner_and_hud_lines() {
        let mut state = State::new(Pattern::ALL.to_vec());
        let p = params();
        let b = banner(&state, &p);
        assert!(
            b.starts_with("PATTERN [1/11] STRAIGHT-LINE GRID | eye both | auto off"),
            "{b}"
        );
        assert!(b.contains("tests the distortion model"));
        let steps = |pat: Pattern, _d| pat.steps(&p);
        state.handle(crate::state::Event::Goto(2), &steps);
        let h = hud_lines(&state, &p);
        assert!(
            h[0].starts_with("[3/11] PER-CHANNEL GRIDS STEP 1/4 ALL (WHITE)"),
            "{h:?}"
        );
        assert!(h.iter().all(|l| l.len() <= 64));
        assert!(h.iter().any(|l| l.contains("COLOURED FRINGES")));
        assert_eq!(h.last().unwrap(), HUD_CONTROLS[1], "{h:?}");
        // only characters the font has
        for l in &h {
            for c in l.chars() {
                assert!(
                    c.is_ascii_uppercase() || c.is_ascii_digit() || " [](),./%:|-?".contains(c),
                    "{c:?} in {l}"
                );
            }
        }
    }
}
