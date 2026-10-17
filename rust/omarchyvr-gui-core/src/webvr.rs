//! Web pages and VR (docs/research/webxr.md): what the user's browser can do with the headset, and "Open a web page
//! in VR" for the Work desk.
//!
//! Findings behind it (2026-10-17, read-only on this PC):
//! * **WebXR immersive VR does not reach the headset from any browser packaged for Arch.** Chrome/Chromium build their
//!   OpenXR backend only for Windows and Android (the Linux `chrome` binary has no OpenXR loader, no `XR_KHR_*` string),
//!   so `navigator.xr.isSessionSupported("immersive-vr")` never says yes, whatever runtime runs. Firefox has no WebXR
//!   on desktop Linux; Arch's WebKitGTK (Epiphany) is built without OpenXR. No app switch can change that.
//! * **Even with a WebXR browser** a page cannot enter VR "on load": `requestSession("immersive-vr")` needs a user
//!   click (transient activation) unless the browser is already in VR (`sessiongranted`).
//! * **WebGPU** is on by default in Chrome/Chromium 147+ on Wayland with NVIDIA 535.183.01+ (gpuweb Implementation
//!   Status); no flags are needed and `--enable-unsafe-webgpu` (which turns off the GPU blocklist) is not used.
//! * What works today: Work's VR screens are Hyprland outputs (the main desktop is the main VR screen), so a browser
//!   window opened during Work is a flat screen in the headset. [`open_plan`] opens a URL there.
//!
//! Everything here reads files or runs `<browser> --version`; nothing is written and no profile is touched.
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Chrome/Chromium version from which WebGPU is on by default for NVIDIA on Wayland (gpuweb Implementation Status).
pub const WEBGPU_CHROME_NVIDIA_WAYLAND: u32 = 147;
/// Oldest NVIDIA driver Chrome enables WebGPU on (Linux, Wayland).
pub const WEBGPU_NVIDIA_MIN: (u32, u32, u32) = (535, 183, 1);
/// A WebGPU shader cache written this recently counts as "used on this PC".
pub const USED_RECENTLY: Duration = Duration::from_secs(14 * 24 * 3600);

/// The browser family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Chrome,
    Chromium,
    /// Brave, Vivaldi, Edge, Opera, Helium: Chromium inside.
    ChromiumBased,
    Firefox,
    Other,
}

impl Kind {
    /// From the desktop entry id (`google-chrome.desktop`) and its `Exec=` program.
    pub fn of(desktop_id: &str, exec: &str) -> Kind {
        let s = format!("{} {}", desktop_id, exec).to_lowercase();
        if s.contains("google-chrome") || s.contains("/opt/google/chrome") {
            Kind::Chrome
        } else if s.contains("chromium") {
            Kind::Chromium
        } else if ["brave", "vivaldi", "microsoft-edge", "opera", "helium"].iter().any(|b| s.contains(b)) {
            Kind::ChromiumBased
        } else if ["firefox", "librewolf", "zen", "floorp", "waterfox"].iter().any(|b| s.contains(b)) {
            Kind::Firefox
        } else {
            Kind::Other
        }
    }

    fn chromium_family(self) -> bool {
        matches!(self, Kind::Chrome | Kind::Chromium | Kind::ChromiumBased)
    }
}

/// An answer in plain words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Works as it is.
    Yes(String),
    /// Works with a change the user can make (a newer browser, a flag); the text says which.
    Maybe(String),
    /// Not possible on this PC today; the text says why.
    No(String),
}

impl Verdict {
    pub fn text(&self) -> &str {
        match self {
            Verdict::Yes(t) | Verdict::Maybe(t) | Verdict::No(t) => t,
        }
    }
}

/// The first program of the main `Exec=` line of a desktop entry (what `omarchy-launch-browser` runs).
pub fn exec_of(desktop: &str) -> Option<String> {
    let mut main = false;
    for l in desktop.lines() {
        let l = l.trim();
        if l.starts_with('[') {
            main = l == "[Desktop Entry]";
            continue;
        }
        if main {
            if let Some(rest) = l.strip_prefix("Exec=") {
                return rest.split_whitespace().next().map(str::to_string);
            }
        }
    }
    None
}

/// The major version from `--version` output ("Google Chrome 155.0.8059.39", "Chromium 152.0.7977.82 Arch Linux",
/// "Mozilla Firefox 150.0").
pub fn major_version(out: &str) -> Option<u32> {
    out.split_whitespace().find_map(|w| {
        let first = w.split('.').next()?;
        (w.contains('.') && !first.is_empty() && first.chars().all(|c| c.is_ascii_digit())).then(|| first.parse().ok()).flatten()
    })
}

/// `/sys/module/nvidia/version` ("610.57.04") as numbers.
pub fn nvidia_version(text: &str) -> Option<(u32, u32, u32)> {
    let mut it = text.trim().split('.').map(|p| p.parse::<u32>().ok());
    let a = it.next()??;
    let b = it.next().flatten().unwrap_or(0);
    let c = it.next().flatten().unwrap_or(0);
    Some((a, b, c))
}

/// Does the browser draw on Wayland? Its flags file (`~/.config/chrome-flags.conf`) decides; without a choice there,
/// Chrome follows the session (Wayland when `WAYLAND_DISPLAY` is set).
pub fn on_wayland(flags_conf: Option<&str>, wayland_session: bool) -> bool {
    let mut choice = None;
    for l in flags_conf.unwrap_or("").lines().map(str::trim).filter(|l| !l.starts_with('#')) {
        for f in l.split_whitespace() {
            if let Some(v) = f.strip_prefix("--ozone-platform=").or_else(|| f.strip_prefix("--ozone-platform-hint=")) {
                // --ozone-platform wins over the hint: only let the hint decide when nothing chose yet
                if f.starts_with("--ozone-platform=") || choice.is_none() {
                    choice = Some(match v {
                        "wayland" => true,
                        "x11" => false,
                        _ => wayland_session,
                    });
                }
            }
        }
    }
    choice.unwrap_or(wayland_session)
}

/// WebGPU in this browser on this PC.
pub fn webgpu(kind: Kind, major: Option<u32>, wayland: bool, nvidia: Option<(u32, u32, u32)>) -> Verdict {
    if kind == Kind::Firefox {
        return Verdict::No("Firefox has WebGPU on by default only on Windows and macOS so far; use Chrome or Chromium for WebGPU pages".into());
    }
    if !kind.chromium_family() {
        return Verdict::No("this browser has no WebGPU on Linux; use Chrome or Chromium for WebGPU pages".into());
    }
    let Some(nv) = nvidia else {
        return Verdict::Maybe("depends on the graphics card (Chrome turns it on for Intel Gen12+ from version 144); chrome://gpu says".into());
    };
    if nv < WEBGPU_NVIDIA_MIN {
        return Verdict::Maybe(format!("the NVIDIA driver {}.{}.{} is older than 535.183.01, which Chrome needs for WebGPU", nv.0, nv.1, nv.2));
    }
    if !wayland {
        return Verdict::Maybe("the browser runs on X11; Chrome turns WebGPU on for NVIDIA on Wayland only".into());
    }
    match major {
        Some(m) if m >= WEBGPU_CHROME_NVIDIA_WAYLAND => Verdict::Yes(format!("on by default (version {m}, Wayland, NVIDIA {}.{}); no flags needed", nv.0, nv.1)),
        Some(m) => Verdict::Maybe(format!("version {m} is older than {WEBGPU_CHROME_NVIDIA_WAYLAND}: update the browser to get WebGPU on NVIDIA")),
        None => Verdict::Maybe(format!("on by default from version {WEBGPU_CHROME_NVIDIA_WAYLAND} (Wayland, NVIDIA); the version could not be read")),
    }
}

/// WebXR immersive VR (a page that enters the headset) in this browser on Linux.
pub fn webxr(kind: Kind) -> Verdict {
    match kind {
        Kind::Chrome | Kind::Chromium | Kind::ChromiumBased => Verdict::No(
            "Chrome and Chromium on Linux have no OpenXR: their WebXR reaches a headset only on Windows and Android, so no page can enter VR from them here".into(),
        ),
        Kind::Firefox => Verdict::No("Firefox has no WebXR on desktop Linux".into()),
        Kind::Other => Verdict::No("no browser packaged for Arch connects WebXR to OpenXR".into()),
    }
}

/// The OpenXR runtime a program started outside the app sees (`~/.config/openxr/1/active_runtime.json`): its target
/// path (after links) and text.
pub fn outside_runtime(target: Option<&Path>, text: Option<&str>) -> String {
    let (Some(t), Some(text)) = (target, text) else {
        return "none (no ~/.config/openxr/1/active_runtime.json)".into();
    };
    let p = t.to_string_lossy().to_lowercase();
    if text.contains("VALVE_runtime_is_steamvr") || p.contains("steamvr") {
        "SteamVR; a browser does not use it either way".into()
    } else if p.contains("monado") || text.to_lowercase().contains("monado") {
        "Monado".into()
    } else {
        format!("{}", t.display())
    }
}

/// Where the browser keeps its WebGPU shader cache (profile `Default`).
pub fn dawn_cache(home: &Path, kind: Kind) -> Option<PathBuf> {
    let dir = match kind {
        Kind::Chrome => ".config/google-chrome",
        Kind::Chromium => ".config/chromium",
        _ => return None,
    };
    Some(home.join(dir).join("Default/DawnWebGPUCache"))
}

/// "used on this PC N ago" from the cache's last change.
pub fn used_line(modified: Option<SystemTime>, now: SystemTime) -> Option<String> {
    let age = now.duration_since(modified?).unwrap_or_default();
    (age <= USED_RECENTLY).then(|| format!("a WebGPU page ran in it {} ago", ago(age)))
}

fn ago(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..=119 => "a minute".into(),
        120..=7199 => format!("{} minutes", s / 60),
        7200..=172_799 => format!("{} hours", s / 3600),
        _ => format!("{} days", s / 86_400),
    }
}

/// A typed address made into a URL: `https://` added, only http(s), no spaces or control characters.
pub fn normalize_url(s: &str) -> Result<String, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("Type the address of the page first".into());
    }
    if s.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("An address has no spaces".into());
    }
    let lower = s.to_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Ok(s.to_string());
    }
    if lower.contains("://") || lower.starts_with("javascript:") || lower.starts_with("file:") || lower.starts_with("data:") {
        return Err("Only web addresses (http or https)".into());
    }
    if !s.contains('.') && !lower.starts_with("localhost") {
        return Err("That does not look like a web address".into());
    }
    Ok(format!("https://{s}"))
}

/// What "Open in VR" does now. `side`: [`crate::app::AppCore::running_side`] (Some(false) = Work).
pub fn open_plan(side: Option<bool>, url: &str) -> Result<Vec<String>, String> {
    match side {
        Some(false) => {}
        Some(true) => return Err("Play is for games: switch to Work to see web pages in the headset".into()),
        None => return Err("Press Work first: web pages show on the VR screens of the desk".into()),
    }
    let url = normalize_url(url)?;
    // Omarchy's own launcher: the user's default browser with its flags; a running browser gets a new tab
    Ok(vec!["omarchy-launch-browser".into(), url])
}

/// What the Desktop page shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// "Google Chrome 155" (the default browser).
    pub browser: String,
    pub webgpu: Verdict,
    /// Evidence the browser ran WebGPU here.
    pub webgpu_used: Option<String>,
    pub webxr: Verdict,
    /// The OpenXR runtime programs started outside the app see.
    pub runtime: String,
}

/// The Desktop page's state.
#[derive(Debug, Clone, Default)]
pub struct WebView {
    pub report: Option<Report>,
    /// The address typed into "Open a web page in VR".
    pub url: String,
}

/// Inputs of [`report`] that come from the machine.
#[derive(Debug, Clone, Default)]
pub struct Probe {
    pub desktop_id: String,
    pub desktop_text: Option<String>,
    pub version_out: Option<String>,
    pub flags_conf: Option<String>,
    pub wayland_session: bool,
    pub nvidia: Option<String>,
    pub dawn_modified: Option<SystemTime>,
    pub runtime_target: Option<PathBuf>,
    pub runtime_text: Option<String>,
}

/// The report from the probe.
pub fn report(p: &Probe, now: SystemTime) -> Report {
    let exec = p.desktop_text.as_deref().and_then(exec_of).unwrap_or_default();
    let kind = Kind::of(&p.desktop_id, &exec);
    let major = p.version_out.as_deref().and_then(major_version);
    let name = match kind {
        Kind::Chrome => "Google Chrome".to_string(),
        Kind::Chromium => "Chromium".to_string(),
        _ if !p.desktop_id.is_empty() => p.desktop_id.trim_end_matches(".desktop").to_string(),
        _ => "no default browser".to_string(),
    };
    Report {
        browser: major.map(|m| format!("{name} {m}")).unwrap_or(name),
        webgpu: webgpu(kind, major, on_wayland(p.flags_conf.as_deref(), p.wayland_session), p.nvidia.as_deref().and_then(nvidia_version)),
        webgpu_used: used_line(p.dawn_modified, now),
        webxr: webxr(kind),
        runtime: outside_runtime(p.runtime_target.as_deref(), p.runtime_text.as_deref()),
    }
}

/// The flags file the Arch launcher of this browser reads.
pub fn flags_conf_path(home: &Path, kind: Kind) -> Option<PathBuf> {
    match kind {
        Kind::Chrome => Some(home.join(".config/chrome-flags.conf")),
        Kind::Chromium => Some(home.join(".config/chromium-flags.conf")),
        _ => None,
    }
}

fn run_out(prog: &str, args: &[&str], limit: Duration) -> Option<String> {
    use std::io::Read;
    let mut ch = std::process::Command::new(prog)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let t0 = std::time::Instant::now();
    loop {
        match ch.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if t0.elapsed() < limit => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = ch.kill();
                let _ = ch.wait();
                return None;
            }
        }
    }
    let mut s = String::new();
    ch.stdout.take()?.read_to_string(&mut s).ok()?;
    Some(s.trim().to_string())
}

/// Read the machine (background thread: runs `xdg-settings` and `<browser> --version`).
pub fn probe_machine(home: &Path) -> Probe {
    let id = run_out("xdg-settings", &["get", "default-web-browser"], Duration::from_secs(5))
        .filter(|s| !s.is_empty())
        .or_else(|| run_out("xdg-mime", &["query", "default", "x-scheme-handler/https"], Duration::from_secs(5)))
        .unwrap_or_default();
    let desktop_text = if id.is_empty() || id.contains('/') {
        None
    } else {
        [home.join(".local/share/applications"), PathBuf::from("/usr/share/applications")].iter().find_map(|d| std::fs::read_to_string(d.join(&id)).ok())
    };
    let exec = desktop_text.as_deref().and_then(exec_of).unwrap_or_default();
    let kind = Kind::of(&id, &exec);
    let version_out = (!exec.is_empty()).then(|| run_out(&exec, &["--version"], Duration::from_secs(8))).flatten();
    let rt = home.join(".config/openxr/1/active_runtime.json");
    Probe {
        flags_conf: flags_conf_path(home, kind).and_then(|p| std::fs::read_to_string(p).ok()),
        wayland_session: std::env::var_os("WAYLAND_DISPLAY").is_some(),
        nvidia: std::fs::read_to_string("/sys/module/nvidia/version").ok(),
        dawn_modified: dawn_cache(home, kind).and_then(|p| std::fs::metadata(p).ok()).and_then(|m| m.modified().ok()),
        runtime_target: std::fs::canonicalize(&rt).ok(),
        runtime_text: std::fs::read_to_string(&rt).ok(),
        desktop_id: id,
        desktop_text,
        version_out,
    }
}

/// The report the screenshots show (this PC on 2026-10-17).
pub fn demo_report() -> Report {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    report(
        &Probe {
            desktop_id: "google-chrome.desktop".into(),
            desktop_text: Some("[Desktop Entry]\nExec=/usr/bin/google-chrome-stable %U\n".into()),
            version_out: Some("Google Chrome 155.0.8059.39".into()),
            flags_conf: Some("--ozone-platform=wayland\n".into()),
            wayland_session: true,
            nvidia: Some("610.57.04".into()),
            dawn_modified: Some(now - Duration::from_secs(3 * 3600)),
            runtime_target: Some("/home/user/.local/share/Steam/steamapps/common/SteamVR/steamxr_linux64.json".into()),
            runtime_text: Some("{\"runtime\":{\"VALVE_runtime_is_steamvr\":true}}".into()),
        },
        now,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHROME: &str = "[Desktop Entry]\nVersion=1.0\nName=Google Chrome\nExec=/usr/bin/google-chrome-stable %U\n\n[Desktop Action new-window]\nExec=/usr/bin/google-chrome-stable\n";

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("omarchyvr-webvr-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_browser_is_found_from_its_desktop_entry() {
        assert_eq!(exec_of(CHROME).as_deref(), Some("/usr/bin/google-chrome-stable"));
        assert_eq!(exec_of("[Desktop Action x]\nExec=/a\n"), None, "only the main section counts");
        assert_eq!(Kind::of("google-chrome.desktop", "/usr/bin/google-chrome-stable"), Kind::Chrome);
        assert_eq!(Kind::of("chromium.desktop", "/usr/bin/chromium"), Kind::Chromium);
        assert_eq!(Kind::of("brave-browser.desktop", "brave"), Kind::ChromiumBased);
        assert_eq!(Kind::of("firefox.desktop", "/usr/lib/firefox/firefox"), Kind::Firefox);
        assert_eq!(Kind::of("org.gnome.Epiphany.desktop", "epiphany"), Kind::Other);
    }

    #[test]
    fn versions_are_read_from_the_programs_words() {
        assert_eq!(major_version("Google Chrome 155.0.8059.39"), Some(155));
        assert_eq!(major_version("Chromium 152.0.7977.82 Arch Linux"), Some(152));
        assert_eq!(major_version("Mozilla Firefox 150.0"), Some(150));
        assert_eq!(major_version("no version"), None);
        assert_eq!(nvidia_version("610.57.04\n"), Some((610, 57, 4)));
        assert_eq!(nvidia_version("535.183.01"), Some((535, 183, 1)));
        assert_eq!(nvidia_version("x"), None);
    }

    #[test]
    fn wayland_follows_the_flags_file_then_the_session() {
        let omarchy = "--ozone-platform=wayland\n--ozone-platform-hint=wayland\n--enable-features=TouchpadOverscrollHistoryNavigation\n";
        assert!(on_wayland(Some(omarchy), false));
        assert!(!on_wayland(Some("--ozone-platform=x11\n"), true));
        assert!(!on_wayland(Some("--ozone-platform-hint=wayland\n--ozone-platform=x11\n"), true), "the platform beats the hint");
        assert!(on_wayland(Some("# --ozone-platform=x11\n"), true), "comments do not count");
        assert!(on_wayland(None, true));
        assert!(!on_wayland(None, false));
    }

    #[test]
    fn webgpu_is_on_by_default_for_this_pc_and_says_why_elsewhere() {
        let nv = Some((610, 57, 4));
        assert!(matches!(webgpu(Kind::Chrome, Some(155), true, nv), Verdict::Yes(t) if t.contains("no flags")));
        assert!(matches!(webgpu(Kind::Chromium, Some(152), true, nv), Verdict::Yes(_)));
        assert!(matches!(webgpu(Kind::Chrome, Some(146), true, nv), Verdict::Maybe(t) if t.contains("update")));
        assert!(matches!(webgpu(Kind::Chrome, Some(155), false, nv), Verdict::Maybe(t) if t.contains("X11")));
        assert!(matches!(webgpu(Kind::Chrome, Some(155), true, Some((535, 54, 3))), Verdict::Maybe(t) if t.contains("535.183.01")));
        assert!(matches!(webgpu(Kind::Chrome, Some(155), true, None), Verdict::Maybe(_)));
        assert!(matches!(webgpu(Kind::Firefox, Some(150), true, nv), Verdict::No(_)));
    }

    #[test]
    fn no_linux_browser_enters_vr() {
        for k in [Kind::Chrome, Kind::Chromium, Kind::ChromiumBased, Kind::Firefox, Kind::Other] {
            assert!(matches!(webxr(k), Verdict::No(_)), "{k:?}");
        }
        assert!(webxr(Kind::Chrome).text().contains("OpenXR"));
    }

    #[test]
    fn the_outside_runtime_is_named() {
        let steam = Path::new("/h/.local/share/Steam/steamapps/common/SteamVR/steamxr_linux64.json");
        assert!(outside_runtime(Some(steam), Some("{\"VALVE_runtime_is_steamvr\":true}")).starts_with("SteamVR"));
        let m = Path::new("/h/.local/lib/omarchyvr/monado/openxr_monado-dev.json");
        assert_eq!(outside_runtime(Some(m), Some("{}")), "Monado");
        assert!(outside_runtime(None, None).starts_with("none"));
    }

    #[test]
    fn addresses_are_checked_before_a_browser_sees_them() {
        assert_eq!(normalize_url("oceano-piratas.vercel.app").unwrap(), "https://oceano-piratas.vercel.app");
        assert_eq!(normalize_url(" https://a.b/c?d=1 ").unwrap(), "https://a.b/c?d=1");
        assert_eq!(normalize_url("http://localhost:8080").unwrap(), "http://localhost:8080");
        assert_eq!(normalize_url("localhost:5173").unwrap(), "https://localhost:5173");
        for bad in ["", "javascript:alert(1)", "file:///etc/passwd", "ftp://x.y", "two words.com", "nodot", "data:text/html,x"] {
            assert!(normalize_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_page_opens_in_vr_only_during_work() {
        assert_eq!(open_plan(Some(false), "oceano-piratas.vercel.app").unwrap(), vec!["omarchy-launch-browser".to_string(), "https://oceano-piratas.vercel.app".into()]);
        assert!(open_plan(None, "a.com").unwrap_err().contains("Press Work"));
        assert!(open_plan(Some(true), "a.com").unwrap_err().contains("Play"));
        assert!(open_plan(Some(false), "javascript:x").is_err());
    }

    #[test]
    fn the_report_reads_the_files_a_probe_finds() {
        let h = tmp("report");
        let now = SystemTime::now();
        let cache = dawn_cache(&h, Kind::Chrome).unwrap();
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::create_dir_all(h.join(".config")).unwrap();
        std::fs::write(flags_conf_path(&h, Kind::Chrome).unwrap(), "--ozone-platform=wayland\n").unwrap();
        let p = Probe {
            desktop_id: "google-chrome.desktop".into(),
            desktop_text: Some(CHROME.into()),
            version_out: Some("Google Chrome 155.0.8059.39".into()),
            flags_conf: std::fs::read_to_string(flags_conf_path(&h, Kind::Chrome).unwrap()).ok(),
            wayland_session: false,
            nvidia: Some("610.57.04".into()),
            dawn_modified: std::fs::metadata(&cache).unwrap().modified().ok(),
            runtime_target: None,
            runtime_text: None,
        };
        let r = report(&p, now);
        assert_eq!(r.browser, "Google Chrome 155");
        assert!(matches!(r.webgpu, Verdict::Yes(_)), "the flags file says Wayland: {:?}", r.webgpu);
        assert!(r.webgpu_used.as_deref().is_some_and(|u| u.contains("ago")));
        assert!(matches!(r.webxr, Verdict::No(_)));
        let old = report(&Probe { dawn_modified: Some(now - USED_RECENTLY - Duration::from_secs(60)), ..p }, now);
        assert_eq!(old.webgpu_used, None);
        let _ = std::fs::remove_dir_all(&h);
    }

    #[test]
    fn the_demo_report_is_this_pc() {
        let r = demo_report();
        assert_eq!(r.browser, "Google Chrome 155");
        assert!(matches!(r.webgpu, Verdict::Yes(_)));
        assert_eq!(r.webgpu_used.as_deref(), Some("a WebGPU page ran in it 3 hours ago"));
        assert!(r.runtime.starts_with("SteamVR"));
    }

    /// Read-only look at this PC (`cargo test -p omarchyvr-gui-core probe_this_pc -- --ignored --nocapture`): runs
    /// `xdg-settings` and `<browser> --version`, reads files under HOME, writes nothing.
    #[test]
    #[ignore]
    fn probe_this_pc() {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        let p = probe_machine(&home);
        println!("{p:#?}\n{:#?}", report(&p, SystemTime::now()));
    }
}
