//! VR backgrounds: the environment around you in the headset (the WayVR skybox), not the desktop wallpaper.
//!
//! * The library lives in `~/.local/share/omarchyvr/backgrounds/<theme>/` (never in git): per theme a `manifest.toml`
//!   (source URL, author, licence, resolutions, checksums), `thumbs/<slug>.jpg` and `<slug>_<8k|4k|2k>.dds`
//!   (equirectangular 360, DXT1; WayVR's skybox reads DDS only, the 8K file is 16 MB on the GPU).
//! * The choice is `~/.config/omarchyvr/background.toml` (`enabled`, `id`, `res`), shared by the app, voice and
//!   `omarchyvr desk start` (which applies it when `enabled`).
//! * [`fetch`] downloads and verifies the library from the curated list `sources.toml`.
pub mod fetch;
pub mod sha256;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The sizes, best first: (label, equirect width).
pub const SIZES: [(&str, u32); 3] = [("8k", 8192), ("4k", 4096), ("2k", 2048)];

/// Where the library and the setting live.
#[derive(Debug, Clone)]
pub struct Dirs {
    /// `~/.local/share/omarchyvr/backgrounds` (`$PIMAX_BACKGROUNDS_DIR`).
    pub library: PathBuf,
    /// `~/.config/omarchyvr/background.toml` (`$PIMAX_BACKGROUND_CONF`).
    pub setting: PathBuf,
}

impl Dirs {
    /// From the environment.
    pub fn from_env() -> Dirs {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"));
        let pick = |key: &str, default: PathBuf| std::env::var_os(key).filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or(default);
        Dirs {
            library: pick("PIMAX_BACKGROUNDS_DIR", home.join(".local/share/omarchyvr/backgrounds")),
            setting: pick("PIMAX_BACKGROUND_CONF", home.join(".config/omarchyvr/background.toml")),
        }
    }

    /// The default places under a given home directory (a test's fake `$HOME`).
    pub fn for_home(home: &Path) -> Dirs {
        Dirs { library: home.join(".local/share/omarchyvr/backgrounds"), setting: home.join(".config/omarchyvr/background.toml") }
    }

    /// Both under one root (tests).
    pub fn under(root: &Path) -> Dirs {
        Dirs { library: root.join("backgrounds"), setting: root.join("config/background.toml") }
    }
}

/// Which resolution to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Res {
    /// The best the picture has, up to 8K.
    #[default]
    Auto,
    #[serde(rename = "8k")]
    K8,
    #[serde(rename = "4k")]
    K4,
    #[serde(rename = "2k")]
    K2,
}

impl Res {
    pub const ALL: [Res; 4] = [Res::Auto, Res::K8, Res::K4, Res::K2];

    pub fn label(self) -> &'static str {
        match self {
            Res::Auto => "Auto",
            Res::K8 => "8K",
            Res::K4 => "4K",
            Res::K2 => "2K",
        }
    }

    pub fn parse(s: &str) -> Option<Res> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => Some(Res::Auto),
            "8k" | "8" => Some(Res::K8),
            "4k" | "4" => Some(Res::K4),
            "2k" | "2" => Some(Res::K2),
            _ => None,
        }
    }

    fn rank(self) -> usize {
        match self {
            Res::Auto | Res::K8 => 0,
            Res::K4 => 1,
            Res::K2 => 2,
        }
    }
}

/// The saved choice (`background.toml`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Setting {
    /// Show the background; false: the real room (passthrough) as the preset says.
    pub enabled: bool,
    /// `<theme>/<slug>`.
    pub id: String,
    pub res: Res,
}

impl Default for Setting {
    fn default() -> Self {
        Setting { enabled: false, id: String::new(), res: Res::Auto }
    }
}

impl Setting {
    /// The saved setting (the default when the file is missing or broken).
    pub fn load(d: &Dirs) -> Setting {
        std::fs::read_to_string(&d.setting).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default()
    }

    /// Atomic save.
    pub fn save(&self, d: &Dirs) -> std::io::Result<()> {
        if let Some(p) = d.setting.parent() {
            std::fs::create_dir_all(p)?;
        }
        let text = format!(
            "# VR background (the Pimax app, \"Max, set background ...\" and `omarchyvr-backgrounds`). id = <theme>/<name>; res = auto|8k|4k|2k\nenabled = {}\nid = {:?}\nres = {:?}\n",
            self.enabled,
            self.id,
            self.res.label().to_ascii_lowercase()
        );
        let tmp = d.setting.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, &d.setting)
    }
}

/// One file of a picture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileInfo {
    /// `8k` | `4k` | `2k`.
    pub res: String,
    /// Relative to the theme folder.
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub size: u64,
    pub sha256: String,
}

/// One picture of the library (an `[[image]]` of a theme manifest).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// `<theme>/<slug>`.
    pub id: String,
    pub theme: String,
    pub title: String,
    pub author: String,
    pub licence: String,
    pub licence_url: String,
    pub source_url: String,
    /// polyhaven | commons | url.
    pub provider: String,
    /// Size of the downloaded original.
    pub source_width: u32,
    pub source_height: u32,
    /// Relative to the theme folder.
    pub thumb: String,
    #[serde(default, rename = "file")]
    pub files: Vec<FileInfo>,
}

/// A theme manifest file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default, rename = "image")]
    pub images: Vec<Entry>,
}

impl Entry {
    pub fn slug(&self) -> &str {
        self.id.split_once('/').map_or(&self.id, |x| x.1)
    }

    /// What voice calls it: `<theme>_<slug>` (spoken "ocean" finds the first ocean picture).
    pub fn voice_name(&self) -> String {
        let s = self.slug();
        if s.starts_with(&self.theme) { s.to_string() } else { format!("{}_{}", self.theme, s) }
    }

    /// Is this what was said or saved: the voice name, the id, or the plain file name (`night-sky`)?
    pub fn matches_name(&self, name: &str) -> bool {
        self.voice_name() == name || self.id == name || self.slug() == name || self.slug() == name.replace('-', "_")
    }

    /// The file to show for a choice: the asked size, else the nearest smaller one, else a larger one.
    pub fn file_for(&self, res: Res) -> Option<&FileInfo> {
        let want = res.rank();
        let rank = |f: &FileInfo| SIZES.iter().position(|(l, _)| *l == f.res).unwrap_or(9);
        self.files
            .iter()
            .filter(|f| rank(f) >= want)
            .min_by_key(|f| rank(f))
            .or_else(|| self.files.iter().min_by_key(|f| std::cmp::Reverse(rank(f))))
    }
}

/// The theme folders of the library, sorted.
pub fn themes(d: &Dirs) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(&d.library)
        .map(|rd| rd.flatten().filter(|e| e.path().join("manifest.toml").is_file()).filter_map(|e| e.file_name().into_string().ok()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// Every picture of the library that has at least one file on disk, themes sorted, pictures by title.
pub fn library(d: &Dirs) -> Vec<Entry> {
    let mut all = Vec::new();
    for t in themes(d) {
        let Some(m) = std::fs::read_to_string(d.library.join(&t).join("manifest.toml")).ok().and_then(|s| toml::from_str::<Manifest>(&s).ok()) else { continue };
        let mut v: Vec<Entry> = m
            .images
            .into_iter()
            .filter(|e| e.theme == t)
            .map(|mut e| {
                e.files.retain(|f| d.library.join(&t).join(&f.path).is_file());
                e
            })
            .filter(|e| !e.files.is_empty())
            .collect();
        v.sort_by(|a, b| a.title.cmp(&b.title));
        all.extend(v);
    }
    all
}

/// Absolute path of the file shown for an entry.
pub fn path_of(d: &Dirs, e: &Entry, res: Res) -> Option<PathBuf> {
    e.file_for(res).map(|f| d.library.join(&e.theme).join(&f.path))
}

/// Absolute path of the thumbnail (if it exists).
pub fn thumb_of(d: &Dirs, e: &Entry) -> Option<PathBuf> {
    let p = d.library.join(&e.theme).join(&e.thumb);
    p.is_file().then_some(p)
}

/// Pick a picture by spoken name: the exact voice name or id, else the first whose name contains the words (a
/// theme word like "ocean" or "hawaii" finds a picture of that theme; the next one after `after` when it is in the
/// same match set, so saying the theme again shows another).
pub fn find(lib: &[Entry], spoken: &str, after: Option<&str>) -> Option<Entry> {
    let norm = |s: &str| s.to_lowercase().replace(['_', '-', '/'], " ").split_whitespace().collect::<Vec<_>>().join(" ");
    let want = norm(spoken);
    if want.is_empty() {
        return None;
    }
    if let Some(e) = lib.iter().find(|e| norm(&e.voice_name()) == want || norm(&e.id) == want) {
        return Some(e.clone());
    }
    let hits: Vec<&Entry> = lib
        .iter()
        .filter(|e| {
            let n = norm(&e.voice_name());
            let t = norm(&e.title);
            n.contains(&want) || t.contains(&want) || (want.len() > 3 && (want.contains(&e.theme) || want.strip_suffix('s').is_some_and(|w| w == e.theme)))
        })
        .collect();
    if hits.is_empty() {
        return None;
    }
    let pos = after.and_then(|a| hits.iter().position(|e| e.voice_name() == a || e.id == a));
    Some(hits[pos.map_or(0, |p| (p + 1) % hits.len())].clone())
}

/// The picture after / before another in library order (wraps).
pub fn step(lib: &[Entry], current: Option<&str>, forward: bool) -> Option<Entry> {
    if lib.is_empty() {
        return None;
    }
    let i = current.and_then(|c| lib.iter().position(|e| e.id == c || e.voice_name() == c));
    let n = lib.len();
    let j = match (i, forward) {
        (Some(i), true) => (i + 1) % n,
        (Some(i), false) => (i + n - 1) % n,
        (None, true) => 0,
        (None, false) => n - 1,
    };
    Some(lib[j].clone())
}

/// What applying the setting means: the `wayvrctl skybox` argument and the passthrough line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub skybox: String,
    pub passthrough: &'static str,
}

/// The plan of a setting: a picture (skybox first, camera off), or the room again (`normal`). None when the picture
/// is missing on disk.
pub fn plan(d: &Dirs, s: &Setting) -> Option<Plan> {
    if !s.enabled {
        return Some(Plan { skybox: "off".into(), passthrough: "normal" });
    }
    let lib = library(d);
    let e = lib.iter().find(|e| e.id == s.id)?;
    let p = path_of(d, e, s.res)?;
    Some(Plan { skybox: p.display().to_string(), passthrough: "off" })
}

/// Voice name -> absolute path of the file to show (the saved resolution). None if it is not a library picture.
pub fn voice_path(d: &Dirs, name: &str) -> Option<String> {
    let lib = library(d);
    let e = lib.iter().find(|e| e.matches_name(name))?;
    path_of(d, e, Setting::load(d).res).map(|p| p.display().to_string())
}

/// Voice names of the library, in library order.
pub fn voice_names(d: &Dirs) -> Vec<String> {
    library(d).iter().map(Entry::voice_name).collect()
}

/// Save the choice made by voice (or the app): a library picture turns the background on, anything else
/// (passthrough, dimmed, black) turns it off. Keeps the resolution.
pub fn remember(d: &Dirs, name: &str) -> std::io::Result<()> {
    let mut s = Setting::load(d);
    match library(d).iter().find(|e| e.matches_name(name)) {
        Some(e) => {
            s.enabled = true;
            s.id = e.id.clone();
        }
        None => s.enabled = false,
    }
    s.save(d)
}

/// Where the passthrough control socket is.
pub fn passthrough_socket() -> PathBuf {
    if let Some(p) = std::env::var_os("PIMAX_PASSTHROUGH_SOCKET").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|p| !p.is_empty()) {
        Some(d) => PathBuf::from(d).join("omarchyvr-passthrough.sock"),
        // SAFETY: getuid cannot fail.
        None => PathBuf::from(format!("/tmp/omarchyvr-passthrough-{}.sock", unsafe { libc_getuid() })),
    }
}

extern "C" {
    #[link_name = "getuid"]
    fn libc_getuid() -> u32;
}

fn passthrough_send(sock: &Path, line: &str) -> Result<String, String> {
    use std::io::{BufRead, BufReader, Write};
    let mut s = std::os::unix::net::UnixStream::connect(sock).map_err(|_| "passthrough is not running".to_string())?;
    let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(2)));
    writeln!(s, "{line}").map_err(|e| e.to_string())?;
    let mut r = String::new();
    BufReader::new(s).read_line(&mut r).map_err(|e| e.to_string())?;
    Ok(r.trim().to_string())
}

/// `wayvrctl`: `$PIMAX_WAYVRCTL`, else the installed one (`~/.local/lib/omarchyvr/wayvr`, else the desk build), else the
/// one on PATH.
pub fn default_wayvrctl() -> PathBuf {
    if let Some(p) = std::env::var_os("PIMAX_WAYVRCTL").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let desk = omarchyvr_config::paths::wayvrctl_bin(&home);
    if desk.is_file() { desk } else { PathBuf::from("wayvrctl") }
}

/// Apply the saved setting to the running session: a picture draws the skybox first, then the camera goes off (no
/// flash); back to the room, the camera first. Err only when nothing could be applied (no session).
pub fn apply(d: &Dirs, wayvrctl: &Path) -> Result<String, String> {
    apply_to(d, wayvrctl, &passthrough_socket())
}

/// [`apply`] with an explicit passthrough socket.
pub fn apply_to(d: &Dirs, wayvrctl: &Path, sock: &Path) -> Result<String, String> {
    let s = Setting::load(d);
    let Some(p) = plan(d, &s) else { return Err("that picture is not downloaded".into()) };
    let sky = || {
        std::process::Command::new(wayvrctl)
            .args(["skybox", &p.skybox])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| format!("{}: {e}", wayvrctl.display()))
            .and_then(|st| if st.success() { Ok(()) } else { Err("WayVR is not running".to_string()) })
    };
    let mut errs = Vec::new();
    if s.enabled {
        if let Err(e) = sky() {
            errs.push(e);
        }
        if let Err(e) = passthrough_send(sock, p.passthrough) {
            errs.push(e);
        }
    } else {
        if let Err(e) = passthrough_send(sock, p.passthrough) {
            errs.push(e);
        }
        if let Err(e) = sky() {
            errs.push(e);
        }
    }
    if errs.len() >= 2 {
        Err(errs.join("; "))
    } else {
        Ok(if s.enabled { format!("background {}", s.id) } else { "background off".into() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn sample(d: &Dirs) {
        for (theme, slug, sizes) in [("ocean", "umhlanga_sunrise", vec!["8k", "4k", "2k"]), ("ocean", "blouberg_sunrise_1", vec!["4k", "2k"]), ("flowers", "wildflower_field", vec!["2k"]), ("hawaii", "maunakea_sunset", vec!["8k"])] {
            let dir = d.library.join(theme);
            std::fs::create_dir_all(dir.join("thumbs")).unwrap();
            let mut files = String::new();
            for r in &sizes {
                let name = format!("{slug}_{r}.dds");
                std::fs::write(dir.join(&name), b"x").unwrap();
                files += &format!("[[image.file]]\nres = \"{r}\"\npath = \"{name}\"\nwidth = 100\nheight = 50\nsize = 1\nsha256 = \"00\"\n");
            }
            let one = format!(
                "[[image]]\nid = \"{theme}/{slug}\"\ntheme = \"{theme}\"\ntitle = \"{slug}\"\nauthor = \"a\"\nlicence = \"CC0\"\nlicence_url = \"u\"\nsource_url = \"s\"\nprovider = \"polyhaven\"\nsource_width = 8192\nsource_height = 4096\nthumb = \"thumbs/{slug}.jpg\"\n{files}\n"
            );
            let mp = dir.join("manifest.toml");
            let old = std::fs::read_to_string(&mp).unwrap_or_default();
            std::fs::write(mp, old + &one).unwrap();
        }
    }

    pub(crate) fn tmp(name: &str) -> Dirs {
        let r = std::env::temp_dir().join(format!("pimax-bg-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&r);
        std::fs::create_dir_all(&r).unwrap();
        Dirs::under(&r)
    }

    #[test]
    fn setting_round_trip_and_default() {
        let d = tmp("setting");
        assert_eq!(Setting::load(&d), Setting::default());
        let s = Setting { enabled: true, id: "ocean/umhlanga_sunrise".into(), res: Res::K4 };
        s.save(&d).unwrap();
        assert_eq!(Setting::load(&d), s);
        std::fs::write(&d.setting, "enabled = maybe").unwrap();
        assert_eq!(Setting::load(&d), Setting::default(), "a broken file is the default");
    }

    #[test]
    fn library_picks_files_by_resolution() {
        let d = tmp("lib");
        sample(&d);
        let lib = library(&d);
        assert_eq!(lib.len(), 4);
        assert_eq!(themes(&d), vec!["flowers", "hawaii", "ocean"]);
        let o = lib.iter().find(|e| e.id == "ocean/umhlanga_sunrise").unwrap();
        assert_eq!(o.file_for(Res::Auto).unwrap().res, "8k");
        assert_eq!(o.file_for(Res::K4).unwrap().res, "4k");
        assert_eq!(o.file_for(Res::K2).unwrap().res, "2k");
        let b = lib.iter().find(|e| e.id == "ocean/blouberg_sunrise_1").unwrap();
        assert_eq!(b.file_for(Res::K8).unwrap().res, "4k", "no 8K: the next best");
        let f = lib.iter().find(|e| e.id == "flowers/wildflower_field").unwrap();
        assert_eq!(f.file_for(Res::K4).unwrap().res, "2k", "only 2K: smaller than asked beats nothing");
        let h = lib.iter().find(|e| e.id == "hawaii/maunakea_sunset").unwrap();
        assert_eq!(h.file_for(Res::K2).unwrap().res, "8k", "only 8K for a 2K ask: the picture still shows");
        assert_eq!(h.voice_name(), "hawaii_maunakea_sunset");
    }

    #[test]
    fn spoken_names_and_stepping() {
        let d = tmp("find");
        sample(&d);
        let lib = library(&d);
        assert_eq!(find(&lib, "flowers", None).unwrap().id, "flowers/wildflower_field");
        assert_eq!(find(&lib, "Hawaii", None).unwrap().id, "hawaii/maunakea_sunset");
        assert_eq!(find(&lib, "ocean", None).unwrap().id, "ocean/blouberg_sunrise_1");
        assert_eq!(find(&lib, "ocean", Some("ocean_blouberg_sunrise_1")).unwrap().id, "ocean/umhlanga_sunrise", "the theme again shows another");
        assert_eq!(find(&lib, "umhlanga", None).unwrap().id, "ocean/umhlanga_sunrise");
        assert!(find(&lib, "volcano", None).is_none());
        assert!(find(&lib, "", None).is_none());
        assert_eq!(step(&lib, Some("hawaii/maunakea_sunset"), true).unwrap().id, lib[2].id);
        assert_eq!(step(&lib, None, true).unwrap().id, lib[0].id);
        assert_eq!(step(&lib, Some(&lib[0].id), false).unwrap().id, lib[3].id);
    }

    #[test]
    fn plans_and_remember() {
        let d = tmp("plan");
        sample(&d);
        assert_eq!(plan(&d, &Setting::default()).unwrap(), Plan { skybox: "off".into(), passthrough: "normal" });
        let s = Setting { enabled: true, id: "ocean/umhlanga_sunrise".into(), res: Res::K4 };
        let p = plan(&d, &s).unwrap();
        assert!(p.skybox.ends_with("ocean/umhlanga_sunrise_4k.dds") && p.passthrough == "off", "{p:?}");
        assert!(plan(&d, &Setting { id: "nope/x".into(), ..s.clone() }).is_none());
        remember(&d, "flowers_wildflower_field").unwrap();
        let l = Setting::load(&d);
        assert!(l.enabled && l.id == "flowers/wildflower_field");
        remember(&d, "passthrough").unwrap();
        let l = Setting::load(&d);
        assert!(!l.enabled && l.id == "flowers/wildflower_field", "off keeps the last picture");
        assert!(voice_path(&d, "hawaii_maunakea_sunset").unwrap().ends_with("hawaii/maunakea_sunset_8k.dds"));
        assert!(voice_path(&d, "sky").is_none());
    }
}
