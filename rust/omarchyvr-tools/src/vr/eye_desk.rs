//! Eye tracking in desk mode: "silently on" by default.
//!
//! * **silent** (default when the eye build and a calibration exist): `PIMAX_EYE=1` for Monado.
//! * **forced** (`--eye`): the same, plus the eye build unless another build is given. (Before 2026-10-17 `--eye`
//!   meant "the gaze pointer as the other conf.d files set it", and the app's `wayvr-desk.conf` defaulted
//!   `gaze_pointer` to true: that is how the eyes became a mouse.)
//! * **off** (`--no-eye`, or no eye build / no calibration): no `PIMAX_EYE`.
//!
//! In EVERY mode `conf.d/55-eye.yaml` is written, and it is the only file that sets WayVR's gaze input keys
//! (`gaze_pointer`, `gaze_target`, `gaze_over_controllers`, `gaze_dwell_ms`, `gaze_refine`) and the learning keys
//! (`gaze_learn`, `gaze_learn_key`). It sorts after the old hand-made
//! `20-gaze.yaml` and the app's `50-pimax-app.yaml`, so nothing older can turn gaze input on. The gaze is only a visual
//! point (`gaze_laser`, the dot) unless the ONE switch `gaze_mouse=on` in `eye.conf` is set (app: Tracking > Eyes >
//! "Eyes as a mouse", default off; user decision 2026-10-17).
//!
//! WayVR ignores keys it does not know, so an older WayVR simply has no laser (and, in silent mode, no pointer).
//! Calibration (`calibration=` in `eye.conf`, docs/EYE.md): with the user's own `eye-calibration.toml` (or `own`) the
//! host-side calibration is applied and nothing extra is sent. Otherwise (`builtin`, or `auto` without an own file)
//! the BASE start list's built-in test calibration (the developer's eyes) is sent to the chip: `PIMAX_EYE_CAL=1`, and
//! `PIMAX_EYE_CALIBRATION=none` so no host-side file is mixed in.
use std::io;
use std::path::{Path, PathBuf};

use super::config::StartOptions;
pub use omarchyvr_config::eye::EyeSettings;
use omarchyvr_config::eye::CalSource;

/// The conf.d file (sorts after `20-gaze.yaml` and the app's `50-pimax-app.yaml`: its gaze keys win).
pub const CONF_NAME: &str = "55-eye.yaml";

/// How desk mode uses the eye tracker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EyeMode {
    Off,
    /// Eye tracking on (the default when possible): a visual point only unless `gaze_mouse=on`.
    Silent,
    /// `--eye`: eye tracking on with the eye build; the gaze input is the same as in `Silent`.
    Forced,
}

/// `~/.config/omarchyvr/eye-calibration.toml` (XDG_CONFIG_HOME respected).
pub fn calibration_path(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    let config = env("XDG_CONFIG_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(env("HOME").unwrap_or_else(|| "/root".into())).join(".config")
    });
    config.join("omarchyvr/eye-calibration.toml")
}

/// The mode, from the flags and what exists. `default_is_eye_build`: the Monado build in use is the eye build
/// (`~/.local/lib/omarchyvr/monado`, the default when built) and its monado-service exists.
/// `calibrated`: a calibration is available (the user's own file, or the built-in one is the one in use).
pub fn decide(eye: bool, no_eye: bool, default_is_eye_build: bool, calibrated: bool) -> (EyeMode, Option<String>) {
    if no_eye {
        return (EyeMode::Off, None);
    }
    if eye {
        return (EyeMode::Forced, None);
    }
    match (default_is_eye_build, calibrated) {
        (true, true) => (EyeMode::Silent, None),
        (false, _) => (EyeMode::Off, Some("eye tracking off: the Monado build is not the installed desk build".into())),
        (true, false) => (
            EyeMode::Off,
            Some("eye tracking off: no ~/.config/omarchyvr/eye-calibration.toml (calibrate in the Pimax app, Eye tab)".into()),
        ),
    }
}

/// Apply the mode to the Monado options (`PIMAX_EYE=1`, and for `--eye` the eye build unless one was given).
pub fn configure(mode: EyeMode, vr: &mut StartOptions, eye_build: &Path, build_given: bool, source: CalSource) {
    if mode == EyeMode::Off {
        return;
    }
    if mode == EyeMode::Forced && !build_given {
        vr.monado_build = eye_build.to_path_buf();
    }
    if !vr.extra_env.iter().any(|(k, _)| k == "PIMAX_EYE") {
        vr.extra_env.push(("PIMAX_EYE".into(), "1".into()));
    }
    if source == CalSource::Builtin {
        for (k, v) in [("PIMAX_EYE_CAL", "1"), ("PIMAX_EYE_CALIBRATION", "none")] {
            if !vr.extra_env.iter().any(|(e, _)| e == k) {
                vr.extra_env.push((k.into(), v.into()));
            }
        }
    }
}

/// Parse `--eye` / `--no-eye` and configure Monado. Returns the mode and a note for the start output.
pub fn parse(
    eye: bool,
    no_eye: bool,
    vr: &mut StartOptions,
    eye_build: &Path,
    build_given: bool,
    env: &dyn Fn(&str) -> Option<String>,
) -> (EyeMode, Option<String>) {
    let default_is_eye_build = vr.monado_build == eye_build && vr.service_path().exists();
    let own = calibration_path(env).is_file();
    let set = EyeSettings::load(&omarchyvr_config::eye::path_from(env));
    let source = set.calibration.source(own);
    let calibrated = own || source == CalSource::Builtin;
    let (mode, note) = decide(eye, no_eye, default_is_eye_build, calibrated);
    configure(mode, vr, eye_build, build_given, source);
    (mode, note)
}

/// The calibration fingerprint WayVR files its gaze lessons under (`gaze_learn_key`): the first 12 hex digits of a
/// 64-bit FNV-1a hash of the calibration file's bytes; "" when there is no file. A new calibration gives a new key,
/// so lessons learned with the old one are discarded.
pub fn calibration_key(path: &Path) -> String {
    let Ok(bytes) = std::fs::read(path) else { return String::new() };
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")[..12].to_string()
}

/// The conf.d YAML for a mode. Always a file: every gaze input key is set here, so an older conf.d file
/// (`20-gaze.yaml`, `50-pimax-app.yaml`) cannot turn gaze input on. Gaze input (`gaze_pointer`: the eyes move the
/// pointer and a trigger clicks where you look; `gaze_target`: `wayvrctl gaze`, which voice "this" reads) only with
/// eye tracking on AND `gaze_mouse=on`; never a dwell click. `gaze_learn` (lessons from clicks; never input) only
/// with eye tracking on and `learn=on`; `gaze_refine` (hold-to-refine) is part of "Eyes as a mouse": same as the
/// mouse. `cal_key`: see [`calibration_key`].
pub fn conf_yaml(mode: EyeMode, set: &EyeSettings, cal_key: &str) -> String {
    let on = mode != EyeMode::Off;
    let mouse = on && set.mouse;
    let learn = on && set.learn;
    format!(
        "# written by `omarchyvr desk start` from ~/.config/omarchyvr/eye.conf (the Pimax app, Tracking > Eyes); do not edit.\n\
         # The gaze is only a visual point unless \"Eyes as a mouse\" (gaze_mouse=on) is set.\n\
         gaze_pointer: {mouse}\ngaze_target: {mouse}\ngaze_over_controllers: {mouse}\ngaze_dwell_ms: 0\n\
         gaze_laser: {}\ngaze_laser_style: dot\ngaze_laser_alpha: 0.35\ngaze_fixation_ms: 200\n\
         gaze_learn: {learn}\ngaze_refine: {mouse}\ngaze_learn_key: \"{cal_key}\"\n",
        on && set.dot
    )
}

/// Write `conf.d/55-eye.yaml` (every mode, see [`conf_yaml`]). Returns a note for the start output (None when off).
pub fn apply_conf(wayvr_home: &Path, mode: EyeMode, set: &EyeSettings, cal_key: &str) -> io::Result<Option<String>> {
    let conf_d = wayvr_home.join("config/wayvr/conf.d");
    let f = conf_d.join(CONF_NAME);
    let y = conf_yaml(mode, set, cal_key);
    std::fs::create_dir_all(&conf_d)?;
    let tmp = conf_d.join(".55-eye.yaml.tmp");
    std::fs::write(&tmp, &y)?;
    std::fs::rename(&tmp, &f)?;
    Ok(match (mode, set.mouse, set.dot) {
        (EyeMode::Off, ..) => None,
        (_, true, _) => Some("eye tracking on, Eyes as a mouse ON (gaze_mouse=on): your eyes move the pointer and can click".into()),
        (_, false, true) => Some("eye tracking on, visual only: a dim gaze dot (never a pointer; --no-eye turns it off)".into()),
        (_, false, false) => Some("eye tracking on, visual only (gaze dot off in eye.conf)".into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decide_flags_and_prerequisites() {
        assert_eq!(decide(false, false, true, true).0, EyeMode::Silent);
        assert_eq!(decide(true, false, false, false).0, EyeMode::Forced, "--eye forces eye tracking on");
        assert_eq!(decide(true, true, true, true).0, EyeMode::Off, "--no-eye wins");
        let (m, n) = decide(false, false, true, false);
        assert!(m == EyeMode::Off && n.unwrap().contains("calibration"));
        let (m, n) = decide(false, false, false, true);
        assert!(m == EyeMode::Off && n.unwrap().contains("installed desk build"));
    }

    fn opts() -> StartOptions {
        let env = |k: &str| (k == "HOME").then(|| "/home/u".to_string());
        crate::vr::parse_options(&mut crate::args::Args(Vec::new()), &env).unwrap()
    }

    #[test]
    fn configure_sets_the_variable_once_and_the_build_only_for_eye() {
        let mut vr = opts();
        vr.monado_build = PathBuf::from("/other");
        configure(EyeMode::Silent, &mut vr, Path::new("/eye"), false, CalSource::Own);
        configure(EyeMode::Silent, &mut vr, Path::new("/eye"), false, CalSource::Own);
        assert_eq!(vr.extra_env.iter().filter(|(k, v)| k == "PIMAX_EYE" && v == "1").count(), 1);
        assert_eq!(vr.monado_build, PathBuf::from("/other"));
        configure(EyeMode::Forced, &mut vr, Path::new("/eye"), true, CalSource::Own);
        assert_eq!(vr.monado_build, PathBuf::from("/other"), "a given build is kept");
        configure(EyeMode::Forced, &mut vr, Path::new("/eye"), false, CalSource::Own);
        assert_eq!(vr.monado_build, PathBuf::from("/eye"));
        let mut off = opts();
        configure(EyeMode::Off, &mut off, Path::new("/eye"), false, CalSource::Builtin);
        assert!(!off.extra_env.iter().any(|(k, _)| k == "PIMAX_EYE"));
        assert!(!format!("{:?}", vr.extra_env).contains("PIMAX_EYE_CAL"), "own calibration: no device-side calibration");
        assert!(!format!("{:?}", off.extra_env).contains("PIMAX_EYE_CAL"), "off sets nothing");
    }

    #[test]
    fn builtin_calibration_sets_the_device_side_variables_once() {
        let mut vr = opts();
        configure(EyeMode::Silent, &mut vr, Path::new("/eye"), false, CalSource::Builtin);
        configure(EyeMode::Silent, &mut vr, Path::new("/eye"), false, CalSource::Builtin);
        for (k, v) in [("PIMAX_EYE", "1"), ("PIMAX_EYE_CAL", "1"), ("PIMAX_EYE_CALIBRATION", "none")] {
            assert_eq!(vr.extra_env.iter().filter(|(a, b)| a == k && b == v).count(), 1, "{k}");
        }
    }

    #[test]
    fn conf_files_have_unique_keys_and_no_gaze_input_by_default() {
        // the user's rule (2026-10-17): by default the gaze is only a visual point, in every mode, --eye included
        for mode in [EyeMode::Silent, EyeMode::Forced, EyeMode::Off] {
            let s = conf_yaml(mode, &EyeSettings::default(), "");
            for k in ["gaze_pointer: false\n", "gaze_target: false\n", "gaze_over_controllers: false\n", "gaze_dwell_ms: 0\n"] {
                assert!(s.contains(k), "{mode:?} lacks {k}: {s}");
            }
            assert_eq!(s.contains("gaze_laser: true\n"), mode != EyeMode::Off, "{mode:?}: the dot only with eye tracking on");
            assert!(s.contains("gaze_refine: false\n"), "{mode:?}: no refine without Eyes as a mouse");
            assert_eq!(s.contains("gaze_learn: true\n"), mode != EyeMode::Off, "{mode:?}: learning only with eye tracking on");
        }
        // the one explicit switch
        let m = conf_yaml(EyeMode::Silent, &EyeSettings { dot: false, mouse: true, ..EyeSettings::default() }, "abc");
        assert!(m.contains("gaze_refine: true\n") && m.contains("gaze_learn_key: \"abc\"\n"));
        assert!(m.contains("gaze_pointer: true\n") && m.contains("gaze_target: true\n") && m.contains("gaze_laser: false\n") && m.contains("gaze_dwell_ms: 0\n"));
        assert!(conf_yaml(EyeMode::Off, &EyeSettings { mouse: true, ..EyeSettings::default() }, "").contains("gaze_pointer: false\n"), "no eye tracking, no gaze mouse");
        let off = conf_yaml(EyeMode::Off, &EyeSettings { mouse: true, ..EyeSettings::default() }, "k");
        assert!(off.contains("gaze_refine: false\n") && off.contains("gaze_learn: false\n"));
        let nolearn = conf_yaml(EyeMode::Silent, &EyeSettings { learn: false, ..EyeSettings::default() }, "");
        assert!(nolearn.contains("gaze_learn: false\n") && nolearn.contains("gaze_pointer: false\n"), "learn off is independent");
        for y in [conf_yaml(EyeMode::Silent, &EyeSettings::default(), ""), m] {
            let keys: Vec<&str> = y.lines().filter(|l| !l.starts_with('#')).map(|l| l.split(':').next().unwrap()).collect();
            let mut u = keys.clone();
            u.sort();
            u.dedup();
            assert_eq!(u.len(), keys.len(), "{y}");
        }

    }

    #[test]
    fn apply_writes_and_removes() {
        let h = crate::vr::test_dir("eye-desk");
        let f = h.join("config/wayvr/conf.d").join(CONF_NAME);
        assert!(apply_conf(&h, EyeMode::Silent, &EyeSettings::default(), "").unwrap().unwrap().contains("visual only"));
        assert_eq!(std::fs::read_to_string(&f).unwrap(), conf_yaml(EyeMode::Silent, &EyeSettings::default(), ""));
        // an old hand-made 20-gaze.yaml (gaze_pointer: true) sorts before 55-eye.yaml, which is written even when off
        std::fs::write(h.join("config/wayvr/conf.d/20-gaze.yaml"), "gaze_pointer: true\ngaze_over_controllers: true\n").unwrap();
        assert!(apply_conf(&h, EyeMode::Off, &EyeSettings::default(), "").unwrap().is_none());
        assert!(std::fs::read_to_string(&f).unwrap().contains("gaze_pointer: false\n"));
        assert!(CONF_NAME > "20-gaze.yaml" && CONF_NAME > super::super::wayvr_conf::CONF_NAME, "sorts last of the gaze files");
        let _ = std::fs::remove_dir_all(h);
    }

    #[test]
    fn desk_start_parses_no_eye_and_keeps_eye() {
        use crate::args::Args;
        let env = |k: &str| (k == "HOME").then(|| "/home/u".to_string());
        let mut a = Args(vec!["--no-eye".into()]);
        let o = crate::vr::desk::parse_desk(&mut a, &env).unwrap();
        assert!(a.0.is_empty() && o.eye_mode == EyeMode::Off && o.eye_note.is_none());
        let mut a = Args(vec!["--eye".into()]);
        let o = crate::vr::desk::parse_desk(&mut a, &env).unwrap();
        assert_eq!(o.eye_mode, EyeMode::Forced);
        assert!(o.vr.extra_env.contains(&("PIMAX_EYE".into(), "1".into())));
        // no eye build under the fake home: off, with a reason
        let o = crate::vr::desk::parse_desk(&mut Args(Vec::new()), &env).unwrap();
        assert!(o.eye_mode == EyeMode::Off && o.eye_note.is_some());
    }

    #[test]
    fn calibration_key_follows_the_file_contents() {
        let d = crate::vr::test_dir("eye-key");
        let f = d.join("eye-calibration.toml");
        assert_eq!(calibration_key(&f), "", "no file, no key");
        std::fs::write(&f, "a = 1\n").unwrap();
        let k1 = calibration_key(&f);
        assert_eq!(k1.len(), 12);
        assert_eq!(k1, calibration_key(&f), "stable");
        std::fs::write(&f, "a = 2\n").unwrap();
        assert_ne!(k1, calibration_key(&f), "a new calibration, a new key");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn calibration_path_follows_xdg() {
        let env = |k: &str| match k {
            "HOME" => Some("/home/u".to_string()),
            _ => None,
        };
        assert_eq!(calibration_path(&env), PathBuf::from("/home/u/.config/omarchyvr/eye-calibration.toml"));
        let xdg = |k: &str| (k == "XDG_CONFIG_HOME").then(|| "/x".to_string());
        assert_eq!(calibration_path(&xdg), PathBuf::from("/x/omarchyvr/eye-calibration.toml"));
    }
}
