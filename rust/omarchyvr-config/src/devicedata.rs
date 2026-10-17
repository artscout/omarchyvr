//! Per-device data a Work or Play start needs, and where each piece comes from (docs/INSTALL.md "What a clean install
//! keeps and imports", docs/PARITY-2026-10-17.html).
//!
//! The rule (user decisions 2026-10-17): the user's own data always wins (read from the headset, imported from a
//! previous install or a Pimax Windows install, kept by the uninstaller); without it the shipped Pimax Dream Air
//! REFERENCE is used and the app says so. Nothing here talks to the headset.
//!
//! | data | the user's own | the shipped reference |
//! |---|---|---|
//! | factory calibration | `~/.config/omarchyvr/headset-calibration.xml` | `pimax_cam::calib::REFERENCE_XML` |
//! | controller LED models | `~/.config/omarchyvr/controllers/` | `pimax_ctrltrack::model::REFERENCE_LEFT/RIGHT` |
//! | lens profile | `~/.config/omarchyvr/lens.conf` | `pimax_lens::REFERENCE_LENS_CONF` |
//! | gyro bias | `~/.cache/omarchyvr/gyro-bias` (the driver measures and saves it) | [`REFERENCE_GYRO_BIAS`] |
//!
//! The reference files live in `rust/pimax-device/devices/dream-air/reference/` (installed with the data copy) and
//! are compiled into the crates that read them, so a clean install works from the build alone.
use std::path::{Path, PathBuf};

/// The reference directory inside the repository and inside the installed data copy (`<data_dir>/data/...`).
pub const REFERENCE_DIR: &str = "rust/pimax-device/devices/dream-air/reference";

/// The gyro bias every start begins with when this PC has none measured yet: the typical value of the reference
/// headset (deg/s, raw IMU axes, the driver's cache format). The driver refines it the first time the headset rests
/// for 2 s and saves the measured value over it.
pub const REFERENCE_GYRO_BIAS: &str = include_str!("../../pimax-device/devices/dream-air/reference/gyro-bias");

/// `PIMAX_BIAS_TIMEOUT_MS` the launcher gives Monado: the start never waits for the headset to lie still. Tracking
/// starts at once with the saved bias (or [`REFERENCE_GYRO_BIAS`]) and the still window keeps running in the
/// background (pimax_hmd.c `bias_timeout`: "it is refined the next time the headset rests").
pub const BIAS_TIMEOUT_MS: &str = "200";

/// The reference files are used unless `PIMAX_NO_REFERENCE` is set to something other than `0` (the same switch as in
/// `pimax-cam`, `pimax-lens` and `pimax-ctrltrack`).
pub fn reference_allowed(env: &dyn Fn(&str) -> Option<String>) -> bool {
    !env("PIMAX_NO_REFERENCE").is_some_and(|v| !v.is_empty() && v != "0")
}

/// The installed reference directory (`<data_dir>/data/rust/pimax-device/devices/dream-air/reference`).
pub fn reference_dir(home: &Path) -> PathBuf {
    crate::paths::data_dir(home).join("data").join(REFERENCE_DIR)
}

fn cache_root(home: &Path, env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    env("XDG_CACHE_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join(".cache"))
}

/// The driver's gyro bias cache: `$XDG_CACHE_HOME/omarchyvr/gyro-bias`, else `~/.cache/omarchyvr/gyro-bias`
/// (pimax_hmd.c `bias_cache_path`).
pub fn gyro_bias_path(home: &Path, env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    cache_root(home, env).join("omarchyvr/gyro-bias")
}

/// The cache of the names before the rename (`<cache>/pimax/gyro-bias`); the driver still reads it when the new one
/// is missing.
pub fn legacy_gyro_bias_path(home: &Path, env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    cache_root(home, env).join("pimax/gyro-bias") // names:keep
}

/// Three numbers in deg/s, each below 5 in size (the driver's own plausibility rule); None otherwise.
pub fn parse_gyro_bias(text: &str) -> Option<[f32; 3]> {
    let v: Vec<f32> = text.split_whitespace().map(|w| w.parse::<f32>()).collect::<Result<_, _>>().ok()?;
    (v.len() == 3 && v.iter().all(|x| x.is_finite() && x.abs() < 5.0)).then(|| [v[0], v[1], v[2]])
}

/// True when `text` holds the reference value (not a bias measured on this PC).
pub fn is_reference_gyro_bias(text: &str) -> bool {
    parse_gyro_bias(text).is_some() && parse_gyro_bias(text) == parse_gyro_bias(REFERENCE_GYRO_BIAS)
}

/// What [`ensure_gyro_bias`] found or did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GyroSeed {
    /// A bias measured on this PC is saved.
    Saved,
    /// The reference value is saved (written earlier; the driver replaces it once the headset rests).
    Reference,
    /// Copied from the cache of the names before the rename.
    Legacy(PathBuf),
    /// Nothing was saved: the reference value was written now.
    WroteReference,
}

impl GyroSeed {
    /// One plain line for the start output.
    pub fn describe(&self) -> String {
        match self {
            GyroSeed::Saved => "gyro: the bias measured on this PC (refined again whenever the headset rests)".into(),
            GyroSeed::Reference | GyroSeed::WroteReference => {
                "gyro: the Dream Air default bias for now; it is measured and saved the first time the headset rests for 2 seconds".into()
            }
            GyroSeed::Legacy(p) => format!("gyro: the bias of the previous install ({})", p.display()),
        }
    }
}

/// Make sure the driver has a gyro bias to start with, so no start waits for the headset to lie still (user
/// decision 2026-10-17: "gyro calibration should be default"): keep a valid saved one; else copy the legacy cache;
/// else write [`REFERENCE_GYRO_BIAS`]. A broken file is replaced. Errors only when the cache cannot be written.
pub fn ensure_gyro_bias(home: &Path, env: &dyn Fn(&str) -> Option<String>) -> std::io::Result<GyroSeed> {
    let path = gyro_bias_path(home, env);
    if let Some(t) = std::fs::read_to_string(&path).ok().filter(|t| parse_gyro_bias(t).is_some()) {
        return Ok(if is_reference_gyro_bias(&t) { GyroSeed::Reference } else { GyroSeed::Saved });
    }
    let legacy = legacy_gyro_bias_path(home, env);
    let (text, seed) = match std::fs::read_to_string(&legacy).ok().filter(|t| parse_gyro_bias(t).is_some()) {
        Some(t) => (t, GyroSeed::Legacy(legacy)),
        None => (REFERENCE_GYRO_BIAS.to_string(), GyroSeed::WroteReference),
    };
    write_atomic(&path, text.as_bytes(), 0o644)?;
    Ok(seed)
}

/// Write through a temporary file in the same directory (parents created), with `mode`.
pub fn write_atomic(path: &Path, data: &[u8], mode: u32) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.tmp-{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(mode).open(&tmp)?;
    f.write_all(data)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)
}

/// The directories with the user's own controller LED models, in the order the tracker searches them
/// (mirrors `pimax_ctrltrack::model::LedModel::search_dirs`, without the lab dump in the repository).
pub fn controller_model_dirs(home: &Path) -> [PathBuf; 2] {
    [home.join(".config/omarchyvr/controllers"), home.join(".local/share/omarchyvr/controller-models")]
}

/// A directory with a usable pair: `left.json` + `right.json`, or two `<serial>/controller_params.json`.
pub fn has_controller_pair(dir: &Path) -> bool {
    if dir.join("left.json").is_file() && dir.join("right.json").is_file() {
        return true;
    }
    std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(Result::ok).filter(|e| e.path().join("controller_params.json").is_file()).count() >= 2)
        .unwrap_or(false)
}

/// The user's own controller models (the directory), if any.
pub fn own_controller_models(home: &Path) -> Option<PathBuf> {
    controller_model_dirs(home).into_iter().find(|d| has_controller_pair(d))
}

/// The user's own lens profile (`$XDG_CONFIG_HOME/omarchyvr/lens.conf`, else `~/.config/omarchyvr/lens.conf`), if any.
pub fn own_lens_conf(home: &Path, env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let base = env("XDG_CONFIG_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
    Some(base.join("omarchyvr/lens.conf")).filter(|p| p.is_file())
}

/// The headset's own factory calibration (`~/.config/omarchyvr/headset-calibration.xml`, a non-trivial XML file).
pub fn own_calibration(home: &Path) -> Option<PathBuf> {
    let p = crate::paths::headset_calibration_target(home);
    std::fs::read(&p).ok().filter(|b| b.len() > 200 && b.iter().take(64).any(|&c| c == b'<')).map(|_| p)
}

/// Where each piece of device data comes from right now (for the app and `omarchyvr import-previous --status`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sources {
    /// The headset's own factory calibration (else the reference).
    pub calibration: Option<PathBuf>,
    /// The user's own controller models (else the reference).
    pub controllers: Option<PathBuf>,
    /// The user's own lens profile (else the reference profile).
    pub lens: Option<PathBuf>,
    /// The saved gyro bias: Some(true) measured on this PC, Some(false) the reference value, None nothing saved yet
    /// (the next start writes the reference value).
    pub gyro_measured: Option<bool>,
}

impl Sources {
    /// Look at the disk.
    pub fn read(home: &Path, env: &dyn Fn(&str) -> Option<String>) -> Sources {
        let gyro = std::fs::read_to_string(gyro_bias_path(home, env)).ok().filter(|t| parse_gyro_bias(t).is_some());
        Sources {
            calibration: own_calibration(home),
            controllers: own_controller_models(home),
            lens: own_lens_conf(home, env),
            gyro_measured: gyro.map(|t| !is_reference_gyro_bias(&t)),
        }
    }

    /// The pieces that use the shipped reference now, in plain words (empty: everything is the user's own).
    pub fn reference_in_use(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.calibration.is_none() {
            v.push("headset calibration");
        }
        if self.controllers.is_none() {
            v.push("controller models");
        }
        if self.lens.is_none() {
            v.push("lens profile");
        }
        if self.gyro_measured != Some(true) {
            v.push("gyro bias");
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("omarchyvr-devicedata-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn the_reference_gyro_bias_is_plausible() {
        let b = parse_gyro_bias(REFERENCE_GYRO_BIAS).unwrap();
        assert!(b.iter().all(|x| x.abs() < 2.0), "{b:?}");
        assert!(is_reference_gyro_bias(REFERENCE_GYRO_BIAS));
        assert!(!is_reference_gyro_bias("0.1157 0.6422 0.3204\n"));
        assert_eq!(parse_gyro_bias("1 2"), None);
        assert_eq!(parse_gyro_bias("9 0 0"), None, "the driver refuses anything above 5 deg/s");
        assert_eq!(parse_gyro_bias("a b c"), None);
    }

    #[test]
    fn a_start_never_waits_for_a_still_headset_the_bias_is_seeded() {
        let h = home("seed");
        // nothing anywhere: the reference value is written, the next start finds it
        assert_eq!(ensure_gyro_bias(&h, &no_env).unwrap(), GyroSeed::WroteReference);
        let p = gyro_bias_path(&h, &no_env);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), REFERENCE_GYRO_BIAS);
        assert_eq!(ensure_gyro_bias(&h, &no_env).unwrap(), GyroSeed::Reference);
        // the driver measured one: kept as it is
        std::fs::write(&p, "0.1157 0.6422 0.3204\n").unwrap();
        assert_eq!(ensure_gyro_bias(&h, &no_env).unwrap(), GyroSeed::Saved);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "0.1157 0.6422 0.3204\n");
        // a broken file is replaced; the legacy cache wins over the reference
        std::fs::write(&p, "garbage").unwrap();
        let legacy = legacy_gyro_bias_path(&h, &no_env);
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, "0.2492 0.7927 0.3365\n").unwrap();
        assert_eq!(ensure_gyro_bias(&h, &no_env).unwrap(), GyroSeed::Legacy(legacy.clone()));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "0.2492 0.7927 0.3365\n");
        // XDG_CACHE_HOME is honoured like the driver does
        let x = h.join("xdg-cache");
        let env = |k: &str| (k == "XDG_CACHE_HOME").then(|| x.display().to_string());
        assert_eq!(gyro_bias_path(&h, &env), x.join("omarchyvr/gyro-bias"));
        let _ = std::fs::remove_dir_all(&h);
    }

    #[test]
    fn sources_say_what_is_the_users_own_and_what_is_the_reference() {
        let h = home("sources");
        let s = Sources::read(&h, &no_env);
        assert_eq!(s.reference_in_use(), vec!["headset calibration", "controller models", "lens profile", "gyro bias"]);
        let c = h.join(".config/omarchyvr/controllers");
        for sn in ["A", "B"] {
            std::fs::create_dir_all(c.join(sn)).unwrap();
            std::fs::write(c.join(sn).join("controller_params.json"), "{}").unwrap();
        }
        std::fs::write(h.join(".config/omarchyvr/lens.conf"), "fov = 50 50 44 44\n").unwrap();
        std::fs::write(h.join(".config/omarchyvr/headset-calibration.xml"), format!("<?xml version=\"1.0\"?><DeviceConfiguration>{}</DeviceConfiguration>", " ".repeat(300))).unwrap();
        ensure_gyro_bias(&h, &no_env).unwrap();
        let s = Sources::read(&h, &no_env);
        assert_eq!(s.controllers.as_deref(), Some(c.as_path()));
        assert_eq!(s.reference_in_use(), vec!["gyro bias"], "the seeded default is still the reference");
        std::fs::write(gyro_bias_path(&h, &no_env), "0.1 0.6 0.3\n").unwrap();
        assert!(Sources::read(&h, &no_env).reference_in_use().is_empty());
        let _ = std::fs::remove_dir_all(&h);
    }
}
