//! `omarchyvr record start|stop|status` (also `omarchyvr-record ...`) and `omarchyvr-record run` (the recorder itself).
use crate::settings::{settings_path, Settings};
use crate::status::{self, Status};
use omarchyvr_components::components::proc::{self as pc, ClientStop, ProcSpec, RealClock, RealProcs, RealSpawner, StartError};
use omarchyvr_components::util::Roots;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const USAGE: &str = "omarchyvr record - record what the headset shows to ~/Videos/pimax/pimax-YYYYmmdd-HHMMSS.mp4

usage: omarchyvr record start [options] [--save]   start recording (the VR session must run)
       omarchyvr record stop                       stop and finish the file (always playable)
       omarchyvr record status                     what is being recorded
       omarchyvr record tracking start|stop|status [--scenario S] [--seconds N]
                                               cameras + logs of the session into one folder (for comparing with Windows);
                                               S: on-desk worn-still look-around walk-room fast-turns controllers-in-out play-session custom
options (defaults from ~/.config/omarchyvr/record.toml; --save writes the given ones there):
  --eyes left|both          one eye, lens-undistorted (default) or both side by side
  --fps 30|60|72            frame rate (default 30)
  --quality low|medium|high (default high)
  --scale F                 0 = fit --max-height (default); 0.1-1.0 = fraction of the eye's resolution
  --max-height N            per-eye height cap (default 1080)
  --codec h264|hevc         (default h264)
  --encoder auto|nvenc|cpu  auto = NVENC, else x264 on the CPU (default auto)
  --dir DIR                 output directory (default ~/Videos/pimax)
  --audio on|off            record what you hear into the video (default on)
  --mic on|off              also mix in your microphone (default off)
omarchyvr record start --brief / stop --brief: the saved settings, and one short line (\"Recording\", \"Saved: name.mp4\") for voice
Design and reliability notes: docs/RECORD.md";

/// The session's recorder pid file and comm.
pub const PID_NAME: &str = "record";
pub const COMM: &str = "omarchyvr-recor"; // the kernel cuts comm at 15 characters
/// How long `stop` waits for the file to be finished.
pub const STOP_GRACE: Duration = Duration::from_secs(25);

#[derive(Debug, Clone, PartialEq)]
pub enum Cmd {
    Start { overrides: Vec<(String, String)>, save: bool },
    Stop,
    Status,
    Run { overrides: Vec<(String, String)> },
    Help,
    /// `start --brief` / `stop --brief`: the saved settings, and one short line for voice ("Recording", "Saved: x.mp4").
    Brief { stop: bool },
    /// `tracking start [scenario] [seconds] | stop | status`.
    Tracking(Track),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Track {
    Start { scenario: String, seconds: u64 },
    Stop,
    Status,
}

fn parse_tracking(args: &[String]) -> Result<Cmd, String> {
    let sub = args.first().map(String::as_str).unwrap_or("status");
    let mut scenario = "custom".to_string();
    let mut seconds = 0u64;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--scenario" | "--seconds" => {
                let v = args.get(i + 1).ok_or_else(|| format!("{} needs a value", args[i]))?;
                if args[i] == "--seconds" {
                    seconds = v.parse().map_err(|_| format!("bad --seconds {v}"))?;
                } else {
                    crate::tracking::label(v).ok_or_else(|| format!("unknown scenario {v}"))?;
                    scenario = v.clone();
                }
                i += 1;
            }
            a => return Err(format!("unexpected argument {a}")),
        }
        i += 1;
    }
    match sub {
        "start" => Ok(Cmd::Tracking(Track::Start { scenario, seconds })),
        "stop" if i == 1 => Ok(Cmd::Tracking(Track::Stop)),
        "status" if i == 1 => Ok(Cmd::Tracking(Track::Status)),
        other => Err(format!("unknown tracking command {other}")),
    }
}

/// Run a tracking command with the real process table.
pub fn tracking(r: &Roots, t: &Track) -> Result<String, String> {
    use crate::tracking as tr;
    let procs = RealProcs::new(&r.proc_);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    match t {
        Track::Start { scenario, seconds } => tr::start(r, scenario, *seconds, now, crate::run::monado_running(r), &tr::cam_bin(), &RealSpawner, &procs),
        Track::Stop => tr::stop(r, &procs, &|| std::thread::sleep(Duration::from_millis(100)), now),
        Track::Status => Ok(tr::status_line(&tr::view(r, &procs, now))),
    }
}

/// Parse `start|stop|status|run [--key value ...]`.
pub fn parse(args: &[String]) -> Result<Cmd, String> {
    let Some(first) = args.first() else { return Ok(Cmd::Help) };
    if first == "tracking" {
        return parse_tracking(&args[1..]);
    }
    if args.len() == 2 && args[1] == "--brief" && matches!(first.as_str(), "start" | "stop") {
        return Ok(Cmd::Brief { stop: first == "stop" });
    }
    let mut overrides = Vec::new();
    let mut save = false;
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--save" => save = true,
            "-h" | "--help" => return Ok(Cmd::Help),
            _ if a.starts_with("--") => {
                let (k, v) = match a[2..].split_once('=') {
                    Some((k, v)) => (k.to_string(), v.to_string()),
                    None => {
                        i += 1;
                        let v = args.get(i).ok_or_else(|| format!("{a} needs a value"))?;
                        (a[2..].to_string(), v.clone())
                    }
                };
                // validate now (a typo must fail here, not in the detached recorder)
                Settings::default().set(&k, &v).map_err(|e| e.0)?;
                overrides.push((k, v));
            }
            _ => return Err(format!("unexpected argument {a}")),
        }
        i += 1;
    }
    Ok(match first.as_str() {
        "start" => Cmd::Start { overrides, save },
        "stop" if overrides.is_empty() => Cmd::Stop,
        "status" if overrides.is_empty() => Cmd::Status,
        "run" => Cmd::Run { overrides },
        "help" | "-h" | "--help" => Cmd::Help,
        "stop" | "status" => return Err(format!("{first} takes no options")),
        other => return Err(format!("unknown record command {other}")),
    })
}

/// The file settings with the flags on top.
pub fn effective(config_dir: &Path, overrides: &[(String, String)]) -> Result<Settings, String> {
    let mut s = Settings::load(&settings_path(config_dir)).map_err(|e| e.0)?;
    for (k, v) in overrides {
        s.set(k, v).map_err(|e| e.0)?;
    }
    Ok(s)
}

/// The recorder binary: `$PIMAX_RECORD_BIN`, else next to this executable, else `omarchyvr-record` on the PATH.
pub fn recorder_bin() -> PathBuf {
    if let Some(p) = std::env::var_os("PIMAX_RECORD_BIN").filter(|v| !v.is_empty()) {
        return p.into();
    }
    if let Ok(exe) = std::env::current_exe() {
        let sib = exe.with_file_name("omarchyvr-record");
        if sib.exists() {
            return sib;
        }
    }
    "omarchyvr-record".into()
}

pub fn pid_file(session_dir: &Path) -> PathBuf {
    session_dir.join(format!("{PID_NAME}.pid"))
}

/// The running recorder's pid (pid file + comm), if any.
pub fn running_pid(r: &Roots) -> Option<u32> {
    let pid = pc::read_pid(&pid_file(&r.session_dir))?;
    let procs = RealProcs::new(&r.proc_);
    use pc::Procs;
    (procs.is_alive(pid) && procs.comm(pid).as_deref() == Some(COMM)).then_some(pid)
}

/// The spawn spec of the recorder (also what omarchyvrd respawns: the argv carries the settings).
pub fn spec(bin: &Path, s: &Settings, session_dir: &Path) -> ProcSpec {
    let mut args = vec!["run".to_string()];
    args.extend(s.to_args());
    ProcSpec { name: PID_NAME, program: bin.to_path_buf(), args, env: Vec::new(), log: session_dir.join("record.log") }
}

/// `start`: Ok(the status line) once recording, Err(why) otherwise.
pub fn start(r: &Roots, overrides: &[(String, String)], save: bool) -> Result<String, String> {
    let s = effective(&r.config, overrides)?;
    if save {
        s.save(&settings_path(&r.config)).map_err(|e| format!("cannot save the settings: {e}"))?;
    }
    if let Some(pid) = running_pid(r) {
        let st = Status::load(&status::status_path(&r.session_dir)).unwrap_or_default();
        return Err(format!("already recording (pid {pid}): {}", st.summary()));
    }
    if !crate::run::monado_running(r) {
        return Err("no VR session: start one first (omarchyvr desk start)".into());
    }
    let sp = spec(&recorder_bin(), &s, &r.session_dir);
    let procs = RealProcs::new(&r.proc_);
    let clock = RealClock::new();
    let t0 = Instant::now();
    let pid = match pc::start_client(&sp, &r.session_dir, &RealSpawner, &procs, &clock, Duration::from_millis(300)) {
        Ok(p) => p,
        Err(StartError::Spawn(e)) => return Err(format!("cannot start {}: {e}", sp.program.display())),
        Err(StartError::DiedEarly { tail, .. }) => {
            let st = Status::load(&status::status_path(&r.session_dir)).unwrap_or_default();
            return Err(if st.state == "error" { st.summary() } else { format!("the recorder exited at once: {}", tail.join(" | ")) });
        }
    };
    // wait until it records (the tap attaches within about 1 s) or gives up
    let sp_path = status::status_path(&r.session_dir);
    use pc::Procs;
    while t0.elapsed() < Duration::from_secs(10) {
        let st = Status::load(&sp_path).unwrap_or_default();
        if st.pid == pid && st.state == "recording" {
            return Ok(st.summary());
        }
        if !procs.is_alive(pid) {
            let _ = std::fs::remove_file(pid_file(&r.session_dir));
            return Err(if st.pid == pid { st.summary() } else { "the recorder exited".into() });
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(format!("started (pid {pid}), still waiting for the first frame: omarchyvr record status"))
}

/// `stop`: SIGINT by pid, wait for the file to be finished. Ok(the last status line).
pub fn stop(r: &Roots) -> Result<String, String> {
    let procs = RealProcs::new(&r.proc_);
    let clock = RealClock::new();
    let pf = pid_file(&r.session_dir);
    match pc::stop_client("record", &pf, COMM, &procs, &clock, STOP_GRACE, &mut |_| {}) {
        ClientStop::NotRunning => {
            let _ = std::fs::remove_file(&pf);
            Ok(format!("not recording ({})", Status::load(&status::status_path(&r.session_dir)).map(|s| s.summary()).unwrap_or_default()))
        }
        ClientStop::Exited => {
            let _ = std::fs::remove_file(&pf);
            Ok(Status::load(&status::status_path(&r.session_dir)).map(|s| s.summary()).unwrap_or_else(|| "stopped".into()))
        }
        ClientStop::StillAlive => Err(format!("the recorder is still finishing after {} s (it never blocks the session; omarchyvr record status)", STOP_GRACE.as_secs())),
        ClientStop::CannotSignal(m) => Err(m),
    }
}

/// `stop --brief`: "Saved: pimax-20261016-120000.mp4", or why nothing was saved, in plain words.
pub fn stop_brief(r: &Roots) -> Result<String, String> {
    if running_pid(r).is_none() {
        return Ok("Not recording".into());
    }
    stop(r)?;
    let st = Status::load(&status::status_path(&r.session_dir)).unwrap_or_default();
    Ok(match (st.file.is_empty(), st.state.as_str()) {
        (false, _) => format!("Saved: {}", st.file.rsplit('/').next().unwrap_or(&st.file)),
        (true, "error") => format!("Not saved: {}", st.note),
        (true, _) => "Nothing was recorded".into(),
    })
}

/// `status`: one line.
pub fn status_line(r: &Roots) -> String {
    let st = Status::load(&status::status_path(&r.session_dir)).unwrap_or_default();
    match running_pid(r) {
        Some(_) => st.summary(),
        None if matches!(st.state.as_str(), "recording" | "starting" | "finalizing") => {
            format!("not recording (the recorder exited without finishing its status; last file: {})", if st.file.is_empty() { "none" } else { &st.file })
        }
        None => st.summary(),
    }
}

/// Run a command; returns the process exit code (2 for usage errors, 1 for failures).
pub fn main(args: &[String]) -> i32 {
    let r = Roots::from_env();
    let cmd = match parse(args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("omarchyvr record: {e}\n\n{USAGE}");
            return 2;
        }
    };
    let res = match cmd {
        Cmd::Help => {
            println!("{USAGE}");
            return 0;
        }
        Cmd::Run { overrides } => {
            return match effective(&r.config, &overrides) {
                Ok(s) => crate::run::run(&s, &r),
                Err(e) => {
                    eprintln!("omarchyvr-record: {e}");
                    2
                }
            }
        }
        Cmd::Start { overrides, save } => start(&r, &overrides, save),
        Cmd::Tracking(t) => tracking(&r, &t),
        Cmd::Stop => stop(&r),
        Cmd::Brief { stop: false } => start(&r, &[], false).map(|_| "Recording".to_string()),
        Cmd::Brief { stop: true } => stop_brief(&r),
        Cmd::Status => Ok(status_line(&r)),
    };
    match res {
        Ok(line) => {
            println!("{line}");
            0
        }
        Err(e) => {
            eprintln!("omarchyvr record: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_the_commands() {
        assert_eq!(parse(&a(&[])).unwrap(), Cmd::Help);
        assert_eq!(parse(&a(&["stop"])).unwrap(), Cmd::Stop);
        assert_eq!(parse(&a(&["status"])).unwrap(), Cmd::Status);
        assert_eq!(
            parse(&a(&["start", "--eyes", "both", "--fps=60", "--quality", "high", "--save"])).unwrap(),
            Cmd::Start { overrides: vec![("eyes".into(), "both".into()), ("fps".into(), "60".into()), ("quality".into(), "high".into())], save: true }
        );
        assert!(matches!(parse(&a(&["run", "--max-height", "720"])).unwrap(), Cmd::Run { .. }));
    }

    #[test]
    fn rejects_bad_input_before_spawning_anything() {
        assert!(parse(&a(&["start", "--fps", "45"])).is_err());
        assert!(parse(&a(&["start", "--eyes"])).is_err());
        assert!(parse(&a(&["start", "--colour", "red"])).is_err());
        assert!(parse(&a(&["start", "now"])).is_err());
        assert!(parse(&a(&["stop", "--fps", "30"])).is_err());
        assert!(parse(&a(&["record"])).is_err());
    }

    #[test]
    fn flags_override_the_file() {
        let d = omarchyvr_components::util::TempDir::new("record-cli-eff");
        std::fs::write(settings_path(d.path()), "eyes = \"both\"\nfps = 60\n").unwrap();
        let s = effective(d.path(), &[("fps".into(), "72".into())]).unwrap();
        assert_eq!((s.eyes, s.fps), (crate::settings::Eyes::Both, 72));
    }

    #[test]
    fn the_spawn_spec_carries_the_settings_for_a_respawn() {
        let s = Settings { fps: 60, ..Settings::default() };
        let sp = spec(Path::new("/b/omarchyvr-record"), &s, Path::new("/tmp/sess"));
        assert_eq!(sp.name, "record");
        assert_eq!(sp.args[0], "run");
        assert!(sp.args.windows(2).any(|w| w == ["--fps", "60"]));
        assert_eq!(sp.log, PathBuf::from("/tmp/sess/record.log"));
        assert_eq!(pc::comm_of(&sp.program), COMM);
        // the whole argv parses back as a run with the same settings
        let Cmd::Run { overrides } = parse(&sp.args).unwrap() else { panic!() };
        let mut t = Settings::default();
        for (k, v) in overrides {
            t.set(&k, &v).unwrap();
        }
        assert_eq!(t, s);
    }

    #[test]
    fn brief_commands_are_start_and_stop_with_the_saved_settings() {
        assert_eq!(parse(&a(&["start", "--brief"])).unwrap(), Cmd::Brief { stop: false });
        assert_eq!(parse(&a(&["stop", "--brief"])).unwrap(), Cmd::Brief { stop: true });
        assert!(parse(&a(&["status", "--brief"])).is_err());
    }

    #[test]
    fn audio_flags_reach_the_recorder() {
        let c = parse(&a(&["start", "--audio", "off", "--mic=on"])).unwrap();
        assert_eq!(c, Cmd::Start { overrides: vec![("audio".into(), "off".into()), ("mic".into(), "on".into())], save: false });
        assert!(parse(&a(&["start", "--audio", "maybe"])).is_err());
    }

    #[test]
    fn status_without_a_recorder_says_so() {
        let d = omarchyvr_components::util::TempDir::new("record-cli-status");
        let r = Roots::under(d.path());
        std::fs::create_dir_all(&r.session_dir).unwrap();
        assert_eq!(status_line(&r), "not recording");
        Status { state: "recording".into(), file: "/v/a.mp4".into(), pid: 999_999, ..Status::default() }.save(&status::status_path(&r.session_dir));
        assert!(status_line(&r).contains("exited without finishing"), "{}", status_line(&r));
        assert!(stop(&r).unwrap().starts_with("not recording"));
    }
}
