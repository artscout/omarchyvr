//! Controller angle (PIMAX_CTRL_ANGLE, docs/CONTROLLERS-INHAND.md): the pitch in degrees, -20..+70, that the Monado
//! Pimax driver adds to the grip AND aim pose of both controllers. Positive = the pointer tilts up. Default +30.
//!
//! * Start value: the `PIMAX_CTRL_ANGLE` environment variable the launcher sets (`--ctrl-angle DEG`, recorded in
//!   `start-argv.txt`; the saved setting is `ctrl_angle` in setup.toml).
//! * Run-time change: `$XDG_RUNTIME_DIR/pimax-ctrl-angle` holds the degrees; the driver re-reads it within 100 ms
//!   and keeps the file (it is the current value, not a one-shot request).
use std::path::{Path, PathBuf};

pub const MIN_DEG: i32 = -20;
pub const MAX_DEG: i32 = 70;
pub const DEFAULT_DEG: i32 = 30;

/// Parse a degree value and check the range; Err is a plain-words message.
pub fn parse(s: &str) -> Result<i32, String> {
    let v: f64 = s.trim().parse().map_err(|_| format!("controller angle \"{s}\" is not a number"))?;
    if !v.is_finite() || v < MIN_DEG as f64 || v > MAX_DEG as f64 {
        return Err(format!("controller angle {s} must be {MIN_DEG}..{MAX_DEG} degrees"));
    }
    Ok(v.round() as i32)
}

/// The driver's live file.
pub fn live_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("pimax-ctrl-angle")
}

/// Write the live value (atomic rename); the running session's driver applies it without a restart.
pub fn write_live(runtime_dir: &Path, deg: i32) -> Result<(), String> {
    let deg = deg.clamp(MIN_DEG, MAX_DEG);
    let p = live_path(runtime_dir);
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, format!("{deg}\n")).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &p).map_err(|e| format!("cannot write {}: {e}", p.display()))
}

/// The saved setting: `ctrl_angle` in the active profile's setup.toml (None: no active profile or not set).
pub fn saved(store: &crate::Store) -> Option<i32> {
    let id = store.active()?;
    store.read_setup(&id).ok()?.ctrl_angle
}

/// Save the setting in the active profile's setup.toml. Ok(false): no active profile (nothing written; the app keeps
/// the value in its own state and passes `--ctrl-angle` at every start anyway).
pub fn save(store: &crate::Store, deg: i32) -> Result<bool, String> {
    let Some(id) = store.active() else { return Ok(false) };
    let mut s = store.read_setup(&id).map_err(|e| format!("cannot read setup.toml: {e}"))?;
    let deg = deg.clamp(MIN_DEG, MAX_DEG);
    if s.ctrl_angle == Some(deg) {
        return Ok(true);
    }
    s.ctrl_angle = Some(deg);
    store.write_setup(&id, &s).map_err(|e| format!("cannot write setup.toml: {e}"))?;
    Ok(true)
}

/// The start arguments with `--ctrl-angle` set to `deg` (replaced in place, or added): a live change is kept by the
/// recorded start (start-argv.txt), so a restart uses the new value.
pub fn argv_with_angle(args: &[String], deg: i32) -> Vec<String> {
    let deg = deg.clamp(MIN_DEG, MAX_DEG).to_string();
    let mut v = args.to_vec();
    match v.iter().position(|a| a == "--ctrl-angle" || a.starts_with("--ctrl-angle=")) {
        Some(i) if v[i] == "--ctrl-angle" => {
            if i + 1 < v.len() {
                v[i + 1] = deg;
            } else {
                v.push(deg);
            }
        }
        Some(i) => v[i] = format!("--ctrl-angle={deg}"),
        None => {
            v.push("--ctrl-angle".into());
            v.push(deg);
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_and_rounding() {
        assert_eq!(parse("30"), Ok(30));
        assert_eq!(parse("-20"), Ok(-20));
        assert_eq!(parse("70"), Ok(70));
        assert_eq!(parse("12.6"), Ok(13));
        assert!(parse("71").is_err());
        assert!(parse("-21").is_err());
        assert!(parse("up").is_err());
        assert!(parse("NaN").is_err());
    }

    #[test]
    fn argv_angle_replaced_or_added() {
        let sv = |x: &[&str]| x.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(argv_with_angle(&sv(&["--6dof"]), 40), sv(&["--6dof", "--ctrl-angle", "40"]));
        assert_eq!(argv_with_angle(&sv(&["--ctrl-angle", "30", "--6dof"]), -5), sv(&["--ctrl-angle", "-5", "--6dof"]));
        assert_eq!(argv_with_angle(&sv(&["--ctrl-angle=30"]), 99), sv(&["--ctrl-angle=70"]));
    }

    #[test]
    fn saved_in_the_active_profile() {
        let d = std::env::temp_dir().join(format!("ctrl-angle-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let store = crate::Store::new(d.join("cfg"), d.join("cache"));
        assert_eq!(saved(&store), None);
        assert_eq!(save(&store, 45), Ok(false), "no active profile: nothing written");
        let id = crate::store::ProfileId::new("TESTSERIAL", "default").unwrap();
        store.create_profile(&id, "cant_deg = 2.0\n").unwrap();
        std::os::unix::fs::symlink(store.profile_dir(&id), d.join("cfg").join("active")).unwrap();
        assert_eq!(save(&store, 45), Ok(true));
        assert_eq!(saved(&store), Some(45));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn live_file_is_the_degrees() {
        let d = std::env::temp_dir().join(format!("ctrl-angle-test-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        write_live(&d, 45).unwrap();
        assert_eq!(std::fs::read_to_string(live_path(&d)).unwrap(), "45\n");
        write_live(&d, 999).unwrap();
        assert_eq!(std::fs::read_to_string(live_path(&d)).unwrap(), "70\n");
        let _ = std::fs::remove_dir_all(&d);
    }
}
