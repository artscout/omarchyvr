//! The tracking switches of plan phase 1 (docs/TRACKING-STABILITY.md), saved in the active profile's setup.toml so the
//! app and the launcher share them: `new_exposure`, `room_glide`, `ctrl_steady`, `drift_hold` (true/false; absent = on). The
//! launcher treats a saved `false` like its kill-switch flag (`--old-exposure`, `--no-room-glide`, `--no-ctrl-steady`, `--no-drift-hold`).

/// The setup.toml keys, in the order of the app's switches.
pub const KEYS: [&str; 4] = ["new_exposure", "room_glide", "ctrl_steady", "drift_hold"];

/// The saved values (None: not saved, or no active profile).
pub fn saved(store: &crate::Store) -> [Option<bool>; 4] {
    let Some(s) = store.active().and_then(|id| store.read_setup(&id).ok()) else { return [None; 4] };
    KEYS.map(|k| match s.extra.get(k).map(|v| v.trim()) {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    })
}

/// Save one switch (index into [`KEYS`]). Ok(false): no active profile, nothing written.
pub fn save(store: &crate::Store, idx: usize, on: bool) -> Result<bool, String> {
    let key = KEYS.get(idx).ok_or("no such switch")?;
    let Some(id) = store.active() else { return Ok(false) };
    let mut s = store.read_setup(&id).map_err(|e| format!("cannot read setup.toml: {e}"))?;
    if s.extra.get(*key).map(|v| v.trim()) == Some(if on { "true" } else { "false" }) {
        return Ok(true);
    }
    s.extra.insert(key.to_string(), on.to_string());
    store.write_setup(&id, &s).map_err(|e| format!("cannot write setup.toml: {e}"))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_in_the_active_profile() {
        let d = std::env::temp_dir().join(format!("tracking-switches-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let store = crate::Store::new(d.join("cfg"), d.join("cache"));
        assert_eq!(saved(&store), [None; 4]);
        assert_eq!(save(&store, 1, false), Ok(false));
        let id = crate::store::ProfileId::new("TESTSERIAL", "default").unwrap();
        store.create_profile(&id, "cant_deg = 2.0\n").unwrap();
        std::os::unix::fs::symlink(store.profile_dir(&id), d.join("cfg").join("active")).unwrap();
        assert_eq!(save(&store, 1, false), Ok(true));
        assert_eq!(save(&store, 2, true), Ok(true));
        assert_eq!(saved(&store), [None, Some(false), Some(true), None]);
        let _ = std::fs::remove_dir_all(&d);
    }
}
