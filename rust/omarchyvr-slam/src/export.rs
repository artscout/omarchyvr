//! Basalt files for a Dream Air: calibration (from the factory XML), VIO config (a Basalt base config with our
//! overrides) and the Monado `SLAM_CONFIG` toml with absolute paths and the GUI off.

use pimax_cam::calib::DeviceCalib;
use pimax_cam::json::{self, Value};
use pimax_cam::slam::{self, ImuNoise, SlamSetup};
use std::path::{Path, PathBuf};

/// Our changes to Basalt's generic (EuRoC) VIO config: 640x480 fisheye at 25 Hz instead of 752x480 pinhole at
/// 20 Hz, so a finer detection grid (more points per image); everything else stays at Basalt's tested values.
pub const OVERRIDES: &[(&str, f64)] = &[("config.optical_flow_detection_grid_size", 40.0)];

/// Basalt's generic EuRoC VIO config of the pinned Basalt commit (installer/components.lock, BSD-3-Clause,
/// THIRD-PARTY.md), unmodified: the base every 6DoF start used on the project PC (`~/src/basalt/data/euroc`). It is
/// the last fallback, so a clean install without a Basalt source tree still starts 6DoF (2026-10-17: the installer
/// installs only `msdmo_config.json`, and the export found none of its candidates).
pub const BUILTIN_BASE: &str = include_str!("../data/basalt-euroc_config.json");

/// What the export records as the base when [`BUILTIN_BASE`] is used.
pub const BUILTIN_BASE_NAME: &str = "built in: Basalt df6e970 data/euroc/euroc_config.json";

/// Candidate base configs, best first (the installed Basalt's copy, which ./install.sh puts there, before Basalt's
/// own msdmo config); with none on disk the export uses [`BUILTIN_BASE`].
pub fn base_config_candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(h) = std::env::var_os("HOME").map(PathBuf::from) {
        v.push(omarchyvr_config::paths::src_dir(&h).join("basalt/data/euroc/euroc_config.json"));
        v.push(h.join(".local/share/basalt/euroc_config.json"));
        v.push(omarchyvr_config::paths::lib_dir(&h).join("basalt/share/basalt/euroc_config.json"));
        v.push(h.join(".local/share/basalt/msdmo_config.json"));
    }
    v
}

/// Serialise a JSON value (pretty, 2 spaces).
pub fn to_json(v: &Value, indent: usize, out: &mut String) {
    let pad = |n: usize| " ".repeat(n);
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Num(x) => {
            if x.fract() == 0.0 && x.abs() < 1e15 {
                out.push_str(&format!("{}", *x as i64));
            } else {
                out.push_str(&json::num(*x));
            }
        }
        Value::Str(s) => out.push_str(&json::quote(s)),
        Value::Arr(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                to_json(x, indent, out);
            }
            out.push(']');
        }
        Value::Obj(m) => {
            out.push_str("{\n");
            for (i, (k, x)) in m.iter().enumerate() {
                out.push_str(&pad(indent + 2));
                out.push_str(&json::quote(k));
                out.push_str(": ");
                to_json(x, indent + 2, out);
                if i + 1 < m.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&pad(indent));
            out.push('}');
        }
    }
}

/// Base config text with [`OVERRIDES`] applied. Integer-valued keys keep integer formatting.
pub fn vio_config(base: &str) -> Result<String, String> {
    let mut v = json::parse(base).map_err(|e| e.to_string())?;
    let Value::Obj(top) = &mut v else {
        return Err("base config: not an object".into());
    };
    let inner = top
        .iter_mut()
        .find(|(k, _)| k == "value0")
        .map(|(_, v)| v)
        .ok_or("base config: no value0")?;
    let Value::Obj(m) = inner else {
        return Err("base config: value0 not an object".into());
    };
    for (k, x) in OVERRIDES {
        match m.iter_mut().find(|(kk, _)| kk == k) {
            Some((_, val)) => *val = Value::Num(*x),
            None => m.push((k.to_string(), Value::Num(*x))),
        }
    }
    let mut s = String::new();
    to_json(&v, 0, &mut s);
    s.push('\n');
    Ok(s)
}

/// The Monado SLAM_CONFIG toml for Basalt.
pub fn toml(calib: &Path, config: &Path, show_gui: bool) -> String {
    format!(
        "# Basalt SLAM tracker options for the Pimax Dream Air (written by omarchyvr-slam export)\n\
         # Use: SLAM_CONFIG=<this file> (Monado), or monado-cli slambatch <euroc dir> <this file> <out dir>\n\
         show-gui={}\n\
         cam-calib=\"{}\"\n\
         config-path=\"{}\"\n\
         marg-data=\"\"\n\
         print-queue=0\n\
         use-double=0\n\
         deterministic=0\n\
         num-threads=0\n",
        show_gui as i32,
        calib.display(),
        config.display()
    )
}

/// Files written by [`export`].
#[derive(Clone, Debug)]
pub struct Exported {
    /// Calibration json.
    pub calib: PathBuf,
    /// VIO config json.
    pub config: PathBuf,
    /// SLAM_CONFIG toml.
    pub toml: PathBuf,
    /// Base config used.
    pub base: PathBuf,
}

/// Write everything into `dir` (absolute paths inside the toml).
pub fn export(
    dir: &Path,
    cal: &DeviceCalib,
    setup: &SlamSetup,
    imu: &ImuNoise,
    base: Option<&Path>,
    show_gui: bool,
) -> Result<Exported, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let dir = dir
        .canonicalize()
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    // an explicit --base must exist; otherwise the first candidate on disk, else the built-in EuRoC config
    let (base, text) = match base {
        Some(b) => (b.to_path_buf(), std::fs::read_to_string(b).map_err(|e| format!("{}: {e}", b.display()))?),
        None => match base_config_candidates().into_iter().find(|p| p.is_file()) {
            Some(b) => {
                let t = std::fs::read_to_string(&b).map_err(|e| format!("{}: {e}", b.display()))?;
                (b, t)
            }
            None => (PathBuf::from(BUILTIN_BASE_NAME), BUILTIN_BASE.to_string()),
        },
    };
    let calib = slam::write_basalt_files(
        &dir,
        cal,
        setup,
        imu,
        &[("vio_config_base", base.display().to_string())],
    )?;
    let config = dir.join("pimax-basalt-config.json");
    std::fs::write(&config, vio_config(&text)?)
        .map_err(|e| format!("{}: {e}", config.display()))?;
    let toml_p = dir.join("pimax-basalt.toml");
    std::fs::write(&toml_p, toml(&calib, &config, show_gui))
        .map_err(|e| format!("{}: {e}", toml_p.display()))?;
    Ok(Exported {
        calib,
        config,
        toml: toml_p,
        base,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_and_roundtrip() {
        let base = "{\"value0\": {\"config.optical_flow_type\": \"frame_to_frame\", \"config.optical_flow_detection_grid_size\": 50, \"config.vio_obs_std_dev\": 0.5, \"config.optical_flow_recall_max_patch_norms\": [1.74, 0.96]}}";
        let out = vio_config(base).unwrap();
        let v = json::parse(&out).unwrap();
        let m = v.get("value0").unwrap();
        assert_eq!(
            m.get("config.optical_flow_detection_grid_size")
                .unwrap()
                .as_f64(),
            Some(40.0)
        );
        assert!(out.contains("\"config.optical_flow_detection_grid_size\": 40,"));
        assert_eq!(m.get("config.vio_obs_std_dev").unwrap().as_f64(), Some(0.5));
        assert_eq!(
            m.get("config.optical_flow_type").unwrap().as_str(),
            Some("frame_to_frame")
        );
    }

    #[test]
    #[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
    fn export_writes_all_files() {
        let cal = DeviceCalib::load(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../analysis/headset-calibration.xml"
        )))
        .unwrap();
        let dir = std::env::temp_dir().join(format!("omarchyvr-slam-export-{}", std::process::id()));
        let base = dir.join("base.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&base, "{\"value0\": {\"config.vio_max_states\": 3}}").unwrap();
        let e = export(
            &dir,
            &cal,
            &SlamSetup::default(),
            &ImuNoise::default(),
            Some(&base),
            false,
        )
        .unwrap();
        let t = std::fs::read_to_string(&e.toml).unwrap();
        assert!(t.contains("show-gui=0"));
        assert!(t.contains(&format!("cam-calib=\"{}\"", e.calib.display())));
        assert!(e.calib.is_absolute());
        assert!(e.config.is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_built_in_base_is_basalts_euroc_config_with_our_override() {
        let out = vio_config(BUILTIN_BASE).unwrap();
        let v = json::parse(&out).unwrap();
        let m = v.get("value0").unwrap();
        assert_eq!(m.get("config.optical_flow_detection_grid_size").unwrap().as_f64(), Some(40.0));
        assert_eq!(m.get("config.optical_flow_type").unwrap().as_str(), Some("frame_to_frame"));
    }

    #[test]
    fn a_clean_install_exports_6dof_with_the_shipped_reference_calibration() {
        let cal = DeviceCalib::parse(pimax_cam::calib::REFERENCE_XML).unwrap();
        let dir = std::env::temp_dir().join(format!("omarchyvr-slam-export-ref-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let base = dir.join("base.json");
        std::fs::write(&base, BUILTIN_BASE).unwrap();
        let e = export(&dir, &cal, &SlamSetup::default(), &ImuNoise::default(), Some(&base), false).unwrap();
        assert!(e.calib.is_file() && e.config.is_file() && e.toml.is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
