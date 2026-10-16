//! Profile store: layout, atomic writes, backups, restore, import, activation.
use crate::setup::Setup;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Files a profile may hold. Anything else is refused.
pub const MANAGED_FILES: [&str; 6] = [
    "lens.conf",
    "setup.toml",
    "meta.toml",
    "eye-correction.toml",
    "cam-calib-overlay.json",
    "passthrough.toml",
];
/// Backups kept per serial.
const KEEP_BACKUPS: usize = 30;

/// Errors of the store.
#[derive(Debug)]
pub enum ConfigError {
    /// Filesystem error with context.
    Io(String, std::io::Error),
    /// Serial or profile name is not path-safe.
    BadName(String),
    /// File name not in [`MANAGED_FILES`].
    BadFile(String),
    /// lens.conf or setup.toml failed validation.
    Invalid(String),
    /// Profile or backup does not exist.
    NotFound(String),
    /// Profile already exists.
    Exists(String),
    /// Profile belongs to another headset and no confirmation was given.
    SerialMismatch {
        /// Serial of the profile.
        profile: String,
        /// Serial of the connected headset.
        connected: String,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Io(c, e) => write!(f, "{c}: {e}"),
            ConfigError::BadName(n) => write!(f, "'{n}' is not a valid serial/profile name (letters, digits, - _ . only)"),
            ConfigError::BadFile(n) => write!(f, "'{n}' is not a profile file"),
            ConfigError::Invalid(m) => write!(f, "invalid: {m}"),
            ConfigError::NotFound(n) => write!(f, "not found: {n}"),
            ConfigError::Exists(n) => write!(f, "already exists: {n}"),
            ConfigError::SerialMismatch { profile, connected } => write!(
                f,
                "profile is for headset {profile} but {connected} is connected; confirm explicitly to use it anyway"
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

fn io<T>(ctx: impl Into<String>, r: std::io::Result<T>) -> Result<T, ConfigError> {
    r.map_err(|e| ConfigError::Io(ctx.into(), e))
}

/// Result of comparing a profile's serial with the connected headset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SerialCheck {
    /// Same headset.
    Match,
    /// Different headset.
    Mismatch,
    /// No headset connected: cannot tell.
    Unknown,
}

/// Compare serials (case-sensitive, as printed by the headset).
pub fn check_serial(profile_serial: &str, connected: Option<&str>) -> SerialCheck {
    match connected {
        None => SerialCheck::Unknown,
        Some(c) if c == profile_serial => SerialCheck::Match,
        Some(_) => SerialCheck::Mismatch,
    }
}

/// `<cache>/<serial>/gyro-bias`: the proposed per-serial path (the driver change is a separate task).
pub fn gyro_bias_path(cache_root: &Path, serial: &str) -> PathBuf {
    cache_root.join(serial).join("gyro-bias")
}

/// `<cache>/gyro-bias`: the path the driver uses today (shared by all serials).
pub fn legacy_gyro_bias_path(cache_root: &Path) -> PathBuf {
    cache_root.join("gyro-bias")
}

/// A profile: headset serial and profile name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProfileId {
    /// Headset serial.
    pub serial: String,
    /// Profile name (`default`).
    pub name: String,
}

impl ProfileId {
    /// Validate both parts.
    pub fn new(serial: &str, name: &str) -> Result<Self, ConfigError> {
        check_name(serial)?;
        check_name(name)?;
        Ok(Self {
            serial: serial.into(),
            name: name.into(),
        })
    }
}

impl fmt::Display for ProfileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.serial, self.name)
    }
}

fn check_name(n: &str) -> Result<(), ConfigError> {
    let ok = !n.is_empty()
        && n.len() <= 80
        && !n.starts_with('.')
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if ok {
        Ok(())
    } else {
        Err(ConfigError::BadName(n.into()))
    }
}

/// One backup directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupInfo {
    /// Serial it belongs to.
    pub serial: String,
    /// Directory name `YYYYMMDD-HHMMSS-nn-reason`.
    pub name: String,
    /// Full path.
    pub path: PathBuf,
    /// Files inside.
    pub files: Vec<String>,
}

/// The profile store.
#[derive(Clone)]
pub struct Store {
    root: PathBuf,
    cache: PathBuf,
    clock: std::sync::Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl Store {
    /// Store at explicit directories with the wall clock.
    pub fn new(root: impl Into<PathBuf>, cache: impl Into<PathBuf>) -> Self {
        Self::with_clock(root, cache, || {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs())
        })
    }

    /// Store with an injected clock (seconds since the epoch, UTC), for tests.
    pub fn with_clock(
        root: impl Into<PathBuf>,
        cache: impl Into<PathBuf>,
        clock: impl Fn() -> u64 + Send + Sync + 'static,
    ) -> Self {
        Self {
            root: root.into(),
            cache: cache.into(),
            clock: std::sync::Arc::new(clock),
        }
    }

    /// Default locations: `$XDG_CONFIG_HOME/pimax` or `~/.config/pimax`, `$XDG_CACHE_HOME/pimax` or `~/.cache/pimax`.
    pub fn from_env(env: &dyn Fn(&str) -> Option<String>) -> Self {
        let home = env("HOME").unwrap_or_else(|| ".".into());
        let dir = |var: &str, rel: &str| match env(var).filter(|v| !v.is_empty()) {
            Some(x) => PathBuf::from(x).join("pimax"),
            None => PathBuf::from(&home).join(rel).join("pimax"),
        };
        Self::new(
            dir("XDG_CONFIG_HOME", ".config"),
            dir("XDG_CACHE_HOME", ".cache"),
        )
    }

    /// Config root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Cache root.
    pub fn cache(&self) -> &Path {
        &self.cache
    }

    /// Directory of a profile.
    pub fn profile_dir(&self, id: &ProfileId) -> PathBuf {
        self.root.join("profiles").join(&id.serial).join(&id.name)
    }

    fn backups_dir(&self, serial: &str) -> PathBuf {
        self.root.join("backups").join(serial)
    }

    /// All profiles, sorted.
    pub fn list_profiles(&self) -> Vec<ProfileId> {
        let mut v = Vec::new();
        for s in sorted_dirs(&self.root.join("profiles")) {
            for n in sorted_dirs(&self.root.join("profiles").join(&s)) {
                if let Ok(id) = ProfileId::new(&s, &n) {
                    v.push(id);
                }
            }
        }
        v
    }

    /// The active profile (target of the `active` symlink), if it is a valid profile path.
    pub fn active(&self) -> Option<ProfileId> {
        let t = fs::read_link(self.root.join("active")).ok()?;
        let mut comps = t.components().rev();
        let name = comps.next()?.as_os_str().to_str()?.to_string();
        let serial = comps.next()?.as_os_str().to_str()?.to_string();
        ProfileId::new(&serial, &name).ok()
    }

    /// Create an empty profile with `lens.conf` text (validated) and a `meta.toml`.
    pub fn create_profile(&self, id: &ProfileId, lens_text: &str) -> Result<(), ConfigError> {
        let dir = self.profile_dir(id);
        if dir.exists() {
            return Err(ConfigError::Exists(id.to_string()));
        }
        validate_lens(lens_text)?;
        io(
            format!("create {}", dir.display()),
            fs::create_dir_all(&dir),
        )?;
        self.write_file(id, "lens.conf", lens_text.as_bytes())?;
        let meta = format!(
            "serial = \"{}\"\nname = \"{}\"\ncreated = \"{}\"\ntool = \"pimax-config {}\"\n",
            id.serial,
            id.name,
            stamp(self.now()),
            env!("CARGO_PKG_VERSION")
        );
        self.write_file(id, "meta.toml", meta.as_bytes())
    }

    /// Read a profile file; `None` if absent.
    pub fn read_file(&self, id: &ProfileId, file: &str) -> Result<Option<Vec<u8>>, ConfigError> {
        check_file(file)?;
        let p = self.profile_dir(id).join(file);
        match fs::read(&p) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(ConfigError::Io(format!("read {}", p.display()), e)),
        }
    }

    /// Read a profile file as text.
    pub fn read_text(&self, id: &ProfileId, file: &str) -> Result<Option<String>, ConfigError> {
        Ok(self
            .read_file(id, file)?
            .map(|b| String::from_utf8_lossy(&b).into_owned()))
    }

    /// Read and parse `lens.conf` with the `pimax-lens` parser. Returns the text and the parser warnings.
    pub fn read_lens(&self, id: &ProfileId) -> Result<(String, Vec<String>), ConfigError> {
        let t = self
            .read_text(id, "lens.conf")?
            .ok_or_else(|| ConfigError::NotFound(format!("{id}/lens.conf")))?;
        let (_, w) = pimax_lens::config::parse(&t, 1.0);
        Ok((
            t,
            w.iter()
                .map(|w| format!("line {}: {}", w.line, w.message))
                .collect(),
        ))
    }

    /// Write `lens.conf` (validated, backed up, atomic).
    pub fn write_lens(&self, id: &ProfileId, text: &str) -> Result<(), ConfigError> {
        self.write_file(id, "lens.conf", text.as_bytes())
    }

    /// Read `setup.toml` (defaults if absent).
    pub fn read_setup(&self, id: &ProfileId) -> Result<Setup, ConfigError> {
        match self.read_text(id, "setup.toml")? {
            None => Ok(Setup::default()),
            Some(t) => Setup::parse(&t).map_err(ConfigError::Invalid),
        }
    }

    /// Write `setup.toml`.
    pub fn write_setup(&self, id: &ProfileId, s: &Setup) -> Result<(), ConfigError> {
        self.write_file(id, "setup.toml", s.to_toml().as_bytes())
    }

    /// The one write path: name check, validation, backup of the current profile, atomic replace.
    pub fn write_file(&self, id: &ProfileId, file: &str, bytes: &[u8]) -> Result<(), ConfigError> {
        check_file(file)?;
        let text = String::from_utf8_lossy(bytes);
        match file {
            "lens.conf" => validate_lens(&text)?,
            "setup.toml" => {
                Setup::parse(&text).map_err(ConfigError::Invalid)?;
            }
            _ => {}
        }
        let dir = self.profile_dir(id);
        io(
            format!("create {}", dir.display()),
            fs::create_dir_all(&dir),
        )?;
        if self.has_files(id) {
            self.backup(id, &format!("write-{}", file.replace('.', "-")))?;
        }
        atomic_write(&dir.join(file), bytes)
    }

    fn has_files(&self, id: &ProfileId) -> bool {
        MANAGED_FILES
            .iter()
            .any(|f| self.profile_dir(id).join(f).is_file())
    }

    fn now(&self) -> u64 {
        (self.clock)()
    }

    /// Copy the whole profile into `backups/<serial>/<stamp>-<nn>-<reason>/` and rotate old backups.
    pub fn backup(&self, id: &ProfileId, reason: &str) -> Result<BackupInfo, ConfigError> {
        let reason: String = reason
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .take(40)
            .collect();
        // `<stamp>-<nn>-<reason>`: nn counts backups of the same second, so name order is time order
        let st = stamp(self.now());
        let bdir = self.backups_dir(&id.serial);
        let mut seq = sorted_dirs(&bdir)
            .iter()
            .filter(|n| n.starts_with(&st))
            .count()
            + 1;
        let mut name = format!("{st}-{seq:02}-{reason}");
        while bdir.join(&name).exists() {
            seq += 1;
            name = format!("{st}-{seq:02}-{reason}");
        }
        let dest = bdir.join(&name);
        io(
            format!("create {}", dest.display()),
            fs::create_dir_all(&dest),
        )?;
        let src = self.profile_dir(id);
        let mut files = Vec::new();
        for f in MANAGED_FILES {
            let p = src.join(f);
            if p.is_file() {
                io(format!("copy {}", p.display()), fs::copy(&p, dest.join(f)))?;
                files.push(f.to_string());
            }
        }
        // remember which profile it came from (restore picks the target explicitly; this is for humans)
        let _ = fs::write(dest.join(".from"), id.to_string());
        self.rotate(&id.serial);
        Ok(BackupInfo {
            serial: id.serial.clone(),
            name,
            path: dest,
            files,
        })
    }

    fn rotate(&self, serial: &str) {
        let list = self.list_backups(serial);
        for old in list.iter().skip(KEEP_BACKUPS) {
            let _ = fs::remove_dir_all(&old.path);
        }
    }

    /// Backups of a serial, newest first.
    pub fn list_backups(&self, serial: &str) -> Vec<BackupInfo> {
        if check_name(serial).is_err() {
            return Vec::new();
        }
        let mut names = sorted_dirs(&self.backups_dir(serial));
        names.reverse();
        names
            .into_iter()
            .map(|name| {
                let path = self.backups_dir(serial).join(&name);
                let files = MANAGED_FILES
                    .iter()
                    .filter(|f| path.join(f).is_file())
                    .map(|f| f.to_string())
                    .collect();
                BackupInfo {
                    serial: serial.into(),
                    name,
                    path,
                    files,
                }
            })
            .collect()
    }

    /// Make the profile equal to the backup. The current state is backed up first (`pre-restore`).
    pub fn restore(&self, id: &ProfileId, backup: &BackupInfo) -> Result<(), ConfigError> {
        check_name(&backup.name)?;
        let src = self.backups_dir(&id.serial).join(&backup.name);
        if !src.is_dir() {
            return Err(ConfigError::NotFound(format!("backup {}", backup.name)));
        }
        let dir = self.profile_dir(id);
        io(
            format!("create {}", dir.display()),
            fs::create_dir_all(&dir),
        )?;
        if self.has_files(id) {
            self.backup(id, "pre-restore")?;
        }
        for f in MANAGED_FILES {
            let from = src.join(f);
            let to = dir.join(f);
            if from.is_file() {
                let bytes = io(format!("read {}", from.display()), fs::read(&from))?;
                atomic_write(&to, &bytes)?;
            } else if to.is_file() {
                io(format!("remove {}", to.display()), fs::remove_file(&to))?;
            }
        }
        Ok(())
    }

    /// Text of a managed file inside a backup.
    pub fn read_backup_text(&self, backup: &BackupInfo, file: &str) -> Option<String> {
        check_file(file).ok()?;
        fs::read(backup.path.join(file))
            .ok()
            .map(|b| String::from_utf8_lossy(&b).into_owned())
    }

    /// Import the legacy files (`<root>/lens.conf`, `eye-correction.toml`, `cam-calib-overlay.json`) into a new profile.
    /// The legacy files stay where they are. Returns the imported file names.
    pub fn import_legacy(&self, id: &ProfileId) -> Result<Vec<String>, ConfigError> {
        let lens = self.root.join("lens.conf");
        let lens_text = fs::read_to_string(&lens)
            .map_err(|e| ConfigError::Io(format!("read {}", lens.display()), e))?;
        self.create_profile(id, &lens_text)?;
        let mut done = vec!["lens.conf".to_string()];
        for f in ["eye-correction.toml", "cam-calib-overlay.json"] {
            let p = self.root.join(f);
            if let Ok(b) = fs::read(&p) {
                self.write_file(id, f, &b)?;
                done.push(f.into());
            }
        }
        let meta_path = self.profile_dir(id).join("meta.toml");
        let mut meta = fs::read_to_string(&meta_path).unwrap_or_default();
        meta += &format!(
            "imported_from = \"{}\"\nimported = \"{}\"\n",
            self.root.display(),
            done.join(" ")
        );
        self.write_file(id, "meta.toml", meta.as_bytes())?;
        Ok(done)
    }

    /// The serial named in the header comment of the legacy `lens.conf` (`... serial P511... ;`), if any.
    pub fn legacy_lens_serial(&self) -> Option<String> {
        let t = fs::read_to_string(self.root.join("lens.conf")).ok()?;
        serial_in_comments(&t)
    }

    /// Make `id` the active profile: `<root>/active` symlink. With `replace_legacy`, also turn `<root>/lens.conf` into a
    /// symlink to `active/lens.conf` (a regular file is first copied to `backups/legacy/`), so the old path keeps working
    /// for monado-service. A profile of another serial needs `confirm_mismatch`.
    pub fn activate(
        &self,
        id: &ProfileId,
        connected_serial: Option<&str>,
        confirm_mismatch: bool,
        replace_legacy: bool,
    ) -> Result<(), ConfigError> {
        if !self.profile_dir(id).is_dir() {
            return Err(ConfigError::NotFound(id.to_string()));
        }
        if check_serial(&id.serial, connected_serial) == SerialCheck::Mismatch && !confirm_mismatch
        {
            return Err(ConfigError::SerialMismatch {
                profile: id.serial.clone(),
                connected: connected_serial.unwrap_or("").into(),
            });
        }
        io(
            format!("create {}", self.root.display()),
            fs::create_dir_all(&self.root),
        )?;
        let target = PathBuf::from("profiles").join(&id.serial).join(&id.name);
        replace_symlink(&target, &self.root.join("active"))?;
        if replace_legacy {
            let legacy = self.root.join("lens.conf");
            if let Ok(m) = fs::symlink_metadata(&legacy) {
                if m.file_type().is_file() {
                    let dest = self.root.join("backups").join("legacy");
                    io("create legacy backup dir", fs::create_dir_all(&dest))?;
                    io(
                        "back up legacy lens.conf",
                        fs::copy(
                            &legacy,
                            dest.join(format!("{}-lens.conf", stamp(self.now()))),
                        ),
                    )?;
                }
            }
            replace_symlink(Path::new("active/lens.conf"), &legacy)?;
        }
        Ok(())
    }
}

/// Serial from a comment such as `# headset Pimax Dream Air serial P511X0X000000000002; ...`.
pub fn serial_in_comments(text: &str) -> Option<String> {
    for l in text.lines().filter(|l| l.trim_start().starts_with('#')) {
        let mut it = l.split_whitespace();
        while let Some(w) = it.next() {
            if w == "serial" {
                let s: String = it
                    .next()?
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
                    .collect();
                if !s.is_empty() {
                    return Some(s);
                }
            }
        }
    }
    None
}

fn check_file(f: &str) -> Result<(), ConfigError> {
    if MANAGED_FILES.contains(&f) {
        Ok(())
    } else {
        Err(ConfigError::BadFile(f.into()))
    }
}

fn validate_lens(text: &str) -> Result<(), ConfigError> {
    let (_, w) = pimax_lens::config::parse(text, 1.0);
    if w.is_empty() {
        Ok(())
    } else {
        Err(ConfigError::Invalid(
            w.iter()
                .map(|w| format!("lens.conf line {}: {}", w.line, w.message))
                .collect::<Vec<_>>()
                .join("; "),
        ))
    }
}

fn sorted_dirs(p: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(p)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    v.sort();
    v
}

/// Temp file in the same directory, fsync, rename, fsync the directory.
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), ConfigError> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let tmp = dir.join(format!(
        ".{}.tmp{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("f"),
        std::process::id()
    ));
    let res = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::rename(&tmp, path)?;
        if let Ok(d) = fs::File::open(dir) {
            let _ = d.sync_all();
        }
        Ok(())
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    io(format!("write {}", path.display()), res)
}

/// Replace `link` by a symlink to `target` atomically (never replaces a directory).
fn replace_symlink(target: &Path, link: &Path) -> Result<(), ConfigError> {
    if let Ok(m) = fs::symlink_metadata(link) {
        if m.is_dir() {
            return Err(ConfigError::Invalid(format!(
                "{} is a directory",
                link.display()
            )));
        }
    }
    let tmp = link.with_extension(format!("lnk{}", std::process::id()));
    let _ = fs::remove_file(&tmp);
    io("create symlink", std::os::unix::fs::symlink(target, &tmp))?;
    let r = fs::rename(&tmp, link);
    if r.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    io(format!("replace {}", link.display()), r)
}

/// `YYYYMMDD-HHMMSS` (UTC) of seconds since the epoch.
pub fn stamp(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    // civil from days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    pub(crate) struct Tmp(pub PathBuf);
    impl Tmp {
        pub fn new(tag: &str) -> Tmp {
            static N: AtomicU64 = AtomicU64::new(0);
            let p = std::env::temp_dir().join(format!(
                "pimax-config-test-{}-{}-{tag}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const LENS: &str = "# headset Pimax Dream Air serial P511TEST; made by test\ncant_deg = 2.0\n[left]\ncenter = 0.027 0.009\n";

    fn store(t: &Tmp) -> (Store, Arc<AtomicU64>) {
        let clock = Arc::new(AtomicU64::new(1_760_000_000));
        let c = clock.clone();
        let s = Store::with_clock(t.0.join("cfg"), t.0.join("cache"), move || {
            c.fetch_add(1, Ordering::SeqCst)
        });
        (s, clock)
    }

    #[test]
    fn stamp_known_value() {
        assert_eq!(stamp(0), "19700101-000000");
        assert_eq!(stamp(1_760_000_000), "20251009-085320");
    }

    #[test]
    fn names_are_path_safe() {
        assert!(ProfileId::new("../x", "default").is_err());
        assert!(ProfileId::new("P1", "a/b").is_err());
        assert!(ProfileId::new("", "a").is_err());
        assert!(ProfileId::new("P511X0X000000000002", "default").is_ok());
    }

    #[test]
    fn create_write_read_round_trip_with_backups() {
        let t = Tmp::new("rw");
        let (s, _) = store(&t);
        let id = ProfileId::new("P511TEST", "default").unwrap();
        s.create_profile(&id, LENS).unwrap();
        assert_eq!(s.list_profiles(), vec![id.clone()]);
        let (txt, warn) = s.read_lens(&id).unwrap();
        assert_eq!(txt, LENS);
        assert!(warn.is_empty());
        assert!(s.list_backups("P511TEST").len() <= 1); // meta.toml write backed up lens.conf only
        let n0 = s.list_backups("P511TEST").len();
        let new = LENS.replace("2.0", "2.5");
        s.write_lens(&id, &new).unwrap();
        assert_eq!(s.list_backups("P511TEST").len(), n0 + 1);
        let b = &s.list_backups("P511TEST")[0];
        assert_eq!(s.read_backup_text(b, "lens.conf").unwrap(), LENS);
        assert_eq!(s.read_lens(&id).unwrap().0, new);
        // no temp files left behind
        let left: Vec<_> = fs::read_dir(s.profile_dir(&id))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with('.'))
            .collect();
        assert!(left.is_empty());
    }

    #[test]
    fn invalid_lens_is_refused_and_leaves_file_alone() {
        let t = Tmp::new("bad");
        let (s, _) = store(&t);
        let id = ProfileId::new("P511TEST", "default").unwrap();
        s.create_profile(&id, LENS).unwrap();
        let before = s.list_backups("P511TEST").len();
        let e = s.write_lens(&id, "cant_deg = 999\n").unwrap_err();
        assert!(matches!(e, ConfigError::Invalid(_)), "{e}");
        assert_eq!(s.read_lens(&id).unwrap().0, LENS);
        assert_eq!(s.list_backups("P511TEST").len(), before);
        assert!(matches!(
            s.write_file(&id, "../evil", b"x").unwrap_err(),
            ConfigError::BadFile(_)
        ));
    }

    #[test]
    fn restore_returns_previous_state_and_backs_up_current() {
        let t = Tmp::new("restore");
        let (s, _) = store(&t);
        let id = ProfileId::new("P511TEST", "default").unwrap();
        s.create_profile(&id, LENS).unwrap();
        s.write_setup(
            &id,
            &Setup {
                fps: Some(90),
                ..Default::default()
            },
        )
        .unwrap();
        let lens2 = LENS.replace("2.0", "3.0");
        s.write_lens(&id, &lens2).unwrap();
        // newest backup holds LENS + setup.toml; restoring it brings back cant 2.0
        let b = s.list_backups("P511TEST")[0].clone();
        assert_eq!(s.read_backup_text(&b, "lens.conf").unwrap(), LENS);
        s.restore(&id, &b).unwrap();
        assert_eq!(s.read_lens(&id).unwrap().0, LENS);
        // the state before the restore is itself backed up
        let newest = &s.list_backups("P511TEST")[0];
        assert!(newest.name.ends_with("pre-restore"), "{}", newest.name);
        assert_eq!(s.read_backup_text(newest, "lens.conf").unwrap(), lens2);
    }

    #[test]
    fn restore_removes_files_absent_from_backup() {
        let t = Tmp::new("restore2");
        let (s, _) = store(&t);
        let id = ProfileId::new("P511TEST", "default").unwrap();
        s.create_profile(&id, LENS).unwrap();
        let b = s.backup(&id, "manual").unwrap();
        s.write_file(&id, "passthrough.toml", b"x = 1\n").unwrap();
        s.restore(&id, &b).unwrap();
        assert!(s.read_file(&id, "passthrough.toml").unwrap().is_none());
    }

    #[test]
    fn backups_rotate_to_30() {
        let t = Tmp::new("rot");
        let (s, _) = store(&t);
        let id = ProfileId::new("P511TEST", "default").unwrap();
        s.create_profile(&id, LENS).unwrap();
        for i in 0..40 {
            s.write_setup(
                &id,
                &Setup {
                    fps: Some(if i % 2 == 0 { 72 } else { 90 }),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        assert_eq!(s.list_backups("P511TEST").len(), 30);
    }

    #[test]
    fn same_second_backups_do_not_collide() {
        let t = Tmp::new("coll");
        let s = Store::with_clock(t.0.join("cfg"), t.0.join("cache"), || 1_760_000_000);
        let id = ProfileId::new("P511TEST", "default").unwrap();
        s.create_profile(&id, LENS).unwrap();
        let a = s.backup(&id, "x").unwrap();
        let b = s.backup(&id, "x").unwrap();
        assert_ne!(a.path, b.path);
    }

    #[test]
    fn serial_check_and_activate() {
        let t = Tmp::new("act");
        let (s, _) = store(&t);
        let id = ProfileId::new("P511TEST", "default").unwrap();
        s.create_profile(&id, LENS).unwrap();
        assert_eq!(check_serial("A", Some("A")), SerialCheck::Match);
        assert_eq!(check_serial("A", Some("B")), SerialCheck::Mismatch);
        assert_eq!(check_serial("A", None), SerialCheck::Unknown);
        let e = s.activate(&id, Some("OTHER"), false, false).unwrap_err();
        assert!(matches!(e, ConfigError::SerialMismatch { .. }));
        assert!(s.active().is_none());
        s.activate(&id, Some("OTHER"), true, false).unwrap();
        assert_eq!(s.active(), Some(id.clone()));
        // switching again is atomic and works
        let id2 = ProfileId::new("P511TEST", "second").unwrap();
        s.create_profile(&id2, LENS).unwrap();
        s.activate(&id2, Some("P511TEST"), false, false).unwrap();
        assert_eq!(s.active(), Some(id2));
    }

    #[test]
    fn legacy_import_keeps_old_files_and_activation_links_them() {
        let t = Tmp::new("legacy");
        let (s, _) = store(&t);
        fs::create_dir_all(s.root()).unwrap();
        fs::write(s.root().join("lens.conf"), LENS).unwrap();
        fs::write(s.root().join("eye-correction.toml"), "dx = 1\n").unwrap();
        fs::write(s.root().join("cam-calib-overlay.json"), "{}").unwrap();
        assert_eq!(s.legacy_lens_serial().as_deref(), Some("P511TEST"));
        let id = ProfileId::new("P511TEST", "default").unwrap();
        let done = s.import_legacy(&id).unwrap();
        assert_eq!(
            done,
            vec!["lens.conf", "eye-correction.toml", "cam-calib-overlay.json"]
        );
        assert_eq!(
            s.read_text(&id, "eye-correction.toml").unwrap().unwrap(),
            "dx = 1\n"
        );
        assert!(
            s.root().join("lens.conf").is_file(),
            "legacy file untouched"
        );
        assert!(s
            .read_text(&id, "meta.toml")
            .unwrap()
            .unwrap()
            .contains("imported_from"));
        s.activate(&id, Some("P511TEST"), false, true).unwrap();
        let l = s.root().join("lens.conf");
        assert!(fs::symlink_metadata(&l).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_to_string(&l).unwrap(), LENS); // old path still reads the profile's lens.conf
        assert_eq!(
            fs::read_dir(s.root().join("backups/legacy"))
                .unwrap()
                .count(),
            1
        );
        // importing twice is refused
        assert!(matches!(
            s.import_legacy(&id).unwrap_err(),
            ConfigError::Exists(_)
        ));
    }

    #[test]
    fn gyro_bias_paths() {
        let c = Path::new("/c/pimax");
        assert_eq!(gyro_bias_path(c, "S1"), Path::new("/c/pimax/S1/gyro-bias"));
        assert_eq!(legacy_gyro_bias_path(c), Path::new("/c/pimax/gyro-bias"));
    }

    #[test]
    fn from_env_uses_xdg() {
        let s = Store::from_env(&|k| match k {
            "HOME" => Some("/h".into()),
            "XDG_CONFIG_HOME" => Some("/x".into()),
            _ => None,
        });
        assert_eq!(s.root(), Path::new("/x/pimax"));
        assert_eq!(s.cache(), Path::new("/h/.cache/pimax"));
    }
}
