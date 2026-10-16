//! Per-game OpenVR runtime and xrizer crash handling (docs/GAME-MODE.md, "Games that crash with xrizer").
//!
//! xrizer (OpenVR on top of OpenXR/Monado) aborts a game when the game calls an OpenVR function xrizer does not
//! have yet (Half-Life: Alyx: `IVRCompositor::GetPosesForFrame` is `todo!()`; xrizer's panic hook aborts). We do not
//! patch xrizer. From our side:
//!
//! * **Evidence**: xrizer truncates `~/.local/state/xrizer/xrizer.txt` at every game start, so the panic text is
//!   lost. [`archive_log`] copies it to `~/.local/state/omarchyvr/xrizer-logs/<unix>-<tag>.txt` (the newest 20 are kept)
//!   when a game ends and before every `vr start`.
//! * **Detection**: after a game ends, [`judge`] reads the archived xrizer log (panic text) and `coredumpctl info`
//!   text ([`parse_coredump_info`]: signal 6 and xrizer / `vrclient.so` frames). Only xrizer evidence counts: a game
//!   that aborts on its own is not blamed on xrizer.
//! * **Per-game runtime**: `~/.config/omarchyvr/game-runtime.json` holds the games that run with SteamVR (a built-in
//!   seed plus the games found crashing, plus the user's own choices). `omarchyvr vr start --game APPID` starts that
//!   session with SteamVR when the game is listed ([`resolve`]); every other start is unchanged. Monado/xrizer and
//!   SteamVR cannot share the headset, so the choice is per session, not per launch: the next Monado start puts
//!   xrizer back in `openvrpaths.vrpath` by itself (`ensure_xrizer`).
//!
//! CLI: `omarchyvr vr game-runtime [list|set APPID steamvr|xrizer|default [NAME]|next APPID|none|check APPID SINCE_UNIX|archive]`.
use super::steamvr::Games;
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

/// Half-Life: Alyx (Source 2): its `GetPosesForFrame` call is `todo!()` in xrizer.
pub const ALYX: &str = "546560";
/// Kept archived xrizer logs.
pub const KEEP_LOGS: usize = 20;

/// Where an entry came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Built in (known to crash with xrizer).
    Seed,
    /// Found crashing with xrizer here.
    Crash,
    /// Set by the user.
    User,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Source::Seed => "seed",
            Source::Crash => "crash",
            Source::User => "user",
        }
    }
    fn parse(s: &str) -> Source {
        match s {
            "crash" => Source::Crash,
            "user" => Source::User,
            _ => Source::Seed,
        }
    }
}

/// One game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub appid: String,
    pub name: String,
    pub runtime: Games,
    /// Plain words: why.
    pub why: String,
    pub source: Source,
}

/// The built-in list: games sure to call what xrizer does not have.
pub fn seed() -> Vec<Entry> {
    vec![Entry {
        appid: ALYX.into(),
        name: "Half-Life: Alyx".into(),
        runtime: Games::SteamVr,
        why: "xrizer aborts it: IVRCompositor::GetPosesForFrame is not implemented (todo!) in xrizer".into(),
        source: Source::Seed,
    }]
}

/// The saved list (`game-runtime.json`): the user's and found entries over the seed, and the game Play starts for next.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Store {
    pub saved: BTreeMap<String, Entry>,
    /// The game the next Play is for (`--game`), if any.
    pub next: Option<String>,
}

/// `~/.config/omarchyvr/game-runtime.json`.
pub fn store_path(home: &Path) -> PathBuf {
    home.join(".config/omarchyvr/game-runtime.json")
}

/// `~/.local/state/omarchyvr/xrizer-logs`.
pub fn archive_dir(home: &Path) -> PathBuf {
    home.join(".local/state/omarchyvr/xrizer-logs")
}

/// xrizer's own log (truncated at each game start).
pub fn xrizer_log(home: &Path) -> PathBuf {
    home.join(".local/state/xrizer/xrizer.txt")
}

fn valid_appid(s: &str) -> bool {
    !s.is_empty() && s.len() <= 12 && s.bytes().all(|b| b.is_ascii_digit())
}

impl Store {
    /// Parse the file text (bad input gives an empty store).
    pub fn parse(text: &str) -> Store {
        let mut st = Store::default();
        let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else { return st };
        if let Some(g) = v.get("games").and_then(|g| g.as_object()) {
            for (id, e) in g {
                if !valid_appid(id) {
                    continue;
                }
                let s = |k: &str| e.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
                let Some(runtime) = Games::parse(&s("runtime")) else { continue };
                st.saved.insert(id.clone(), Entry { appid: id.clone(), name: s("name"), runtime, why: s("why"), source: Source::parse(&s("source")) });
            }
        }
        st.next = v.get("next").and_then(|x| x.as_str()).filter(|s| valid_appid(s)).map(String::from);
        st
    }

    pub fn to_text(&self) -> String {
        let mut games = serde_json::Map::new();
        for (id, e) in &self.saved {
            games.insert(
                id.clone(),
                serde_json::json!({ "runtime": e.runtime.name(), "name": e.name, "why": e.why, "source": e.source.name() }),
            );
        }
        let mut root = serde_json::Map::new();
        root.insert("games".into(), games.into());
        if let Some(n) = &self.next {
            root.insert("next".into(), n.clone().into());
        }
        format!("{}\n", serde_json::to_string_pretty(&serde_json::Value::Object(root)).unwrap())
    }

    pub fn load(home: &Path) -> Store {
        std::fs::read_to_string(store_path(home)).map(|t| Store::parse(&t)).unwrap_or_default()
    }

    pub fn save(&self, home: &Path) -> io::Result<()> {
        let p = store_path(home);
        std::fs::create_dir_all(p.parent().unwrap())?;
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, self.to_text())?;
        std::fs::rename(tmp, p)
    }

    /// Every game with a known runtime: the seed, overridden by the saved entries. Sorted by name.
    pub fn list(&self) -> Vec<Entry> {
        let mut m: BTreeMap<String, Entry> = seed().into_iter().map(|e| (e.appid.clone(), e)).collect();
        for (id, e) in &self.saved {
            m.insert(id.clone(), e.clone());
        }
        let mut v: Vec<Entry> = m.into_values().collect();
        v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.appid.cmp(&b.appid)));
        v
    }

    /// The runtime a listed game wants (None = not listed: the global choice counts).
    pub fn runtime_of(&self, appid: &str) -> Option<Games> {
        self.list().into_iter().find(|e| e.appid == appid).map(|e| e.runtime)
    }

    /// Set a game's runtime (a user or crash entry replaces the seed one).
    pub fn set(&mut self, appid: &str, name: &str, runtime: Games, why: &str, source: Source) {
        let name = if name.is_empty() { self.list().into_iter().find(|e| e.appid == appid).map(|e| e.name).unwrap_or_default() } else { name.to_string() };
        self.saved.insert(appid.to_string(), Entry { appid: appid.into(), name, runtime, why: why.into(), source });
    }

    /// Back to the default (drops the saved entry; a seed entry applies again).
    pub fn forget(&mut self, appid: &str) {
        self.saved.remove(appid);
        if self.next.as_deref() == Some(appid) {
            self.next = None;
        }
    }
}

/// The runtime of a start for `--game APPID`: a listed game's own, else the global `Games run with` choice.
/// A listed game that wants xrizer (the user's override of the seed) stays on Monado even when the global choice is SteamVR.
pub fn resolve(store: &Store, appid: Option<&str>, global: Games) -> Games {
    appid.and_then(|a| store.runtime_of(a)).unwrap_or(global)
}

/// The game's name from Steam's `appmanifest_<id>.acf` (`"name"  "..."`), looked up in the usual library folder.
pub fn steam_app_name(home: &Path, appid: &str) -> Option<String> {
    let p = home.join(".local/share/Steam/steamapps").join(format!("appmanifest_{appid}.acf"));
    acf_name(&std::fs::read_to_string(p).ok()?)
}

/// `"name"  "Half-Life: Alyx"` of an appmanifest text.
pub fn acf_name(text: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let mut q = l.split('"').filter(|s| !s.trim().is_empty());
        (q.next()? == "name").then(|| q.next().map(str::to_string)).flatten()
    })
}

// ---- crash evidence -------------------------------------------------------------------------------------------

/// The panic message of an xrizer log: "panicked at src/compositor.rs:313:9: not yet implemented".
pub fn xrizer_panic(log: &str) -> Option<String> {
    let lines: Vec<&str> = log.lines().collect();
    for (i, l) in lines.iter().enumerate() {
        if let Some(pos) = l.find("panicked at ") {
            let at = l[pos + "panicked at ".len()..].trim().trim_end_matches(':');
            // Rust >= 1.73 puts the message on the next line; older ones after the location
            let (loc, inline) = match at.split_once(": ") {
                Some((a, b)) => (a, Some(b.to_string())),
                None => (at, None),
            };
            let msg = inline.or_else(|| lines.get(i + 1).map(|m| m.trim().to_string())).filter(|m| !m.is_empty());
            return Some(match msg {
                Some(m) => format!("panicked at {loc}: {m}"),
                None => format!("panicked at {loc}"),
            });
        }
    }
    lines.iter().find(|l| l.contains("not yet implemented")).map(|l| l.trim().to_string())
}

/// One core dump of `coredumpctl info`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Core {
    /// Process id.
    pub pid: String,
    /// Signal number (6 = SIGABRT, 11 = SIGSEGV).
    pub signal: u32,
    /// `Executable:`.
    pub exe: String,
    /// A stack frame or module line names xrizer or its `vrclient.so`.
    pub xrizer_frames: bool,
}

/// Parse `coredumpctl info` text (one block per dump, each starting at a `PID:` line).
pub fn parse_coredump_info(text: &str) -> Vec<Core> {
    let mut out: Vec<Core> = Vec::new();
    for l in text.lines() {
        let t = l.trim();
        if let Some(v) = t.strip_prefix("PID:") {
            out.push(Core { pid: v.split_whitespace().next().unwrap_or("").to_string(), ..Default::default() });
            continue;
        }
        let Some(c) = out.last_mut() else { continue };
        if let Some(v) = t.strip_prefix("Signal:") {
            c.signal = v.split_whitespace().next().and_then(|n| n.parse().ok()).unwrap_or(0);
        } else if let Some(v) = t.strip_prefix("Executable:") {
            c.exe = v.trim().to_string();
        } else if t.contains("xrizer") || t.contains("vrclient.so") {
            c.xrizer_frames = true;
        }
    }
    out
}

/// What went wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crash {
    /// Plain words for the log and the app.
    pub why: String,
}

/// Was this game's end an xrizer abort? Evidence: a panic in xrizer's log, or a SIGABRT core dump with xrizer frames.
/// An abort without any xrizer trace is not blamed on xrizer.
pub fn judge(xrizer_log_text: Option<&str>, cores: &[Core]) -> Option<Crash> {
    let panic = xrizer_log_text.and_then(xrizer_panic);
    let core = cores.iter().find(|c| c.signal == 6 && c.xrizer_frames);
    match (panic, core) {
        (Some(p), _) => Some(Crash { why: format!("xrizer {p}") }),
        (None, Some(c)) => Some(Crash { why: format!("the game aborted (SIGABRT, pid {}) inside xrizer", c.pid) }),
        (None, None) => None,
    }
}

// ---- log archive -----------------------------------------------------------------------------------------------

/// Copy xrizer's log to the archive (`<now>-<tag>.txt`) unless it is empty or the same as the newest archived copy;
/// keep the newest [`KEEP_LOGS`]. Returns the new file.
pub fn archive_log(home: &Path, tag: &str, now: u64) -> io::Result<Option<PathBuf>> {
    let src = xrizer_log(home);
    let text = match std::fs::read(&src) {
        Ok(t) if !t.is_empty() => t,
        _ => return Ok(None),
    };
    let dir = archive_dir(home);
    std::fs::create_dir_all(&dir)?;
    let mut files = archived(&dir);
    if let Some(last) = files.last() {
        if std::fs::read(last).ok().as_deref() == Some(&text) {
            return Ok(None);
        }
    }
    let tag: String = tag.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(24).collect();
    let dst = dir.join(format!("{now}-{tag}.txt"));
    std::fs::write(&dst, &text)?;
    files.push(dst.clone());
    while files.len() > KEEP_LOGS {
        let _ = std::fs::remove_file(files.remove(0));
    }
    Ok(Some(dst))
}

/// The archived logs, oldest first (names start with the unix time).
pub fn archived(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<(u64, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            let t = n.split('-').next()?.parse::<u64>().ok()?;
            n.ends_with(".txt").then(|| (t, e.path()))
        })
        .collect();
    v.sort();
    v.into_iter().map(|x| x.1).collect()
}

// ---- after a game ended --------------------------------------------------------------------------------------------

/// `coredumpctl info` for the dumps since `since_unix` (empty text when none or when it cannot run; coreutils
/// `timeout` ends a stuck one after 15 s).
pub fn coredump_text(since_unix: u64) -> String {
    use std::process::{Command, Stdio};
    Command::new("timeout")
        .args(["15", "coredumpctl", "info", "--no-pager", &format!("--since=@{since_unix}")])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// The result of looking at a game that just ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub appid: String,
    pub name: String,
    pub crash: Option<Crash>,
    /// The archived xrizer log, when one was saved.
    pub log: Option<PathBuf>,
}

/// Archive xrizer's log, look for the crash evidence, and when it is an xrizer abort save the game in the list
/// (runtime SteamVR, source crash). A game the user set to xrizer by hand is not changed.
pub fn after_game(home: &Path, appid: &str, since_unix: u64, now: u64, core_text: &str) -> Verdict {
    let log = archive_log(home, appid, now).ok().flatten();
    // judge the log of this run (the file as it is now, or, when nothing differed from an archive, the file itself)
    let text = std::fs::read_to_string(xrizer_log(home)).ok();
    let crash = judge(text.as_deref(), &parse_coredump_info(core_text));
    let mut store = Store::load(home);
    let name = steam_app_name(home, appid).or_else(|| store.list().into_iter().find(|e| e.appid == appid).map(|e| e.name)).unwrap_or_else(|| format!("game {appid}"));
    if let Some(c) = &crash {
        let user_pinned = store.saved.get(appid).is_some_and(|e| e.source == Source::User && e.runtime == Games::Monado);
        if !user_pinned {
            store.set(appid, &name, Games::SteamVr, &c.why, Source::Crash);
            store.next = Some(appid.to_string());
            let _ = store.save(home);
        }
    }
    let _ = since_unix;
    Verdict { appid: appid.into(), name, crash, log }
}

/// The plain-words message after a crash.
pub fn crash_message(v: &Verdict) -> Option<String> {
    let c = v.crash.as_ref()?;
    Some(format!(
        "{} closed because xrizer (the OpenVR layer for Monado) cannot run it ({}). It is saved as 'Runs with SteamVR': press Play again and the session starts with SteamVR for this game. Evidence kept{}.",
        v.name,
        c.why,
        v.log.as_ref().map(|p| format!(" in {}", p.display())).unwrap_or_default()
    ))
}

// ---- CLI ---------------------------------------------------------------------------------------------------------------

pub fn cli(what: &[String], home: &Path) -> Result<(), String> {
    let mut st = Store::load(home);
    let save = |st: &Store| st.save(home).map_err(|e| format!("cannot save {}: {e}", store_path(home).display()));
    match what.first().map(String::as_str).unwrap_or("list") {
        "list" => {
            for e in st.list() {
                println!("{}\t{}\t{}\t{}\t{}", e.appid, e.runtime.name(), e.source.name(), e.name, e.why);
            }
            println!("next game: {}", st.next.as_deref().unwrap_or("none"));
        }
        "set" => {
            let id = what.get(1).filter(|s| valid_appid(s)).ok_or("set APPID steamvr|xrizer|default [NAME]")?;
            match what.get(2).map(String::as_str) {
                Some("default") => st.forget(id),
                Some(r) => {
                    let g = Games::parse(r).ok_or("runtime: steamvr | xrizer | default")?;
                    let name = what.get(3..).map(|w| w.join(" ")).unwrap_or_default();
                    st.set(id, &name, g, "set by you", Source::User);
                }
                None => return Err("set APPID steamvr|xrizer|default [NAME]".into()),
            }
            save(&st)?;
            println!("saved");
        }
        "next" => {
            st.next = match what.get(1).map(String::as_str) {
                Some("none") | None => None,
                Some(id) if valid_appid(id) => Some(id.to_string()),
                _ => return Err("next APPID|none".into()),
            };
            save(&st)?;
            println!("next game: {}", st.next.as_deref().unwrap_or("none"));
        }
        "archive" => match archive_log(home, "manual", now_unix()).map_err(|e| e.to_string())? {
            Some(p) => println!("saved {}", p.display()),
            None => println!("nothing new to save"),
        },
        "check" => {
            let id = what.get(1).filter(|s| valid_appid(s)).ok_or("check APPID SINCE_UNIX")?;
            let since: u64 = what.get(2).and_then(|s| s.parse().ok()).ok_or("check APPID SINCE_UNIX")?;
            let v = after_game(home, id, since, now_unix(), &coredump_text(since));
            println!("{}", crash_message(&v).unwrap_or_else(|| format!("{}: no xrizer crash found", v.name)));
        }
        w => return Err(format!("unknown choice {w} (list|set|next|archive|check)")),
    }
    Ok(())
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PANIC_LOG: &str = "[INFO xrizer] init\n[INFO xrizer::compositor] WaitGetPoses\nthread 'main' panicked at src/compositor.rs:313:9:\nnot yet implemented\nstack backtrace:\n";
    const CORE_TEXT: &str = "           PID: 4242 (hlvr)\n           UID: 1000 (u)\n        Signal: 6 (ABRT)\n    Executable: /home/u/.steam/hlvr\n       Message: Process 4242 (hlvr) of user 1000 dumped core.\n\n                Stack trace of thread 4242:\n                #0  0x00007f raise (libc.so.6 + 0x3)\n                #5  0x00007f std::panicking::rust_panic (vrclient.so + 0x1)\n\n           PID: 99 (other)\n        Signal: 11 (SEGV)\n    Executable: /usr/bin/other\n";

    fn scratch(n: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("omarchyvr-gr-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn panic_text_is_found_in_old_and_new_rust_formats() {
        assert_eq!(xrizer_panic(PANIC_LOG).as_deref(), Some("panicked at src/compositor.rs:313:9: not yet implemented"));
        assert_eq!(xrizer_panic("thread 'main' panicked at src/x.rs:1:1: boom\n").as_deref(), Some("panicked at src/x.rs:1:1: boom"));
        assert_eq!(xrizer_panic("fine\nnot yet implemented\n").as_deref(), Some("not yet implemented"));
        assert_eq!(xrizer_panic("[INFO] all good\n"), None);
    }

    #[test]
    fn coredump_text_is_split_per_dump() {
        let c = parse_coredump_info(CORE_TEXT);
        assert_eq!(c.len(), 2);
        assert_eq!((c[0].pid.as_str(), c[0].signal, c[0].xrizer_frames), ("4242", 6, true));
        assert_eq!(c[0].exe, "/home/u/.steam/hlvr");
        assert_eq!((c[1].signal, c[1].xrizer_frames), (11, false));
        assert!(parse_coredump_info("-- No coredumps found --").is_empty());
    }

    #[test]
    fn judge_needs_xrizer_evidence() {
        assert!(judge(Some(PANIC_LOG), &[]).unwrap().why.contains("compositor.rs:313"));
        let c = parse_coredump_info(CORE_TEXT);
        assert!(judge(None, &c).unwrap().why.contains("SIGABRT"));
        // a segfault, or an abort without xrizer frames, or a clean log: not xrizer's doing
        assert_eq!(judge(Some("[INFO] ok\n"), &c[1..]), None);
        let plain = Core { signal: 6, ..Default::default() };
        assert_eq!(judge(None, &[plain]), None);
    }

    #[test]
    fn store_roundtrip_seed_and_overrides() {
        let mut s = Store::default();
        assert_eq!(s.runtime_of(ALYX), Some(Games::SteamVr), "seed");
        assert_eq!(s.runtime_of("12345"), None);
        s.set("12345", "Some Game", Games::SteamVr, "crash", Source::Crash);
        s.set(ALYX, "", Games::Monado, "set by you", Source::User);
        s.next = Some("12345".into());
        let t = s.to_text();
        let b = Store::parse(&t);
        assert_eq!(b, s);
        assert_eq!(b.runtime_of(ALYX), Some(Games::Monado), "the user overrides the seed");
        assert_eq!(b.list().iter().find(|e| e.appid == ALYX).unwrap().name, "Half-Life: Alyx", "the name of the seed is kept");
        let mut b = b;
        b.forget(ALYX);
        assert_eq!(b.runtime_of(ALYX), Some(Games::SteamVr), "default = the seed again");
        b.forget("12345");
        assert_eq!(b.next, None);
        assert_eq!(Store::parse("garbage"), Store::default());
        assert_eq!(Store::parse(r#"{"games":{"../x":{"runtime":"steamvr"},"7":{"runtime":"nope"}}}"#), Store::default());
    }

    #[test]
    fn resolve_follows_the_list_then_the_global_choice() {
        let mut s = Store::default();
        assert_eq!(resolve(&s, Some(ALYX), Games::Monado), Games::SteamVr);
        assert_eq!(resolve(&s, Some("1"), Games::Monado), Games::Monado);
        assert_eq!(resolve(&s, None, Games::SteamVr), Games::SteamVr);
        s.set(ALYX, "", Games::Monado, "x", Source::User);
        assert_eq!(resolve(&s, Some(ALYX), Games::SteamVr), Games::Monado);
    }

    #[test]
    fn store_file_save_and_load() {
        let h = scratch("store");
        assert_eq!(Store::load(&h), Store::default());
        let mut s = Store::default();
        s.set("5", "Five", Games::SteamVr, "w", Source::Crash);
        s.save(&h).unwrap();
        assert_eq!(Store::load(&h), s);
    }

    #[test]
    fn acf_name_is_read() {
        assert_eq!(acf_name("\"AppState\"\n{\n\t\"appid\"\t\t\"546560\"\n\t\"name\"\t\t\"Half-Life: Alyx\"\n}\n").as_deref(), Some("Half-Life: Alyx"));
        assert_eq!(acf_name("{}"), None);
    }

    #[test]
    fn archive_keeps_a_copy_skips_duplicates_and_rotates() {
        let h = scratch("archive");
        assert_eq!(archive_log(&h, "1", 10).unwrap(), None, "no log yet");
        let log = xrizer_log(&h);
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        std::fs::write(&log, "run 1\n").unwrap();
        let p = archive_log(&h, "546560", 100).unwrap().unwrap();
        assert!(p.ends_with("100-546560.txt"));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "run 1\n");
        assert_eq!(archive_log(&h, "546560", 101).unwrap(), None, "same text twice");
        for i in 0..(KEEP_LOGS + 3) {
            std::fs::write(&log, format!("run {}\n", i + 2)).unwrap();
            archive_log(&h, "g", 200 + i as u64).unwrap().unwrap();
        }
        let files = archived(&archive_dir(&h));
        assert_eq!(files.len(), KEEP_LOGS);
        assert!(!files.iter().any(|f| f.ends_with("100-546560.txt")), "the oldest went");
    }

    #[test]
    fn after_a_crash_the_game_is_saved_and_the_message_names_it() {
        let h = scratch("after");
        let log = xrizer_log(&h);
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        std::fs::write(&log, PANIC_LOG).unwrap();
        let acf = h.join(".local/share/Steam/steamapps");
        std::fs::create_dir_all(&acf).unwrap();
        std::fs::write(acf.join("appmanifest_777.acf"), "\"name\"\t\"Test VR\"\n").unwrap();
        let v = after_game(&h, "777", 0, 500, "");
        assert_eq!(v.name, "Test VR");
        assert!(v.log.is_some());
        let m = crash_message(&v).unwrap();
        assert!(m.contains("Test VR") && m.contains("SteamVR") && m.contains("compositor.rs:313"), "{m}");
        let st = Store::load(&h);
        assert_eq!(st.saved["777"].source, Source::Crash);
        assert_eq!(st.next.as_deref(), Some("777"));
        // a clean run changes nothing
        std::fs::write(&log, "[INFO] fine\n").unwrap();
        let v = after_game(&h, "888", 0, 600, "");
        assert!(v.crash.is_none() && crash_message(&v).is_none());
        assert!(!Store::load(&h).saved.contains_key("888"));
    }

    #[test]
    fn a_game_the_user_pinned_to_xrizer_is_not_moved() {
        let h = scratch("pinned");
        let mut s = Store::default();
        s.set("777", "Test VR", Games::Monado, "set by you", Source::User);
        s.save(&h).unwrap();
        let log = xrizer_log(&h);
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        std::fs::write(&log, PANIC_LOG).unwrap();
        let v = after_game(&h, "777", 0, 1, "");
        assert!(v.crash.is_some());
        assert_eq!(Store::load(&h).runtime_of("777"), Some(Games::Monado));
    }

    #[test]
    fn cli_set_next_and_list() {
        let h = scratch("cli");
        let w = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        cli(&w(&["set", "42", "steamvr", "Some", "Game"]), &h).unwrap();
        cli(&w(&["next", "42"]), &h).unwrap();
        let s = Store::load(&h);
        assert_eq!((s.runtime_of("42"), s.saved["42"].name.as_str(), s.next.as_deref()), (Some(Games::SteamVr), "Some Game", Some("42")));
        cli(&w(&["set", "42", "default"]), &h).unwrap();
        assert_eq!(Store::load(&h).runtime_of("42"), None);
        assert!(cli(&w(&["set", "x", "steamvr"]), &h).is_err());
        assert!(cli(&w(&["set", "42", "windows"]), &h).is_err());
    }
}
