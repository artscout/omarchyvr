//! The Record card (Home): options, start/stop and the live state of `omarchyvr record` (crate `omarchyvr-record`).
//!
//! The options are the recorder's own `record.toml` (the CLI and voice read the same file); a click saves it. The
//! state is `record.status` in the session dir, polled about once a second while the card is drawn. Start and stop
//! block for seconds (they wait for the recorder), so they run as background tasks; the UI thread only reads the
//! [`RecordView`]. Demo mode shows a fixture and touches nothing; in dry-run mode nothing is started or written.
use crate::app::AppCore;
use omarchyvr_components::util::Roots;
use omarchyvr_record::settings::{settings_path, Eyes, Quality, Settings, FPS_CHOICES};
use omarchyvr_record::status::{status_path, Status};
pub use omarchyvr_record::tracking;
use std::time::{Duration, Instant};

/// How often the status file is read while the card is visible.
pub const POLL: Duration = Duration::from_secs(1);

/// Labels of the option rows, in the order of [`Settings`]' choices.
pub const EYES: [&str; 2] = ["Left eye", "Both eyes"];
/// FPS labels (index = [`FPS_CHOICES`]).
pub const FPS: [&str; 3] = ["30", "60", "72"];
/// Quality labels.
pub const QUALITY: [&str; 3] = ["Low", "Medium", "High"];

/// What the card shows for the recorder's state.
#[derive(Debug, Clone, PartialEq)]
pub enum Indicator {
    /// Not recording; the last saved file, when there is one.
    Idle { last: Option<String> },
    /// The recorder started and waits for the first frame.
    Starting,
    /// Recording: "m:ss" and the file name.
    Recording { clock: String, file: String },
    /// The file is being finished.
    Finishing { file: String },
    /// The recorder gave up (the note says why).
    Error { note: String },
}

/// `m:ss` of a number of seconds.
pub fn clock(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Is this recorder error the GPU safety gate refusing (the page then offers the System page)?
pub fn gate_blocked(note: &str) -> bool {
    note.starts_with(omarchyvr_record::run::GATE_BLOCK_PREFIX)
}

/// The base name of a path.
pub fn base_name(p: &str) -> String {
    p.rsplit('/').next().unwrap_or(p).to_string()
}

/// The indicator for a status; `alive` is whether the recorder process runs (a status file left by a recorder that
/// died counts as idle).
pub fn indicator(st: Option<&Status>, alive: bool) -> Indicator {
    let Some(st) = st else { return Indicator::Idle { last: None } };
    let last = || Some(st.file.clone()).filter(|f| !f.is_empty());
    if !alive && matches!(st.state.as_str(), "starting" | "recording" | "finalizing") {
        return Indicator::Idle { last: last() };
    }
    match st.state.as_str() {
        "recording" => Indicator::Recording { clock: clock(st.seconds()), file: base_name(&st.file) },
        "starting" => Indicator::Starting,
        "finalizing" => Indicator::Finishing { file: base_name(&st.file) },
        "error" => Indicator::Error { note: if st.note.is_empty() { "the recorder failed".into() } else { st.note.clone() } },
        _ => Indicator::Idle { last: last() },
    }
}

/// Is the recorder busy (the Start button is off, Stop shows)?
pub fn active(i: &Indicator) -> bool {
    matches!(i, Indicator::Starting | Indicator::Recording { .. } | Indicator::Finishing { .. })
}

/// Index of the current choice in the three option lists.
pub fn eyes_index(s: &Settings) -> usize {
    if s.eyes == Eyes::Left { 0 } else { 1 }
}
/// See [`eyes_index`].
pub fn fps_index(s: &Settings) -> usize {
    FPS_CHOICES.iter().position(|f| *f == s.fps).unwrap_or(0)
}
/// See [`eyes_index`].
pub fn quality_index(s: &Settings) -> usize {
    match s.quality {
        Quality::Low => 0,
        Quality::Medium => 1,
        Quality::High => 2,
    }
}

/// The option rows the card edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opt {
    /// Left / both.
    Eyes,
    /// 30 / 60 / 72.
    Fps,
    /// Low / medium / high.
    Quality,
    /// 0 = no sound, 1 = what you hear.
    Audio,
    /// 0 = off, 1 = your microphone mixed in.
    Mic,
}

/// Apply choice `i` of `opt` to `s` (false: out of range).
pub fn apply_choice(s: &mut Settings, opt: Opt, i: usize) -> bool {
    match opt {
        Opt::Eyes => s.eyes = [Eyes::Left, Eyes::Both].get(i).copied().unwrap_or(s.eyes),
        Opt::Fps => match FPS_CHOICES.get(i) {
            Some(f) => s.fps = *f,
            None => return false,
        },
        Opt::Quality => match [Quality::Low, Quality::Medium, Quality::High].get(i) {
            Some(q) => s.quality = *q,
            None => return false,
        },
        Opt::Audio => s.audio = i == 1,
        Opt::Mic => s.mic = i == 1,
    }
    i < match opt {
        Opt::Eyes | Opt::Audio | Opt::Mic => 2,
        _ => 3,
    }
}

/// The folder to open: the last file's directory, else the configured one (`~` expanded).
pub fn folder(last: Option<&str>, s: &Settings, home: &std::path::Path) -> std::path::PathBuf {
    match last.and_then(|f| std::path::Path::new(f).parent()).filter(|p| !p.as_os_str().is_empty()) {
        Some(p) => p.to_path_buf(),
        None => s.out_dir(home),
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// The card's state.
#[derive(Debug, Clone, Default)]
pub struct RecordView {
    /// `record.toml` as last read (None until the first poll).
    pub settings: Option<Settings>,
    /// `record.status` as last read.
    pub status: Option<Status>,
    /// The recorder process runs.
    pub alive: bool,
    /// What the running (or last) start/stop is: "Starting..." / "Stopping...".
    pub op: &'static str,
    /// The last start/stop outcome (Ok: the status line).
    pub result: Option<Result<String, String>>,
    /// When the status was read.
    pub at: Option<Instant>,
    /// Why the settings could not be read or saved.
    pub settings_error: Option<String>,
    /// The tracking-data take (scenario list index, duration list index, last read state, last outcome).
    pub track_scenario: usize,
    pub track_duration: usize,
    pub track: Option<tracking::View>,
    pub track_result: Option<Result<String, String>>,
    pub track_at: Option<Instant>,
}

impl RecordView {
    /// The settings to show (defaults until read).
    pub fn current(&self) -> Settings {
        self.settings.clone().unwrap_or_default()
    }
    /// What to draw for the state.
    pub fn indicator(&self) -> Indicator {
        indicator(self.status.as_ref(), self.alive)
    }
    /// The last saved file, while idle.
    pub fn last_file(&self) -> Option<String> {
        match self.indicator() {
            Indicator::Idle { last } => last,
            _ => None,
        }
    }
}

fn demo_status() -> Status {
    Status {
        state: "recording".into(),
        pid: 1,
        file: "/home/demo/Videos/pimax/pimax-2026-10-15_20-00-00.mp4".into(),
        eyes: "left".into(),
        fps: 60,
        quality: "medium".into(),
        encoder: "h264_nvenc".into(),
        width: 1920,
        height: 1080,
        frames: 60 * 83,
        ..Status::default()
    }
}

impl AppCore {
    /// Keep the card fresh (called while it is drawn): the status about once a second, read in the background.
    pub fn want_record(&mut self) {
        if self.demo {
            if self.record.settings.is_none() {
                self.record.settings = Some(Settings::default());
                self.record.status = Some(demo_status());
                self.record.alive = true;
            }
            return;
        }
        if self.record.at.is_some_and(|t| t.elapsed() < POLL) || self.task_busy("record-poll") {
            return;
        }
        self.record.at = Some(Instant::now());
        let need_settings = self.record.settings.is_none();
        self.task(Some("record-poll"), move || {
            let r = Roots::from_env();
            let st = Status::load(&status_path(&r.session_dir));
            let alive = omarchyvr_record::cli::running_pid(&r).is_some();
            let settings = need_settings.then(|| Settings::load(&settings_path(&r.config)));
            Box::new(move |c: &mut AppCore| {
                c.record.status = st;
                c.record.alive = alive;
                match settings {
                    Some(Ok(s)) => c.record.settings = Some(s),
                    Some(Err(e)) => {
                        c.record.settings = Some(Settings::default());
                        c.record.settings_error = Some(e.to_string());
                    }
                    None => {}
                }
            })
        });
    }

    /// Keep the tracking-take state fresh (about once a second while the card is drawn). A timed take that ran
    /// out is finished here, so the folder is complete when it shows "Saved".
    pub fn want_track(&mut self) {
        if self.demo {
            self.record.track.get_or_insert(tracking::View::Idle { last: Some((self.paths.home.join(".local/share/omarchyvr/tracking-recordings/20261015-200000-walk-room"), 412 << 20)) });
            return;
        }
        if self.record.track_at.is_some_and(|t| t.elapsed() < POLL) || self.task_busy("track-poll") {
            return;
        }
        self.record.track_at = Some(Instant::now());
        self.task(Some("track-poll"), move || {
            let r = Roots::from_env();
            let v = tracking::view(&r, &omarchyvr_components::components::proc::RealProcs::new(&r.proc_), unix_now());
            Box::new(move |c: &mut AppCore| c.record.track = Some(v))
        });
    }

    /// Start a take with the chosen scenario and duration (never touches the session).
    pub fn track_start(&mut self) {
        let sc = tracking::SCENARIOS[self.record.track_scenario.min(tracking::SCENARIOS.len() - 1)].0;
        let secs = tracking::DURATIONS[self.record.track_duration.min(tracking::DURATIONS.len() - 1)].0;
        self.track_op("track-op", move |r| omarchyvr_record::cli::tracking(r, &omarchyvr_record::cli::Track::Start { scenario: sc.into(), seconds: secs }));
    }

    /// Stop the take and collect the logs.
    pub fn track_stop(&mut self) {
        self.track_op("track-op", |r| omarchyvr_record::cli::tracking(r, &omarchyvr_record::cli::Track::Stop));
    }

    fn track_op(&mut self, key: &'static str, f: impl FnOnce(&Roots) -> Result<String, String> + Send + 'static) {
        if self.task_busy(key) {
            return;
        }
        if self.demo || crate::cmd::dry_run() {
            self.record.track_result = Some(Err(format!("{}: nothing was started", if self.demo { "demo" } else { "dry run" })));
            return;
        }
        self.record.track_result = None;
        self.task(Some(key), move || {
            let r = Roots::from_env();
            let res = f(&r);
            let v = tracking::view(&r, &omarchyvr_components::components::proc::RealProcs::new(&r.proc_), unix_now());
            Box::new(move |c: &mut AppCore| {
                c.record.track_result = Some(res);
                c.record.track = Some(v);
                c.record.track_at = None;
            })
        });
    }

    /// Open the folder of the last take (or the takes folder).
    pub fn track_open_folder(&mut self) {
        if self.demo || crate::cmd::dry_run() {
            return;
        }
        let dir = match &self.record.track {
            Some(tracking::View::Idle { last: Some((d, _)) }) | Some(tracking::View::Recording { dir: d, .. }) => d.clone(),
            _ => tracking::root(&Roots::from_env()),
        };
        std::thread::spawn(move || {
            let _ = std::process::Command::new("xdg-open").arg(&dir).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().map(|mut c| c.wait());
        });
    }

    /// A start or stop runs.
    pub fn record_busy(&self) -> bool {
        self.task_busy("record-op")
    }

    /// Change one option and save `record.toml` (the CLI and voice use the same defaults).
    pub fn record_set(&mut self, opt: Opt, i: usize) {
        let mut s = self.record.current();
        if !apply_choice(&mut s, opt, i) {
            return;
        }
        self.record.settings = Some(s.clone());
        if self.demo || crate::cmd::dry_run() {
            return;
        }
        self.task(Some("record-save"), move || {
            let r = Roots::from_env();
            let res = s.save(&settings_path(&r.config));
            Box::new(move |c: &mut AppCore| {
                c.record.settings_error = res.err().map(|e| format!("cannot save record.toml: {e}"));
            })
        });
    }

    /// Start recording with the saved options.
    pub fn record_start(&mut self) {
        self.record_op("Starting...", |r| omarchyvr_record::cli::start(r, &[], false));
    }

    /// Stop recording (waits for the file to be finished).
    pub fn record_stop(&mut self) {
        self.record_op("Stopping...", omarchyvr_record::cli::stop);
    }

    fn record_op(&mut self, op: &'static str, f: impl FnOnce(&Roots) -> Result<String, String> + Send + 'static) {
        if self.record_busy() {
            return;
        }
        if self.demo || crate::cmd::dry_run() {
            self.record.result = Some(Err(format!("{}: nothing was started", if self.demo { "demo" } else { "dry run" })));
            return;
        }
        self.record.op = op;
        self.record.result = None;
        self.task(Some("record-op"), move || {
            let r = Roots::from_env();
            let res = f(&r);
            let st = Status::load(&status_path(&r.session_dir));
            let alive = omarchyvr_record::cli::running_pid(&r).is_some();
            Box::new(move |c: &mut AppCore| {
                c.record.result = Some(res);
                c.record.status = st;
                c.record.alive = alive;
                c.record.op = "";
                c.record.at = None;
            })
        });
    }

    /// Open the folder of the last recording (or the output folder) with xdg-open, detached.
    pub fn record_open_folder(&mut self) {
        if self.demo || crate::cmd::dry_run() {
            return;
        }
        let dir = folder(self.record.last_file().as_deref(), &self.record.current(), &self.paths.home);
        std::thread::spawn(move || {
            let child = std::process::Command::new("xdg-open")
                .arg(&dir)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
            match child {
                Ok(mut c) => {
                    let _ = c.wait();
                }
                Err(e) => log::warn!("xdg-open {}: {e}", dir.display()),
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(state: &str, frames: u64) -> Status {
        Status { state: state.into(), file: "/v/pimax/a.mp4".into(), fps: 60, frames, ..Status::default() }
    }

    #[test]
    fn clock_text() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(83.9), "1:23");
        assert_eq!(clock(3600.0), "60:00");
        assert_eq!(clock(-4.0), "0:00");
    }

    #[test]
    fn indicator_follows_the_state() {
        assert_eq!(indicator(None, false), Indicator::Idle { last: None });
        assert_eq!(indicator(Some(&st("recording", 60 * 83)), true), Indicator::Recording { clock: "1:23".into(), file: "a.mp4".into() });
        assert_eq!(indicator(Some(&st("starting", 0)), true), Indicator::Starting);
        assert_eq!(indicator(Some(&st("finalizing", 10)), true), Indicator::Finishing { file: "a.mp4".into() });
        assert_eq!(indicator(Some(&st("idle", 600)), false), Indicator::Idle { last: Some("/v/pimax/a.mp4".into()) });
        let mut e = st("error", 0);
        e.note = "no ffmpeg".into();
        assert_eq!(indicator(Some(&e), false), Indicator::Error { note: "no ffmpeg".into() });
    }

    #[test]
    fn a_dead_recorder_is_not_recording() {
        assert_eq!(indicator(Some(&st("recording", 600)), false), Indicator::Idle { last: Some("/v/pimax/a.mp4".into()) });
        assert!(!active(&indicator(Some(&st("recording", 600)), false)));
        assert!(active(&indicator(Some(&st("recording", 600)), true)));
        assert!(!active(&Indicator::Error { note: "x".into() }));
    }

    #[test]
    fn choices_round_trip_through_the_settings() {
        let mut s = Settings::default();
        for (opt, i) in [(Opt::Eyes, 1), (Opt::Fps, 2), (Opt::Quality, 2), (Opt::Audio, 0), (Opt::Mic, 1)] {
            assert!(apply_choice(&mut s, opt, i));
        }
        assert_eq!((eyes_index(&s), fps_index(&s), quality_index(&s)), (1, 2, 2));
        assert_eq!((s.eyes, s.fps, s.quality), (Eyes::Both, 72, Quality::High));
        assert!(!s.audio && s.mic);
        assert!(!apply_choice(&mut s, Opt::Fps, 3));
        assert!(!apply_choice(&mut s, Opt::Eyes, 2));
        assert_eq!(s.fps, 72);
        // the file round trip (what record_set saves and the CLI reads)
        let dir = std::env::temp_dir().join(format!("pimax-gui-record-{}", std::process::id()));
        let path = dir.join("record.toml");
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap(), s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn folder_of_the_last_file_or_the_configured_one() {
        let s = Settings::default();
        let home = std::path::Path::new("/home/u");
        assert_eq!(folder(Some("/v/pimax/a.mp4"), &s, home), std::path::PathBuf::from("/v/pimax"));
        assert_eq!(folder(None, &s, home), s.out_dir(home));
    }

    #[test]
    fn demo_card_has_a_recording_and_changes_nothing_on_disk() {
        let mut a = AppCore::demo(true, omarchyvr_config::Store::new(std::env::temp_dir().join(format!("pimax-rec-{}-a", std::process::id())), std::env::temp_dir().join(format!("pimax-rec-{}-b", std::process::id()))));
        a.want_record();
        assert!(matches!(a.record.indicator(), Indicator::Recording { .. }));
        a.record_set(Opt::Fps, 2);
        assert_eq!(a.record.current().fps, 72);
        a.record_stop();
        assert!(a.record.result.as_ref().is_some_and(|r| r.is_err()));
    }
}
