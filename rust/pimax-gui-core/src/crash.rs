//! Crash, hang and renderer-error reports: `~/.local/state/pimax/crashes/<utc>-<kind>/report.txt`, written by the
//! app's panic hook, its hang watchdog, its renderer error path, or by the supervisor when the window process died
//! from a signal. A report says what happened, what the app was doing (page, job, last action), on which system
//! (version, kernel, GPU driver, compositor, renderer) and the log lines before it. The next start shows reports that
//! were not looked at yet.
//!
//! Nothing here sends anything anywhere (see [`crate::upload`]).
use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// What happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A Rust panic that ended the window process.
    Panic,
    /// A signal (SIGSEGV, SIGABRT, ...): usually a graphics driver or a library.
    Native,
    /// The renderer failed (no OpenGL/Vulkan surface, a failed buffer swap).
    Renderer,
    /// The window stopped answering (UI thread blocked).
    Hang,
    /// Killed from outside (SIGKILL: out of memory, or "terminate" in the desktop's not-responding dialog).
    Killed,
    /// An unexpected exit code.
    Exit,
    /// A panic the app caught and survived (a page, a background task).
    Recovered,
}

impl Kind {
    /// Short name (directory suffix, `kind:` line).
    pub fn name(self) -> &'static str {
        match self {
            Kind::Panic => "panic",
            Kind::Native => "native",
            Kind::Renderer => "renderer",
            Kind::Hang => "hang",
            Kind::Killed => "killed",
            Kind::Exit => "exit",
            Kind::Recovered => "recovered",
        }
    }

    /// From [`Kind::name`].
    pub fn parse(s: &str) -> Option<Kind> {
        Some(match s.trim() {
            "panic" => Kind::Panic,
            "native" => Kind::Native,
            "renderer" => Kind::Renderer,
            "hang" => Kind::Hang,
            "killed" => Kind::Killed,
            "exit" => Kind::Exit,
            "recovered" => Kind::Recovered,
            _ => return None,
        })
    }

    /// For people.
    pub fn title(self) -> &'static str {
        match self {
            Kind::Panic => "The app crashed (internal error)",
            Kind::Native => "The app crashed (graphics driver or system library)",
            Kind::Renderer => "The window's renderer failed",
            Kind::Hang => "The app stopped responding",
            Kind::Killed => "The app was killed",
            Kind::Exit => "The app exited unexpectedly",
            Kind::Recovered => "An internal error was caught",
        }
    }

    /// Ended the window process (a recovered panic did not).
    pub fn fatal(self) -> bool {
        self != Kind::Recovered
    }
}

/// One report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// Kind.
    pub kind: Kind,
    /// UTC time (`2026-10-14T19:40:01.123Z`).
    pub when: String,
    /// One line.
    pub summary: String,
    /// Message, location, backtrace, signal, exit status.
    pub details: String,
    /// What the app was doing (page, job, last action, renderer, uptime).
    pub context: Vec<(String, String)>,
    /// Version, OS, kernel, GPU driver, compositor.
    pub system: Vec<(String, String)>,
    /// App log lines before it.
    pub log_tail: Vec<String>,
    /// Last stderr lines of the window process (drivers print there).
    pub stderr_tail: Vec<String>,
    /// Other files in the report directory (minidump).
    pub files: Vec<String>,
}

impl Report {
    /// A report of `kind` now, with the current context and system info.
    pub fn new(kind: Kind, summary: impl Into<String>, details: impl Into<String>) -> Report {
        Report {
            kind,
            when: crate::applog::now_utc(),
            summary: summary.into(),
            details: details.into(),
            context: context(),
            system: system(),
            log_tail: Vec::new(),
            stderr_tail: Vec::new(),
            files: Vec::new(),
        }
    }

    /// The `report.txt` text.
    pub fn render(&self) -> String {
        let mut o = format!(
            "Pimax app crash report\nkind: {}\nwhen: {}\nsummary: {}\n",
            self.kind.name(),
            self.when,
            self.summary.replace('\n', " ")
        );
        let kv = |o: &mut String, name: &str, v: &[(String, String)]| {
            *o += &format!("\n[{name}]\n");
            for (k, x) in v {
                *o += &format!("{k}: {}\n", x.replace('\n', " "));
            }
        };
        o += "\n[details]\n";
        o += self.details.trim_end();
        o += "\n";
        kv(&mut o, "context", &self.context);
        kv(&mut o, "system", &self.system);
        for (name, lines) in [("log", &self.log_tail), ("stderr", &self.stderr_tail), ("files", &self.files)] {
            o += &format!("\n[{name}]\n");
            for l in lines.iter() {
                o += l;
                o += "\n";
            }
        }
        o
    }

    /// Read a `report.txt` back.
    pub fn parse(text: &str) -> Option<Report> {
        let mut lines = text.lines();
        if lines.next()? != "Pimax app crash report" {
            return None;
        }
        let mut r = Report {
            kind: Kind::Exit,
            when: String::new(),
            summary: String::new(),
            details: String::new(),
            context: Vec::new(),
            system: Vec::new(),
            log_tail: Vec::new(),
            stderr_tail: Vec::new(),
            files: Vec::new(),
        };
        let mut kind = None;
        let mut section = String::new();
        for l in lines {
            if l.starts_with('[') && l.ends_with(']') && ["[details]", "[context]", "[system]", "[log]", "[stderr]", "[files]"].contains(&l) {
                section = l[1..l.len() - 1].to_string();
                continue;
            }
            let pair = || l.split_once(": ").map(|(k, v)| (k.to_string(), v.to_string()));
            match section.as_str() {
                "" => {
                    if let Some(v) = l.strip_prefix("kind: ") {
                        kind = Kind::parse(v);
                    } else if let Some(v) = l.strip_prefix("when: ") {
                        r.when = v.to_string();
                    } else if let Some(v) = l.strip_prefix("summary: ") {
                        r.summary = v.to_string();
                    }
                }
                "details" => {
                    if !r.details.is_empty() {
                        r.details.push('\n');
                    }
                    r.details.push_str(l);
                }
                "context" => r.context.extend(pair()),
                "system" => r.system.extend(pair()),
                "log" => r.log_tail.push(l.to_string()),
                "stderr" => r.stderr_tail.push(l.to_string()),
                "files" => r.files.push(l.to_string()),
                _ => {}
            }
        }
        r.kind = kind?;
        r.details = r.details.trim_end().to_string();
        for v in [&mut r.log_tail, &mut r.stderr_tail, &mut r.files] {
            while v.last().is_some_and(|l| l.is_empty()) {
                v.pop();
            }
        }
        Some(r)
    }

    /// A context value.
    pub fn ctx(&self, key: &str) -> Option<&str> {
        self.context.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

// ---------------------------------------------------------------------------------------------------------------
// What the app is doing (breadcrumbs) and the system info, kept for the panic hook.

fn ctx_map() -> &'static Mutex<BTreeMap<&'static str, String>> {
    static C: OnceLock<Mutex<BTreeMap<&'static str, String>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// Remember what the app is doing (`page`, `job`, `action`, `renderer`, ...) for a later report. Cheap; empty
/// removes the key.
pub fn note(key: &'static str, value: impl Into<String>) {
    let v = value.into();
    if let Ok(mut m) = ctx_map().lock() {
        if v.is_empty() {
            m.remove(key);
        } else if m.get(key) != Some(&v) {
            m.insert(key, v);
        }
    }
}

fn started() -> &'static Instant {
    static T: OnceLock<Instant> = OnceLock::new();
    T.get_or_init(Instant::now)
}

/// The breadcrumbs plus the uptime. Never blocks (it runs in the panic hook).
pub fn context() -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = match ctx_map().try_lock() {
        Ok(m) => m.iter().map(|(k, x)| (k.to_string(), x.clone())).collect(),
        Err(_) => vec![("context".into(), "(busy)".into())],
    };
    v.push(("uptime".into(), format!("{:.0} s", started().elapsed().as_secs_f64())));
    v.push(("pid".into(), std::process::id().to_string()));
    v
}

static SYSTEM: OnceLock<Vec<(String, String)>> = OnceLock::new();

/// Set the system info once (collected in the background at startup: it reads files).
pub fn set_system(v: Vec<(String, String)>) {
    let _ = SYSTEM.set(v);
}

/// The system info (version only until [`set_system`] ran).
pub fn system() -> Vec<(String, String)> {
    SYSTEM.get().cloned().unwrap_or_else(|| vec![("app".into(), version_line())])
}

/// `0.1.0 (git abc1234, built 2026-10-14)` from what the binary was built with ([`set_build_info`]).
pub fn version_line() -> String {
    let b = BUILD.get();
    format!(
        "{}{}",
        env!("CARGO_PKG_VERSION"),
        b.map(|(git, date)| format!(" (git {git}, built {date})")).unwrap_or_default()
    )
}

static BUILD: OnceLock<(String, String)> = OnceLock::new();

/// The git hash and build date of the app binary (its build script knows them).
pub fn set_build_info(git: &str, date: &str) {
    let _ = BUILD.set((git.to_string(), date.to_string()));
}

/// The GNU build id of an ELF file (hex), from its `PT_NOTE` segments. Release binaries are stripped: the build id
/// names the exact binary, so a minidump of it can be symbolicated with the matching debug file later.
pub fn build_id(path: &Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let mut h = [0u8; 64];
    f.read_exact(&mut h).ok()?;
    if &h[0..4] != b"\x7fELF" || h[4] != 2 || h[5] != 1 {
        return None; // 64-bit little endian only
    }
    let u16_at = |b: &[u8], o: usize| u16::from_le_bytes([b[o], b[o + 1]]) as u64;
    let u32_at = |b: &[u8], o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as u64;
    let u64_at = |b: &[u8], o: usize| {
        let mut a = [0u8; 8];
        a.copy_from_slice(&b[o..o + 8]);
        u64::from_le_bytes(a)
    };
    let (phoff, phentsize, phnum) = (u64_at(&h, 32), u16_at(&h, 54), u16_at(&h, 56));
    for i in 0..phnum.min(64) {
        f.seek(SeekFrom::Start(phoff + i * phentsize)).ok()?;
        let mut ph = [0u8; 56];
        f.read_exact(&mut ph).ok()?;
        if u32_at(&ph, 0) != 4 {
            continue; // not PT_NOTE
        }
        let (off, size) = (u64_at(&ph, 8), u64_at(&ph, 32).min(1 << 16));
        f.seek(SeekFrom::Start(off)).ok()?;
        let mut notes = vec![0u8; size as usize];
        f.read_exact(&mut notes).ok()?;
        let mut p = 0usize;
        let align = |x: usize| (x + 3) & !3;
        while p + 12 <= notes.len() {
            let (namesz, descsz, ty) = (u32_at(&notes, p) as usize, u32_at(&notes, p + 4) as usize, u32_at(&notes, p + 8));
            let name_at = p + 12;
            let desc_at = name_at + align(namesz);
            if desc_at + descsz > notes.len() {
                break;
            }
            if ty == 3 && notes.get(name_at..name_at + namesz) == Some(b"GNU\0") {
                return Some(notes[desc_at..desc_at + descsz].iter().map(|b| format!("{b:02x}")).collect());
            }
            p = desc_at + align(descsz);
        }
    }
    None
}

/// Called by the panic hook for a fatal panic: asks for a minidump of this process (the window registers it when
/// the supervisor's crash server is reachable).
static DUMP_REQUEST: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// Register the minidump request of fatal panics.
pub fn set_dump_request(f: Box<dyn Fn() + Send + Sync>) {
    let _ = DUMP_REQUEST.set(f);
}

/// Read the system info: OS, kernel, GPU driver, session, compositor; plus `extra` (renderer, ...).
pub fn collect_system(env: &dyn Fn(&str) -> Option<String>, extra: &[(&str, String)]) -> Vec<(String, String)> {
    let read = |p: &str| std::fs::read_to_string(p).ok();
    let mut v = vec![("app".to_string(), version_line())];
    if let Some(id) = std::env::current_exe().ok().and_then(|e| build_id(&e)) {
        v.push(("build id".into(), id));
    }
    let os = read("/etc/os-release")
        .and_then(|t| t.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=").map(|s| s.trim_matches('"').to_string())));
    v.push(("os".into(), os.unwrap_or_else(|| "?".into())));
    v.push(("kernel".into(), read("/proc/sys/kernel/osrelease").map(|s| s.trim().to_string()).unwrap_or_else(|| "?".into())));
    let nv = read("/proc/driver/nvidia/version").and_then(|t| t.lines().next().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")));
    v.push(("gpu driver".into(), nv.unwrap_or_else(|| "no NVIDIA driver loaded".into())));
    let session = env("XDG_SESSION_TYPE").unwrap_or_else(|| if env("WAYLAND_DISPLAY").is_some() { "wayland".into() } else { "?".into() });
    v.push(("session".into(), session));
    let desk = env("XDG_CURRENT_DESKTOP")
        .or_else(|| env("HYPRLAND_INSTANCE_SIGNATURE").map(|_| "Hyprland".into()))
        .unwrap_or_else(|| "?".into());
    v.push(("desktop".into(), desk));
    for (k, x) in extra {
        v.push((k.to_string(), x.clone()));
    }
    v
}

// ---------------------------------------------------------------------------------------------------------------
// Files.

/// `<state>/crashes`.
pub fn crashes_dir(state: &Path) -> PathBuf {
    state.join("crashes")
}

/// Marker in a report directory: the user has seen it.
pub const SEEN: &str = "seen";

/// Write a report into a new directory; returns the directory.
pub fn write(state: &Path, r: &Report) -> std::io::Result<PathBuf> {
    let base = crashes_dir(state);
    std::fs::create_dir_all(&base)?;
    let stamp = r.when.replace([':', '.'], "").replace('-', "");
    let mut dir = base.join(format!("{stamp}-{}", r.kind.name()));
    let mut i = 1;
    while dir.exists() {
        i += 1;
        dir = base.join(format!("{stamp}-{}-{i}", r.kind.name()));
    }
    std::fs::create_dir_all(&dir)?;
    let tmp = dir.join("report.txt.tmp");
    std::fs::write(&tmp, r.render())?;
    std::fs::rename(&tmp, dir.join("report.txt"))?;
    if !r.kind.fatal() {
        // a caught panic is in the logs and problem reports, but not shown as a crash at the next start
        let _ = std::fs::write(dir.join(SEEN), "");
    }
    Ok(dir)
}

/// Read the report of a directory.
pub fn read(dir: &Path) -> Option<Report> {
    Report::parse(&std::fs::read_to_string(dir.join("report.txt")).ok()?)
}

/// All reports, newest first: (directory, report, seen).
pub fn list(state: &Path) -> Vec<(PathBuf, Report, bool)> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(crashes_dir(state))
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect())
        .unwrap_or_default();
    dirs.sort();
    dirs.reverse();
    dirs.into_iter()
        .filter_map(|d| {
            let r = read(&d)?;
            let seen = d.join(SEEN).exists();
            Some((d, r, seen))
        })
        .collect()
}

/// Crash reports the user has not seen yet, newest first.
pub fn unseen(state: &Path) -> Vec<(PathBuf, Report)> {
    list(state).into_iter().filter(|(_, r, seen)| !seen && r.kind.fatal()).map(|(d, r, _)| (d, r)).collect()
}

/// Mark a report as seen.
pub fn mark_seen(dir: &Path) -> std::io::Result<()> {
    std::fs::write(dir.join(SEEN), "")
}

/// Keep the newest `keep` report directories.
pub fn prune(state: &Path, keep: usize) {
    for (d, _, _) in list(state).into_iter().skip(keep) {
        let _ = std::fs::remove_dir_all(d);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Panics.

thread_local! {
    static CATCHING: Cell<bool> = const { Cell::new(false) };
}

/// Run `f`, catching a panic (a page, a background task). The panic hook records it as [`Kind::Recovered`].
pub fn catching<R>(f: impl FnOnce() -> R) -> std::thread::Result<R> {
    let before = CATCHING.with(|c| c.replace(true));
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    CATCHING.with(|c| c.set(before));
    r
}

/// Panics seen by the hook in this process (caught or not).
pub static PANICS: AtomicU64 = AtomicU64::new(0);

/// Install the panic hook: every panic writes a report (fatal on the main thread unless caught) and is logged; the
/// previous hook still prints to stderr.
pub fn install_panic_hook(state: PathBuf) {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        PANICS.fetch_add(1, Ordering::Relaxed);
        let caught = CATCHING.with(|c| c.get());
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("unnamed").to_string();
        let msg = crate::worker::panic_text(info.payload());
        let loc = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_default();
        let kind = if caught || name != "main" { Kind::Recovered } else { Kind::Panic };
        let summary = format!("panic in thread '{name}' at {loc}: {msg}");
        crate::applog::error_unless_logging(&summary);
        let bt = std::backtrace::Backtrace::force_capture();
        let mut r = Report::new(kind, summary, format!(
            "{msg}\n  at {loc}\n  thread {name}{}\n\nbacktrace (function names only in unstripped builds; a fatal panic also gets a minidump):\n{bt}",
            if caught { " (caught: the app went on)" } else { "" }
        ));
        r.log_tail = crate::applog::recent(200);
        let _ = write(&state, &r);
        if kind == Kind::Panic {
            if let Some(f) = DUMP_REQUEST.get() {
                f();
            }
        }
        prev(info);
    }));
}

// ---------------------------------------------------------------------------------------------------------------
// Hangs.

static BEAT_MS: AtomicU64 = AtomicU64::new(0);

/// The UI thread is alive (call once per frame).
pub fn beat() {
    // +1: zero means "no beat yet"
    BEAT_MS.store(started().elapsed().as_millis() as u64 + 1, Ordering::Relaxed);
}

/// Time since the last [`beat`] (zero before the first one: a window that never drew is not "hung").
pub fn since_beat() -> Duration {
    let last = BEAT_MS.load(Ordering::Relaxed);
    if last == 0 {
        return Duration::ZERO;
    }
    Duration::from_millis((started().elapsed().as_millis() as u64 + 1).saturating_sub(last))
}

/// The hang marker of a window process, read by the supervisor when that process is killed.
pub fn hang_marker(state: &Path, pid: u32) -> PathBuf {
    crashes_dir(state).join(format!(".hang-{pid}"))
}

/// The threads of this process: name, state and kernel wait channel (where a blocked thread sleeps).
pub fn thread_states() -> Vec<String> {
    let mut v = Vec::new();
    let Ok(rd) = std::fs::read_dir("/proc/self/task") else { return v };
    let mut tids: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    tids.sort_by_key(|t| t.parse::<u64>().unwrap_or(0));
    for t in tids {
        let base = Path::new("/proc/self/task").join(&t);
        let rd = |f: &str| std::fs::read_to_string(base.join(f)).unwrap_or_default().trim().to_string();
        let stat = rd("stat");
        let state = stat.rsplit_once(") ").map(|(_, r)| r.split(' ').next().unwrap_or("?").to_string()).unwrap_or_default();
        v.push(format!("{t} {:<16} state {state} wchan {}", rd("comm"), rd("wchan")));
    }
    v
}

/// Watch the UI thread: when it has not called [`beat`] for `limit`, write a hang report and the hang marker (once
/// per stall); when it comes back, log how long it was stuck and remove the marker.
pub fn start_watchdog(state: PathBuf, limit: Duration) {
    let _ = std::thread::Builder::new().name("pimax-watchdog".into()).spawn(move || {
        let marker = hang_marker(&state, std::process::id());
        let mut stalled: Option<Duration> = None;
        loop {
            std::thread::sleep(Duration::from_millis(500));
            let gap = since_beat();
            match stalled {
                None if gap >= limit => {
                    stalled = Some(gap);
                    let summary = format!("the window did not answer for {:.0} s (UI thread blocked)", gap.as_secs_f64());
                    log::error!("{summary}");
                    let mut r = Report::new(Kind::Hang, summary, format!("threads (tid, name, state, kernel wait channel):\n{}", thread_states().join("\n")));
                    r.log_tail = crate::applog::recent(200);
                    if let Ok(dir) = write(&state, &r) {
                        let _ = std::fs::write(&marker, dir.display().to_string());
                    }
                }
                Some(_) if gap < limit => {
                    log::warn!("the window answers again after a stall");
                    stalled = None;
                    let _ = std::fs::remove_file(&marker);
                }
                _ => {}
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pimax-crash-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn report_round_trip_and_listing() {
        let st = tmp("rt");
        // keys of this test only: the notes are global, and other tests running at the same time set "page"
        note("test-rt-page", "Desk");
        note("test-rt-job", "Start desk mode (--eye)");
        note("test-rt-job", "");
        let mut r = Report::new(Kind::Native, "SIGSEGV in the window process", "signal 11\nsecond line");
        r.log_tail = vec!["a".into(), "b: c".into()];
        r.stderr_tail = vec!["libEGL warning".into()];
        r.files = vec!["minidump.dmp".into()];
        let text = r.render();
        let back = Report::parse(&text).unwrap();
        assert_eq!(back, r);
        assert_eq!(back.ctx("test-rt-page"), Some("Desk"));
        assert_eq!(back.ctx("test-rt-job"), None, "an empty note removes the key");
        let d1 = write(&st, &r).unwrap();
        let d2 = write(&st, &Report::new(Kind::Recovered, "caught", "x")).unwrap();
        assert_ne!(d1, d2);
        let all = list(&st);
        assert_eq!(all.len(), 2);
        let un = unseen(&st);
        assert_eq!(un.len(), 1, "a caught panic is not a crash to show");
        assert_eq!(un[0].1.kind, Kind::Native);
        mark_seen(&un[0].0).unwrap();
        assert!(unseen(&st).is_empty());
        prune(&st, 1);
        assert_eq!(list(&st).len(), 1);
        assert!(Report::parse("something else").is_none());
        let _ = std::fs::remove_dir_all(st);
    }

    #[test]
    fn caught_panics_are_recovered_and_the_beat_ages() {
        assert!(catching(|| 5).is_ok());
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let r = catching(|| panic!("x"));
        std::panic::set_hook(prev);
        assert!(r.is_err());
        beat();
        assert!(since_beat() < Duration::from_secs(1));
        assert!(!thread_states().is_empty());
        let id = build_id(&std::env::current_exe().unwrap());
        assert!(id.as_ref().is_some_and(|i| i.len() >= 16 && i.chars().all(|c| c.is_ascii_hexdigit())), "{id:?}");
        assert_eq!(build_id(Path::new("/proc/self/status")), None);
        assert!(thread_states()[0].contains("state"));
        let sys = collect_system(&|k| (k == "XDG_SESSION_TYPE").then(|| "wayland".to_string()), &[("renderer", "wgpu".into())]);
        assert!(sys.iter().any(|(k, v)| k == "session" && v == "wayland"));
        assert!(sys.iter().any(|(k, v)| k == "renderer" && v == "wgpu"));
    }
}
