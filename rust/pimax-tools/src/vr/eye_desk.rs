//! Eye tracking in desk mode: "silently on" by default.
//!
//! * **silent** (default when the eye build and `~/.config/pimax/eye-calibration.toml` exist): `PIMAX_EYE=1` for
//!   Monado and `conf.d/55-eye.yaml` for WayVR with `gaze_pointer: false`, `gaze_target: true`, `gaze_laser: true`:
//!   the gaze never hovers, clicks or moves the mouse; a small dim dot shows where the eyes land and
//!   `wayvrctl gaze` / `pimax-voice gaze` report what is looked at (for "close this", "move that to workspace 2").
//! * **pointer** (`--eye`, the old meaning): `PIMAX_EYE=1` (+ the eye build unless another build is given); the gaze
//!   pointer stays as the other conf.d files set it (`20-gaze.yaml`, the app's `50-pimax-app.yaml`); 55-eye.yaml
//!   only adds `gaze_target: true` so `wayvrctl gaze` works too.
//! * **off** (`--no-eye`, or no eye build / no calibration): no `PIMAX_EYE`, 55-eye.yaml removed.
//!
//! WayVR ignores keys it does not know, so an older WayVR simply has no laser (and, in silent mode, no pointer).
//! No headset bytes are involved: the device-side calibration (`PIMAX_EYE_CAL`) is never set here.
use std::io;
use std::path::{Path, PathBuf};

use super::config::StartOptions;

/// The conf.d file (sorts after the app's `50-pimax-app.yaml`, so silent mode wins over its `gaze_pointer`).
pub const CONF_NAME: &str = "55-eye.yaml";

/// How desk mode uses the eye tracker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EyeMode {
    Off,
    /// Silent tracking + laser, no pointer (the default when possible).
    Silent,
    /// `--eye`: the gaze pointer as configured, plus silent tracking for `wayvrctl gaze`.
    Pointer,
}

/// `~/.config/pimax/eye-calibration.toml` (XDG_CONFIG_HOME respected).
pub fn calibration_path(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    let config = env("XDG_CONFIG_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(env("HOME").unwrap_or_else(|| "/root".into())).join(".config")
    });
    config.join("pimax/eye-calibration.toml")
}

/// The mode, from the flags and what exists. `default_is_eye_build`: the Monado build in use is the eye build
/// (`~/src/monado-eye/build`, the default when built) and its monado-service exists.
pub fn decide(eye: bool, no_eye: bool, default_is_eye_build: bool, calibrated: bool) -> (EyeMode, Option<String>) {
    if no_eye {
        return (EyeMode::Off, None);
    }
    if eye {
        return (EyeMode::Pointer, None);
    }
    match (default_is_eye_build, calibrated) {
        (true, true) => (EyeMode::Silent, None),
        (false, _) => (EyeMode::Off, Some("eye tracking off: the Monado build is not ~/src/monado-eye/build".into())),
        (true, false) => (
            EyeMode::Off,
            Some("eye tracking off: no ~/.config/pimax/eye-calibration.toml (calibrate in the Pimax app, Eye tab)".into()),
        ),
    }
}

/// Apply the mode to the Monado options (`PIMAX_EYE=1`, and for `--eye` the eye build unless one was given).
pub fn configure(mode: EyeMode, vr: &mut StartOptions, eye_build: &Path, build_given: bool) {
    if mode == EyeMode::Off {
        return;
    }
    if mode == EyeMode::Pointer && !build_given {
        vr.monado_build = eye_build.to_path_buf();
    }
    if !vr.extra_env.iter().any(|(k, _)| k == "PIMAX_EYE") {
        vr.extra_env.push(("PIMAX_EYE".into(), "1".into()));
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
    let calibrated = calibration_path(env).is_file();
    let (mode, note) = decide(eye, no_eye, default_is_eye_build, calibrated);
    configure(mode, vr, eye_build, build_given);
    (mode, note)
}

/// The conf.d YAML for a mode (None = no file).
pub fn conf_yaml(mode: EyeMode) -> Option<&'static str> {
    match mode {
        EyeMode::Off => None,
        EyeMode::Silent => Some(
            "# written by `pimax desk start` (eye tracking silently on; --no-eye turns it off, --eye = gaze pointer); do not edit\n\
             gaze_pointer: false\ngaze_target: true\ngaze_laser: true\ngaze_laser_style: dot\ngaze_laser_alpha: 0.35\n\
             gaze_fixation_ms: 200\n",
        ),
        EyeMode::Pointer => Some(
            "# written by `pimax desk start --eye` (gaze pointer as configured, plus `wayvrctl gaze`); do not edit\n\
             gaze_target: true\ngaze_fixation_ms: 200\n",
        ),
    }
}

/// Write or remove `conf.d/55-eye.yaml`. Returns a note for the start output.
pub fn apply_conf(wayvr_home: &Path, mode: EyeMode) -> io::Result<Option<String>> {
    let conf_d = wayvr_home.join("config/wayvr/conf.d");
    let f = conf_d.join(CONF_NAME);
    let Some(y) = conf_yaml(mode) else {
        if f.exists() {
            std::fs::remove_file(&f)?;
        }
        return Ok(None);
    };
    std::fs::create_dir_all(&conf_d)?;
    let tmp = conf_d.join(".55-eye.yaml.tmp");
    std::fs::write(&tmp, y)?;
    std::fs::rename(&tmp, &f)?;
    Ok(Some(match mode {
        EyeMode::Silent => "eye tracking silently on: a dim gaze dot, `wayvrctl gaze` for voice (no pointer; --no-eye turns it off)".into(),
        _ => "eye tracking on with the gaze pointer (--eye); `wayvrctl gaze` reports the target".into(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decide_flags_and_prerequisites() {
        assert_eq!(decide(false, false, true, true).0, EyeMode::Silent);
        assert_eq!(decide(true, false, false, false).0, EyeMode::Pointer, "--eye keeps its meaning");
        assert_eq!(decide(true, true, true, true).0, EyeMode::Off, "--no-eye wins");
        let (m, n) = decide(false, false, true, false);
        assert!(m == EyeMode::Off && n.unwrap().contains("calibration"));
        let (m, n) = decide(false, false, false, true);
        assert!(m == EyeMode::Off && n.unwrap().contains("monado-eye"));
    }

    fn opts() -> StartOptions {
        let env = |k: &str| (k == "HOME").then(|| "/home/u".to_string());
        crate::vr::parse_options(&mut crate::args::Args(Vec::new()), &env).unwrap()
    }

    #[test]
    fn configure_sets_the_variable_once_and_the_build_only_for_eye() {
        let mut vr = opts();
        vr.monado_build = PathBuf::from("/other");
        configure(EyeMode::Silent, &mut vr, Path::new("/eye"), false);
        configure(EyeMode::Silent, &mut vr, Path::new("/eye"), false);
        assert_eq!(vr.extra_env.iter().filter(|(k, v)| k == "PIMAX_EYE" && v == "1").count(), 1);
        assert_eq!(vr.monado_build, PathBuf::from("/other"));
        configure(EyeMode::Pointer, &mut vr, Path::new("/eye"), true);
        assert_eq!(vr.monado_build, PathBuf::from("/other"), "a given build is kept");
        configure(EyeMode::Pointer, &mut vr, Path::new("/eye"), false);
        assert_eq!(vr.monado_build, PathBuf::from("/eye"));
        let mut off = opts();
        configure(EyeMode::Off, &mut off, Path::new("/eye"), false);
        assert!(!off.extra_env.iter().any(|(k, _)| k == "PIMAX_EYE"));
        assert!(!format!("{:?}", vr.extra_env).contains("PIMAX_EYE_CAL"), "no device-side calibration");
    }

    #[test]
    fn conf_files_have_unique_keys_and_silent_has_no_pointer() {
        let s = conf_yaml(EyeMode::Silent).unwrap();
        assert!(s.contains("gaze_pointer: false\n") && s.contains("gaze_laser: true\n") && s.contains("gaze_target: true\n"));
        for y in [s, conf_yaml(EyeMode::Pointer).unwrap()] {
            let keys: Vec<&str> = y.lines().filter(|l| !l.starts_with('#')).map(|l| l.split(':').next().unwrap()).collect();
            let mut u = keys.clone();
            u.sort();
            u.dedup();
            assert_eq!(u.len(), keys.len(), "{y}");
        }
        assert!(!conf_yaml(EyeMode::Pointer).unwrap().contains("gaze_pointer"));
        assert!(conf_yaml(EyeMode::Off).is_none());
    }

    #[test]
    fn apply_writes_and_removes() {
        let h = crate::vr::test_dir("eye-desk");
        let f = h.join("config/wayvr/conf.d").join(CONF_NAME);
        assert!(apply_conf(&h, EyeMode::Silent).unwrap().unwrap().contains("silently"));
        assert_eq!(std::fs::read_to_string(&f).unwrap(), conf_yaml(EyeMode::Silent).unwrap());
        assert!(apply_conf(&h, EyeMode::Off).unwrap().is_none());
        assert!(!f.exists());
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
        assert_eq!(o.eye_mode, EyeMode::Pointer);
        assert!(o.vr.extra_env.contains(&("PIMAX_EYE".into(), "1".into())));
        // no eye build under the fake home: off, with a reason
        let o = crate::vr::desk::parse_desk(&mut Args(Vec::new()), &env).unwrap();
        assert!(o.eye_mode == EyeMode::Off && o.eye_note.is_some());
    }

    #[test]
    fn calibration_path_follows_xdg() {
        let env = |k: &str| match k {
            "HOME" => Some("/home/u".to_string()),
            _ => None,
        };
        assert_eq!(calibration_path(&env), PathBuf::from("/home/u/.config/pimax/eye-calibration.toml"));
        let xdg = |k: &str| (k == "XDG_CONFIG_HOME").then(|| "/x".to_string());
        assert_eq!(calibration_path(&xdg), PathBuf::from("/x/pimax/eye-calibration.toml"));
    }
}
