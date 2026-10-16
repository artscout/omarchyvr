//! `pimax record tracking start|stop|status`: one folder per take with everything needed to compare the
//! tracking with Windows. A leaf: it only reads (the camera broker, copies of log files) and never writes into
//! the session. Details: docs/RECORD.md.
//!
//! A take folder `~/.local/share/pimax/tracking-recordings/YYYYmmdd-HHMMSS-<scenario>/` holds
//! * `cameras.pxcam`: both cameras, every frame with its MCU timestamp (the format of `~/src/cam-data`), read from
//!   the camera broker (or the free camera when no session runs). Not recorded when the session holds the camera
//!   without a broker: the take says so in `take.txt`.
//! * the part of the session's `monado.log`, `ctrltrack.log` and `ctrl-imu-{left,right}.csv` written during the
//!   take, plus `session.txt`, `start-argv.txt`, and the performance stats at start and end.
//! * `take.txt`: scenario, times, what was and was not captured.
use pimax_components::components::proc::{ProcSpec, Procs, Signal, Spawner};
use pimax_components::util::Roots;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// The scenarios offered in the app (slug, label).
pub const SCENARIOS: [(&str, &str); 8] = [
    ("on-desk", "Still on desk"),
    ("worn-still", "Worn, sitting still"),
    ("look-around", "Look around"),
    ("walk-room", "Walk the room"),
    ("fast-turns", "Fast turns"),
    ("controllers-in-out", "Controllers in and out of view"),
    ("play-session", "Play session"),
    ("custom", "Custom"),
];
/// Durations offered in the app (seconds; 0 = until Stop).
pub const DURATIONS: [(u64, &str); 5] = [(30, "30 s"), (60, "1 min"), (120, "2 min"), (300, "5 min"), (0, "Until Stop")];
pub const CAM_COMM: &str = "pimax-cam";
const LOGS: [&str; 5] = ["monado.log", "ctrltrack.log", "ctrl-imu-left.csv", "ctrl-imu-right.csv", "head-imu.csv"];
/// While this file exists the session keeps writing its IMU logs past their first minutes (pimax-cam imulog.rs,
/// pimax-hands fuse.rs), so a take late in a session still gets the head and controller IMU.
const IMU_FLAG: &str = "pimax-imu-log.on";
const COPIES: [&str; 2] = ["session.txt", "start-argv.txt"];

/// Where takes are saved.
pub fn root(r: &Roots) -> PathBuf {
    r.home.join(".local/share/pimax/tracking-recordings")
}
fn state_path(r: &Roots) -> PathBuf {
    r.runtime.join("pimax-record-tracking.state")
}
/// Label of a scenario slug.
pub fn label(slug: &str) -> Option<&'static str> {
    SCENARIOS.iter().find(|s| s.0 == slug).map(|s| s.1)
}
/// `YYYYmmdd-HHMMSS` in local time.
pub fn stamp(unix: i64) -> String {
    // SAFETY: localtime_r writes into the tm we own.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let t = unix as libc::time_t;
    unsafe { libc::localtime_r(&t, &mut tm) };
    format!("{:04}{:02}{:02}-{:02}{:02}{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec)
}

/// The camera recorder binary: `$PIMAX_CAM_BIN`, next to this executable, else on the PATH.
pub fn cam_bin() -> PathBuf {
    if let Some(p) = std::env::var_os("PIMAX_CAM_BIN").filter(|v| !v.is_empty()) {
        return p.into();
    }
    if let Ok(exe) = std::env::current_exe() {
        let sib = exe.with_file_name("pimax-cam");
        if sib.exists() {
            return sib;
        }
    }
    "pimax-cam".into()
}

/// The saved state of the running (or last) take.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    pub dir: PathBuf,
    pub scenario: String,
    pub start_unix: i64,
    /// 0 = until Stop.
    pub seconds: u64,
    pub cam_pid: u32,
    pub finished: bool,
    /// Log sizes at the start (name, bytes).
    pub offsets: Vec<(String, u64)>,
}

impl State {
    fn save(&self, p: &Path) -> std::io::Result<()> {
        let mut s = format!(
            "dir={}\nscenario={}\nstart={}\nseconds={}\ncam_pid={}\nfinished={}\n",
            self.dir.display(),
            self.scenario,
            self.start_unix,
            self.seconds,
            self.cam_pid,
            self.finished as u8
        );
        for (n, o) in &self.offsets {
            s.push_str(&format!("off.{n}={o}\n"));
        }
        fs::write(p, s)
    }
    fn load(p: &Path) -> Option<State> {
        let t = fs::read_to_string(p).ok()?;
        let mut st = State::default();
        for l in t.lines() {
            let (k, v) = l.split_once('=')?;
            match k {
                "dir" => st.dir = v.into(),
                "scenario" => st.scenario = v.into(),
                "start" => st.start_unix = v.parse().ok()?,
                "seconds" => st.seconds = v.parse().ok()?,
                "cam_pid" => st.cam_pid = v.parse().ok()?,
                "finished" => st.finished = v == "1",
                _ => {
                    if let Some(n) = k.strip_prefix("off.") {
                        st.offsets.push((n.into(), v.parse().ok()?));
                    }
                }
            }
        }
        (!st.dir.as_os_str().is_empty()).then_some(st)
    }
}

/// What the UI shows.
#[derive(Debug, Clone, PartialEq)]
pub enum View {
    Idle { last: Option<(PathBuf, u64)> },
    Recording { dir: PathBuf, elapsed: u64, seconds: u64, bytes: u64 },
}

/// Total size of the files under `p`.
pub fn dir_size(p: &Path) -> u64 {
    let Ok(rd) = fs::read_dir(p) else { return 0 };
    rd.flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_size(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

/// Human size.
pub fn human(b: u64) -> String {
    if b >= 1 << 30 {
        format!("{:.1} GB", b as f64 / (1u64 << 30) as f64)
    } else if b >= 1 << 20 {
        format!("{:.0} MB", b as f64 / (1u64 << 20) as f64)
    } else {
        format!("{} kB", b / 1024)
    }
}

fn cam_alive(procs: &dyn Procs, pid: u32) -> bool {
    pid != 0 && procs.is_alive(pid) && procs.comm(pid).as_deref() == Some(CAM_COMM)
}

/// Start a take. `session`: a VR/desk session runs. Ok(one line).
pub fn start(r: &Roots, scenario: &str, seconds: u64, now: i64, session: bool, cam: &Path, sp: &dyn Spawner, procs: &dyn Procs) -> Result<String, String> {
    if label(scenario).is_none() {
        return Err(format!("unknown scenario {scenario}: one of {}", SCENARIOS.map(|s| s.0).join(", ")));
    }
    finish(r, procs, now);
    if let Some(s) = State::load(&state_path(r)).filter(|s| !s.finished) {
        return Err(format!("already recording to {}", s.dir.display()));
    }
    let dir = root(r).join(format!("{}-{scenario}", stamp(now)));
    fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let broker = r.runtime.join("pimax-cam-broker.sock");
    let (source, note) = if broker.exists() {
        (Some("broker"), "cameras: from the camera broker".to_string())
    } else if session {
        (None, "cameras NOT recorded: the session holds the camera and no camera broker runs (restart the session with 6DoF to get one)".to_string())
    } else {
        (Some(""), "cameras: read directly (no session)".to_string())
    };
    let mut cam_pid = 0;
    if let Some(src) = source {
        let mut args = vec!["-n".to_string(), "19".into(), cam.display().to_string(), "record".into(), dir.join("cameras.pxcam").display().to_string()];
        args.extend(["--seconds".into(), if seconds == 0 { "86400".to_string() } else { seconds.to_string() }]);
        if !src.is_empty() {
            args.extend(["--source".into(), src.into()]);
        }
        let spec = ProcSpec { name: "trackrec", program: "nice".into(), args, env: vec![], log: dir.join("cameras.log") };
        cam_pid = sp.spawn(&spec).map_err(|e| format!("cannot start {}: {e}", cam.display()))?;
    }
    let mut st = State { dir: dir.clone(), scenario: scenario.into(), start_unix: now, seconds, cam_pid, finished: false, offsets: vec![] };
    for n in LOGS {
        st.offsets.push((n.into(), fs::metadata(r.session_dir.join(n)).map(|m| m.len()).unwrap_or(0)));
    }
    let _ = fs::copy(r.perf_stats(), dir.join("perf-start.txt"));
    let _ = fs::write(
        dir.join("take.txt"),
        format!("scenario={scenario} ({})\nstarted={}\nplanned_seconds={}\nsession={}\n{note}\n", label(scenario).unwrap_or(""), stamp(now), seconds, if session { "running" } else { "none" }),
    );
    st.save(&state_path(r)).map_err(|e| format!("cannot save the state: {e}"))?;
    let _ = fs::write(r.runtime.join(IMU_FLAG), format!("{}\n", dir.display()));
    Ok(format!("recording tracking data to {} ({note})", dir.display()))
}

/// Copy what a log gained since `off` (csv: with its header line, whole lines only: the writers flush in blocks, so
/// the size at the start of a take can fall inside a line) into `dst`.
fn copy_tail(src: &Path, off: u64, dst: &Path, csv: bool) -> std::io::Result<u64> {
    let mut f = fs::File::open(src)?;
    let len = f.metadata()?.len();
    let start = if len < off { 0 } else { off }; // rotated or truncated: take it all
    let mut out = fs::File::create(dst)?;
    if !csv {
        f.seek(SeekFrom::Start(start))?;
        return std::io::copy(&mut f, &mut out);
    }
    if start > 0 {
        let mut head = Vec::new();
        let mut b = [0u8; 1];
        while head.len() < 512 && f.read(&mut b)? == 1 {
            head.push(b[0]);
            if b[0] == b'\n' {
                break;
            }
        }
        out.write_all(&head)?;
    }
    let mut body = Vec::new();
    f.seek(SeekFrom::Start(start.saturating_sub(1)))?;
    f.read_to_end(&mut body)?;
    // body[0] is the byte before the slice (when start > 0): a partial first line is skipped
    let from = if start == 0 {
        0
    } else if body.first() == Some(&b'\n') {
        1
    } else {
        body.iter().position(|&b| b == b'\n').map_or(body.len(), |i| i + 1)
    };
    // and a partial last line (still in the writer's buffer) is left out; NUL bytes (a file truncated while a
    // second writer kept its offset) are dropped
    let to = body.iter().rposition(|&b| b == b'\n').map_or(from, |i| i + 1).max(from);
    let body: Vec<u8> = body[from..to].iter().copied().filter(|&b| b != 0).collect();
    out.write_all(&body)?;
    Ok(body.len() as u64)
}

/// Collect the logs once the camera recorder has ended. Idempotent. Returns the finished state.
pub fn finish(r: &Roots, procs: &dyn Procs, now: i64) -> Option<State> {
    let sp = state_path(r);
    let mut st = State::load(&sp)?;
    if st.finished || cam_alive(procs, st.cam_pid) {
        return None;
    }
    let _ = fs::remove_file(r.runtime.join(IMU_FLAG));
    let mut got = vec![];
    for (n, off) in &st.offsets {
        let src = r.session_dir.join(n);
        if src.exists() && copy_tail(&src, *off, &st.dir.join(n), n.ends_with(".csv")).is_ok() {
            got.push(n.clone());
        }
    }
    for n in COPIES {
        if fs::copy(r.session_dir.join(n), st.dir.join(n)).is_ok() {
            got.push(n.into());
        }
    }
    let _ = fs::copy(r.perf_stats(), st.dir.join("perf-end.txt"));
    if let Ok(mut f) = fs::OpenOptions::new().append(true).open(st.dir.join("take.txt")) {
        let _ = writeln!(f, "ended={}\nseconds={}\nlogs: {}", stamp(now), now - st.start_unix, if got.is_empty() { "none (no session logs found)".into() } else { got.join(", ") });
    }
    st.finished = true;
    let _ = st.save(&sp);
    Some(st)
}

/// Stop the take: ask the camera recorder to end (it flushes its file), wait, then collect the logs.
pub fn stop(r: &Roots, procs: &dyn Procs, sleep: &dyn Fn(), now: i64) -> Result<String, String> {
    let st = State::load(&state_path(r)).ok_or("not recording")?;
    if !st.finished && cam_alive(procs, st.cam_pid) {
        let _ = procs.signal(st.cam_pid, Signal::Int);
        for _ in 0..100 {
            if !cam_alive(procs, st.cam_pid) {
                break;
            }
            sleep();
        }
        if cam_alive(procs, st.cam_pid) {
            return Err("the camera recorder is still writing; try again in a moment".into());
        }
    }
    finish(r, procs, now);
    let st = State::load(&state_path(r)).ok_or("not recording")?;
    Ok(format!("saved {} ({})", st.dir.display(), human(dir_size(&st.dir))))
}

/// The state for the UI and `status` (also finishes a take whose timer ran out).
pub fn view(r: &Roots, procs: &dyn Procs, now: i64) -> View {
    finish(r, procs, now);
    let Some(st) = State::load(&state_path(r)) else { return View::Idle { last: None } };
    if st.finished {
        View::Idle { last: Some((st.dir.clone(), dir_size(&st.dir))) }
    } else {
        View::Recording { bytes: dir_size(&st.dir), elapsed: (now - st.start_unix).max(0) as u64, seconds: st.seconds, dir: st.dir }
    }
}

/// One line for `status`.
pub fn status_line(v: &View) -> String {
    match v {
        View::Idle { last: None } => "not recording tracking data".into(),
        View::Idle { last: Some((d, b)) } => format!("not recording; last take {} ({})", d.display(), human(*b)),
        View::Recording { dir, elapsed, seconds, bytes } => {
            let of = if *seconds > 0 { format!(" of {seconds} s") } else { String::new() };
            format!("recording tracking data: {elapsed} s{of}, {} -> {}", human(*bytes), dir.display())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pimax_components::util::TempDir;
    use std::cell::RefCell;
    use std::io;

    struct Sp(RefCell<Vec<ProcSpec>>);
    impl Spawner for Sp {
        fn spawn(&self, s: &ProcSpec) -> io::Result<u32> {
            self.0.borrow_mut().push(s.clone());
            Ok(77)
        }
    }
    struct Pr(RefCell<bool>);
    impl Procs for Pr {
        fn is_alive(&self, _: u32) -> bool {
            *self.0.borrow()
        }
        fn comm(&self, _: u32) -> Option<String> {
            Some(CAM_COMM.into())
        }
        fn signal(&self, _: u32, _: Signal) -> io::Result<()> {
            *self.0.borrow_mut() = false;
            Ok(())
        }
        fn find_by_comm(&self, _: &str) -> Vec<u32> {
            vec![]
        }
        fn environ(&self, _: u32) -> Option<Vec<(String, String)>> {
            None
        }
        fn age_secs(&self, _: u32) -> Option<f64> {
            None
        }
    }

    fn setup() -> (TempDir, Roots) {
        let d = TempDir::new("track-rec");
        let r = Roots::under(d.path());
        fs::create_dir_all(&r.session_dir).unwrap();
        fs::create_dir_all(&r.runtime).unwrap();
        fs::write(r.session_dir.join("monado.log"), "old line\n").unwrap();
        fs::write(r.session_dir.join("ctrl-imu-left.csv"), "t,ax\n1,2\n").unwrap();
        (d, r)
    }

    #[test]
    fn a_take_copies_only_what_was_written_during_it() {
        let (_d, r) = setup();
        let sp = Sp(RefCell::new(vec![]));
        let pr = Pr(RefCell::new(true));
        fs::write(r.runtime.join("pimax-cam-broker.sock"), "").unwrap();
        let msg = start(&r, "walk-room", 0, 1_000_000, true, Path::new("/b/pimax-cam"), &sp, &pr).unwrap();
        assert!(msg.contains("walk-room") && msg.contains("broker"), "{msg}");
        let a = sp.0.borrow()[0].args.join(" ");
        assert!(a.contains("-n 19 /b/pimax-cam record") && a.contains("cameras.pxcam") && a.contains("--source broker") && a.contains("86400"), "{a}");
        assert!(matches!(view(&r, &pr, 1_000_010), View::Recording { elapsed: 10, .. }));
        assert!(start(&r, "on-desk", 0, 1_000_011, true, Path::new("c"), &sp, &pr).unwrap_err().contains("already"));
        // the session writes while we record
        let mut f = fs::OpenOptions::new().append(true).open(r.session_dir.join("monado.log")).unwrap();
        writeln!(f, "new line").unwrap();
        let mut f = fs::OpenOptions::new().append(true).open(r.session_dir.join("ctrl-imu-left.csv")).unwrap();
        writeln!(f, "3,4").unwrap();
        let out = stop(&r, &pr, &|| {}, 1_000_030).unwrap();
        assert!(out.starts_with("saved "), "{out}");
        let st = State::load(&state_path(&r)).unwrap();
        assert!(st.finished);
        assert_eq!(fs::read_to_string(st.dir.join("monado.log")).unwrap(), "new line\n");
        assert_eq!(fs::read_to_string(st.dir.join("ctrl-imu-left.csv")).unwrap(), "t,ax\n3,4\n");
        assert!(fs::read_to_string(st.dir.join("take.txt")).unwrap().contains("seconds=30"));
        assert!(matches!(view(&r, &pr, 1_000_040), View::Idle { last: Some(_) }));
    }

    #[test]
    fn csv_slices_hold_whole_lines_and_the_imu_flag_lives_as_long_as_the_take() {
        let (_d, r) = setup();
        let sp = Sp(RefCell::new(vec![]));
        let pr = Pr(RefCell::new(true));
        fs::write(r.runtime.join("pimax-cam-broker.sock"), "").unwrap();
        // the writer flushed half a line before the take started
        fs::write(r.session_dir.join("head-imu.csv"), "# pimax-head-imu v1\nI,1,0\nI,2,0\nI,3").unwrap();
        start(&r, "fast-turns", 0, 2_000_000, true, Path::new("c"), &sp, &pr).unwrap();
        assert!(r.runtime.join(IMU_FLAG).exists(), "the session keeps logging the IMU during the take");
        let mut f = fs::OpenOptions::new().append(true).open(r.session_dir.join("head-imu.csv")).unwrap();
        write!(f, ",5\nI,4,0\nI,5").unwrap(); // ... and stops inside a line
        stop(&r, &pr, &|| {}, 2_000_010).unwrap();
        let st = State::load(&state_path(&r)).unwrap();
        assert_eq!(fs::read_to_string(st.dir.join("head-imu.csv")).unwrap(), "# pimax-head-imu v1\nI,4,0\n");
        assert!(!r.runtime.join(IMU_FLAG).exists());
    }

    #[test]
    fn a_timed_take_finishes_itself_when_the_recorder_ends() {
        let (_d, r) = setup();
        let sp = Sp(RefCell::new(vec![]));
        let pr = Pr(RefCell::new(true));
        start(&r, "on-desk", 30, 5_000, false, Path::new("c"), &sp, &pr).unwrap();
        let a = sp.0.borrow()[0].args.join(" ");
        assert!(a.contains("--seconds 30") && !a.contains("--source"), "no session and no broker: read the camera directly: {a}");
        *pr.0.borrow_mut() = false; // the timer ran out
        assert!(matches!(view(&r, &pr, 5_031), View::Idle { last: Some(_) }));
        assert!(status_line(&view(&r, &pr, 5_031)).starts_with("not recording; last take"));
    }

    #[test]
    fn a_session_without_a_broker_never_touches_the_camera() {
        let (_d, r) = setup();
        let sp = Sp(RefCell::new(vec![]));
        let pr = Pr(RefCell::new(true));
        let msg = start(&r, "play-session", 60, 9_000, true, Path::new("c"), &sp, &pr).unwrap();
        assert!(sp.0.borrow().is_empty() && msg.contains("NOT recorded"), "{msg}");
        assert!(matches!(view(&r, &pr, 9_001), View::Idle { .. }), "no camera process: the logs are collected at once");
    }

    #[test]
    fn unknown_scenarios_are_refused_and_labels_exist() {
        let (_d, r) = setup();
        let sp = Sp(RefCell::new(vec![]));
        assert!(start(&r, "dance", 0, 1, false, Path::new("c"), &sp, &Pr(RefCell::new(false))).is_err());
        assert!(SCENARIOS.iter().all(|s| label(s.0) == Some(s.1)));
        assert_eq!(human(3 << 20), "3 MB");
    }
}
