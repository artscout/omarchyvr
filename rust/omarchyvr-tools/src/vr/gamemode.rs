//! "Game mode: free the graphics card while playing" and the "Compositor time budget".
//!
//! Game mode (Play only): at `vr start` the local voice AI (`omarchyvr-ai`, llama-server on the Vulkan GPU; `pause_ai`, default on)
//! is stopped the clean way (`omarchyvr-ai stop`: SIGTERM, never SIGKILL by us). Sunshine is NEVER stopped or paused (user rule);
//! only an old state file that lists it is honoured at restore (start again), what was running is written to `<session dir>/gamemode-paused.txt`,
//! and exactly that is started again at `vr stop` (and by `omarchyvr vr gamemode restore`, which the app runs at start
//! when no session is up: crash recovery). It is an optional step: every failure is a plain-words line, VR goes on.
//! Desk (Work) mode never calls it.
//!
//! Compositor time budget (`auto` | `6` | `8`, default auto): `U_PACING_COMP_MIN_TIME_MS` of Monado's pacer
//! (u_pacing_compositor_fake.c: a float in ms, default 3.0): the least time the compositor reserves between its own
//! wake-up and the display time. A bigger value makes it start the frame earlier (more margin against late commits,
//! a little more latency). `auto` = leave Monado's default. Set by [`super::config::build_env`].
//!
//! Settings: ~/.config/omarchyvr/gamemode.toml (`pause_ai = true|false`, the old `game_mode` key = `pause_ai`; `comp_budget = "auto"|"6"|"8"`).
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Settings file.
pub fn settings_path(home: &Path) -> PathBuf {
    home.join(".config/omarchyvr/gamemode.toml")
}

/// The user units tried for Sunshine (the one the package ships first).
pub const SUNSHINE_UNITS: [&str; 2] = ["app-dev.lizardbyte.app.Sunshine.service", "sunshine.service"];

/// Compositor time budget choices: (saved value, label).
pub const BUDGETS: [(&str, &str); 3] = [("auto", "Auto"), ("6", "6 ms"), ("8", "8 ms")];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// Pause the local voice AI while playing (default on).
    pub pause_ai: bool,
    /// "auto" | "6" | "8"
    pub comp_budget: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { pause_ai: true, comp_budget: "auto".into() }
    }
}

fn value<'a>(t: &'a str, key: &str) -> Option<&'a str> {
    t.lines().find_map(|l| {
        let l = l.split('#').next()?.trim();
        Some(l.strip_prefix(key)?.trim().strip_prefix('=')?.trim().trim_matches('"'))
    })
}

/// Parse the settings text (unknown or bad values give the defaults).
pub fn parse_settings(t: &str) -> Settings {
    let mut s = Settings::default();
    let flag = |v: Option<&str>| match v {
        Some("false" | "off") => Some(false),
        Some("true" | "on") => Some(true),
        _ => None,
    };
    // The old `game_mode` key meant "AI" (and, before this change, Sunshine too): now it is pause_ai only.
    if let Some(b) = flag(value(t, "game_mode")) {
        s.pause_ai = b;
    }
    if let Some(b) = flag(value(t, "pause_ai")) {
        s.pause_ai = b;
    }
    // `pause_sunshine` in an old file is ignored (Sunshine is never paused) and dropped at the next save.
    if let Some(b) = value(t, "comp_budget").filter(|b| BUDGETS.iter().any(|x| x.0 == *b)) {
        s.comp_budget = b.to_string();
    }
    s
}

pub fn load(home: &Path) -> Settings {
    std::fs::read_to_string(settings_path(home)).map(|t| parse_settings(&t)).unwrap_or_default()
}

pub fn save(home: &Path, s: &Settings) -> std::io::Result<()> {
    let p = settings_path(home);
    std::fs::create_dir_all(p.parent().unwrap())?;
    std::fs::write(
        p,
        format!("# Game mode and compositor time budget (Pimax app)\npause_ai = {}\ncomp_budget = \"{}\"\n", s.pause_ai, s.comp_budget),
    )
}

/// `U_PACING_COMP_MIN_TIME_MS` for a saved budget (None = auto: Monado's own 3 ms).
pub fn budget_env(budget: &str) -> Option<String> {
    matches!(budget, "6" | "8").then(|| format!("{budget}.0"))
}

/// The programs used (replaced by fakes in tests).
#[derive(Debug, Clone)]
pub struct Tools {
    pub systemctl: PathBuf,
    pub omarchyvr_ai: PathBuf,
    /// Where processes are listed (`/proc`; tests give an empty dir, so a real monado-service or vrserver on the
    /// machine running the tests does not change their result).
    pub proc_root: PathBuf,
}

impl Tools {
    pub fn real(home: &Path) -> Tools {
        Tools { systemctl: "systemctl".into(), omarchyvr_ai: super::config::tools_dir(home).join("omarchyvr-ai"), proc_root: "/proc".into() }
    }
}

/// Run a helper, retrying a few times on ETXTBSY: a script that was just written can be briefly "busy" while another
/// thread of this process forks (its child holds the write descriptor until it execs).
fn retry_busy<T>(mut f: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    let mut n = 0;
    loop {
        match f() {
            Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) && n < 20 => {
                n += 1;
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            r => return r,
        }
    }
}

fn state_file(session_dir: &Path) -> PathBuf {
    session_dir.join("gamemode-paused.txt")
}

/// What game mode paused (names for people): from the state file; empty = nothing.
pub fn paused(session_dir: &Path) -> Vec<String> {
    paused_from(&std::fs::read_to_string(state_file(session_dir)).unwrap_or_default())
}

/// [`paused`] from the state file's text.
pub fn paused_from(t: &str) -> Vec<String> {
    t.lines()
        .filter_map(|l| match l.split_once('=')?.0 {
            "sunshine" => Some("Sunshine".to_string()),
            "ai" => Some("local AI".to_string()),
            _ => None,
        })
        .collect()
}

/// "Paused for game mode: Sunshine, local AI" (None when nothing is paused).
pub fn paused_line(session_dir: &Path) -> Option<String> {
    let p = paused(session_dir);
    (!p.is_empty()).then(|| format!("Paused for game mode: {}", p.join(", ")))
}

fn systemctl(t: &Tools, args: &[&str]) -> Option<bool> {
    let o = retry_busy(|| Command::new(&t.systemctl).arg("--user").args(args).stdin(Stdio::null()).output()).ok()?;
    Some(o.status.success())
}

/// "gpu" | "cpu" when the local AI server runs (from `omarchyvr-ai status`).
fn running_ai(t: &Tools) -> Option<&'static str> {
    if !t.omarchyvr_ai.is_file() {
        return None;
    }
    let o = retry_busy(|| Command::new(&t.omarchyvr_ai).arg("status").stdin(Stdio::null()).output()).ok()?;
    let out = String::from_utf8_lossy(&o.stdout);
    let l = out.lines().find(|l| l.starts_with("server:"))?;
    if l.contains("stopped") {
        None
    } else if l.contains(", cpu,") {
        Some("cpu")
    } else {
        Some("gpu")
    }
}

/// Pause the local AI (only when its switch is on; Sunshine is never touched). A state file already there (a restart of a running
/// session) is kept: the list of what to bring back must never lose entries. Returns a plain-words line.
pub fn on_start(s: &Settings, t: &Tools, session_dir: &Path) -> Option<String> {
    if !s.pause_ai {
        return None;
    }
    let mut keep: Vec<String> = std::fs::read_to_string(state_file(session_dir)).map(|x| x.lines().map(String::from).collect()).unwrap_or_default();
    let mut notes = Vec::new();
    if let Some(mode) = s.pause_ai.then(|| running_ai(t)).flatten() {
        let ok = retry_busy(|| Command::new(&t.omarchyvr_ai).arg("stop").stdin(Stdio::null()).output()).is_ok_and(|o| o.status.success());
        if ok {
            keep.retain(|l| !l.starts_with("ai="));
            keep.push(format!("ai={mode}"));
        } else {
            notes.push("the local AI could not be paused".to_string());
        }
    }
    if !keep.is_empty() {
        let _ = std::fs::create_dir_all(session_dir);
        let _ = std::fs::write(state_file(session_dir), keep.join("\n") + "\n");
    }
    let mut out = if paused(session_dir).is_empty() { String::new() } else { format!("game mode: paused {}", paused(session_dir).join(", ")) };
    for n in notes {
        if !out.is_empty() {
            out.push_str("; ");
        }
        out.push_str(&format!("game mode: {n} (VR goes on)"));
    }
    (!out.is_empty()).then_some(out)
}

/// The arguments that bring the local AI back after a pause, or up at a Work start: `omarchyvr-ai up` starts every slot
/// ai.toml asks for (the action model, the router, the speech server when the video memory fits). `serve` started the
/// action model only, so the speech server stayed down after a game-mode pause (live 2026-10-16: the whole Work
/// session used the CPU speech fallback).
pub fn up_args(cpu: bool) -> Vec<String> {
    let mut v = vec!["up".to_string(), "--wait".into(), "0".into()];
    if cpu {
        v.push("--cpu".into());
    }
    v
}

/// Work with voice: bring up what ai.toml asks for when Local AI is on (`omarchyvr-ai up --wait 0`, detached; it keeps a
/// server that fits and starts the missing ones, the speech server included). The voice listener tries the speech
/// server again after a pause, so it switches to it once it answers. Off: nothing.
pub fn local_ai_up(t: &Tools, cfg: &omarchyvr_config::ai::AiConfig) -> Option<String> {
    if !cfg.local_models_on() {
        return None;
    }
    let spawned = Command::new(&t.omarchyvr_ai).args(up_args(false)).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    Some(match spawned {
        Ok(_) => "local AI: starting what is missing (speech server included when it fits)".to_string(),
        Err(e) => format!("local AI: could not run omarchyvr-ai up: {e}"),
    })
}

/// Start again exactly what was paused. The state file goes first; the AI is started detached (it takes a while to load).
pub fn on_stop(t: &Tools, session_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(state_file(session_dir)).ok()?;
    let _ = std::fs::remove_file(state_file(session_dir));
    let mut back = Vec::new();
    let mut failed = Vec::new();
    for l in text.lines() {
        match l.split_once('=') {
            Some(("sunshine", u)) if SUNSHINE_UNITS.contains(&u) => {
                if systemctl(t, &["start", u]) == Some(true) { back.push("Sunshine") } else { failed.push("Sunshine") }
            }
            Some(("ai", m)) if m == "gpu" || m == "cpu" => {
                let spawned = retry_busy(|| {
                    let mut c = Command::new(&t.omarchyvr_ai);
                    c.args(up_args(m == "cpu"));
                    c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
                });
                if spawned.is_ok() { back.push("local AI") } else { failed.push("local AI") }
            }
            _ => {}
        }
    }
    let mut out = String::new();
    if !back.is_empty() {
        out = format!("game mode: started again: {}", back.join(", "));
    }
    if !failed.is_empty() {
        out.push_str(&format!("{}game mode: could not start again: {}", if out.is_empty() { "" } else { "; " }, failed.join(", ")));
    }
    (!out.is_empty()).then_some(out)
}

/// A monado-service or SteamVR's vrserver is alive (by process name): `restore` must not start the AI again into a session.
fn session_running(proc_root: &Path) -> bool {
    std::fs::read_dir(proc_root).into_iter().flatten().flatten().any(|e| {
        e.file_name().to_string_lossy().bytes().all(|b| b.is_ascii_digit())
            && std::fs::read_to_string(e.path().join("comm")).is_ok_and(|c| matches!(c.trim(), "monado-service" | "vrserver"))
    })
}

/// `omarchyvr vr gamemode [status|on|off|budget auto|6|8|restore]`.
pub fn cli(what: &[String], home: &Path, session_dir: &Path) -> Result<(), String> {
    let mut s = load(home);
    match what.first().map(String::as_str).unwrap_or("status") {
        w @ ("on" | "off") => {
            s.pause_ai = w == "on";
            save(home, &s).map_err(|e| e.to_string())?;
            println!("Game mode (pause the local AI): {w}");
        }
        "budget" => {
            let b = what.get(1).map(|b| b.trim_end_matches("ms").trim().to_string()).unwrap_or_default();
            if !BUDGETS.iter().any(|x| x.0 == b) {
                return Err("budget: auto | 6 | 8 (ms)".into());
            }
            s.comp_budget = b;
            save(home, &s).map_err(|e| e.to_string())?;
            println!("Compositor time budget: {} (next start)", s.comp_budget);
        }
        "restore" => {
            let t = Tools::real(home);
            if session_running(&t.proc_root) {
                println!("A VR session is running: nothing started again");
                return Ok(());
            }
            if let Some(l) = on_stop(&t, session_dir) {
                println!("{l}");
            }
        }
        "status" => {
            println!("Pause the local AI: {}", if s.pause_ai { "on" } else { "off" });
            println!("Compositor time budget: {}", s.comp_budget);
            println!("{}", paused_line(session_dir).unwrap_or_else(|| "Nothing paused".into()));
        }
        w => return Err(format!("unknown choice {w} (status|on|off|budget auto|6|8|restore)")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn script(p: &Path, body: &str) {
        std::fs::write(p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Fake systemctl (logs calls; Sunshine "active" while <dir>/sun-up exists) and fake omarchyvr-ai.
    fn fakes(d: &Path, ai: &str) -> Tools {
        let _ = std::fs::create_dir_all(d.join("proc"));
        let t = Tools { systemctl: d.join("systemctl"), omarchyvr_ai: d.join("omarchyvr-ai"), proc_root: d.join("proc") };
        script(
            &t.systemctl,
            &format!(
                "echo \"$@\" >> {d}/calls\ncase \"$2\" in\n is-active) [ \"$4\" = app-dev.lizardbyte.app.Sunshine.service ] && [ -e {d}/sun-up ] ;;\n stop) rm -f {d}/sun-up ;;\n start) touch {d}/sun-up ;;\nesac",
                d = d.display()
            ),
        );
        script(
            &t.omarchyvr_ai,
            &format!(
                "echo \"ai $@\" >> {d}/calls\ncase \"$1\" in\n status) echo '{ai}' ;;\nesac",
                d = d.display(),
                ai = ai
            ),
        );
        std::fs::write(d.join("sun-up"), "").unwrap();
        t
    }

    fn calls(d: &Path) -> String {
        std::fs::read_to_string(d.join("calls")).unwrap_or_default()
    }

    #[test]
    fn the_session_scan_reads_the_given_proc_dir() {
        let d = crate::vr::test_dir("gm-proc");
        let p = d.join("proc");
        std::fs::create_dir_all(p.join("self")).unwrap();
        std::fs::create_dir_all(p.join("123")).unwrap();
        std::fs::write(p.join("123/comm"), "bash\n").unwrap();
        assert!(!session_running(&p));
        std::fs::create_dir_all(p.join("456")).unwrap();
        std::fs::write(p.join("456/comm"), "monado-service\n").unwrap();
        assert!(session_running(&p));
        std::fs::write(p.join("456/comm"), "vrserver\n").unwrap();
        assert!(session_running(&p));
    }

    #[test]
    fn budgets_list_matches_the_app() {
        assert_eq!(BUDGETS.len(), 3);
        assert_eq!(BUDGETS[0].0, "auto");
    }

    #[test]
    fn settings_and_budget() {
        assert_eq!(parse_settings(""), Settings::default());
        assert!(Settings::default().pause_ai);
        let s = parse_settings("game_mode = false # x\ncomp_budget = \"8\"");
        assert!(!s.pause_ai);
        let o = parse_settings("game_mode = true");
        assert!(o.pause_ai); // old key: AI only
        // an old pause_sunshine line is ignored and dropped at the next save
        let n = parse_settings("game_mode = false\npause_ai = true\npause_sunshine = true");
        assert!(n.pause_ai);
        assert_eq!(s.comp_budget, "8");
        assert_eq!(parse_settings("comp_budget = \"99\"").comp_budget, "auto");
        assert_eq!(budget_env("auto"), None);
        assert_eq!(budget_env("6").as_deref(), Some("6.0"));
        assert_eq!(budget_env("8").as_deref(), Some("8.0"));
        let h = crate::vr::test_dir("gm-settings");
        let home = h.join(".config/omarchyvr");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(settings_path(&h), "pause_ai = true\npause_sunshine = true\n").unwrap();
        save(&h, &load(&h)).unwrap();
        assert!(!std::fs::read_to_string(settings_path(&h)).unwrap().contains("sunshine"));
        save(&h, &s).unwrap();
        assert_eq!(load(&h), s);
    }

    #[test]
    fn pauses_and_restores_exactly_what_ran() {
        let d = crate::vr::test_dir("gm-both");
        let t = fakes(&d, "server:  pid 5 on http://127.0.0.1:8080/v1, cpu, m.gguf, ready");
        let sess = d.join("sess");
        let l = on_start(&Settings::default(), &t, &sess).unwrap();
        assert_eq!(l, "game mode: paused local AI");
        assert!(!calls(&d).contains("stop app-dev"), "Sunshine is never stopped");
        assert!(calls(&d).contains("ai stop"));
        // a state file from an older version that stopped Sunshine: it is started again, never stopped
        let mut st = std::fs::read_to_string(sess.join("gamemode-paused.txt")).unwrap();
        st.push_str("sunshine=app-dev.lizardbyte.app.Sunshine.service\n");
        std::fs::write(sess.join("gamemode-paused.txt"), st).unwrap();
        let l = on_stop(&t, &sess).unwrap();
        assert!(l.contains("started again: Sunshine, local AI") || l.contains("started again: local AI, Sunshine"), "{l}");
        assert!(paused(&sess).is_empty());
        // the AI comes back detached: give a loaded test machine time (up to 10 s)
        for _ in 0..200 {
            if calls(&d).contains("ai up --wait 0 --cpu") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(calls(&d).contains("--user start app-dev.lizardbyte.app.Sunshine.service"));
        assert!(calls(&d).contains("ai up --wait 0 --cpu"));
        assert_eq!(on_stop(&t, &sess), None); // nothing left to restore
    }

    #[test]
    fn nothing_running_nothing_paused_and_off_does_nothing() {
        let d = crate::vr::test_dir("gm-none");
        let t = fakes(&d, "server:  stopped");
        std::fs::remove_file(d.join("sun-up")).unwrap();
        assert_eq!(on_start(&Settings::default(), &t, &d.join("s")), None);
        assert!(!calls(&d).contains("stop"));
        let d = crate::vr::test_dir("gm-off");
        let t = fakes(&d, "server:  pid 5, gpu (all layers)");
        let off = Settings { pause_ai: false, ..Settings::default() };
        assert_eq!(on_start(&off, &t, &d.join("s")), None);
        assert_eq!(calls(&d), "");
    }

    #[test]
    fn restart_keeps_the_list_and_missing_tools_never_fail() {
        let d = crate::vr::test_dir("gm-keep");
        let t = fakes(&d, "server:  stopped");
        let sess = d.join("s");
        std::fs::create_dir_all(&sess).unwrap();
        std::fs::write(sess.join("gamemode-paused.txt"), "ai=gpu\n").unwrap();
        let both = Settings::default();
        assert_eq!(on_start(&both, &t, &sess), Some("game mode: paused local AI".into())); // AI already paused by the first start: kept
        let none = Tools { systemctl: d.join("nope"), omarchyvr_ai: d.join("nope2"), proc_root: d.join("proc") };
        assert_eq!(on_start(&both, &none, &d.join("s2")), None);
    }

    #[test]
    fn default_never_touches_sunshine() {
        let d = crate::vr::test_dir("gm-default-sun");
        let t = fakes(&d, "server:  pid 5, gpu (all layers)"); // Sunshine is up, AI runs
        let l = on_start(&Settings::default(), &t, &d.join("s")).unwrap();
        assert_eq!(l, "game mode: paused local AI");
        assert!(!calls(&d).contains("Sunshine"), "{}", calls(&d));
        assert!(d.join("sun-up").exists());
        let ai_off = Settings { pause_ai: false, ..Settings::default() };
        let d = crate::vr::test_dir("gm-default-sun2");
        let t = fakes(&d, "server:  pid 5, gpu (all layers)");
        assert_eq!(on_start(&ai_off, &t, &d.join("s")), None);
        assert_eq!(calls(&d), "");
    }
}
