//! Profiles and backups (moved here from pimax-setup's model): per-serial profiles of `pimax-config`, import of the
//! legacy `~/.config/pimax` files, activation with the serial check, backups, diff, restore.
use pimax_config::{check_serial, diff_lines, BackupInfo, ConfigError, DiffLine, ProfileId, SerialCheck, Store, MANAGED_FILES};

/// What a profile is compared with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffTarget {
    /// The selected backup.
    Backup(usize),
    /// The legacy `~/.config/pimax` files.
    Legacy,
}

/// One row of the profile list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRow {
    /// Profile.
    pub id: ProfileId,
    /// Is it the `active` symlink target?
    pub active: bool,
    /// Serial comparison with the connected headset.
    pub serial: SerialCheck,
}

/// The Profiles page state.
#[derive(Clone)]
pub struct Profiles {
    /// Store.
    pub store: Store,
    /// Serial of the connected headset.
    pub serial: Option<String>,
    /// Profiles on disk.
    pub rows: Vec<ProfileRow>,
    /// Selected profile.
    pub selected: Option<ProfileId>,
    /// Backups of the selected profile's serial.
    pub backups: Vec<BackupInfo>,
    /// Selected backup.
    pub backup_sel: Option<usize>,
    /// Diff per file.
    pub diff: Vec<(String, Vec<DiffLine>)>,
    /// User confirmed using a profile of another serial.
    pub confirm_mismatch: bool,
    /// Also replace the legacy lens.conf by a symlink on activate.
    pub link_legacy: bool,
    /// Result of the last action.
    pub message: String,
}

impl Profiles {
    /// Load the store.
    pub fn new(store: Store, serial: Option<String>) -> Profiles {
        let mut p = Profiles {
            store,
            serial,
            rows: Vec::new(),
            selected: None,
            backups: Vec::new(),
            backup_sel: None,
            diff: Vec::new(),
            confirm_mismatch: false,
            link_legacy: false,
            message: String::new(),
        };
        p.reload();
        p
    }

    /// New headset serial (connected / disconnected).
    pub fn set_serial(&mut self, serial: Option<String>) {
        if serial != self.serial {
            self.serial = serial;
            self.reload();
        }
    }

    /// Re-read profiles and backups.
    pub fn reload(&mut self) {
        let active = self.store.active();
        self.rows = self
            .store
            .list_profiles()
            .into_iter()
            .map(|id| ProfileRow {
                active: active.as_ref() == Some(&id),
                serial: check_serial(&id.serial, self.serial.as_deref()),
                id,
            })
            .collect();
        if self.selected.as_ref().is_some_and(|s| !self.rows.iter().any(|p| &p.id == s)) {
            self.selected = None;
        }
        if self.selected.is_none() {
            self.selected = self
                .rows
                .iter()
                .find(|p| p.active)
                .or_else(|| self.rows.first())
                .map(|p| p.id.clone());
        }
        self.reload_backups();
    }

    /// Select a profile.
    pub fn select(&mut self, id: ProfileId) {
        self.selected = Some(id);
        self.confirm_mismatch = false;
        self.reload_backups();
    }

    fn reload_backups(&mut self) {
        self.backups = self.selected.as_ref().map(|s| self.store.list_backups(&s.serial)).unwrap_or_default();
        self.backup_sel = None;
        self.diff.clear();
    }

    /// Compute a diff.
    pub fn compute_diff(&mut self, target: DiffTarget) {
        self.diff.clear();
        let Some(id) = self.selected.clone() else { return };
        for f in MANAGED_FILES {
            let cur = self.store.read_text(&id, f).ok().flatten();
            let other = match target {
                DiffTarget::Backup(i) => self.backups.get(i).and_then(|b| self.store.read_backup_text(b, f)),
                DiffTarget::Legacy => {
                    if matches!(f, "lens.conf" | "eye-correction.toml" | "cam-calib-overlay.json") {
                        std::fs::read_to_string(self.store.root().join(f)).ok()
                    } else {
                        None
                    }
                }
            };
            if matches!(target, DiffTarget::Legacy) && other.is_none() {
                continue;
            }
            if cur == other {
                continue;
            }
            self.diff.push((f.to_string(), diff_lines(other.as_deref().unwrap_or(""), cur.as_deref().unwrap_or(""))));
        }
        if self.diff.is_empty() {
            self.message = "No differences.".into();
        }
    }

    fn report(&mut self, what: &str, r: Result<(), ConfigError>) -> bool {
        match r {
            Ok(()) => {
                self.message = format!("{what}: done");
                true
            }
            Err(e) => {
                self.message = format!("{what} failed: {e}");
                false
            }
        }
    }

    /// Import the legacy files into `<serial>/default` (serial of the headset, else the legacy lens.conf's).
    pub fn import_legacy(&mut self) {
        let Some(serial) = self.serial.clone().or_else(|| self.store.legacy_lens_serial()) else {
            self.message = "Import failed: no headset connected and the legacy lens.conf names no serial".into();
            return;
        };
        let id = match ProfileId::new(&serial, "default") {
            Ok(i) => i,
            Err(e) => {
                self.message = format!("Import failed: {e}");
                return;
            }
        };
        match self.store.import_legacy(&id) {
            Ok(files) => {
                self.message = format!("Imported {} into {id}", files.join(", "));
                self.selected = Some(id);
                self.reload();
            }
            Err(e) => self.message = format!("Import failed: {e}"),
        }
    }

    /// Restore the selected backup (the current state is backed up first).
    pub fn restore_selected(&mut self) {
        let (Some(id), Some(i)) = (self.selected.clone(), self.backup_sel) else {
            self.message = "Select a profile and a backup first".into();
            return;
        };
        let Some(b) = self.backups.get(i).cloned() else { return };
        let r = self.store.restore(&id, &b);
        if self.report(&format!("Restore {}", b.name), r) {
            self.reload();
        }
    }

    /// Back up the selected profile.
    pub fn backup_selected(&mut self) {
        if let Some(id) = self.selected.clone() {
            let r = self.store.backup(&id, "manual").map(|_| ());
            if self.report("Backup", r) {
                self.reload_backups();
            }
        }
    }

    /// Activate the selected profile.
    pub fn activate_selected(&mut self) {
        let Some(id) = self.selected.clone() else { return };
        let r = self.store.activate(&id, self.serial.as_deref(), self.confirm_mismatch, self.link_legacy);
        if self.report(&format!("Activate {id}"), r) {
            self.reload();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    const LENS: &str = "# headset serial SER1\ncant_deg = 2.0\n[left]\ncenter = 0.027 0.009\n";

    struct Tmp(PathBuf);
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn setup(tag: &str, serial: Option<&str>) -> (Profiles, Tmp) {
        let d = std::env::temp_dir().join(format!("pimax-gui-core-prof-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("cfg")).unwrap();
        fs::write(d.join("cfg/lens.conf"), LENS).unwrap();
        let store = Store::new(d.join("cfg"), d.join("cache"));
        (Profiles::new(store, serial.map(str::to_string)), Tmp(d))
    }

    #[test]
    fn import_activate_backup_restore_flow() {
        let (mut m, _t) = setup("flow", Some("SER1"));
        m.import_legacy();
        assert_eq!(m.rows.len(), 1, "{}", m.message);
        let id = m.rows[0].id.clone();
        assert_eq!(id, ProfileId::new("SER1", "default").unwrap());
        assert_eq!(m.rows[0].serial, SerialCheck::Match);
        m.activate_selected();
        assert!(m.rows[0].active, "{}", m.message);
        let new = LENS.replace("2.0", "2.5");
        m.store.write_lens(&id, &new).unwrap();
        m.backup_selected();
        m.store.write_lens(&id, &LENS.replace("2.0", "3.5")).unwrap();
        m.select(id.clone());
        assert!(m.backups.len() >= 2);
        m.backup_sel = Some(0);
        m.compute_diff(DiffTarget::Backup(0));
        assert!(m.diff.iter().any(|(f, _)| f == "lens.conf"));
        m.restore_selected();
        assert_eq!(m.store.read_lens(&id).unwrap().0, new, "{}", m.message);
        m.compute_diff(DiffTarget::Legacy);
        assert!(m.diff.iter().any(|(f, _)| f == "lens.conf"));
    }

    #[test]
    fn other_serial_needs_confirmation_and_import_needs_a_serial() {
        let (mut m, _t) = setup("ser", Some("SER1"));
        let other = ProfileId::new("OTHER", "default").unwrap();
        m.store.create_profile(&other, LENS).unwrap();
        m.reload();
        m.select(other.clone());
        assert_eq!(m.rows[0].serial, SerialCheck::Mismatch);
        m.activate_selected();
        assert!(m.message.contains("another") || m.message.contains("confirm"), "{}", m.message);
        assert!(m.store.active().is_none());
        m.confirm_mismatch = true;
        m.activate_selected();
        assert_eq!(m.store.active(), Some(other));
        // the legacy lens.conf names SER1, so import works without a headset too
        let (mut n, _t2) = setup("noser", None);
        n.import_legacy();
        assert_eq!(n.rows.len(), 1, "{}", n.message);
    }
}
