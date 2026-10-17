//! names:keep-file (this module reads the places of the names before the rename)
//! "Import from the previous install": `omarchyvr import-previous`, run by `./install.sh` and by the app's automatic
//! setup (its first user step, no password), and from the app's Checks page.
//!
//! After a clean uninstall and reinstall the user's own per-device data and settings are still on this PC, in older
//! places: the names before the rename (`~/.config/pimax`, `~/.cache/pimax`, `~/.local/share/pimax`), the lab tree
//! (`~/pimax-re`) with its copy of Pimax's Windows data (`pimax-data/ProgramData-Pimax`), a mounted shared Windows
//! drive (`/mnt/shared`), and the uninstaller's own backups (`~/omarchyvr-settings-backup-*.tar.gz`,
//! `~/omarchyvr-home-backup-*.tar.gz`, newest first). This copies what is missing into the current places.
//!
//! Rules: a file that is there is never overwritten (the seeded reference gyro bias is not the user's: a measured one
//! replaces it); nothing is deleted; `ai.toml` (the cloud keys) is never read or copied here (the uninstaller keeps it
//! in place); imported files are written mode 600; every run lists what it did in plain words and records it in
//! `~/.local/state/omarchyvr/import.txt` (with the backups it looked at, so the next check is cheap).
use crate::devicedata;
use std::collections::HashMap;
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};

/// The state file, relative to `$HOME`.
pub const STATE_FILE: &str = ".local/state/omarchyvr/import.txt";

/// Where a mounted shared Windows drive is (`omarchyvr mount-shared`).
pub const SHARED_MOUNT: &str = "/mnt/shared";

/// Largest backup member that is read into memory (a room map is about 0.5 MB).
const MAX_MEMBER: u64 = 64 << 20;

/// Largest backup archive that is opened at all.
const MAX_ARCHIVE: u64 = 8 << 30;

/// How a wanted item is copied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// One file.
    File,
    /// A directory with everything below it.
    Dir,
}

/// One thing a previous install may have left.
#[derive(Debug, Clone)]
struct Want {
    /// Plain words.
    what: &'static str,
    /// The current place, relative to `$HOME`.
    dest: &'static str,
    kind: Kind,
    /// Older places relative to `$HOME`, best first (also looked for inside the backups, with `dest` itself).
    old: &'static [&'static str],
    /// Per-device data (kept by the uninstaller from now on) rather than a preference.
    device: bool,
}

/// Everything the import looks for (the headset calibration, the controller models and the gyro bias have their own
/// steps: they have more sources or replace a reference value).
const WANTS: &[Want] = &[
    Want { what: "lens profile", dest: ".config/omarchyvr/lens.conf", kind: Kind::File, old: &[".config/pimax/lens.conf"], device: true },
    Want { what: "lens variants", dest: ".config/omarchyvr/lens-ipd", kind: Kind::Dir, old: &[".config/pimax/lens-ipd"], device: true },
    Want { what: "IPD", dest: ".config/omarchyvr/ipd.env", kind: Kind::File, old: &[".config/pimax/ipd.env"], device: true },
    Want { what: "eye calibration", dest: ".config/omarchyvr/eye-calibration.toml", kind: Kind::File, old: &[".config/pimax/eye-calibration.toml"], device: true },
    Want { what: "eye correction", dest: ".config/omarchyvr/eye-correction.toml", kind: Kind::File, old: &[".config/pimax/eye-correction.toml"], device: true },
    Want { what: "controller-to-camera fit", dest: ".config/omarchyvr/ctrl-cam-extrinsics.json", kind: Kind::File, old: &[".config/pimax/ctrl-cam-extrinsics.json"], device: true },
    Want { what: "controller gyro fit", dest: ".config/omarchyvr/ctrl-imu.json", kind: Kind::File, old: &[".config/pimax/ctrl-imu.json"], device: true },
    Want { what: "6DoF IMU noise", dest: ".config/omarchyvr/slam-imu.json", kind: Kind::File, old: &[".config/pimax/slam-imu.json"], device: true },
    Want { what: "room map", dest: ".local/share/omarchyvr/roommap", kind: Kind::Dir, old: &[".local/share/pimax/roommap"], device: true },
    Want { what: "room settings", dest: ".config/omarchyvr/room.toml", kind: Kind::File, old: &[".config/pimax/room.toml"], device: false },
    Want { what: "room setup", dest: ".config/omarchyvr/room-setup.toml", kind: Kind::File, old: &[".config/pimax/room-setup.toml"], device: false },
    Want { what: "desk anchor", dest: ".config/omarchyvr/anchor.toml", kind: Kind::File, old: &[".config/pimax/anchor.toml"], device: false },
    Want { what: "PC paths (shared drive)", dest: ".config/omarchyvr/paths.toml", kind: Kind::File, old: &[".config/pimax/paths.toml"], device: false },
    Want { what: "Work first-run answers", dest: ".config/omarchyvr/work-setup.toml", kind: Kind::File, old: &[], device: false },
    Want { what: "VR background", dest: ".config/omarchyvr/background.toml", kind: Kind::File, old: &[".config/pimax/background.toml"], device: false },
    Want { what: "game mode", dest: ".config/omarchyvr/gamemode.toml", kind: Kind::File, old: &[".config/pimax/gamemode.toml"], device: false },
    Want { what: "games run with", dest: ".config/omarchyvr/games.toml", kind: Kind::File, old: &[".config/pimax/games.toml"], device: false },
    Want { what: "per-game runtime", dest: ".config/omarchyvr/game-runtime.json", kind: Kind::File, old: &[".config/pimax/game-runtime.json"], device: false },
    Want { what: "recording", dest: ".config/omarchyvr/record.toml", kind: Kind::File, old: &[".config/pimax/record.toml"], device: false },
    Want { what: "brightness", dest: ".config/omarchyvr/display.toml", kind: Kind::File, old: &[".config/pimax/display.toml"], device: false },
    Want { what: "sound", dest: ".config/omarchyvr/audio.toml", kind: Kind::File, old: &[".config/pimax/audio.toml"], device: false },
    Want { what: "eye tracking settings", dest: ".config/omarchyvr/eye.conf", kind: Kind::File, old: &[".config/pimax/eye.conf"], device: false },
    Want { what: "Work panels", dest: ".config/omarchyvr/work-panels.conf", kind: Kind::File, old: &[".config/pimax/work-panels.conf"], device: false },
    Want { what: "WayVR settings", dest: ".config/omarchyvr/wayvr-desk.conf", kind: Kind::File, old: &[".config/pimax/wayvr-desk.conf"], device: false },
    Want { what: "auto mode", dest: ".config/omarchyvr/auto.toml", kind: Kind::File, old: &[".config/pimax/auto.toml"], device: false },
];

/// Relative paths (from `$HOME`) of everything the uninstaller now keeps as device data (`installer/uninstall.sh`
/// reads the same list from `omarchyvr import-previous --device-paths`).
pub fn device_paths() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = WANTS.iter().filter(|w| w.device).map(|w| w.dest).collect();
    v.extend([".config/omarchyvr/headset-calibration.xml", ".config/omarchyvr/controllers", ".cache/omarchyvr/gyro-bias"]);
    v
}

/// What happened to one item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// Already in its current place (nothing to do).
    Present,
    /// Copied now (or, in a dry run, would be) from this source.
    Imported(String),
    /// No copy anywhere on this PC.
    NotFound,
    /// Could not be written.
    Failed(String),
}

/// One line of the result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub what: String,
    pub state: State,
}

impl Line {
    fn new(what: &str, state: State) -> Line {
        Line { what: what.to_string(), state }
    }

    /// One plain sentence.
    pub fn describe(&self) -> String {
        match &self.state {
            State::Present => format!("{}: already there", self.what),
            State::Imported(from) => format!("{}: brought back from {from}", self.what),
            State::NotFound => format!("{}: no copy on this PC", self.what),
            State::Failed(e) => format!("{}: could not be copied ({e})", self.what),
        }
    }
}

/// Where the import looks (all overridable for the tests).
#[derive(Debug, Clone)]
pub struct Places {
    pub home: PathBuf,
    /// The lab tree (`~/pimax-re`).
    pub lab: PathBuf,
    /// Clones that may hold a copy of Pimax's Windows data (`<clone>/pimax-data/ProgramData-Pimax`).
    pub clones: Vec<PathBuf>,
    /// A mounted shared Windows drive (searched for `ProgramData-Pimax` / `ProgramData/Pimax`), if any.
    pub shared: Option<PathBuf>,
    /// Backup archives, newest first.
    pub backups: Vec<PathBuf>,
    /// `$XDG_CACHE_HOME` when it is set (the driver's gyro-bias cache follows it); never read from the process
    /// environment inside [`run`] / [`pending`], so a test's scratch home stays scratch.
    pub xdg_cache: Option<PathBuf>,
}

impl Places {
    /// The environment the device-data paths see: only `XDG_CACHE_HOME`, from [`Places::xdg_cache`].
    fn env(&self) -> impl Fn(&str) -> Option<String> + '_ {
        move |k: &str| (k == "XDG_CACHE_HOME").then(|| self.xdg_cache.as_ref().map(|c| c.display().to_string())).flatten()
    }
}

impl Places {
    /// The places of this PC.
    pub fn for_home(home: &Path) -> Places {
        let shared = Path::new(SHARED_MOUNT);
        let mounted = std::fs::read_to_string("/proc/mounts")
            .map(|m| m.lines().any(|l| l.split_whitespace().nth(1) == Some(SHARED_MOUNT)))
            .unwrap_or(false);
        Places {
            home: home.to_path_buf(),
            lab: home.join("pimax-re"),
            clones: vec![home.join("omarchyvr"), crate::paths::repo_dir(home)],
            shared: mounted.then(|| shared.to_path_buf()),
            backups: list_backups(home),
            xdg_cache: std::env::var_os("XDG_CACHE_HOME").filter(|v| !v.is_empty()).map(PathBuf::from),
        }
    }
}

/// `~/omarchyvr-settings-backup-*.tar.gz` and `~/omarchyvr-home-backup-*.tar.gz`, newest first (by modification time).
pub fn list_backups(home: &Path) -> Vec<PathBuf> {
    let mut v: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(home)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter(|e| {
                    let n = e.file_name().to_string_lossy().into_owned();
                    (n.starts_with("omarchyvr-settings-backup-") || n.starts_with("omarchyvr-home-backup-")) && n.ends_with(".tar.gz")
                })
                .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
                .collect()
        })
        .unwrap_or_default();
    v.sort_by_key(|e| std::cmp::Reverse(e.0));
    v.into_iter().map(|(_, p)| p).collect()
}

// ---- a small streaming tar.gz reader (GNU and ustar/pax names) ----------------------------------------------------

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

fn tar_size(f: &[u8]) -> io::Result<u64> {
    if f[0] & 0x80 != 0 {
        // GNU base-256
        return Ok(f[1..].iter().fold(0u64, |a, &b| a.saturating_mul(256).saturating_add(u64::from(b))));
    }
    let s = cstr(f);
    let s = s.trim_matches(|c: char| c == ' ' || c == '\0');
    if s.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(s, 8).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad tar size"))
}

fn skip(r: &mut dyn Read, n: u64) -> io::Result<()> {
    let got = io::copy(&mut r.take(n), &mut io::sink())?;
    if got < n {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "truncated tar"));
    }
    Ok(())
}

fn read_n(r: &mut dyn Read, n: u64) -> io::Result<Vec<u8>> {
    let mut v = Vec::with_capacity(n as usize);
    r.take(n).read_to_end(&mut v)?;
    if (v.len() as u64) < n {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "truncated tar"));
    }
    Ok(v)
}

/// `path=` of a pax extended header.
fn pax_path(data: &[u8]) -> Option<String> {
    String::from_utf8_lossy(data).lines().find_map(|l| l.split_once(' ').and_then(|(_, kv)| kv.strip_prefix("path=")).map(str::to_string))
}

/// Read every regular file of a gzip'ed tar stream whose name `want` accepts (and that is at most `max` bytes).
pub fn tar_files(r: impl Read, want: &dyn Fn(&str) -> bool, max: u64, on: &mut dyn FnMut(String, Vec<u8>)) -> io::Result<()> {
    let mut r = BufReader::new(flate2::read::MultiGzDecoder::new(BufReader::new(r)));
    let mut hdr = [0u8; 512];
    let (mut long, mut pax): (Option<String>, Option<String>) = (None, None);
    loop {
        let mut got = 0;
        while got < 512 {
            let n = r.read(&mut hdr[got..])?;
            if n == 0 {
                break;
            }
            got += n;
        }
        if got == 0 || hdr.iter().all(|&b| b == 0) {
            return Ok(()); // end of the archive
        }
        if got < 512 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "truncated tar header"));
        }
        let size = tar_size(&hdr[124..136])?;
        let pad = size.div_ceil(512) * 512 - size;
        match hdr[156] {
            b'L' => {
                long = Some(cstr(&read_n(&mut r, size)?));
                skip(&mut r, pad)?;
                continue;
            }
            b'x' => {
                pax = pax_path(&read_n(&mut r, size)?);
                skip(&mut r, pad)?;
                continue;
            }
            b'g' | b'K' => {
                skip(&mut r, size + pad)?;
                continue;
            }
            _ => {}
        }
        let name = pax.take().or(long.take()).unwrap_or_else(|| {
            let (n, prefix) = (cstr(&hdr[0..100]), cstr(&hdr[345..500]));
            if &hdr[257..262] == b"ustar" && !prefix.is_empty() { format!("{prefix}/{n}") } else { n }
        });
        let regular = matches!(hdr[156], b'0' | 0);
        if regular && size <= max && want(&name) {
            let data = read_n(&mut r, size)?;
            skip(&mut r, pad)?;
            on(name, data);
        } else {
            skip(&mut r, size + pad)?;
        }
    }
}

/// A member name relative to `$HOME`: the backups store `/home/<user>/...` (settings backup) or `.config/...` (home
/// backup). Any `home/<name>/` prefix is dropped, so a backup made under another login still matches.
fn member_rel(name: &str, home: &Path) -> String {
    let n = name.trim_start_matches("./").trim_start_matches('/');
    let h = home.to_string_lossy();
    let h = h.trim_start_matches('/').trim_end_matches('/');
    if let Some(r) = n.strip_prefix(h).and_then(|r| r.strip_prefix('/')) {
        return r.to_string();
    }
    match n.strip_prefix("home/").and_then(|r| r.split_once('/')) {
        Some((_, rest)) => rest.to_string(),
        None => n.to_string(),
    }
}

/// Never touched here: the cloud keys (the uninstaller keeps them; their own handling).
fn is_keys(rel: &str) -> bool {
    rel.rsplit('/').next().is_some_and(|f| f.starts_with("ai.toml"))
}

/// The members of the backups that any wanted item could use, by relative path; the first (newest) archive that has
/// a path wins. `scanned` gets every archive that was read.
fn read_backups(p: &Places, rels: &[String], prefixes: &[String], scanned: &mut Vec<PathBuf>) -> HashMap<String, (PathBuf, Vec<u8>)> {
    let mut out: HashMap<String, (PathBuf, Vec<u8>)> = HashMap::new();
    for b in &p.backups {
        if std::fs::metadata(b).map(|m| m.len() > MAX_ARCHIVE).unwrap_or(true) {
            scanned.push(b.clone());
            continue;
        }
        // looked at, whatever comes of it (an unreadable archive must not stay "pending" for ever)
        scanned.push(b.clone());
        let Ok(f) = std::fs::File::open(b) else { continue };
        let home = p.home.clone();
        let want = |name: &str| {
            let rel = member_rel(name, &home);
            !is_keys(&rel) && (rels.contains(&rel) || prefixes.iter().any(|d| rel.starts_with(&format!("{d}/"))))
        };
        let mut found: Vec<(String, Vec<u8>)> = Vec::new();
        // a damaged archive still gives the members before the damage
        let _ = tar_files(f, &want, MAX_MEMBER, &mut |name, data| found.push((member_rel(&name, &p.home), data)));
        for (rel, data) in found {
            out.entry(rel).or_insert_with(|| (b.clone(), data));
        }
    }
    out
}

fn tilde(home: &Path, p: &Path) -> String {
    match p.strip_prefix(home) {
        Ok(r) => format!("~/{}", r.display()),
        Err(_) => p.display().to_string(),
    }
}

fn copy_file(from: &Path, to: &Path) -> io::Result<()> {
    devicedata::write_atomic(to, &std::fs::read(from)?, 0o600)
}

/// Copy a directory tree (regular files only; links are not followed).
fn copy_dir(from: &Path, to: &Path) -> io::Result<usize> {
    let mut n = 0;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let t = e.file_type()?;
        let dst = to.join(e.file_name());
        if t.is_dir() {
            n += copy_dir(&e.path(), &dst)?;
        } else if t.is_file() {
            copy_file(&e.path(), &dst)?;
            n += 1;
        }
    }
    Ok(n)
}

fn exists(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok()
}

/// A lens.conf of the old place may name a LUT file in the old directory: point it at the new one.
fn fix_lens_paths(text: &[u8]) -> Vec<u8> {
    String::from_utf8_lossy(text).replace("/.config/pimax/", "/.config/omarchyvr/").into_bytes()
}

/// Folders with Pimax's `controller_params.json` (one per controller serial) in a copy of Pimax's Windows data.
fn pimax_controller_dirs(h1: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(h1)
        .map(|rd| rd.filter_map(Result::ok).map(|e| e.path()).filter(|d| d.join("controller_params.json").is_file()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// The `slam` directories of every copy of Pimax's Windows data under `root` (bounded walk: depth 6, 20000 dirs).
fn find_pimax_slam(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut seen = 0usize;
    while let Some((d, depth)) = stack.pop() {
        seen += 1;
        if seen > 20_000 {
            break;
        }
        let name = d.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let parent = d.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        if name == "programdata-pimax" || (name == "pimax" && parent == "programdata") {
            let slam = d.join("slam");
            if slam.is_dir() {
                out.push(slam);
            }
            continue;
        }
        if depth >= 6 {
            continue;
        }
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.filter_map(Result::ok) {
                if e.file_type().is_ok_and(|t| t.is_dir()) {
                    stack.push((e.path(), depth + 1));
                }
            }
        }
    }
    out.sort();
    out
}

/// Every place with a copy of Pimax's Windows `slam` directory: the lab dump, the clones, the shared drive.
fn slam_dirs(p: &Places) -> Vec<PathBuf> {
    let mut v = Vec::new();
    for root in std::iter::once(&p.lab).chain(p.clones.iter()) {
        let s = root.join("pimax-data/ProgramData-Pimax/slam");
        if s.is_dir() && !v.contains(&s) {
            v.push(s);
        }
    }
    if let Some(s) = &p.shared {
        for d in find_pimax_slam(s) {
            if !v.contains(&d) {
                v.push(d);
            }
        }
    }
    v
}

/// Run the import (`dry`: say what would be copied, change nothing). Returns the lines in a fixed order.
pub fn run(p: &Places, dry: bool) -> Vec<Line> {
    let home = &p.home;
    let env = p.env();
    let mut lines = Vec::new();
    let mut scanned = Vec::new();
    // what the backups could provide
    let mut rels: Vec<String> = Vec::new();
    let mut prefixes: Vec<String> = vec![".config/pimax/controllers".into(), ".config/omarchyvr/controllers".into()];
    for w in WANTS {
        let all = std::iter::once(w.dest).chain(w.old.iter().copied());
        match w.kind {
            Kind::File => rels.extend(all.map(str::to_string)),
            Kind::Dir => prefixes.extend(all.map(str::to_string)),
        }
    }
    rels.extend([".cache/pimax/gyro-bias", ".cache/omarchyvr/gyro-bias", ".config/pimax/headset-calibration.xml", ".config/omarchyvr/headset-calibration.xml"].map(str::to_string));
    let backups = read_backups(p, &rels, &prefixes, &mut scanned);
    let from_backup = |rel: &str| backups.get(rel).map(|(b, d)| (format!("{} ({rel})", tilde(home, b)), d.clone()));

    // 1. the headset's factory calibration (private, mode 600): the same older places as the app's migration
    // (gui-core calibration.rs), plus copies of Pimax's Windows data
    let cal = crate::paths::headset_calibration_target(home);
    lines.push(if devicedata::own_calibration(home).is_some() {
        Line::new("headset calibration", State::Present)
    } else {
        let looks = |b: &[u8]| b.len() > 200 && b.iter().take(64).any(|&c| c == b'<');
        let mut src: Option<(String, Vec<u8>)> = crate::paths::headset_calibration_old(home)
            .into_iter()
            .find_map(|q| std::fs::read(&q).ok().filter(|b| looks(b)).map(|b| (tilde(home, &q), b)));
        if src.is_none() {
            src = slam_dirs(p).iter().find_map(|s| {
                let mut x: Vec<PathBuf> = std::fs::read_dir(s).ok()?.filter_map(Result::ok).map(|e| e.path())
                    .filter(|f| f.file_name().is_some_and(|n| n.to_string_lossy().starts_with("device_calibration") && n.to_string_lossy().ends_with(".xml")))
                    .collect();
                x.sort();
                x.into_iter().find_map(|f| std::fs::read(&f).ok().filter(|b| looks(b)).map(|b| (tilde(home, &f), b)))
            });
        }
        if src.is_none() {
            src = from_backup(".config/omarchyvr/headset-calibration.xml").or_else(|| from_backup(".config/pimax/headset-calibration.xml"));
        }
        match src {
            None => Line::new("headset calibration", State::NotFound),
            Some((from, _)) if dry => Line::new("headset calibration", State::Imported(from)),
            Some((from, b)) => match devicedata::write_atomic(&cal, &b, 0o600) {
                Ok(()) => Line::new("headset calibration", State::Imported(from)),
                Err(e) => Line::new("headset calibration", State::Failed(e.to_string())),
            },
        }
    });

    // 2. the controller LED models: Pimax's per-serial folders, or left/right files of an older config
    let dest = home.join(".config/omarchyvr/controllers");
    lines.push(if devicedata::own_controller_models(home).is_some() {
        Line::new("controller models", State::Present)
    } else {
        let old = home.join(".config/pimax/controllers");
        let pimax = slam_dirs(p).into_iter().map(|s| s.join("H1")).find(|h1| pimax_controller_dirs(h1).len() >= 2);
        let backup_files: Vec<(&String, &(PathBuf, Vec<u8>))> = backups.iter().filter(|(r, _)| r.contains("/controllers/")).collect();
        if devicedata::has_controller_pair(&old) {
            if dry {
                Line::new("controller models", State::Imported(tilde(home, &old)))
            } else {
                match copy_dir(&old, &dest) {
                    Ok(_) => Line::new("controller models", State::Imported(tilde(home, &old))),
                    Err(e) => Line::new("controller models", State::Failed(e.to_string())),
                }
            }
        } else if let Some(h1) = pimax {
            let dirs = pimax_controller_dirs(&h1);
            let r: io::Result<()> = if dry {
                Ok(())
            } else {
                dirs.iter().try_for_each(|d| {
                    let name = d.file_name().unwrap_or_default();
                    copy_file(&d.join("controller_params.json"), &dest.join(name).join("controller_params.json"))
                })
            };
            match r {
                Ok(()) => Line::new("controller models", State::Imported(format!("{} ({} controllers, Pimax's Windows data)", tilde(home, &h1), dirs.len()))),
                Err(e) => Line::new("controller models", State::Failed(e.to_string())),
            }
        } else if backup_files.len() >= 2 {
            let from = tilde(home, &backup_files[0].1 .0);
            let r: io::Result<()> = if dry {
                Ok(())
            } else {
                backup_files.iter().try_for_each(|(rel, (_, data))| {
                    let tail = rel.split_once("/controllers/").map(|x| x.1).unwrap_or(rel.as_str());
                    devicedata::write_atomic(&dest.join(tail), data, 0o600)
                })
            };
            match r {
                Ok(()) => Line::new("controller models", State::Imported(from)),
                Err(e) => Line::new("controller models", State::Failed(e.to_string())),
            }
        } else {
            Line::new("controller models", State::NotFound)
        }
    });

    // 3. the gyro bias: a measured one (old cache, backups) replaces a missing or reference value
    let gyro = devicedata::gyro_bias_path(home, &env);
    let have = std::fs::read_to_string(&gyro).ok().filter(|t| devicedata::parse_gyro_bias(t).is_some());
    lines.push(if have.as_deref().is_some_and(|t| !devicedata::is_reference_gyro_bias(t)) {
        Line::new("gyro bias", State::Present)
    } else {
        let legacy = devicedata::legacy_gyro_bias_path(home, &env);
        let measured = |b: &[u8]| {
            let t = String::from_utf8_lossy(b);
            devicedata::parse_gyro_bias(&t).is_some() && !devicedata::is_reference_gyro_bias(&t)
        };
        let src = std::fs::read(&legacy).ok().filter(|b| measured(b)).map(|b| (tilde(home, &legacy), b))
            .or_else(|| from_backup(".cache/omarchyvr/gyro-bias").filter(|(_, b)| measured(b)))
            .or_else(|| from_backup(".cache/pimax/gyro-bias").filter(|(_, b)| measured(b)));
        match src {
            None if have.is_some() => Line::new("gyro bias (the Dream Air default until the headset rests once)", State::Present),
            None => Line::new("gyro bias", State::NotFound),
            Some((from, _)) if dry => Line::new("gyro bias", State::Imported(from)),
            Some((from, b)) => match devicedata::write_atomic(&gyro, &b, 0o644) {
                Ok(()) => Line::new("gyro bias", State::Imported(from)),
                Err(e) => Line::new("gyro bias", State::Failed(e.to_string())),
            },
        }
    });

    // 4. the rest: files and directories with an older place and/or a backup copy
    for w in WANTS {
        let dst = home.join(w.dest);
        if exists(&dst) {
            lines.push(Line::new(w.what, State::Present));
            continue;
        }
        let on_disk = w.old.iter().map(|o| home.join(o)).find(|o| match w.kind {
            Kind::File => o.is_file(),
            Kind::Dir => o.is_dir(),
        });
        let lens = w.dest.ends_with("lens.conf");
        // one file: from the older place on disk, else the newest backup that has it
        let file_src: Option<(String, io::Result<Vec<u8>>)> = match (&on_disk, w.kind) {
            (Some(o), Kind::File) => Some((tilde(home, o), std::fs::read(o))),
            (None, Kind::File) => std::iter::once(w.dest).chain(w.old.iter().copied()).find_map(&from_backup).map(|(f, b)| (f, Ok(b))),
            _ => None,
        };
        let line = match (on_disk, w.kind) {
            (_, Kind::File) => match file_src {
                None => State::NotFound,
                Some((from, Err(e))) => State::Failed(format!("{from}: {e}")),
                // a lens.conf with the shipped profile's numbers adds nothing: no second copy is kept
                Some((_, Ok(b))) if lens && same_lens_numbers(&String::from_utf8_lossy(&b), pimax_lens::REFERENCE_LENS_CONF) => {
                    lines.push(Line::new("lens profile (the same as the shipped Dream Air profile)", State::Present));
                    continue;
                }
                Some((from, Ok(_))) if dry => State::Imported(from),
                Some((from, Ok(b))) => {
                    let b = if lens { fix_lens_paths(&b) } else { b };
                    match devicedata::write_atomic(&dst, &b, 0o600) {
                        Ok(()) => State::Imported(from),
                        Err(e) => State::Failed(e.to_string()),
                    }
                }
            },
            (Some(o), Kind::Dir) => match if dry { Ok(0) } else { copy_dir(&o, &dst) } {
                Ok(_) => State::Imported(tilde(home, &o)),
                Err(e) => State::Failed(e.to_string()),
            },
            (None, Kind::Dir) => {
                // the newest archive that has anything below one of the item's directories
                let pick = std::iter::once(w.dest).chain(w.old.iter().copied()).find_map(|d| {
                    let files: Vec<(&String, &(PathBuf, Vec<u8>))> = backups.iter().filter(|(r, _)| r.starts_with(&format!("{d}/"))).collect();
                    (!files.is_empty()).then_some((d, files))
                });
                match pick {
                    None => State::NotFound,
                    Some((_, files)) if dry => State::Imported(tilde(home, &files[0].1 .0)),
                    Some((d, files)) => {
                        let from = tilde(home, &files[0].1 .0);
                        let r: io::Result<()> = files.iter().try_for_each(|(rel, (_, data))| {
                            devicedata::write_atomic(&dst.join(&rel[d.len() + 1..]), data, 0o600)
                        });
                        match r {
                            Ok(()) => State::Imported(from),
                            Err(e) => State::Failed(e.to_string()),
                        }
                    }
                }
            }
        };
        lines.push(Line::new(w.what, line));
    }
    if !dry {
        save_state(home, &scanned, &lines);
    }
    lines
}

/// Two lens.conf texts with the same settings (comments and blank lines ignored).
fn same_lens_numbers(a: &str, b: &str) -> bool {
    let norm = |t: &str| t.lines().map(|l| l.split('#').next().unwrap_or("").trim().to_string()).filter(|l| !l.is_empty()).collect::<Vec<_>>();
    norm(a) == norm(b)
}

/// Fingerprint of a backup archive: size and modification time.
fn fingerprint(p: &Path) -> Option<String> {
    let m = std::fs::metadata(p).ok()?;
    let t = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    Some(format!("{} {t}", m.len()))
}

fn save_state(home: &Path, scanned: &[PathBuf], lines: &[Line]) {
    let mut s = String::from("# omarchyvr import-previous: the backups looked at and what was found (the app shows it)\n");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    s.push_str(&format!("when {now}\n"));
    for b in scanned {
        if let Some(f) = fingerprint(b) {
            s.push_str(&format!("scanned {f} {}\n", b.display()));
        }
    }
    for l in lines {
        s.push_str(&format!("line {}\n", l.describe()));
    }
    let _ = devicedata::write_atomic(&home.join(STATE_FILE), s.as_bytes(), 0o600);
}

/// The last run: (unix time, its plain lines). None before the first run.
pub fn last_run(home: &Path) -> Option<(u64, Vec<String>)> {
    let t = std::fs::read_to_string(home.join(STATE_FILE)).ok()?;
    let when = t.lines().find_map(|l| l.strip_prefix("when ")).and_then(|v| v.trim().parse().ok()).unwrap_or(0);
    Some((when, t.lines().filter_map(|l| l.strip_prefix("line ")).map(str::to_string).collect()))
}

/// Cheap check for the app (no archive is opened): the reasons an import would bring something back now. Empty =
/// nothing to do. Looks at backups not looked at yet, and at older places on disk for items that are missing.
pub fn pending(p: &Places) -> Vec<String> {
    let home = &p.home;
    let mut v = Vec::new();
    let state = std::fs::read_to_string(home.join(STATE_FILE)).unwrap_or_default();
    let new_backups = p.backups.iter().filter(|b| fingerprint(b).is_none_or(|f| !state.contains(&format!("scanned {f} {}", b.display())))).count();
    if new_backups > 0 {
        v.push(format!("{new_backups} backup{} of a previous install not looked at yet", if new_backups == 1 { "" } else { "s" }));
    }
    if devicedata::own_controller_models(home).is_none()
        && (devicedata::has_controller_pair(&home.join(".config/pimax/controllers"))
            || slam_dirs(&Places { shared: None, ..p.clone() }).iter().any(|s| pimax_controller_dirs(&s.join("H1")).len() >= 2))
    {
        v.push("your controller models (Pimax's Windows data on this PC)".into());
    }
    if devicedata::own_calibration(home).is_none() && crate::paths::headset_calibration_old(home).iter().any(|q| q.is_file()) {
        v.push("your headset calibration".into());
    }
    let env = p.env();
    let gyro = std::fs::read_to_string(devicedata::gyro_bias_path(home, &env)).ok().filter(|t| devicedata::parse_gyro_bias(t).is_some());
    let legacy = std::fs::read_to_string(devicedata::legacy_gyro_bias_path(home, &env)).ok().filter(|t| devicedata::parse_gyro_bias(t).is_some() && !devicedata::is_reference_gyro_bias(t));
    if gyro.as_deref().is_none_or(devicedata::is_reference_gyro_bias) && legacy.is_some() {
        v.push("your measured gyro bias".into());
    }
    for w in WANTS {
        let old = w.old.iter().map(|o| home.join(o)).find(|o| exists(o));
        // an old lens.conf with the shipped profile's numbers brings nothing (the import does not keep a copy of it)
        let same_as_shipped = |o: &Path| {
            w.dest.ends_with("lens.conf") && std::fs::read_to_string(o).is_ok_and(|t| same_lens_numbers(&t, pimax_lens::REFERENCE_LENS_CONF))
        };
        if !exists(&home.join(w.dest)) && old.as_deref().is_some_and(|o| !same_as_shipped(o)) {
            v.push(format!("your {}", w.what));
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn home(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("omarchyvr-import-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A gzip'ed tar with these (name, data) members; names longer than 100 bytes get a GNU `L` record.
    pub(crate) fn targz(members: &[(&str, &[u8])]) -> Vec<u8> {
        fn header(name: &str, size: usize, flag: u8) -> [u8; 512] {
            let mut h = [0u8; 512];
            let n = name.as_bytes();
            h[..n.len().min(100)].copy_from_slice(&n[..n.len().min(100)]);
            h[100..107].copy_from_slice(b"0000644");
            h[108..115].copy_from_slice(b"0001750");
            h[116..123].copy_from_slice(b"0001750");
            h[124..135].copy_from_slice(format!("{size:011o}").as_bytes());
            h[136..147].copy_from_slice(b"14000000000");
            h[156] = flag;
            h[257..263].copy_from_slice(b"ustar ");
            h[263..265].copy_from_slice(b" \0");
            h[148..156].copy_from_slice(b"        ");
            let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
            h[148..155].copy_from_slice(format!("{sum:06o}\0").as_bytes());
            h
        }
        let mut t = Vec::new();
        let pad = |t: &mut Vec<u8>| t.resize(t.len().div_ceil(512) * 512, 0);
        for (name, data) in members {
            if name.len() > 100 {
                let mut n = name.as_bytes().to_vec();
                n.push(0);
                t.extend_from_slice(&header("././@LongLink", n.len(), b'L'));
                t.extend_from_slice(&n);
                pad(&mut t);
            }
            t.extend_from_slice(&header(name, data.len(), b'0'));
            t.extend_from_slice(data);
            pad(&mut t);
        }
        t.extend_from_slice(&[0u8; 1024]);
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&t).unwrap();
        gz.finish().unwrap()
    }

    fn places(h: &Path) -> Places {
        Places { home: h.to_path_buf(), lab: h.join("pimax-re"), clones: vec![h.join("omarchyvr")], shared: None, backups: list_backups(h), xdg_cache: None }
    }

    fn put(p: &Path, data: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, data).unwrap();
    }

    #[test]
    fn the_tar_reader_finds_members_and_long_names_and_never_returns_keys() {
        let long = format!("/home/u/{}/x.txt", "d".repeat(120));
        let gz = targz(&[("/home/u/.config/pimax/ipd.env", b"OXR_DEBUG_IPD_MM=64.0\n"), (&long, b"deep"), ("/home/u/.config/pimax/ai.toml", b"key")]);
        let mut got = Vec::new();
        tar_files(&gz[..], &|_| true, 1 << 20, &mut |n, d| got.push((n, d))).unwrap();
        assert_eq!(got.len(), 3);
        assert_eq!(got[1].0, long);
        assert_eq!(member_rel(&got[0].0, Path::new("/home/u")), ".config/pimax/ipd.env");
        assert_eq!(member_rel(".config/pimax/ipd.env", Path::new("/home/u")), ".config/pimax/ipd.env");
        assert_eq!(member_rel("/home/other/.cache/pimax/gyro-bias", Path::new("/home/u")), ".cache/pimax/gyro-bias");
        assert!(is_keys(".config/pimax/ai.toml") && is_keys(".config/omarchyvr/ai.toml.bak-1") && !is_keys(".config/pimax/ipd.env"));
        // a size cap skips big members without breaking the stream
        let mut small = Vec::new();
        tar_files(&gz[..], &|_| true, 5, &mut |n, _| small.push(n)).unwrap();
        assert_eq!(small, vec![long.clone(), "/home/u/.config/pimax/ai.toml".to_string()]);
    }

    #[test]
    fn a_clean_home_gets_the_users_data_back_from_the_lab_and_the_backups() {
        let h = home("full");
        // the lab tree: the headset calibration and Pimax's Windows data with two controller folders
        put(&h.join("pimax-re/analysis/headset-calibration.xml"), &format!("<?xml version=\"1.0\"?>\n<DeviceConfiguration deviceUID=\"x\">{}</DeviceConfiguration>\n", " ".repeat(300)));
        let h1 = h.join("pimax-re/pimax-data/ProgramData-Pimax/slam/H1");
        put(&h1.join("SN2/controller_params.json"), "{\"Hand\":\"Right\"}");
        put(&h1.join("SN1/controller_params.json"), "{\"Hand\":\"Left\"}");
        // the uninstaller's backups: an older one with the old names, a newer one with only the new names
        let old = targz(&[
            (&format!("{}/.config/pimax/ipd.env", h.display()), b"OXR_DEBUG_IPD_MM=64.0\n"),
            (&format!("{}/.config/pimax/lens.conf", h.display()), b"fov = 40 41 42 43\nlut = /home/u/.config/pimax/lens-ipd/x.bin\n"),
            (&format!("{}/.config/pimax/ai.toml", h.display()), b"api_key = \"never\"\n"),
            (&format!("{}/.cache/pimax/gyro-bias", h.display()), b"0.2492 0.7927 0.3365\n"),
            (&format!("{}/.local/share/pimax/roommap/home/map.bin", h.display()), b"MAP"),
            (&format!("{}/.local/share/pimax/roommap/home/room.json", h.display()), b"{}"),
            (&format!("{}/.config/pimax/eye-calibration.toml", h.display()), b"version = 2\n"),
        ]);
        std::fs::write(h.join("omarchyvr-settings-backup-20261017-083404.tar.gz"), old).unwrap();
        let newer = targz(&[(&format!("{}/.config/omarchyvr/games.toml", h.display()), b"runtime = \"steamvr\"\n")]);
        let nb = h.join("omarchyvr-settings-backup-20261017-120119.tar.gz");
        std::fs::write(&nb, newer).unwrap();
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
        std::fs::File::options().write(true).open(&nb).unwrap().set_modified(later).unwrap();
        // a reference gyro bias was seeded by a start: the measured one of the backup replaces it
        let env = |_: &str| None;
        devicedata::ensure_gyro_bias(&h, &env).unwrap();
        let p = places(&h);
        assert_eq!(p.backups[0], nb, "newest first");
        assert!(!pending(&p).is_empty());
        // dry run: says what it would do, changes nothing
        let dry = run(&p, true);
        assert!(dry.iter().any(|l| l.what == "controller models" && matches!(l.state, State::Imported(_))), "{dry:?}");
        assert!(!h.join(".config/omarchyvr/controllers").exists());
        let lines = run(&p, false);
        let get = |w: &str| lines.iter().find(|l| l.what == w).unwrap_or_else(|| panic!("{w}: {lines:?}")).state.clone();
        assert!(matches!(get("headset calibration"), State::Imported(f) if f.contains("pimax-re/analysis")));
        assert!(matches!(get("controller models"), State::Imported(f) if f.contains("2 controllers")));
        assert!(h.join(".config/omarchyvr/controllers/SN1/controller_params.json").is_file());
        assert!(devicedata::own_controller_models(&h).is_some());
        assert!(matches!(get("gyro bias"), State::Imported(_)));
        assert_eq!(std::fs::read_to_string(devicedata::gyro_bias_path(&h, &env)).unwrap(), "0.2492 0.7927 0.3365\n");
        assert!(matches!(get("IPD"), State::Imported(f) if f.contains("083404")));
        assert_eq!(get("games run with"), State::Imported("~/omarchyvr-settings-backup-20261017-120119.tar.gz (.config/omarchyvr/games.toml)".to_string()));
        assert!(matches!(get("room map"), State::Imported(_)));
        assert_eq!(std::fs::read(h.join(".local/share/omarchyvr/roommap/home/map.bin")).unwrap(), b"MAP");
        assert!(matches!(get("eye calibration"), State::Imported(_)));
        let lens = std::fs::read_to_string(h.join(".config/omarchyvr/lens.conf")).unwrap();
        assert!(lens.contains("/.config/omarchyvr/lens-ipd/x.bin"), "the LUT path follows the move: {lens}");
        assert_eq!(get("VR background"), State::NotFound);
        // the keys are never brought back by the import
        assert!(!h.join(".config/omarchyvr/ai.toml").exists());
        // private files are mode 600
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(h.join(".config/omarchyvr/headset-calibration.xml")).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        // nothing is overwritten by a second run, and the backups are not pending any more
        std::fs::write(h.join(".config/omarchyvr/ipd.env"), "OXR_DEBUG_IPD_MM=61.0\n").unwrap();
        let again = run(&p, false);
        assert!(again.iter().all(|l| !matches!(l.state, State::Imported(_))), "{again:?}");
        assert_eq!(std::fs::read_to_string(h.join(".config/omarchyvr/ipd.env")).unwrap(), "OXR_DEBUG_IPD_MM=61.0\n");
        assert!(pending(&p).is_empty(), "{:?}", pending(&p));
        let (_, last) = last_run(&h).unwrap();
        assert!(last.iter().any(|l| l.starts_with("IPD: already there")));
        let _ = std::fs::remove_dir_all(&h);
    }

    #[test]
    fn nothing_anywhere_is_not_an_error() {
        let h = home("empty");
        let p = places(&h);
        assert!(pending(&p).is_empty());
        let lines = run(&p, false);
        assert!(lines.iter().all(|l| matches!(l.state, State::NotFound | State::Present)), "{lines:?}");
        assert!(lines.iter().any(|l| l.describe() == "controller models: no copy on this PC"));
        let _ = std::fs::remove_dir_all(&h);
    }

    #[test]
    fn a_lens_conf_equal_to_the_shipped_profile_is_not_kept_twice() {
        let h = home("lens");
        put(&h.join(".config/pimax/lens.conf"), &format!("# my old header\n{}", pimax_lens::REFERENCE_LENS_CONF));
        assert!(pending(&places(&h)).is_empty(), "the same numbers as the shipped profile bring nothing back");
        let lines = run(&places(&h), false);
        assert!(!h.join(".config/omarchyvr/lens.conf").exists());
        assert!(lines.iter().any(|l| l.what.contains("the same as the shipped Dream Air profile")), "{lines:?}");
        let _ = std::fs::remove_dir_all(&h);
    }

    #[test]
    fn the_uninstaller_keeps_exactly_the_device_data() {
        let d = device_paths();
        for p in [".config/omarchyvr/headset-calibration.xml", ".config/omarchyvr/controllers", ".cache/omarchyvr/gyro-bias", ".config/omarchyvr/lens.conf", ".config/omarchyvr/ipd.env", ".config/omarchyvr/eye-calibration.toml", ".local/share/omarchyvr/roommap"] {
            assert!(d.contains(&p), "{p}");
        }
        assert!(!d.iter().any(|p| p.contains("ai.toml") || p.contains("gui-state") || p.contains("games.toml")));
        // installer/uninstall.sh keeps the same list (its DEVICE_DATA, plus the factory calibration's own lines)
        let sh = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../installer/uninstall.sh")).unwrap();
        for p in d {
            let want = if let Some(r) = p.strip_prefix(".config/") {
                format!("$CONF/{r}")
            } else if let Some(r) = p.strip_prefix(".local/share/omarchyvr/") {
                format!("$SHARE/{r}")
            } else {
                format!("$HOME/{p}")
            };
            assert!(sh.contains(&want), "installer/uninstall.sh does not keep {want}");
        }
    }
}
