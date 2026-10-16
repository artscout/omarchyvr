//! Desktop actions for voice (`@desk ...` in the catalogue): typing, keys, the browser (search, open a site, find a
//! tab) and dictation.
//!
//! Safety:
//! * typing and keys go only to the focused window (`wtype` = the Wayland virtual keyboard: the focused surface);
//! * into a terminal (foot, kitty, alacritty, ...) text is typed only after a spoken "yes" (the caller asks: see
//!   [`needs_confirm`]), and Enter is never pressed there (nor Ctrl+C / Ctrl+Z: copy uses Ctrl+Shift+C);
//! * every typed string is logged to `~/.local/state/pimax/logs/typed.log` (and stderr).

use crate::act::Runner;
use crate::windows::{self, Client};
use std::time::Duration;

/// Window classes that run a shell.
pub const TERMINALS: &[&str] = &["foot", "kitty", "alacritty", "ghostty", "xterm", "wezterm", "konsole", "terminator", "st-256color", "urxvt"];

/// Most characters typed by one command.
pub const TYPE_MAX: usize = 200;
/// Most tabs looked at when searching a tab by cycling.
pub const TAB_SCAN_MAX: usize = 25;

pub fn is_terminal(class: &str) -> bool {
    let c = class.to_lowercase();
    TERMINALS.iter().any(|t| c == *t || c.ends_with(&format!(".{t}")) || c.contains(t))
}

/// Known sites by spoken name.
pub const SITES: &[(&str, &str)] = &[
    ("youtube", "https://www.youtube.com"),
    ("github", "https://github.com"),
    ("gmail", "https://mail.google.com"),
    ("mail", "https://mail.google.com"),
    ("google", "https://www.google.com"),
    ("maps", "https://maps.google.com"),
    ("google maps", "https://maps.google.com"),
    ("calendar", "https://calendar.google.com"),
    ("drive", "https://drive.google.com"),
    ("reddit", "https://www.reddit.com"),
    ("wikipedia", "https://en.wikipedia.org"),
    ("netflix", "https://www.netflix.com"),
    ("twitter", "https://x.com"),
    ("x", "https://x.com"),
    ("chatgpt", "https://chatgpt.com"),
    ("claude", "https://claude.ai"),
    ("amazon", "https://www.amazon.com"),
    ("spotify web", "https://open.spotify.com"),
    ("hacker news", "https://news.ycombinator.com"),
    ("whatsapp", "https://web.whatsapp.com"),
    ("twitch", "https://www.twitch.tv"),
];

/// Percent-encoding for a query string.
pub fn urlencode(s: &str) -> String {
    let mut o = String::new();
    for b in s.trim().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => o.push(b as char),
            b' ' => o.push('+'),
            _ => o += &format!("%{b:02X}"),
        }
    }
    o
}

/// A spoken site ("youtube", "github.com", "the youtube") -> (a name to look for in titles, URL). Unknown names
/// without a dot are None.
pub fn site_url(spoken: &str) -> Option<(String, String)> {
    let s = spoken.trim().trim_start_matches("the ").trim_end_matches(" website").trim_end_matches(" site").trim().to_lowercase();
    let s = s.replace(" dot com", ".com").replace(" dot org", ".org").replace(" dot net", ".net").replace(" dot io", ".io");
    if s.is_empty() {
        return None;
    }
    if let Some((n, u)) = SITES.iter().find(|(n, _)| *n == s || s == format!("{n}.com")) {
        return Some((n.to_string(), u.to_string()));
    }
    let host: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if host.contains('.') && host.chars().all(|c| c.is_ascii_alphanumeric() || "-./_".contains(c)) {
        let name = host.split('.').next().unwrap_or("").to_string();
        let url = if host.starts_with("http") { host.clone() } else { format!("https://{host}") };
        return Some((name, url));
    }
    None
}

fn log_typed(r: &dyn Runner, what: &str, class: &str) {
    if !r.live() {
        return; // a dry run, the audit, a test: nothing was typed
    }
    eprintln!("voice: typed into {class}: {what:?}");
    let state = std::env::var("XDG_STATE_HOME").ok().filter(|s| s.starts_with('/')).map(std::path::PathBuf::from).unwrap_or_else(|| crate::catalogue::expand("~/.local/state"));
    let p = state.join("pimax/logs/typed.log");
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let _ = writeln!(f, "{t} {class} {what:?}");
    }
}

fn focused(r: &mut dyn Runner) -> Option<Client> {
    windows::hyprctl_json(r, "activewindow").ok().and_then(|j| windows::parse_active(&j).ok().flatten())
}

fn clients(r: &mut dyn Runner) -> Vec<Client> {
    let mut c = windows::hyprctl_json(r, "clients").and_then(|j| windows::parse_clients(&j)).unwrap_or_default();
    c.sort_by_key(|c| c.focus);
    c
}

/// A typing command that needs a spoken "yes" first: the focused window is a terminal. Returns the question.
pub fn needs_confirm(args: &[String], r: &mut dyn Runner) -> Option<String> {
    match args.first().map(String::as_str) {
        Some("type") => {
            let f = focused(r)?;
            is_terminal(&f.class).then(|| format!("type \"{}\" into the terminal", args[1..].join(" ")))
        }
        Some("dictate") => {
            let f = focused(r)?;
            is_terminal(&f.class).then(|| "dictate into the terminal".to_string())
        }
        Some("key") if args.get(1).is_some_and(|k| matches!(k.as_str(), "enter" | "return")) => {
            let f = focused(r)?;
            is_terminal(&f.class).then(|| "press Enter in the terminal".to_string())
        }
        _ => None,
    }
}

/// `wtype` the text into the focused window (no Enter). `confirmed`: a terminal is allowed (after "yes").
pub fn type_text(text: &str, confirmed: bool, r: &mut dyn Runner) -> Result<String, String> {
    // hesitations are never typed ("uh something in the", user decision 2026-10-16)
    let space = if text.ends_with(' ') { " " } else { "" };
    let text = crate::command::strip_fillers(text) + space;
    let text: String = text.chars().filter(|c| !c.is_control()).take(TYPE_MAX).collect();
    if text.trim().is_empty() {
        return Err("Nothing to type".into());
    }
    let f = focused(r).ok_or("No window is focused")?;
    if is_terminal(&f.class) && !confirmed {
        return Err("Not typing into a terminal without a yes".into());
    }
    // -s: a short pause first (the virtual keyboard's keymap reaches the window before the first key; without it
    // the first character was lost), -d: a few ms between keys (browsers drop keys typed too fast)
    r.run(&["wtype".into(), "-s".into(), "80".into(), "-d".into(), "4".into(), "--".into(), text.clone()], true).map_err(|e| format!("wtype: {e}"))?;
    log_typed(r, &text, &f.class);
    Ok(format!("Typed \"{}\"", short(&text, 40)))
}

fn short(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        s.chars().take(n).collect::<String>() + "..."
    } else {
        s.to_string()
    }
}

/// One named key or shortcut into the focused window. Terminals: Enter only when `allow_terminal` (no confirmation
/// policy for it, or after the yes); copy/paste use Ctrl+Shift there; no Ctrl+Z / Ctrl+A.
pub fn press(name: &str, allow_terminal: bool, r: &mut dyn Runner) -> Result<String, String> {
    let f = focused(r).ok_or("No window is focused")?;
    let term = is_terminal(&f.class);
    let n = name.trim().to_lowercase();
    // (wtype arguments, what to say)
    let (args, say): (Vec<&str>, &str) = match n.as_str() {
        "enter" | "return" => {
            if term && !allow_terminal {
                return Err("Not pressing Enter in a terminal without a yes".into());
            }
            (vec!["-k", "Return"], "Enter")
        }
        "escape" | "esc" => (vec!["-k", "Escape"], "Escape"),
        "tab" => (vec!["-k", "Tab"], "Tab"),
        "backspace" | "delete" => (vec!["-k", "BackSpace"], "Backspace"),
        "space" => (vec!["-k", "space"], "Space"),
        "select all" => {
            if term {
                return Err("Not in a terminal".into());
            }
            (vec!["-M", "ctrl", "-k", "a", "-m", "ctrl"], "Select all")
        }
        "copy" => {
            if term {
                (vec!["-M", "ctrl", "-M", "shift", "-k", "c", "-m", "shift", "-m", "ctrl"], "Copy")
            } else {
                (vec!["-M", "ctrl", "-k", "c", "-m", "ctrl"], "Copy")
            }
        }
        "paste" => {
            if term {
                (vec!["-M", "ctrl", "-M", "shift", "-k", "v", "-m", "shift", "-m", "ctrl"], "Paste")
            } else {
                (vec!["-M", "ctrl", "-k", "v", "-m", "ctrl"], "Paste")
            }
        }
        "undo" => {
            if term {
                return Err("No undo in a terminal".into());
            }
            (vec!["-M", "ctrl", "-k", "z", "-m", "ctrl"], "Undo")
        }
        "redo" => {
            if term {
                return Err("No redo in a terminal".into());
            }
            (vec!["-M", "ctrl", "-M", "shift", "-k", "z", "-m", "shift", "-m", "ctrl"], "Redo")
        }
        "page down" | "scroll down" => (vec!["-k", "Next"], "Page down"),
        "page up" | "scroll up" => (vec!["-k", "Prior"], "Page up"),
        "top" | "home" => (vec!["-M", "ctrl", "-k", "Home", "-m", "ctrl"], "Top"),
        "bottom" | "end" => (vec!["-M", "ctrl", "-k", "End", "-m", "ctrl"], "Bottom"),
        "reload" | "refresh" => {
            if !windows::is_browser(&f.class) {
                return Err("Reload: only in a browser".into());
            }
            (vec!["-k", "F5"], "Reload")
        }
        "back" => {
            if !windows::is_browser(&f.class) {
                return Err("Back: only in a browser".into());
            }
            (vec!["-M", "alt", "-k", "Left", "-m", "alt"], "Back")
        }
        "forward" => {
            if !windows::is_browser(&f.class) {
                return Err("Forward: only in a browser".into());
            }
            (vec!["-M", "alt", "-k", "Right", "-m", "alt"], "Forward")
        }
        other => return Err(format!("No key \"{other}\"")),
    };
    let mut argv = vec!["wtype".to_string()];
    argv.extend(args.iter().map(|s| s.to_string()));
    r.run(&argv, true).map_err(|e| format!("wtype: {e}"))?;
    log_typed(r, &format!("<{say}>"), &f.class);
    Ok(say.to_string())
}

/// Focus the most recent browser window; None when there is none.
pub fn focus_browser(r: &mut dyn Runner) -> Option<Client> {
    let b = clients(r).into_iter().find(|c| windows::is_browser(&c.class) && !c.class.contains("--app") && !c.class.starts_with("chrome-"))?;
    windows::focus_address(r, &b.address).ok()?;
    Some(b)
}

/// Open a URL in the browser (a new tab in the running one, or a new browser), its window focused.
pub fn open_url(url: &str, r: &mut dyn Runner) -> Result<(), String> {
    let had = focus_browser(r).is_some();
    r.run(&["omarchy-launch-browser".into(), url.into()], false)?;
    if had {
        // the URL opens as a tab of the focused browser window; keep it in front
        std::thread::sleep(Duration::from_millis(150));
        let _ = focus_browser(r);
    }
    Ok(())
}

/// "launch or focus" for the browser.
pub fn browser(r: &mut dyn Runner) -> Result<String, String> {
    if let Some(b) = focus_browser(r) {
        return Ok(format!("Browser (workspace {})", b.workspace));
    }
    r.run(&["omarchy-launch-browser".into()], false)?;
    Ok("Browser".into())
}

pub fn search(q: &str, r: &mut dyn Runner) -> Result<String, String> {
    let q: String = q.chars().filter(|c| !c.is_control()).take(TYPE_MAX).collect();
    if q.trim().is_empty() {
        return Err("Search for what?".into());
    }
    open_url(&format!("https://www.google.com/search?q={}", urlencode(&q)), r)?;
    log_typed(r, &format!("search: {q}"), "browser");
    Ok(format!("Searching \"{}\"", short(&q, 40)))
}

pub fn title_has(title: &str, q: &str) -> bool {
    let t = title.to_lowercase();
    let words: Vec<String> = crate::command::words(q).into_iter().filter(|w| w.len() >= 2 && !crate::command::FILLERS.contains(&w.as_str()) && w != "tab").collect();
    !words.is_empty() && words.iter().all(|w| t.contains(w.as_str()))
}

/// A browser tab by title: a window showing it, else cycle the tabs of each browser window (bounded, back to the
/// first tab when not found). `Ok(false)` = not found.
pub fn find_tab(q: &str, cycle: bool, r: &mut dyn Runner) -> Result<bool, String> {
    let cs = clients(r);
    if let Some(c) = cs.iter().find(|c| title_has(&c.title, q)) {
        windows::focus_address(r, &c.address)?;
        return Ok(true);
    }
    for b in cs.iter().filter(|c| cycle && windows::is_browser(&c.class)) {
        windows::focus_address(r, &b.address)?;
        let start = b.title.clone();
        for _ in 0..TAB_SCAN_MAX {
            r.run(&["wtype".into(), "-M".into(), "ctrl".into(), "-k".into(), "Tab".into(), "-m".into(), "ctrl".into()], true)?;
            std::thread::sleep(Duration::from_millis(90));
            let f = focused(r);
            if f.as_ref().is_none_or(|f| f.address != b.address) {
                break; // the browser did not keep the focus: stop pressing keys elsewhere
            }
            let t = f.map(|f| f.title).unwrap_or_default();
            if title_has(&t, q) {
                return Ok(true);
            }
            if t == start {
                break; // went all the way round
            }
        }
    }
    Ok(false)
}

/// "go to youtube": a tab/window with it, else open the site. `Ok(None)` = not a site and no tab. `cycle`: also
/// look through the hidden tabs of each browser window (Ctrl+Tab, bounded).
pub fn tab_or_site(q: &str, cycle: bool, r: &mut dyn Runner) -> Result<Option<String>, String> {
    if find_tab(q, cycle, r)? {
        return Ok(Some(format!("Tab \"{}\"", short(q, 30))));
    }
    if let Some((_, url)) = site_url(q) {
        open_url(&url, r)?;
        return Ok(Some(format!("Opening {url}")));
    }
    Ok(None)
}

/// Video files "play X" may open.
const VIDEO_EXT: &[&str] = &["mkv", "mp4", "avi", "webm", "mov", "m4v", "wmv", "mpg", "mpeg", "ts", "flv"];

/// The folders searched for videos: ~/Downloads, ~/Videos and the XDG videos folder (never the whole home).
pub fn video_dirs() -> Vec<std::path::PathBuf> {
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let mut v = vec![home.join("Downloads"), home.join("Videos")];
    if let Ok(t) = std::fs::read_to_string(home.join(".config/user-dirs.dirs")) {
        for l in t.lines() {
            if let Some(x) = l.strip_prefix("XDG_VIDEOS_DIR=") {
                let p = x.trim_matches('"').replace("$HOME", &home.to_string_lossy());
                v.push(std::path::PathBuf::from(p));
            }
        }
    }
    v.sort();
    v.dedup();
    v
}

/// The video whose file name has every word of `q` (case-insensitive), in `dirs` up to 3 folders deep (at most 5,000
/// entries): the biggest, then the newest. (path, how many matched).
pub fn find_video(q: &str, dirs: &[std::path::PathBuf]) -> Option<(std::path::PathBuf, usize)> {
    let words: Vec<String> = crate::command::words(q)
        .into_iter()
        .filter(|w| w.len() >= 2 && !crate::command::FILLERS.contains(&w.as_str()) && !matches!(w.as_str(), "movie" | "film" | "video" | "file" | "it" | "already" | "downloaded" | "pelicula"))
        .collect();
    if words.is_empty() {
        return None;
    }
    let mut hits: Vec<(u64, std::time::SystemTime, std::path::PathBuf)> = Vec::new();
    let mut seen = 0usize;
    let mut stack: Vec<(std::path::PathBuf, u32)> = dirs.iter().map(|d| (d.clone(), 0)).collect();
    while let Some((d, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            seen += 1;
            if seen > 5000 {
                break;
            }
            let p = e.path();
            let Ok(md) = e.metadata() else { continue };
            if md.is_dir() {
                if depth < 3 && !e.file_name().to_string_lossy().starts_with('.') {
                    stack.push((p, depth + 1));
                }
                continue;
            }
            let ext = p.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
            if !VIDEO_EXT.contains(&ext.as_str()) {
                continue;
            }
            let name = crate::command::words(&p.file_stem().map(|s| s.to_string_lossy().replace(['.', '_'], " ")).unwrap_or_default()).join(" ");
            if words.iter().all(|w| name.contains(w.as_str())) {
                hits.push((md.len(), md.modified().unwrap_or(std::time::UNIX_EPOCH), p));
            }
        }
    }
    let n = hits.len();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    hits.into_iter().next().map(|h| (h.2, n))
}

/// "Play Dune": the video, fullscreen in mpv (else the desktop's default player).
pub fn play(q: &str, r: &mut dyn Runner) -> Result<String, String> {
    let Some((path, n)) = find_video(q, &video_dirs()) else {
        return Err(format!("No video called \"{}\" in Downloads or Videos", short(q.trim(), 30)));
    };
    let name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let p = path.to_string_lossy().into_owned();
    // the player opens on the main screen: cinema mode shows that one, big, in front
    if let Some(m) = crate::windows::vr_screens(r).into_iter().find(|s| s.slot == "main" && !s.focused) {
        if !m.output.is_empty() && m.output.chars().all(|c| c.is_alphanumeric() || "-_.:".contains(c)) {
            let _ = crate::windows::dispatch(r, &format!("hl.dsp.focus({{ monitor = \"{}\" }})", m.output));
        }
    }
    let mpv = std::path::Path::new("/usr/bin/mpv").exists();
    let argv: Vec<String> = if mpv { vec!["mpv".into(), "--fs".into(), "--force-window=immediate".into(), "--".into(), p] } else { vec!["xdg-open".into(), p] };
    r.run(&argv, false)?;
    let more = if n > 1 { format!(" (the biggest of {n})") } else { String::new() };
    Ok(format!("Playing {name}{more}"))
}

/// Run an `@desk` action. `confirmed` = a terminal may be typed into (after the yes, or no policy for it).
pub fn execute(args: &[String], confirmed: bool, r: &mut dyn Runner) {
    let rest = args[1.min(args.len())..].join(" ");
    let res = match args.first().map(String::as_str) {
        Some("type") => type_text(&rest, confirmed, r),
        Some("key") => press(&rest, confirmed, r),
        Some("browser") => browser(r),
        Some("play") => play(&rest, r),
        Some("search") => search(&rest, r),
        Some("open") => match site_url(&rest) {
            Some((name, url)) => {
                // already open in a window: just show it
                if find_tab(&name, false, r).unwrap_or(false) {
                    Ok(format!("Tab \"{name}\""))
                } else {
                    open_url(&url, r).map(|_| format!("Opening {url}"))
                }
            }
            None => search(&rest, r),
        },
        Some("tab") => match tab_or_site(&rest, true, r) {
            Ok(Some(s)) => Ok(s),
            Ok(None) => Err(format!("No tab with \"{rest}\"")),
            Err(e) => Err(e),
        },
        other => Err(format!("Unknown desk action {other:?}")),
    };
    match res {
        Ok(s) => r.toast(&s),
        Err(e) => r.toast(&e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_video_by_name_in_the_video_folders_only() {
        let d = std::env::temp_dir().join(format!("pimax-video-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("Downloads/films")).unwrap();
        std::fs::write(d.join("Downloads/Dune.Part.Two.2024.sample.mkv"), vec![0u8; 10]).unwrap();
        std::fs::write(d.join("Downloads/films/Dune.2021.1080p.mkv"), vec![0u8; 100]).unwrap();
        std::fs::write(d.join("Downloads/dune-notes.txt"), "x").unwrap();
        let (p, n) = find_video("Dune", &[d.join("Downloads")]).unwrap();
        assert_eq!((p.file_name().unwrap().to_str().unwrap(), n), ("Dune.2021.1080p.mkv", 2), "the biggest of the matches");
        assert!(find_video("the movie dune part two", &[d.join("Downloads")]).unwrap().0.ends_with("Dune.Part.Two.2024.sample.mkv"));
        assert!(find_video("arrival", &[d.join("Downloads")]).is_none());
        assert!(find_video("the movie", &[d.join("Downloads")]).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn sites_and_urls() {
        assert_eq!(site_url("YouTube").unwrap().1, "https://www.youtube.com");
        assert_eq!(site_url("github dot com").unwrap().1, "https://github.com");
        assert_eq!(site_url("news.ycombinator.com").unwrap(), ("news".into(), "https://news.ycombinator.com".into()));
        assert!(site_url("my friend").is_none());
        assert!(site_url("rm -rf;x.com").is_none());
        assert_eq!(urlencode("how to get friends in Bali?"), "how+to+get+friends+in+Bali%3F");
        assert!(is_terminal("foot") && is_terminal("com.mitchellh.ghostty") && !is_terminal("google-chrome"));
        assert!(title_has("Funny cats - YouTube - Google Chrome", "the youtube tab"));
        assert!(!title_has("GitHub", "youtube"));
    }
}
