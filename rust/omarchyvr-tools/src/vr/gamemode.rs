//! Play's settings: the graphics card belongs to the game, "Eye tracking in games", Light game mode and the
//! "Compositor time budget".
//!
//! Play frees the graphics card (user decision 2026-10-17): before `vr start` starts anything, everything of Work and
//! the local AI is stopped by pid ([`super::work::clear_for_play`]: every `omarchyvr-ai` slot, voice, the desk's leftovers);
//! what was stopped is written to `<session dir>/gamemode-paused.txt` for the app's "Stopped for Play" line. Nothing is
//! started again at `vr stop`: the local AI and voice belong to Work and start with the next Work. The old switch
//! `pause_ai` ("Game mode", default on) is read from old files but no longer changes anything: Play always frees the
//! card. Sunshine is NEVER stopped or paused (user rule); only an old state file that lists it is honoured at restore
//! (started again). Desk (Work) mode never calls this.
//!
//! Eye tracking in games (`eye_in_games`, default off; coordinator decision 2026-10-17): Play passes `--eye` or
//! `--no-eye` from it, and `vr start` without either flag uses the saved switch; the resolved choice is recorded in
//! start-argv so restarts keep it. Light game mode no longer decides eye tracking.
//!
//! Compositor time budget (`auto` | `6` | `8`, default auto): `U_PACING_COMP_MIN_TIME_MS` of Monado's pacer
//! (u_pacing_compositor_fake.c: a float in ms, default 3.0): the least time the compositor reserves between its own
//! wake-up and the display time. A bigger value makes it start the frame earlier (more margin against late commits,
//! a little more latency). `auto` = leave Monado's default. Set by [`super::config::build_env`].
//!
//! Settings: ~/.config/omarchyvr/gamemode.toml (`light_game`, `eye_in_games`, `comp_budget = "auto"|"6"|"8"`; old
//! files: `pause_ai` / `game_mode`, kept but without effect).
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
    /// The old "Game mode" switch (pause the local AI while playing). Kept from old files, no effect any more: Play
    /// always stops the local AI before it starts (see the module doc).
    pub pause_ai: bool,
    /// "auto" | "6" | "8"
    pub comp_budget: String,
    /// Light game mode (default on): Play runs only what a game needs (Monado, head 6DoF, controllers and their
    /// optical tracking); no wrist readout, no room anchor / room search. `vr start --light` / `--no-light` override it.
    pub light: bool,
    /// Eye tracking in games (default off): Play starts the eye tracker only when this is on (`vr start --eye` /
    /// `--no-eye` override it for one start).
    pub eye_games: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { pause_ai: true, comp_budget: "auto".into(), light: true, eye_games: false }
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
    if let Some(b) = flag(value(t, "light_game")) {
        s.light = b;
    }
    if let Some(b) = flag(value(t, "eye_in_games")) {
        s.eye_games = b;
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
        format!(
            "# Play settings: light game mode, eye tracking in games, compositor time budget (Pimax app)\npause_ai = {}\nlight_game = {}\neye_in_games = {}\ncomp_budget = \"{}\"\n",
            s.pause_ai, s.light, s.eye_games, s.comp_budget
        ),
    )
}

/// Light game mode for one start: `--light` / `--no-light` win over the saved switch (`light_game`, default on).
/// Both flags at once is an error.
pub fn resolve_light(light: bool, no_light: bool, saved: &Settings) -> Result<bool, String> {
    match (light, no_light) {
        (true, true) => Err("--light and --no-light together".into()),
        (true, false) => Ok(true),
        (false, true) => Ok(false),
        (false, false) => Ok(saved.light),
    }
}

/// Eye tracking for one Play start: `--eye` / `--no-eye` win over the saved switch (`eye_in_games`, default off).
pub fn resolve_eye(eye: bool, no_eye: bool, saved: &Settings) -> Result<bool, String> {
    match (eye, no_eye) {
        (true, true) => Err("--eye and --no-eye together".into()),
        (true, false) => Ok(true),
        (false, true) => Ok(false),
        (false, false) => Ok(saved.eye_games),
    }
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

/// What Play stopped (names for people): from the state file; empty = nothing.
pub fn paused(session_dir: &Path) -> Vec<String> {
    paused_from(&std::fs::read_to_string(state_file(session_dir)).unwrap_or_default())
}

/// [`paused`] from the state file's text (`key=what` lines; one name per kind).
pub fn paused_from(t: &str) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for l in t.lines() {
        let name = match l.split_once('=').map(|x| x.0) {
            Some("sunshine") => "Sunshine",
            Some("ai") => "local AI",
            Some("voice") => "voice",
            Some("desk") => "Work leftovers",
            _ => continue,
        };
        if !v.iter().any(|x| x == name) {
            v.push(name.to_string());
        }
    }
    v
}

/// "Stopped for Play: local AI, voice" (None when nothing was stopped).
pub fn paused_line(session_dir: &Path) -> Option<String> {
    let p = paused(session_dir);
    (!p.is_empty()).then(|| format!("Stopped for Play: {}", p.join(", ")))
}

/// Record what Play stopped before it started (`kind` = `ai`, `voice` or `desk`; `what` for the log). A restart of a
/// running session adds to the list; the list never loses entries until `vr stop`.
pub fn record_stopped(session_dir: &Path, entries: &[(&str, String)]) {
    if entries.is_empty() {
        return;
    }
    let mut keep: Vec<String> = std::fs::read_to_string(state_file(session_dir)).map(|x| x.lines().map(String::from).collect()).unwrap_or_default();
    for (k, w) in entries {
        let line = format!("{k}={}", w.replace('\n', " "));
        if !keep.contains(&line) {
            keep.push(line);
        }
    }
    let _ = std::fs::create_dir_all(session_dir);
    let _ = std::fs::write(state_file(session_dir), keep.join("\n") + "\n");
}

fn systemctl(t: &Tools, args: &[&str]) -> Option<bool> {
    let o = retry_busy(|| Command::new(&t.systemctl).arg("--user").args(args).stdin(Stdio::null()).output()).ok()?;
    Some(o.status.success())
}

/// The arguments that bring the local AI up at a Work start: `omarchyvr-ai up` starts every slot ai.toml asks for
/// (the action model, Lara, the speech server when the video memory fits).
pub fn up_args(cpu: bool) -> Vec<String> {
    let mut v = vec!["up".to_string(), "--wait".into(), "0".into()];
    if cpu {
        v.push("--cpu".into());
    }
    v
}

/// Work with voice: bring up what ai.toml asks for when the local AI has work (`omarchyvr-ai up --wait 0`, detached):
/// the local models, or Lara alone in front of an online brain. The voice listener tries the speech server again
/// after a pause, so it switches to it once it answers. Nothing to load: nothing.
pub fn local_ai_up(t: &Tools, cfg: &omarchyvr_config::ai::AiConfig) -> Option<String> {
    if !cfg.ai_servers_wanted() {
        return None;
    }
    let spawned = Command::new(&t.omarchyvr_ai).args(up_args(false)).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    Some(match spawned {
        Ok(_) => "local AI: starting what is missing (speech server included when it fits)".to_string(),
        Err(e) => format!("local AI: could not run omarchyvr-ai up: {e}"),
    })
}

/// At `vr stop`: nothing of Work is started again (the local AI and voice start with the next Work); only Sunshine
/// entries of an old state file are honoured (this version never stops Sunshine). The state file goes.
pub fn on_stop(t: &Tools, session_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(state_file(session_dir)).ok()?;
    let _ = std::fs::remove_file(state_file(session_dir));
    let mut back = Vec::new();
    let mut failed = Vec::new();
    let mut work = false;
    for l in text.lines() {
        match l.split_once('=') {
            Some(("sunshine", u)) if SUNSHINE_UNITS.contains(&u) => {
                if systemctl(t, &["start", u]) == Some(true) {
                    back.push("Sunshine")
                } else {
                    failed.push("Sunshine")
                }
            }
            Some(("ai" | "voice" | "desk", _)) => work = true,
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
    if work {
        out.push_str(&format!("{}the local AI and voice were stopped for Play; they start again with Work", if out.is_empty() { "" } else { "; " }));
    }
    (!out.is_empty()).then_some(out)
}

/// A monado-service or SteamVR's vrserver is alive (by process name): `restore` must not touch a session.
fn session_running(proc_root: &Path) -> bool {
    std::fs::read_dir(proc_root).into_iter().flatten().flatten().any(|e| {
        e.file_name().to_string_lossy().bytes().all(|b| b.is_ascii_digit())
            && std::fs::read_to_string(e.path().join("comm")).is_ok_and(|c| matches!(c.trim(), "monado-service" | "vrserver"))
    })
}

/// `omarchyvr vr gamemode [status|on|off|light on|off|eye on|off|budget auto|6|8|restore]`.
pub fn cli(what: &[String], home: &Path, session_dir: &Path) -> Result<(), String> {
    let mut s = load(home);
    match what.first().map(String::as_str).unwrap_or("status") {
        w @ ("on" | "off") => {
            // the old "Game mode" switch: kept for old callers, Play always frees the graphics card now
            s.pause_ai = w == "on";
            save(home, &s).map_err(|e| e.to_string())?;
            println!("Play always stops the local AI and voice (the graphics card is for the game); this switch has no effect any more");
        }
        w @ ("light" | "light-on" | "light-off") => {
            // `light on|off` (the app) or `light-on` / `light-off`
            let v = match (w, what.get(1).map(String::as_str)) {
                ("light-on", _) | ("light", Some("on")) => true,
                ("light-off", _) | ("light", Some("off")) => false,
                _ => return Err("light: on | off".into()),
            };
            s.light = v;
            save(home, &s).map_err(|e| e.to_string())?;
            println!("Light game mode: {} (next start)", if v { "on" } else { "off" });
        }
        "eye" => {
            let v = match what.get(1).map(String::as_str) {
                Some("on") => true,
                Some("off") => false,
                _ => return Err("eye: on | off".into()),
            };
            s.eye_games = v;
            save(home, &s).map_err(|e| e.to_string())?;
            println!("Eye tracking in games: {} (next Play)", if v { "on" } else { "off" });
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
                println!("A VR session is running: nothing changed");
                return Ok(());
            }
            if let Some(l) = on_stop(&t, session_dir) {
                println!("{l}");
            }
        }
        "status" => {
            println!("Play stops the local AI and voice: always");
            println!("Light game mode: {}", if s.light { "on" } else { "off" });
            println!("Eye tracking in games: {}", if s.eye_games { "on" } else { "off" });
            println!("Compositor time budget: {}", s.comp_budget);
            println!("{}", paused_line(session_dir).unwrap_or_else(|| "Nothing stopped for Play".into()));
        }
        w => return Err(format!("unknown choice {w} (status|light on|off|eye on|off|budget auto|6|8|restore)")),
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
    fn fakes(d: &Path) -> Tools {
        let _ = std::fs::create_dir_all(d.join("proc"));
        let t = Tools { systemctl: d.join("systemctl"), omarchyvr_ai: d.join("omarchyvr-ai"), proc_root: d.join("proc") };
        script(
            &t.systemctl,
            &format!(
                "echo \"$@\" >> {d}/calls\ncase \"$2\" in\n is-active) [ \"$4\" = app-dev.lizardbyte.app.Sunshine.service ] && [ -e {d}/sun-up ] ;;\n stop) rm -f {d}/sun-up ;;\n start) touch {d}/sun-up ;;\nesac",
                d = d.display()
            ),
        );
        script(&t.omarchyvr_ai, &format!("echo \"ai $@\" >> {d}/calls", d = d.display()));
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
    fn play_records_what_it_stopped_and_stop_starts_nothing_of_work_again() {
        let d = crate::vr::test_dir("gm-play");
        let t = fakes(&d);
        let sess = d.join("sess");
        record_stopped(&sess, &[("ai", "llama-server pid 5".into()), ("voice", "omarchyvr-voice pid 6".into())]);
        // a restart of the session adds, never loses
        record_stopped(&sess, &[("ai", "llama-server pid 5".into()), ("ai", "whisper-server pid 7".into())]);
        assert_eq!(paused(&sess), vec!["local AI".to_string(), "voice".to_string()]);
        assert_eq!(paused_line(&sess).as_deref(), Some("Stopped for Play: local AI, voice"));
        // an old state file that also lists Sunshine: it is started again, never stopped
        let mut st = std::fs::read_to_string(sess.join("gamemode-paused.txt")).unwrap();
        st.push_str("sunshine=app-dev.lizardbyte.app.Sunshine.service\n");
        std::fs::write(sess.join("gamemode-paused.txt"), st).unwrap();
        let l = on_stop(&t, &sess).unwrap();
        assert!(l.contains("started again: Sunshine") && l.contains("start again with Work"), "{l}");
        assert!(calls(&d).contains("--user start app-dev.lizardbyte.app.Sunshine.service"));
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(!calls(&d).contains("ai up"), "the local AI is not started again after Play: {}", calls(&d));
        assert!(paused(&sess).is_empty());
        assert_eq!(on_stop(&t, &sess), None, "nothing left");
    }

    #[test]
    fn nothing_recorded_nothing_said_and_sunshine_is_never_stopped() {
        let d = crate::vr::test_dir("gm-none");
        let t = fakes(&d);
        record_stopped(&d.join("s"), &[]);
        assert_eq!(on_stop(&t, &d.join("s")), None);
        assert!(!calls(&d).contains("stop"), "{}", calls(&d));
        assert!(d.join("sun-up").exists());
    }

    #[test]
    fn a_work_start_brings_the_ai_up_only_when_it_has_work() {
        let d = crate::vr::test_dir("gm-up");
        let t = fakes(&d);
        // the Claude CLI brain (the default): nothing to load
        let cli = omarchyvr_config::ai::AiConfig::parse("").unwrap();
        assert_eq!(local_ai_up(&t, &cli), None);
        // local models on
        let local = omarchyvr_config::ai::AiConfig::parse("voice_local_models = true\n").unwrap();
        assert!(local_ai_up(&t, &local).is_some());
        for _ in 0..100 {
            if calls(&d).contains("ai up --wait 0") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(calls(&d).contains("ai up --wait 0"), "{}", calls(&d));
    }

    #[test]
    fn light_and_eye_switches_default_save_and_flags_win() {
        let d = Settings::default();
        assert!(d.light);
        assert!(!d.eye_games, "eye tracking in games is off by default");
        assert!(!parse_settings("light_game = false\n").light);
        assert!(parse_settings("pause_ai = false\n").light, "an old file keeps the default");
        assert!(parse_settings("eye_in_games = true\n").eye_games);
        let h = std::env::temp_dir().join(format!("omarchyvr-light-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&h);
        save(&h, &Settings { light: false, ..d.clone() }).unwrap();
        assert!(!load(&h).light);
        cli(&["light".into(), "on".into()], &h, &h).unwrap();
        assert!(load(&h).light);
        assert!(cli(&["light".into(), "maybe".into()], &h, &h).is_err());
        cli(&["eye".into(), "on".into()], &h, &h).unwrap();
        assert!(load(&h).eye_games && load(&h).light, "one switch does not change the other");
        assert!(cli(&["eye".into()], &h, &h).is_err());
        assert_eq!(resolve_light(false, false, &d), Ok(true));
        assert_eq!(resolve_light(false, true, &d), Ok(false));
        assert_eq!(resolve_light(true, false, &Settings { light: false, ..d.clone() }), Ok(true));
        assert!(resolve_light(true, true, &d).is_err());
        assert_eq!(resolve_eye(false, false, &d), Ok(false), "saved: off");
        assert_eq!(resolve_eye(true, false, &d), Ok(true));
        assert_eq!(resolve_eye(false, true, &Settings { eye_games: true, ..d.clone() }), Ok(false));
        assert_eq!(resolve_eye(false, false, &Settings { eye_games: true, ..d.clone() }), Ok(true));
        assert!(resolve_eye(true, true, &d).is_err());
        let _ = std::fs::remove_dir_all(&h);
    }
}
