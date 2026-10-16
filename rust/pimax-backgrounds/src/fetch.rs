//! The downloader: the curated list `sources.toml` -> the library. Resumable (`curl -C -`, finished pictures are
//! skipped after a checksum check) and verifying (download size, image decodes and is 2:1, every file's sha256 in
//! the manifest). Uses `curl` and ImageMagick (`magick`) as the engine; only official APIs and direct links:
//! the Poly Haven API, the Wikimedia Commons API and plain NASA/ESA URLs (terms in docs/BACKGROUNDS.md).
use crate::sha256;
use crate::{Dirs, Entry, FileInfo, Manifest, SIZES};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The curated list, compiled in.
pub const SOURCES: &str = include_str!("../sources.toml");

/// Sent with every request (Poly Haven and Wikimedia ask for a unique, contactable agent).
pub const USER_AGENT: &str = "pimax-re-backgrounds/0.1 (personal VR use; https://github.com/laughtt/omarchyvr)";

/// Stop downloading when the library would pass this.
pub const MAX_LIBRARY_BYTES: u64 = 4_000_000_000;

/// One line of `sources.toml`.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Source {
    /// polyhaven | commons | url.
    pub provider: String,
    pub theme: String,
    /// Poly Haven asset id, or the Commons file name (without `File:`), or empty for `url`.
    pub r#ref: String,
    /// Short name file stem; default: the ref, simplified.
    pub slug: String,
    pub title: String,
    // for `url` only (the others read them from the provider's API):
    pub url: String,
    pub author: String,
    pub licence: String,
    pub licence_url: String,
    pub source_url: String,
}

#[derive(Deserialize, Default)]
struct SourceFile {
    #[serde(default, rename = "image")]
    images: Vec<Source>,
}

impl Source {
    pub fn slug(&self) -> String {
        if !self.slug.is_empty() {
            return self.slug.clone();
        }
        slugify(&self.r#ref)
    }

    pub fn id(&self) -> String {
        format!("{}/{}", self.theme, self.slug())
    }
}

/// `File name (stuff).jpg` -> `file_name`.
pub fn slugify(s: &str) -> String {
    let s = s.rsplit_once('.').filter(|(_, e)| e.len() <= 4).map_or(s, |x| x.0);
    let mut o = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            o.push(c.to_ascii_lowercase());
        } else if !o.ends_with('_') && !o.is_empty() {
            o.push('_');
        }
    }
    o.trim_end_matches('_').to_string()
}

/// The parsed curated list.
pub fn sources() -> Result<Vec<Source>, String> {
    let f: SourceFile = toml::from_str(SOURCES).map_err(|e| format!("sources.toml: {e}"))?;
    Ok(f.images)
}

/// Licences that allow a free download for personal use (CC0, public domain, CC BY, CC BY-SA, NASA).
pub fn licence_ok(l: &str) -> bool {
    let l = l.to_lowercase();
    ["cc0", "public domain", "cc by", "cc-by", "nasa"].iter().any(|k| l.contains(k)) && !l.contains("-nc") && !l.contains("-nd")
}

/// Strip HTML tags and entities of a Commons credit.
pub fn strip_html(s: &str) -> String {
    let mut o = String::new();
    let mut tag = false;
    for c in s.chars() {
        match c {
            '<' => tag = true,
            '>' => {
                tag = false;
                o.push(' ');
            }
            c if !tag => o.push(c),
            _ => {}
        }
    }
    let o = o.replace("&amp;", "&").replace("&quot;", "\"").replace("&#39;", "'").replace("&nbsp;", " ");
    o.split_whitespace().collect::<Vec<_>>().join(" ").replace(" ,", ",").trim_matches(|c| c == ',' || c == ' ').to_string()
}

/// What a provider says about one picture.
#[derive(Debug, Clone, Default)]
pub struct Found {
    pub download: String,
    pub size: Option<u64>,
    pub title: String,
    pub author: String,
    pub licence: String,
    pub licence_url: String,
    pub source_url: String,
    pub width: u32,
    pub height: u32,
}

fn curl_base() -> Command {
    let mut c = Command::new("curl");
    c.args(["-L", "--fail", "--silent", "--show-error", "--retry", "3", "--retry-delay", "5", "--max-time", "1800", "-A", USER_AGENT]);
    c
}

fn get_json(url: &str) -> Result<serde_json::Value, String> {
    let o = curl_base().args(["--max-time", "60", url]).output().map_err(|e| format!("curl: {e}"))?;
    if !o.status.success() {
        return Err(format!("{url}: {}", String::from_utf8_lossy(&o.stderr).trim()));
    }
    serde_json::from_slice(&o.stdout).map_err(|e| format!("{url}: {e}"))
}

fn urlenc(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            o.push(b as char);
        } else {
            o += &format!("%{b:02X}");
        }
    }
    o
}

/// Poly Haven: `/files/<id>` (the tonemapped 8K JPG) and `/info/<id>`.
pub fn find_polyhaven(id: &str) -> Result<Found, String> {
    let files = get_json(&format!("https://api.polyhaven.com/files/{id}"))?;
    let t = files.get("tonemapped").ok_or("no tonemapped JPG")?;
    let info = get_json(&format!("https://api.polyhaven.com/info/{id}"))?;
    let authors: Vec<String> = info.get("authors").and_then(|a| a.as_object()).map(|m| m.keys().cloned().collect()).unwrap_or_default();
    Ok(Found {
        download: t.get("url").and_then(|u| u.as_str()).ok_or("no url")?.to_string(),
        size: t.get("size").and_then(|s| s.as_u64()),
        title: info.get("name").and_then(|n| n.as_str()).unwrap_or(id).to_string(),
        author: authors.join(", "),
        licence: "CC0".into(),
        licence_url: "https://creativecommons.org/publicdomain/zero/1.0/".into(),
        source_url: format!("https://polyhaven.com/a/{id}"),
        width: 8192,
        height: 4096,
    })
}

/// Wikimedia Commons: the file's licence and credit from `extmetadata`, the 8192 px wide rendition.
pub fn find_commons(file: &str) -> Result<Found, String> {
    let url = format!(
        "https://commons.wikimedia.org/w/api.php?action=query&format=json&prop=imageinfo&iiprop=url%7Csize%7Cextmetadata&iiurlwidth=8192&iiextmetadatafilter=LicenseShortName%7CLicenseUrl%7CArtist%7CObjectName&titles={}",
        urlenc(&format!("File:{file}"))
    );
    let j = get_json(&url)?;
    let page = j.pointer("/query/pages").and_then(|p| p.as_object()).and_then(|m| m.values().next()).ok_or("not found on Commons")?;
    let ii = page.pointer("/imageinfo/0").ok_or("not found on Commons")?;
    let meta = |k: &str| ii.pointer(&format!("/extmetadata/{k}/value")).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let w = ii.get("width").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let h = ii.get("height").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    // the original file: Wikimedia serves thumbnails only in a few widths (at most 3840), not 8K
    let bytes = ii.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
    if bytes > 200_000_000 {
        return Err(format!("the original is {} MB: too big", bytes / 1_000_000));
    }
    Ok(Found {
        download: ii.get("url").and_then(|u| u.as_str()).ok_or("no url")?.to_string(),
        size: (bytes > 0).then_some(bytes),
        title: strip_html(&meta("ObjectName")),
        author: strip_html(&meta("Artist")),
        licence: strip_html(&meta("LicenseShortName")),
        licence_url: meta("LicenseUrl"),
        source_url: ii.get("descriptionurl").and_then(|u| u.as_str()).unwrap_or("").to_string(),
        width: w,
        height: h,
    })
}

/// A direct link with its terms written in `sources.toml`.
pub fn find_url(s: &Source) -> Result<Found, String> {
    if s.url.is_empty() || s.licence.is_empty() || s.author.is_empty() {
        return Err("a `url` source needs url, author and licence".into());
    }
    Ok(Found { download: s.url.clone(), size: None, title: s.title.clone(), author: s.author.clone(), licence: s.licence.clone(), licence_url: s.licence_url.clone(), source_url: s.source_url.clone(), ..Default::default() })
}

/// `curl -C -` into a part file (resumes); a finished part of the expected size is not asked for again.
pub fn download(url: &str, part: &Path, expect: Option<u64>) -> Result<(), String> {
    if let Some(p) = part.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    let have = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0);
    if have > 0 && expect == Some(have) {
        return Ok(());
    }
    for attempt in 0..2 {
        let st = curl_base().args(["-C", "-", "-o"]).arg(part).arg(url).stdout(Stdio::null()).status().map_err(|e| format!("curl: {e}"))?;
        if st.success() {
            return Ok(());
        }
        // a stale or oversize part (the file changed, or the range was refused): start again once
        if attempt == 0 {
            let _ = std::fs::remove_file(part);
        }
    }
    Err(format!("download failed: {url}"))
}

/// Width and height of an image (ImageMagick), which also proves that it decodes.
pub fn identify(p: &Path) -> Result<(u32, u32), String> {
    let o = Command::new("magick").args(["identify", "-format", "%w %h", &format!("{}[0]", p.display())]).output().map_err(|e| format!("magick: {e}"))?;
    if !o.status.success() {
        return Err(format!("not an image: {}", String::from_utf8_lossy(&o.stderr).lines().next().unwrap_or("")));
    }
    let s = String::from_utf8_lossy(&o.stdout).to_string();
    let mut it = s.split_whitespace().filter_map(|x| x.parse::<u32>().ok());
    match (it.next(), it.next()) {
        (Some(w), Some(h)) => Ok((w, h)),
        _ => Err("cannot read the size".into()),
    }
}

fn magick(args: &[String]) -> Result<(), String> {
    let o = Command::new("nice").args(["-n", "19", "magick", "-define", "jpeg:size=8192x4096", "-limit", "memory", "3GiB", "-limit", "map", "6GiB", "-limit", "disk", "8GiB"]).args(args).output().map_err(|e| format!("magick: {e}"))?;
    if o.status.success() {
        Ok(())
    } else {
        Err(format!("magick: {}", String::from_utf8_lossy(&o.stderr).lines().next().unwrap_or("failed")))
    }
}

/// Make the DDS files (DXT1, no mipmaps: WayVR draws one level) and the thumbnail from the source image.
pub fn convert(src: &Path, dir: &Path, slug: &str, sw: u32) -> Result<(Vec<FileInfo>, String), String> {
    std::fs::create_dir_all(dir.join("thumbs")).map_err(|e| e.to_string())?;
    let input = format!("{}[0]", src.display());
    let mut files = Vec::new();
    for (label, w) in SIZES {
        // 5% slack: a 6000 px panorama is a 4K one, a 10000 px one an 8K one
        if (sw as f32) < w as f32 * 0.95 {
            continue;
        }
        let name = format!("{slug}_{label}.dds");
        let tmp = dir.join(format!(".{name}.tmp.dds"));
        magick(&[input.clone(), "-resize".into(), format!("{w}x{}!", w / 2), "-define".into(), "dds:compression=dxt1".into(), "-define".into(), "dds:mipmaps=0".into(), tmp.display().to_string()])?;
        let fin = dir.join(&name);
        std::fs::rename(&tmp, &fin).map_err(|e| e.to_string())?;
        files.push(FileInfo { res: label.into(), path: name, width: w, height: w / 2, size: std::fs::metadata(&fin).map(|m| m.len()).unwrap_or(0), sha256: sha256::file_hex(&fin).map_err(|e| e.to_string())? });
    }
    if files.is_empty() {
        return Err(format!("only {sw} px wide: too small for a 2K background"));
    }
    let thumb = format!("thumbs/{slug}.png");
    make_thumb(&input, &dir.join(&thumb))?;
    Ok((files, thumb))
}

/// A 384x192 PNG thumbnail (the app decodes PNG only) of an image magick can read.
pub fn make_thumb(input: &str, out: &Path) -> Result<(), String> {
    let tmp = out.with_file_name(format!(".{}.tmp.png", out.file_stem().and_then(|s| s.to_str()).unwrap_or("t")));
    magick(&[input.into(), "-resize".into(), "384x192!".into(), "-strip".into(), tmp.display().to_string()])?;
    std::fs::rename(tmp, out).map_err(|e| e.to_string())
}

/// Pictures whose thumbnail is missing (or an older JPEG one): made again from the smallest DDS. True when the
/// manifest changed.
fn heal_thumbs(dir: &Path, man: &mut Manifest) -> bool {
    let mut changed = false;
    for e in man.images.iter_mut().filter(|e| !e.files.is_empty()) {
        if e.thumb.ends_with(".png") && dir.join(&e.thumb).is_file() {
            continue;
        }
        let Some(small) = e.files.iter().min_by_key(|f| f.width) else { continue };
        let new = format!("thumbs/{}.png", e.slug());
        let _ = std::fs::create_dir_all(dir.join("thumbs"));
        if make_thumb(&format!("{}[0]", dir.join(&small.path).display()), &dir.join(&new)).is_ok() {
            if e.thumb != new {
                let _ = std::fs::remove_file(dir.join(&e.thumb));
            }
            e.thumb = new;
            changed = true;
        }
    }
    changed
}

fn read_manifest(dir: &Path) -> Manifest {
    std::fs::read_to_string(dir.join("manifest.toml")).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default()
}

fn write_manifest(dir: &Path, m: &Manifest) -> Result<(), String> {
    let mut m = m.clone();
    m.images.sort_by(|a, b| a.id.cmp(&b.id));
    let body = toml::to_string_pretty(&m).map_err(|e| e.to_string())?;
    let text = format!("# written by pimax-backgrounds; every picture: source, author, licence, files with sha256\n{body}");
    let tmp = dir.join(".manifest.toml.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(tmp, dir.join("manifest.toml")).map_err(|e| e.to_string())
}

/// Bytes of every file under the library.
pub fn library_bytes(root: &Path) -> u64 {
    fn walk(p: &Path) -> u64 {
        std::fs::read_dir(p).map(|rd| rd.flatten().map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => walk(&e.path()),
            Ok(m) => m.len(),
            _ => 0,
        }).sum()).unwrap_or(0)
    }
    walk(root)
}

/// Free bytes on the disk of `p` (`df`).
pub fn free_bytes(p: &Path) -> Option<u64> {
    let mut q = p;
    while !q.exists() {
        q = q.parent()?;
    }
    let o = Command::new("df").args(["--output=avail", "-B1"]).arg(q).output().ok()?;
    String::from_utf8_lossy(&o.stdout).lines().nth(1)?.trim().parse().ok()
}

/// Is every file of this entry on disk with the right size (and, if `deep`, the right checksum)?
pub fn entry_ok(dir: &Path, e: &Entry, deep: bool) -> bool {
    !e.files.is_empty()
        && dir.join(&e.thumb).is_file()
        // a download made before the sizes were complete (a thumbnail instead of the original) is done again
        && (e.provider == IMPORTED || SIZES.iter().all(|(l, w)| (e.source_width as f32) < *w as f32 * 0.95 || e.files.iter().any(|f| f.res == *l)))
        && e.files.iter().all(|f| {
            let p = dir.join(&f.path);
            std::fs::metadata(&p).is_ok_and(|m| m.len() == f.size) && (!deep || sha256::file_hex(&p).is_ok_and(|h| h == f.sha256))
        })
}

/// What to download.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Only this theme.
    pub theme: Option<String>,
    /// Only this id (`theme/slug`) or slug.
    pub only: Option<String>,
    /// At most this many new pictures.
    pub limit: Option<usize>,
    /// Check the checksum of what is there before skipping it.
    pub deep: bool,
}

/// One progress report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub id: String,
    /// `ok` (already there), `new`, `failed: ...`, `skipped: ...`.
    pub status: String,
}

impl Progress {
    /// `PROGRESS 3/56 ocean/umhlanga_sunrise new`: the line the app reads.
    pub fn line(&self) -> String {
        format!("PROGRESS {}/{} {} {}", self.done, self.total, self.id, self.status)
    }

    pub fn parse(l: &str) -> Option<Progress> {
        let r = l.strip_prefix("PROGRESS ")?;
        let (frac, rest) = r.split_once(' ')?;
        let (d, t) = frac.split_once('/')?;
        let (id, status) = rest.split_once(' ')?;
        Some(Progress { done: d.parse().ok()?, total: t.parse().ok()?, id: id.into(), status: status.into() })
    }
}

/// Download one source into the library; returns the status word.
pub fn fetch_one(d: &Dirs, s: &Source, deep: bool) -> Result<&'static str, String> {
    let dir = d.library.join(&s.theme);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut man = read_manifest(&dir);
    let id = s.id();
    if man.images.iter().any(|e| e.id == id && entry_ok(&dir, e, deep)) {
        return Ok("ok");
    }
    let found = match s.provider.as_str() {
        "polyhaven" => find_polyhaven(&s.r#ref)?,
        "commons" => find_commons(&s.r#ref)?,
        "url" => find_url(s)?,
        p => return Err(format!("unknown provider {p}")),
    };
    if !licence_ok(&found.licence) {
        return Err(format!("licence \"{}\" does not allow a free download: skipped", found.licence));
    }
    let part = d.library.join(".part").join(format!("{}_{}.src", s.theme, s.slug()));
    download(&found.download, &part, found.size)?;
    if let Some(sz) = found.size {
        let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        if have != sz {
            let _ = std::fs::remove_file(&part);
            return Err(format!("downloaded {have} bytes, expected {sz}"));
        }
    }
    let (w, h) = match identify(&part) {
        Ok(x) => x,
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            return Err(e);
        }
    };
    if (w as f32 / h as f32 - 2.0).abs() > 0.1 {
        let _ = std::fs::remove_file(&part);
        return Err(format!("{w}x{h} is not an equirectangular 2:1 panorama"));
    }
    let (files, thumb) = convert(&part, &dir, &s.slug(), w)?;
    let _ = std::fs::remove_file(&part);
    let title = if !s.title.is_empty() { s.title.clone() } else if !found.title.is_empty() { found.title.clone() } else { s.slug().replace('_', " ") };
    let (sw, sh) = if found.width > 0 && s.provider == "commons" { (found.width, found.height) } else { (w, h) };
    let e = Entry {
        id: id.clone(),
        theme: s.theme.clone(),
        title,
        author: found.author,
        licence: found.licence,
        licence_url: found.licence_url,
        source_url: found.source_url,
        provider: s.provider.clone(),
        source_width: sw,
        source_height: sh,
        thumb,
        files,
    };
    man.images.retain(|x| x.id != id);
    man.images.push(e);
    write_manifest(&dir, &man)?;
    Ok("new")
}

/// Download (or check) the selected sources. `out` gets one [`Progress`] per picture. Returns the number of failures.
pub fn run(d: &Dirs, o: &Options, out: &mut dyn FnMut(Progress)) -> Result<usize, String> {
    let all = sources()?;
    let sel: Vec<&Source> = all
        .iter()
        .filter(|s| o.theme.as_ref().is_none_or(|t| &s.theme == t))
        .filter(|s| o.only.as_ref().is_none_or(|x| &s.id() == x || &s.slug() == x))
        .collect();
    let total = sel.len();
    if let Some(free) = free_bytes(&d.library) {
        if free < 2_000_000_000 {
            return Err(format!("only {} MB free on the disk: not downloading", free / 1_000_000));
        }
    }
    let mut failed = 0;
    let mut fresh = 0;
    // the first set (WayVR's skybox folder) joins the library, and pictures dropped from the list leave it
    if o.theme.is_none() && o.only.is_none() {
        let _ = import_skybox(d, &skybox_dir());
        let _ = prune(d);
    }
    for t in crate::themes(d) {
        let dir = d.library.join(&t);
        let mut m = read_manifest(&dir);
        if heal_thumbs(&dir, &mut m) {
            let _ = write_manifest(&dir, &m);
        }
    }
    for (i, s) in sel.iter().enumerate() {
        let id = s.id();
        if library_bytes(&d.library) > MAX_LIBRARY_BYTES {
            out(Progress { done: i + 1, total, id, status: "skipped: the library is at its 4 GB limit".into() });
            continue;
        }
        if o.limit.is_some_and(|l| fresh >= l) {
            // the rest only gets the cheap "already there" check
            let dir = d.library.join(&s.theme);
            let there = read_manifest(&dir).images.iter().any(|e| e.id == id && entry_ok(&dir, e, false));
            out(Progress { done: i + 1, total, id, status: if there { "ok".into() } else { "skipped: limit".into() } });
            continue;
        }
        let status = match fetch_one(d, s, o.deep) {
            Ok(w) => {
                if w == "new" {
                    fresh += 1;
                    // be polite to the free services
                    std::thread::sleep(std::time::Duration::from_millis(1500));
                }
                w.to_string()
            }
            Err(e) => {
                failed += 1;
                format!("failed: {}", e.replace('\n', " "))
            }
        };
        out(Progress { done: i + 1, total, id, status });
    }
    let _ = std::fs::remove_dir_all(d.library.join(".part"));
    Ok(failed)
}

/// Provider name of the pictures taken over from WayVR's own skybox folder.
pub const IMPORTED: &str = "wayvr-skybox";

/// Where the first set of backdrop pictures lives (BC7, `<name>.dds` 4K, `2k/`, `8k/`, `CREDITS.txt`).
pub fn skybox_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("PIMAX_SKYBOX_DIR").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("src/desk-out/wayvr-home-am/config/wayvr/skybox")
}

/// (theme, title) of a picture of the skybox folder by its file stem.
fn import_theme(stem: &str) -> (&'static str, &'static str) {
    match stem {
        "flowers" => ("flowers", "Flower road"),
        "garden" => ("flowers", "Rose garden"),
        "night-sky" => ("night", "Night sky"),
        "ocean-waves" => ("ocean", "Ocean waves"),
        "hawaii" => ("ocean", "Tropical beach"),
        "beach" => ("ocean", "Beach"),
        "mountains" => ("mountains", "Solitary mountain"),
        "forest" => ("forest", "Rainforest trail"),
        "sunset" => ("sunset", "Sunset"),
        "snow" => ("snow", "Snowy lake"),
        "desert" => ("desert", "Desert valley"),
        "city-night" => ("city", "City night"),
        "lake" => ("lake", "Lakeside sunrise"),
        "moon" => ("night", "Moonlit field"),
        "autumn" => ("forest", "Autumn field"),
        "universe" => ("universe", "Milky Way"),
        _ => ("misc", "Picture"),
    }
}

/// The blocks of a skybox `CREDITS.txt`: (file name, source url, author, licence).
pub fn parse_credits(text: &str) -> Vec<(String, String, String, String)> {
    let mut out: Vec<(String, String, String, String)> = Vec::new();
    for l in text.lines() {
        if let Some((name, _)) = l.split_once("  <-  ") {
            if name.ends_with(".dds") {
                out.push((name.trim().to_string(), String::new(), String::new(), String::new()));
            }
            continue;
        }
        let Some(cur) = out.last_mut() else { continue };
        let t = l.trim();
        if let Some(v) = t.strip_prefix("source:") {
            cur.1 = v.trim().to_string();
        } else if let Some(v) = t.strip_prefix("author:") {
            cur.2 = v.trim().to_string();
        } else if let Some(v) = t.strip_prefix("license:") {
            cur.3 = v.trim().split(" (").next().unwrap_or("").to_string();
        }
    }
    out
}

/// Take over the pictures of WayVR's skybox folder (the first 16): symlinked files (nothing is copied), the credits
/// of `CREDITS.txt`, a thumbnail. Their BC7 files are better than the DXT1 ones this tool makes. Returns how many.
pub fn import_skybox(d: &Dirs, from: &Path) -> Result<usize, String> {
    let credits = std::fs::read_to_string(from.join("CREDITS.txt")).map_err(|e| format!("{}: {e}", from.join("CREDITS.txt").display()))?;
    let mut n = 0;
    for (file, source, author, licence) in parse_credits(&credits) {
        let stem = file.trim_end_matches(".dds").to_string();
        let (theme, title) = import_theme(&stem);
        let slug = stem.replace('-', "_");
        let dir = d.library.join(theme);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let mut man = read_manifest(&dir);
        let id = format!("{theme}/{slug}");
        let mut files = Vec::new();
        for (label, sub, w) in [("8k", "8k", 8192u32), ("4k", "", 4096), ("2k", "2k", 2048)] {
            let src = if sub.is_empty() { from.join(&file) } else { from.join(sub).join(&file) };
            if !src.is_file() {
                continue;
            }
            let name = format!("{slug}_{label}.dds");
            let link = dir.join(&name);
            if std::fs::read_link(&link).ok().as_deref() != Some(src.as_path()) {
                let _ = std::fs::remove_file(&link);
                std::os::unix::fs::symlink(&src, &link).map_err(|e| e.to_string())?;
            }
            let prev = man.images.iter().find(|e| e.id == id).and_then(|e| e.files.iter().find(|f| f.res == label));
            let size = std::fs::metadata(&link).map(|m| m.len()).unwrap_or(0);
            let sha256 = match prev {
                Some(p) if p.size == size && !p.sha256.is_empty() => p.sha256.clone(),
                _ => sha256::file_hex(&link).map_err(|e| e.to_string())?,
            };
            files.push(FileInfo { res: label.into(), path: name, width: w, height: w / 2, size, sha256 });
        }
        if files.is_empty() {
            continue;
        }
        let thumb = format!("thumbs/{slug}.png");
        if !dir.join(&thumb).is_file() {
            std::fs::create_dir_all(dir.join("thumbs")).map_err(|e| e.to_string())?;
            let small = files.iter().min_by_key(|f| f.width).map(|f| f.path.clone()).unwrap_or_default();
            let _ = make_thumb(&format!("{}[0]", dir.join(small).display()), &dir.join(&thumb));
        }
        let top = files.iter().map(|f| f.width).max().unwrap_or(4096);
        let e = Entry {
            id: id.clone(),
            theme: theme.into(),
            title: title.into(),
            author,
            licence,
            licence_url: String::new(),
            source_url: source,
            provider: IMPORTED.into(),
            source_width: top,
            source_height: top / 2,
            thumb,
            files,
        };
        man.images.retain(|x| x.id != id);
        man.images.push(e);
        write_manifest(&dir, &man)?;
        n += 1;
    }
    Ok(n)
}

/// Remove pictures that are no longer in `sources.toml` (and are not imported): files, thumbnail, manifest row.
pub fn prune(d: &Dirs) -> Vec<String> {
    let keep: Vec<String> = sources().unwrap_or_default().iter().map(Source::id).collect();
    let mut gone = Vec::new();
    for t in crate::themes(d) {
        let dir = d.library.join(&t);
        let mut m = read_manifest(&dir);
        let before = m.images.len();
        m.images.retain(|e| {
            let stay = e.provider == IMPORTED || keep.contains(&e.id);
            if !stay {
                for f in &e.files {
                    let _ = std::fs::remove_file(dir.join(&f.path));
                }
                let _ = std::fs::remove_file(dir.join(&e.thumb));
                gone.push(e.id.clone());
            }
            stay
        });
        if m.images.len() != before {
            let _ = write_manifest(&dir, &m);
        }
    }
    gone
}

/// Check every file of the library (size; with `deep` also sha256). Returns one line per problem.
pub fn verify(d: &Dirs, deep: bool) -> Vec<String> {
    let mut bad = Vec::new();
    for t in crate::themes(d) {
        let dir: PathBuf = d.library.join(&t);
        for e in read_manifest(&dir).images {
            if !entry_ok(&dir, &e, deep) {
                bad.push(format!("{}: files missing or changed", e.id));
            }
        }
    }
    bad
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_curated_list_is_sound() {
        let v = sources().unwrap();
        assert!(v.len() >= 40, "{}", v.len());
        let mut ids: Vec<String> = v.iter().map(Source::id).collect();
        ids.sort();
        let n = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), n, "every id once");
        for s in &v {
            assert!(matches!(s.provider.as_str(), "polyhaven" | "commons" | "url"), "{s:?}");
            assert!(!s.theme.is_empty() && !s.slug().is_empty(), "{s:?}");
            if s.provider == "url" {
                assert!(s.url.starts_with("https://") && licence_ok(&s.licence) && !s.author.is_empty(), "{s:?}");
            } else {
                assert!(!s.r#ref.is_empty(), "{s:?}");
            }
        }
    }

    #[test]
    fn licences_and_names() {
        for ok in ["CC0", "CC BY 4.0", "CC BY-SA 3.0 IGO", "Public domain", "NASA media usage guidelines (public domain)"] {
            assert!(licence_ok(ok), "{ok}");
        }
        for no in ["CC BY-NC 4.0", "All rights reserved", "CC BY-ND 2.0", ""] {
            assert!(!licence_ok(no), "{no}");
        }
        assert_eq!(slugify("Gemini North Sunset 360 Panorama (2022 04 01 Gemini North 360 Sunset2-CC).jpg"), "gemini_north_sunset_360_panorama_2022_04_01_gemini_north_360_sunset2_cc");
        assert_eq!(slugify("wildflower_field"), "wildflower_field");
        assert_eq!(strip_html("<a href=\"x\">Greg Zaal</a>, <i>Rico</i> &amp; co"), "Greg Zaal, Rico & co");
    }

    #[test]
    fn credits_blocks_are_read() {
        let t = "Skybox credits.\n\nflowers.dds  <-  Flower Road\n   source: https://polyhaven.com/a/flower_road\n   author: Greg Zaal\n   license: CC0 1.0\nuniverse.dds  <-  ESO Milky Way (x)\n   source: https://commons.wikimedia.org/wiki/File:ESO_-_Milky_Way.jpg\n   author: ESO/S. Brunier\n   license: CC BY 4.0 (https://creativecommons.org/licenses/by/4.0/) - credit required\n";
        let c = parse_credits(t);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0], ("flowers.dds".into(), "https://polyhaven.com/a/flower_road".into(), "Greg Zaal".into(), "CC0 1.0".into()));
        assert_eq!((c[1].2.as_str(), c[1].3.as_str()), ("ESO/S. Brunier", "CC BY 4.0"));
    }

    #[test]
    fn import_links_and_prune_removes() {
        let d = crate::tests::tmp("import");
        let from = d.library.parent().unwrap().join("skybox");
        std::fs::create_dir_all(from.join("2k")).unwrap();
        std::fs::write(from.join("CREDITS.txt"), "night-sky.dds  <-  Rogland\n   source: https://x/y\n   author: Greg\n   license: CC0 1.0\n").unwrap();
        std::fs::write(from.join("night-sky.dds"), b"4k").unwrap();
        std::fs::write(from.join("2k/night-sky.dds"), b"2").unwrap();
        // magick cannot read these stand-ins: a missing thumbnail is the only error, the files still link
        let _ = import_skybox(&d, &from);
        let m = read_manifest(&d.library.join("night"));
        assert_eq!(m.images.len(), 1);
        let e = &m.images[0];
        assert_eq!((e.id.as_str(), e.provider.as_str(), e.files.len()), ("night/night_sky", IMPORTED, 2));
        assert!(e.matches_name("night-sky") && e.matches_name("night_night_sky") == false);
        assert!(std::fs::symlink_metadata(d.library.join("night/night_sky_4k.dds")).unwrap().file_type().is_symlink());
        // imported pictures stay; a downloaded one that left the list goes
        let mut m2 = read_manifest(&d.library.join("night"));
        let mut gone = m2.images[0].clone();
        gone.id = "night/old".into();
        gone.provider = "polyhaven".into();
        gone.thumb = "thumbs/old.png".into();
        gone.files = vec![];
        m2.images.push(gone);
        write_manifest(&d.library.join("night"), &m2).unwrap();
        assert_eq!(prune(&d), vec!["night/old".to_string()]);
    }

    #[test]
    fn progress_lines_round_trip() {
        let p = Progress { done: 3, total: 56, id: "ocean/x".into(), status: "failed: no network".into() };
        assert_eq!(Progress::parse(&p.line()), Some(p));
        assert_eq!(Progress::parse("something else"), None);
    }

    #[test]
    fn entry_checks_sizes_and_sums() {
        let d = crate::tests::tmp("entry");
        crate::tests::sample(&d);
        let dir = d.library.join("ocean");
        let m = read_manifest(&dir);
        let mut e = m.images[0].clone();
        assert!(!entry_ok(&dir, &e, false), "sample sums are placeholders, thumbs missing");
        std::fs::write(dir.join(&e.thumb), b"j").unwrap();
        assert!(entry_ok(&dir, &e, false));
        assert!(!entry_ok(&dir, &e, true), "wrong sha256");
        for f in &mut e.files {
            f.sha256 = sha256::file_hex(&dir.join(&f.path)).unwrap();
        }
        assert!(entry_ok(&dir, &e, true));
        std::fs::write(dir.join(&e.files[0].path), b"changed!").unwrap();
        assert!(!entry_ok(&dir, &e, false), "size differs");
    }
}
