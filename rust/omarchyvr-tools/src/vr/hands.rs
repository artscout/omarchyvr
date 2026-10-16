//! `--hands` for `omarchyvr desk start` and `omarchyvr vr start`: camera hand tracking (Monado's Mercury on the two front
//! tracking cameras), docs/HANDS.md. Off by default.
//!
//! Design (the simplest one that cannot break a session):
//! * One Monado build carries everything (ai/CLAUDE.md section 3), hands included: `--hands` only sets the
//!   variables below on the build the start uses anyway (`~/.local/lib/omarchyvr/monado`). Another build only when it is
//!   named: `--hands-build DIR` or `PIMAX_HANDS_BUILD` (service binary AND runtime manifest move together). An
//!   explicit `--monado-build` / `PIMAX_MONADO_BUILD` / `--service-bin` wins and is used as it is.
//! * Hands are on only when that monado-service has the Mercury code (the `PIMAX_HT_HZ` option name in the binary)
//!   and the two greyscale models are installed. Otherwise: a warning, and the session starts as if `--hands` was not
//!   given (no `PIMAX_HANDS`). Hands never stop a start.
//! * Monado gets `PIMAX_HANDS=mercury` and `PIMAX_HT_STATS=1` (a stats line in monado.log every 5 s); `--hands-roles`
//!   adds `PIMAX_HT_ROLES=hands` (the hands take the left/right controller roles, so apps that only read
//!   controllers see pinch = trigger). The caller's own `PIMAX_HT_*` values are inherited and win.
//! * `--hands` needs `--6dof`: Mercury reads the stereo pairs from the camera broker (shared with passthrough and
//!   SLAM), and the hands are world-locked through the 6DoF head pose.
//! * A leaf: nothing depends on it and a failure inside Monado only disables the hands (Monado logs it and goes on).
//!   The flags are in `start-argv.txt` like every start flag, so `restart`, the watchdog and omarchyvrd keep them.
use super::config::StartOptions;
use crate::args::Args;
use crate::CliError;
use std::path::{Path, PathBuf};

/// The separate hands build of the first experiments (relative to `$HOME`); only used when named.
pub const DEFAULT_BUILD: &str = "src/monado-ht/build-ht";
/// An option name only the Mercury-enabled Pimax driver has (the debug option strings are in the binary).
pub const MARKER: &[u8] = b"PIMAX_HT_HZ\0";
/// The models Mercury loads for greyscale cameras.
pub const MODELS: [&str; 2] = ["grayscale_detection_160x160.onnx", "grayscale_keypoint_jan18.onnx"];

/// The outcome of `--hands`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Hands {
    /// Camera hands are on for this start.
    pub on: bool,
    /// `--hands` was given (on or not).
    pub asked: bool,
    /// What to tell the user (Ok note when on, the reason when it fell back).
    pub note: Option<String>,
}

/// The hands build: `--hands-build`, else `PIMAX_HANDS_BUILD`, else `<src dir>/monado-ht/build-ht` (src dir: `paths.toml`, default `~/src`).
pub fn default_build(opt: Option<String>, env: &dyn Fn(&str) -> Option<String>, home: &Path) -> PathBuf {
    opt.or_else(|| env("PIMAX_HANDS_BUILD").filter(|v| !v.is_empty()))
        .map(PathBuf::from)
        .unwrap_or_else(|| omarchyvr_config::paths::src_dir(home).join(DEFAULT_BUILD.trim_start_matches("src/")))
}

/// The directory Monado reads the models from (same search as `u_file_get_hand_tracking_models_dir`).
pub fn models_dir(env: &dyn Fn(&str) -> Option<String>, home: &Path) -> Option<PathBuf> {
    let mut c = Vec::new();
    if let Some(x) = env("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        c.push(PathBuf::from(x));
    }
    c.push(home.join(".local/share"));
    c.push(PathBuf::from("/usr/local/share"));
    c.push(PathBuf::from("/usr/share"));
    c.into_iter().map(|d| d.join("monado/hand-tracking-models")).find(|d| d.is_dir())
}

/// Why the service binary cannot run Mercury, if it cannot.
pub fn service_problem(service: &Path) -> Option<String> {
    match std::fs::read(service) {
        Err(e) => Some(format!("{} not usable ({e}); build it: ninja -C {}", service.display(), build_dir_of(service).display())),
        Ok(data) if !data.windows(MARKER.len()).any(|w| w == MARKER) => {
            Some(format!("{} has no Mercury hand tracking (no PIMAX_HT_HZ in it: rebuild the one full build with camera hands, docs/HANDS.md)", service.display()))
        }
        Ok(_) => None,
    }
}

fn build_dir_of(service: &Path) -> PathBuf {
    // <build>/src/xrt/targets/service/monado-service
    service.ancestors().nth(5).map(Path::to_path_buf).unwrap_or_else(|| service.to_path_buf())
}

/// Why the models are not usable, if they are not.
pub fn models_problem(env: &dyn Fn(&str) -> Option<String>, home: &Path) -> Option<String> {
    let Some(dir) = models_dir(env, home) else {
        return Some("no monado/hand-tracking-models directory (install: docs/HANDS.md)".into());
    };
    MODELS
        .iter()
        .map(|m| dir.join(m))
        .find(|p| !p.is_file())
        .map(|p| format!("{} is missing (install: docs/HANDS.md)", p.display()))
}

/// Parse `--hands`, `--hands-roles`, `--hands-build DIR` and configure Monado. `build_given`: `--monado-build`,
/// `PIMAX_MONADO_BUILD` or `--service-bin` chose the build explicitly. Call after every other option that may move
/// `vr.monado_build` (6DoF, eye).
pub fn parse(
    a: &mut Args,
    env: &dyn Fn(&str) -> Option<String>,
    vr: &mut StartOptions,
    home: &Path,
    build_given: bool,
    sixdof: bool,
) -> Result<Hands, CliError> {
    let on = a.flag("--hands");
    let roles = a.flag("--hands-roles");
    let build_opt = a.opt("--hands-build")?;
    if !on {
        if roles || build_opt.is_some() {
            return Err(CliError::usage("--hands-roles and --hands-build need --hands"));
        }
        return Ok(Hands::default());
    }
    if !sixdof {
        return Err(CliError::usage(
            "--hands needs --6dof (Mercury reads the stereo pairs from the camera broker; the hands are world-locked \
             through the 6DoF head pose)",
        ));
    }
    let explicit = build_given || vr.service_bin.is_some();
    let mut cand = vr.clone();
    // one build carries everything (ai/CLAUDE.md section 3): --hands only sets the variables on the build the
    // start uses; another build only when it is named (--hands-build DIR or PIMAX_HANDS_BUILD)
    let named = build_opt.or_else(|| env("PIMAX_HANDS_BUILD").filter(|v| !v.is_empty()));
    if let (false, Some(b)) = (explicit, named) {
        cand.monado_build = PathBuf::from(b);
    }
    let problem = service_problem(&cand.service_path()).or_else(|| models_problem(env, home));
    if let Some(p) = problem {
        return Ok(Hands { on: false, asked: true, note: Some(format!("--hands: {p}; starting WITHOUT camera hands")) });
    }
    *vr = cand;
    let mut set = |k: &str, v: &str| {
        if env(k).is_none() {
            vr.extra_env.retain(|(n, _)| n != k);
            vr.extra_env.push((k.into(), v.into()));
        }
    };
    set("PIMAX_HANDS", "mercury");
    set("PIMAX_HT_STATS", "1");
    if roles {
        set("PIMAX_HT_ROLES", "hands");
    }
    Ok(Hands {
        on: true,
        asked: true,
        note: Some(format!(
            "camera hands on (Mercury, {}): Monado from {}{}",
            if roles { "hands take the controller roles" } else { "XR_EXT_hand_tracking + hand_interaction" },
            vr.monado_build.display(),
            if explicit { " (as given)" } else { "" }
        )),
    })
}

/// Print the note: stdout when on, a stderr warning when `--hands` fell back.
pub fn print_note(h: &Hands, who: &str) {
    match (&h.note, h.on) {
        (Some(n), true) => vout!("{who}: {n}"),
        (Some(n), false) => verr!("{who}: WARNING: {n}"),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vr::parse_options;

    fn world(name: &str, marker: bool, models: bool) -> PathBuf {
        let d = crate::vr::test_dir(name);
        let svc = d.join("home/.local/lib/omarchyvr/monado/src/xrt/targets/service/monado-service");
        std::fs::create_dir_all(svc.parent().unwrap()).unwrap();
        std::fs::write(&svc, if marker { &b"x\0PIMAX_SLAM\0PIMAX_HT_HZ\0y"[..] } else { &b"x\0PIMAX_SLAM\0y"[..] }).unwrap();
        if models {
            let m = d.join("home/.local/share/monado/hand-tracking-models");
            std::fs::create_dir_all(&m).unwrap();
            for f in MODELS {
                std::fs::write(m.join(f), "onnx").unwrap();
            }
        }
        d
    }

    fn run(d: &Path, args: &[&str], extra: &[(&str, &str)], build_given: bool, sixdof: bool) -> (Result<Hands, CliError>, StartOptions) {
        let home = d.join("home");
        let h = home.display().to_string();
        let ex: Vec<(String, String)> = extra.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let env = move |k: &str| {
            if k == "HOME" {
                return Some(h.clone());
            }
            ex.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone())
        };
        let mut a = Args(args.iter().map(|s| s.to_string()).collect());
        let mut vr = parse_options(&mut Args(Vec::new()), &env).unwrap();
        vr.monado_build = home.join(".local/lib/omarchyvr/monado");
        let r = parse(&mut a, &env, &mut vr, &home, build_given, sixdof);
        if r.is_ok() {
            assert!(a.0.is_empty(), "left over: {:?}", a.0);
        }
        (r, vr)
    }

    fn val<'a>(vr: &'a StartOptions, k: &str) -> Option<&'a str> {
        vr.extra_env.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }

    #[test]
    fn off_by_default_and_sub_flags_need_hands() {
        let d = world("hands-off", true, true);
        let (r, vr) = run(&d, &[], &[], false, true);
        assert_eq!(r.unwrap(), Hands::default());
        assert!(val(&vr, "PIMAX_HANDS").is_none());
        assert!(vr.monado_build.ends_with(".local/lib/omarchyvr/monado"));
        assert!(run(&d, &["--hands-roles"], &[], false, true).0.is_err());
        assert!(run(&d, &["--hands-build", "/x"], &[], false, true).0.is_err());
    }

    #[test]
    fn hands_need_6dof() {
        let d = world("hands-6dof", true, true);
        let e = run(&d, &["--hands"], &[], false, false).0.unwrap_err();
        assert!(e.msg.contains("--6dof") && e.code == 2, "{}", e.msg);
    }

    #[test]
    fn hands_set_the_variables_on_the_one_build() {
        let d = world("hands-on", true, true);
        // a leftover separate hands build is not switched to
        let ht = d.join("home").join(DEFAULT_BUILD).join("src/xrt/targets/service/monado-service");
        std::fs::create_dir_all(ht.parent().unwrap()).unwrap();
        std::fs::write(&ht, b"PIMAX_HT_HZ\0").unwrap();
        let (r, vr) = run(&d, &["--hands", "--hands-roles"], &[], false, true);
        let h = r.unwrap();
        assert!(h.on && h.asked);
        assert!(vr.monado_build.ends_with(".local/lib/omarchyvr/monado"), "{:?}", vr.monado_build);
        assert_eq!(val(&vr, "PIMAX_HANDS"), Some("mercury"));
        assert_eq!(val(&vr, "PIMAX_HT_STATS"), Some("1"));
        assert_eq!(val(&vr, "PIMAX_HT_ROLES"), Some("hands"));
        let (_, vr) = run(&d, &["--hands"], &[], false, true);
        assert!(val(&vr, "PIMAX_HT_ROLES").is_none(), "roles only with --hands-roles");
    }

    #[test]
    fn caller_values_win() {
        let d = world("hands-env", true, true);
        let (_, vr) = run(&d, &["--hands", "--hands-roles"], &[("PIMAX_HT_ROLES", "tracking"), ("PIMAX_HT_STATS", "0")], false, true);
        assert!(val(&vr, "PIMAX_HT_ROLES").is_none() && val(&vr, "PIMAX_HT_STATS").is_none(), "inherited, not overridden");
        assert_eq!(val(&vr, "PIMAX_HANDS"), Some("mercury"));
    }

    #[test]
    fn missing_build_or_models_warn_and_start_without_hands() {
        let d = crate::vr::test_dir("hands-nobuild");
        let (r, vr) = run(&d, &["--hands"], &[], false, true);
        let h = r.unwrap();
        assert!(!h.on && h.asked && h.note.as_deref().unwrap().contains("WITHOUT camera hands"));
        assert!(vr.monado_build.ends_with(".local/lib/omarchyvr/monado"), "normal build kept");
        assert!(val(&vr, "PIMAX_HANDS").is_none());

        let d = world("hands-nomarker", false, true);
        let h = run(&d, &["--hands"], &[], false, true).0.unwrap();
        assert!(!h.on && h.note.unwrap().contains("no Mercury"));

        let d = world("hands-nomodels", true, false);
        let (r, vr) = run(&d, &["--hands"], &[], false, true);
        let h = r.unwrap();
        assert!(!h.on && h.note.unwrap().contains("hand-tracking-models"), "models missing");
        assert!(val(&vr, "PIMAX_HANDS").is_none());
    }

    #[test]
    fn hands_build_option_and_env() {
        let d = world("hands-opt", true, true);
        let other = d.join("other");
        let svc = other.join("src/xrt/targets/service/monado-service");
        std::fs::create_dir_all(svc.parent().unwrap()).unwrap();
        std::fs::write(&svc, b"PIMAX_HT_HZ\0").unwrap();
        let o = other.display().to_string();
        let (_, vr) = run(&d, &["--hands", "--hands-build", &o], &[], false, true);
        assert_eq!(vr.monado_build, other);
        let (_, vr) = run(&d, &["--hands"], &[("PIMAX_HANDS_BUILD", &o)], false, true);
        assert_eq!(vr.monado_build, other);
    }

    #[test]
    fn explicit_build_is_used_as_given() {
        let d = crate::vr::test_dir("hands-explicit");
        let m = d.join("home/.local/share/monado/hand-tracking-models");
        std::fs::create_dir_all(&m).unwrap();
        for f in MODELS {
            std::fs::write(m.join(f), "onnx").unwrap();
        }
        // the explicit build (the desk build here) does not exist: warn, keep it, no hands
        let (r, vr) = run(&d, &["--hands"], &[], true, true);
        assert!(!r.unwrap().on);
        assert!(vr.monado_build.ends_with(".local/lib/omarchyvr/monado"));
        // an explicit build with Mercury: kept, hands on
        let eb = d.join("home/.local/lib/omarchyvr/monado/src/xrt/targets/service/monado-service");
        std::fs::create_dir_all(eb.parent().unwrap()).unwrap();
        std::fs::write(&eb, b"PIMAX_HT_HZ\0").unwrap();
        let (r, vr) = run(&d, &["--hands"], &[], true, true);
        let h = r.unwrap();
        assert!(h.on && h.note.unwrap().contains("(as given)"));
        assert!(vr.monado_build.ends_with(".local/lib/omarchyvr/monado"));
    }
}
