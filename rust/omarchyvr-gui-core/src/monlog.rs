//! Live state from `monado.log` (and the `pimax-ctrltrack` log): 6DoF tracking (`slam:` / `6DoF` lines of the Pimax
//! camera code), eye gaze statistics, controllers, camera rate, and the warnings that matter for the LESSONS rules
//! (row 6: `built without -DPIMAX_CAM=ON`; row 3: the eye chip is held by another process).
//!
//! Lines are fed one at a time ([`MonadoLog::feed`]); [`LogFollower`] reads only the new bytes of a growing file and
//! starts over when the file is replaced (a new session renames the old log to `.prev`). The generic lines (gyro
//! calibration, clients, refresh rate) use the launcher's own parser, `omarchyvr_tools::vr::logs`.
use crate::machine::Machine;
use omarchyvr_tools::vr::logs::{parse_line, LogEvent};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// 6DoF tracker state (the names of `track_state_name` in pimax_camera.c).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrackState {
    /// No SLAM line seen (3DoF session, or not started yet).
    #[default]
    Off,
    /// Tracker created, not trusted yet (3DoF).
    Waiting,
    /// 6DoF tracking.
    Tracking,
    /// Lost: 3DoF, position held.
    Lost,
}

impl TrackState {
    /// Short label.
    pub fn label(self) -> &'static str {
        match self {
            TrackState::Off => "off",
            TrackState::Waiting => "waiting for the tracker (3DoF)",
            TrackState::Tracking => "6DoF tracking",
            TrackState::Lost => "lost (3DoF, position held)",
        }
    }
}

/// What the log says about 6DoF.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SixDofLog {
    /// `6DoF: SLAM tracker up` was seen.
    pub tracker_up: bool,
    /// Current state.
    pub state: TrackState,
    /// Times tracking was (re)acquired.
    pub acquisitions: u64,
    /// Times it was lost.
    pub losses: u64,
    /// Tracker resets.
    pub resets: u64,
    /// Reason of the last state change.
    pub reason: String,
    /// Camera frame sets fed to the tracker.
    pub frame_sets: u64,
    /// IMU samples fed.
    pub imu_samples: u64,
    /// Last correction applied to the output (mm, deg).
    pub correction: Option<(f64, f64)>,
    /// Requested but the build has no camera code (LESSONS row 6).
    pub no_camera_code: bool,
    /// Supervisor errors.
    pub errors: Vec<String>,
    /// The last few state-change lines, newest last.
    pub recent: Vec<String>,
    /// The drift governor's last hold: cm the tracker slid without movement (`6DoF drift:` line).
    pub drift_last_cm: Option<f64>,
    /// Seconds that slide took.
    pub drift_last_secs: Option<f64>,
    /// Session total of held drift in cm.
    pub drift_total_cm: Option<f64>,
}

/// Eye tracking in the runtime.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EyeLog {
    /// `PIMAX_EYE=1: eye gaze pose exposed`.
    pub enabled: bool,
    /// `session started` (true), `start failed` (false), not seen (None).
    pub session: Option<bool>,
    /// The eye chip was held by another process when Monado tried (`kernel driver 'usbfs' is bound ...`).
    pub chip_busy: bool,
    /// Last 5 s window.
    pub stats: Option<EyeStats>,
}

/// One `Pimax eye: state ...` window.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct EyeStats {
    /// Tracker state number.
    pub state: i32,
    /// Gaze queries by apps.
    pub queries: u32,
    /// Window length (s).
    pub secs: f64,
    /// Valid gaze answers.
    pub valid: u32,
    /// Invalid (eyes lost).
    pub invalid: u32,
    /// Stale.
    pub stale: u32,
    /// No sample at all.
    pub no_sample: u32,
    /// Samples per second from the chip.
    pub chip_hz: f64,
}

impl EyeStats {
    /// Share of valid answers in percent.
    pub fn valid_pct(&self) -> f64 {
        if self.queries == 0 {
            0.0
        } else {
            100.0 * f64::from(self.valid) / f64::from(self.queries)
        }
    }
}

/// Controllers and optical controller tracking.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ControllerLog {
    /// Controller slots that sent a first report.
    pub slots_seen: BTreeSet<u32>,
    /// Hand tracking left / right active.
    pub hands: [Option<bool>; 2],
    /// Optical 6DoF controller tracking is on (`PIMAX_CTRL_TRACK=1`).
    pub optical: bool,
    /// Optical fixes and losses.
    pub fixes: u64,
    /// Fixes lost.
    pub fix_lost: u64,
    /// Currently tracked optically.
    pub tracking: bool,
    /// Last fix line (position, LEDs, error).
    pub last_fix: Option<String>,
}

/// Camera pipeline.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CameraLog {
    /// Reads the shared broker.
    pub broker: bool,
    /// Auto exposure (HID 0x51) on.
    pub ae: bool,
    /// `camera pipeline up`.
    pub up: bool,
    /// Last rate (fps) and lost frames.
    pub fps: Option<f64>,
    /// Frames lost so far.
    pub lost: Option<u64>,
    /// Open failures (`camera: open`, busy device).
    pub errors: Vec<String>,
    /// Low-light range stage of the auto exposure (0 normal, 1 gain extended, 2 gain and exposure extended).
    pub ae_stage: u8,
    /// "Tracking light low": even the extended range leaves the cameras too dark (`PIMAX_AE_LIGHT_LOW`).
    pub light_low: bool,
}

/// The sentence the camera library shows in the headset and the app shows on the status page.
pub const LIGHT_LOW_TEXT: &str = "Tracking cameras see too little infrared light \u{2014} daylight or an IR lamp helps";

/// Everything parsed from one session log.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MonadoLog {
    /// Lines fed.
    pub lines: u64,
    /// 6DoF.
    pub sixdof: SixDofLog,
    /// Eye tracking.
    pub eye: EyeLog,
    /// Controllers.
    pub ctrl: ControllerLog,
    /// Camera.
    pub camera: CameraLog,
    /// Gyro calibration finished (or fell back to the cached bias).
    pub gyro_ready: bool,
    /// No measured gyro bias yet: tracking runs with a zero bias (nothing cached: a fresh machine) or a provisional
    /// one (not a real rest). Cleared by the first real rest ("gyro bias done" / "refined at rest").
    pub gyro_unmeasured: bool,
    /// Rate announced to the headset (`pimax_video_prepare(72) -> 0`).
    pub video_hz: Option<u32>,
    /// Clients connected minus disconnected.
    pub clients: i64,
    /// Application names of connected clients, in order.
    pub apps: Vec<String>,
    /// `Server exiting` seen.
    pub exited: bool,
    /// The last error lines.
    pub errors: Vec<String>,
}

/// The line the app shows while no gyro bias was measured on this PC yet.
pub const GYRO_REST_HINT: &str = "Put the headset still on the desk for 2 s to calibrate the gyro";

/// [`GYRO_REST_HINT`] while the log says tracking runs without a measured gyro bias (zero or provisional).
pub fn gyro_hint(m: &MonadoLog) -> Option<&'static str> {
    (m.gyro_ready && m.gyro_unmeasured).then_some(GYRO_REST_HINT)
}

fn push_capped(v: &mut Vec<String>, s: &str, cap: usize) {
    v.push(s.to_string());
    if v.len() > cap {
        let n = v.len() - cap;
        v.drain(..n);
    }
}

/// First unsigned number in `s`.
fn first_u64(s: &str) -> Option<u64> {
    s.split(|c: char| !c.is_ascii_digit()).find(|w| !w.is_empty())?.parse().ok()
}

/// Number right before `suffix` (`"5112 IMU samples"`, suffix `" IMU samples"`).
fn num_before(s: &str, suffix: &str) -> Option<u64> {
    let i = s.find(suffix)?;
    s[..i].rsplit(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

fn float_before(s: &str, suffix: &str) -> Option<f64> {
    let i = s.find(suffix)?;
    s[..i].trim_end().rsplit(' ').next()?.parse().ok()
}

fn after<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    s.find(key).map(|i| &s[i + key.len()..])
}

impl MonadoLog {
    /// Account one line (without the newline).
    pub fn feed(&mut self, line: &str) {
        self.lines += 1;
        let t = line.trim();
        if t.is_empty() {
            return;
        }
        if self.feed_sixdof(t) || self.feed_eye(t) || self.feed_ctrl(t) || self.feed_camera(t) {
            return;
        }
        if let Some(ev) = parse_line(t) {
            match ev {
                LogEvent::GyroBiasDone { .. } | LogEvent::GyroBiasRefined { .. } => {
                    self.gyro_ready = true;
                    self.gyro_unmeasured = false;
                }
                LogEvent::GyroBiasFallback { cached, .. } => {
                    self.gyro_ready = true;
                    self.gyro_unmeasured = !cached;
                }
                LogEvent::GyroBiasProvisional { .. } => {
                    self.gyro_ready = true;
                    self.gyro_unmeasured = true;
                }
                LogEvent::VideoPrepare { fps, rc: 0 } => self.video_hz = Some(fps),
                LogEvent::ClientConnected(_) => self.clients += 1,
                LogEvent::ClientDisconnected(_) => self.clients -= 1,
                LogEvent::ClientApp(n) => self.apps.push(n),
                LogEvent::ServerExiting => self.exited = true,
                LogEvent::Error(l) => push_capped(&mut self.errors, &l, 20),
                _ => {}
            }
        }
    }

    fn feed_sixdof(&mut self, t: &str) -> bool {
        let s = &mut self.sixdof;
        if t.contains("built without -DPIMAX_CAM=ON") {
            s.no_camera_code = true;
            push_capped(&mut s.recent, t, 8);
            return true;
        }
        if let Some(rest) = after(t, "] slam: ").or_else(|| t.strip_prefix("slam: ")) {
            // "N frame sets, N IMU samples (...), N untimed frames skipped; <state>; A acquired, L lost, <reason>"
            s.frame_sets = num_before(rest, " frame sets").unwrap_or(s.frame_sets);
            s.imu_samples = num_before(rest, " IMU samples").unwrap_or(s.imu_samples);
            let parts: Vec<&str> = rest.split("; ").collect();
            if parts.len() >= 3 {
                s.state = match parts[1] {
                    p if p.starts_with("6DoF tracking") => TrackState::Tracking,
                    p if p.starts_with("lost") => TrackState::Lost,
                    _ => TrackState::Waiting,
                };
                let tail = parts[2..].join("; ");
                s.acquisitions = num_before(&tail, " acquired").unwrap_or(s.acquisitions);
                s.losses = num_before(&tail, " lost").unwrap_or(s.losses);
                if let Some(r) = tail.splitn(3, ", ").nth(2) {
                    s.reason = r.to_string();
                }
            }
            return true;
        }
        if let Some(rest) = after(t, "6DoF output: ") {
            let corr = after(rest, "correction ").and_then(|c| {
                Some((float_before(c, " mm")?, float_before(c, " deg")?))
            });
            if corr.is_some() {
                s.correction = corr;
            }
            return true;
        }
        if let Some(rest) = after(t, "6DoF drift: ") {
            // "head tracking slid 12 cm without movement in 10 s; held (session total 150 cm)"
            if let Some(i) = rest.find("slid ") {
                let r = &rest[i + 5..];
                s.drift_last_cm = r.split_whitespace().next().and_then(|w| w.parse().ok()).or(s.drift_last_cm);
                s.drift_last_secs = float_before(r, " s;").or(s.drift_last_secs);
            }
            if let Some(tot) = after(rest, "session total ") {
                s.drift_total_cm = tot.split_whitespace().next().and_then(|w| w.parse().ok()).or(s.drift_total_cm);
            }
            push_capped(&mut s.recent, t, 8);
            return true;
        }
        if let Some(rest) = after(t, "6DoF supervisor: ") {
            push_capped(&mut s.errors, rest, 8);
            return true;
        }
        let Some(rest) = after(t, "6DoF: ") else {
            return false;
        };
        if rest.starts_with("SLAM tracker up") {
            s.tracker_up = true;
            if s.state == TrackState::Off {
                s.state = TrackState::Waiting;
            }
        } else if let Some(r) = rest.strip_prefix("lost (") {
            s.state = TrackState::Lost;
            s.losses += 1;
            s.reason = r.split(')').next().unwrap_or("").to_string();
        } else if let Some(r) = rest.strip_prefix("resetting the tracker (") {
            s.resets = after(r, "reset ").and_then(first_u64).unwrap_or(s.resets + 1);
            s.reason = r.split(';').next().unwrap_or("").to_string();
        } else if let Some(r) = rest.strip_prefix("tracker re-zeroed while still (") {
            // The headset lies still and the tracker drifted: reset with the output held. Not a loss.
            s.resets = after(r, "reset ").and_then(first_u64).unwrap_or(s.resets + 1);
            s.reason = "re-zeroed while still".to_string();
        } else if rest.starts_with("recentered") {
            // position re-anchored; state unchanged
        } else if let Some(i) = rest.find("(acquisition ") {
            s.state = TrackState::Tracking;
            s.acquisitions = first_u64(&rest[i..]).unwrap_or(s.acquisitions + 1);
            s.reason = rest[..i].trim().to_string();
        } else {
            return false;
        }
        push_capped(&mut s.recent, t, 8);
        true
    }

    fn feed_eye(&mut self, t: &str) -> bool {
        let e = &mut self.eye;
        if t.contains("PIMAX_EYE=1: eye gaze pose exposed") {
            e.enabled = true;
        } else if let Some(r) = after(t, "Pimax eye tracking: ") {
            if r.starts_with("session started") {
                e.session = Some(true);
            } else if r.starts_with("start failed") {
                e.session = Some(false);
            } else if r.starts_with("session stopped") {
                e.session = None;
            }
        } else if t.contains("is bound to the EyeChip interface") {
            e.chip_busy = true;
        } else if let Some(r) = after(t, "Pimax eye: state ") {
            // "0, 361 gaze queries in 5.0 s: 0 valid, 0 invalid (eyes lost), 0 stale, 361 no sample; chip samples 0/s"
            let state = r
                .split(',')
                .next()
                .and_then(|x| x.trim().parse::<i32>().ok())
                .unwrap_or(-1);
            e.stats = Some(EyeStats {
                state,
                queries: num_before(r, " gaze queries").unwrap_or(0) as u32,
                secs: after(r, " in ").and_then(|x| x.split_whitespace().next()?.parse().ok()).unwrap_or(0.0),
                valid: num_before(r, " valid").unwrap_or(0) as u32,
                invalid: num_before(r, " invalid").unwrap_or(0) as u32,
                stale: num_before(r, " stale").unwrap_or(0) as u32,
                no_sample: num_before(r, " no sample").unwrap_or(0) as u32,
                chip_hz: after(r, "chip samples ")
                    .and_then(|x| x.split('/').next()?.trim().parse().ok())
                    .unwrap_or(0.0),
            });
            if e.chip_busy && e.stats.is_some_and(|s| s.chip_hz > 0.0) {
                e.chip_busy = false;
            }
        } else {
            return false;
        }
        true
    }

    fn feed_ctrl(&mut self, t: &str) -> bool {
        let c = &mut self.ctrl;
        if let Some(r) = after(t, "first report (slot ") {
            if let Some(n) = first_u64(r) {
                c.slots_seen.insert(n as u32);
            }
        } else if let Some(r) = after(t, "hand tracking left: ") {
            c.hands[0] = Some(r.starts_with("active"));
        } else if let Some(r) = after(t, "hand tracking right: ") {
            c.hands[1] = Some(r.starts_with("active"));
        } else if t.contains("optical 6DoF on") {
            c.optical = true;
        } else if let Some(r) = after(t, "optical fix: ") {
            c.fixes += 1;
            c.tracking = true;
            c.last_fix = Some(r.to_string());
        } else if t.contains("optical fix lost") {
            c.fix_lost += 1;
            c.tracking = false;
        } else {
            return false;
        }
        true
    }

    fn feed_camera(&mut self, t: &str) -> bool {
        let c = &mut self.camera;
        if let Some(r) = after(t, "] camera: ").or_else(|| t.strip_prefix("camera: ")) {
            if r.starts_with("open") || r.contains("busy") || r.contains("failed") {
                push_capped(&mut c.errors, t, 6);
            } else {
                c.fps = r.split_whitespace().next().and_then(|x| x.parse().ok()).or(c.fps);
                c.lost = after(r, "lost ").and_then(first_u64).or(c.lost);
            }
        } else if let Some(r) = after(t, "camera source: ") {
            c.broker = r.starts_with("broker");
        } else if let Some(r) = after(t, "PIMAX_AE_LIGHT_LOW: ") {
            c.light_low = !r.starts_with("cleared");
        } else if let Some(r) = after(t, "PIMAX_AE_EXTENDED: ") {
            if let Some(n) = after(r, "range extended to stage ") {
                c.ae_stage = first_u64(n).unwrap_or(1).min(2) as u8;
            } else if r.starts_with("back to the normal") {
                c.ae_stage = 0;
                c.light_low = false;
            }
        } else if t.contains("auto-exposure ON") {
            c.ae = true;
        } else if t.contains("camera pipeline up") {
            c.up = true;
        } else if t.contains("init_all failed") || t.contains("Device or resource busy") {
            push_capped(&mut c.errors, t, 6);
        } else {
            return false;
        }
        true
    }
}

/// Reads a growing log incrementally.
#[derive(Debug, Clone, Default)]
pub struct LogFollower {
    /// The file.
    pub path: PathBuf,
    ino: u64,
    offset: u64,
    partial: Vec<u8>,
    /// Parsed state of the current file.
    pub log: MonadoLog,
}

/// At most this much of an existing log is read on the first look (a `--debug` log grows by ~150 MB per hour; the
/// counters in the `slam:` lines are cumulative, so the tail is enough).
pub const FIRST_READ_MAX: u64 = 4 << 20;
const CHUNK: usize = 1 << 20;

impl LogFollower {
    /// Follow `path`.
    pub fn new(path: &Path) -> Self {
        LogFollower {
            path: path.to_path_buf(),
            ..Default::default()
        }
    }

    /// Read what was appended since the last call; returns true when something changed. A new file (other inode,
    /// or shorter than what was read) resets the state.
    pub fn poll(&mut self, m: &dyn Machine) -> bool {
        let Some(meta) = m.meta(&self.path) else {
            if self.offset != 0 || self.log.lines != 0 {
                *self = LogFollower::new(&self.path);
                return true;
            }
            return false;
        };
        let mut changed = false;
        if meta.ino != self.ino || meta.len < self.offset {
            let path = self.path.clone();
            *self = LogFollower::new(&path);
            self.ino = meta.ino;
            if meta.len > FIRST_READ_MAX {
                self.offset = meta.len - FIRST_READ_MAX;
                // drop the partial first line
                self.partial.clear();
                if let Some(b) = m.read_range(&self.path, self.offset, 64 * 1024) {
                    if let Some(i) = b.iter().position(|c| *c == b'\n') {
                        self.offset += i as u64 + 1;
                    }
                }
            }
            changed = true;
        }
        while self.offset < meta.len {
            let Some(b) = m.read_range(&self.path, self.offset, CHUNK) else {
                break;
            };
            if b.is_empty() {
                break;
            }
            self.offset += b.len() as u64;
            self.partial.extend_from_slice(&b);
            let mut start = 0;
            while let Some(rel) = self.partial[start..].iter().position(|c| *c == b'\n') {
                let end = start + rel;
                let s = String::from_utf8_lossy(&self.partial[start..end]);
                self.log.feed(s.trim_end_matches('\r'));
                start = end + 1;
            }
            self.partial.drain(..start);
            changed = true;
        }
        changed
    }
}

/// The periodic stats line of `pimax-ctrltrack live` (`live: 125 LED frames, left 100 %, right 95 %, ...`).
pub fn ctrltrack_stats(log_text: &str) -> Option<String> {
    log_text
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with("live: "))
        .map(|l| l.trim().trim_start_matches("live: ").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::{FakeMachine, FileMeta};

    const FIXTURE: &str = include_str!("../tests/fixtures/monado-desk-6dof.log");

    fn parse(text: &str) -> MonadoLog {
        let mut m = MonadoLog::default();
        for l in text.lines() {
            m.feed(l);
        }
        m
    }

    #[test]
    fn real_desk_session_with_6dof_eye_and_controllers() {
        let m = parse(FIXTURE);
        let s = &m.sixdof;
        assert!(s.tracker_up);
        assert_eq!(s.state, TrackState::Tracking);
        assert_eq!(s.acquisitions, 1);
        assert_eq!(s.losses, 0);
        assert_eq!(s.frame_sets, 3764);
        assert_eq!(s.imu_samples, 150_624);
        assert_eq!(s.correction, Some((0.0, 0.0)));
        assert!(!s.no_camera_code);
        assert!(m.eye.enabled);
        assert_eq!(m.eye.session, Some(true));
        assert!(m.eye.chip_busy, "the fixture has the usbfs line and 0 chip samples");
        let st = m.eye.stats.unwrap();
        assert_eq!((st.queries, st.valid, st.no_sample), (360, 0, 360));
        assert_eq!(st.valid_pct(), 0.0);
        assert!(m.ctrl.optical);
        assert_eq!(m.ctrl.slots_seen.iter().copied().collect::<Vec<_>>(), vec![3]);
        assert_eq!(m.ctrl.hands, [Some(true), None]);
        assert!(m.camera.broker && m.camera.ae && m.camera.up);
        assert_eq!(m.camera.fps, Some(50.04));
        assert_eq!(m.camera.lost, Some(0));
        assert_eq!(m.video_hz, Some(72));
        assert!(m.gyro_ready);
        assert_eq!(m.clients, 3);
        assert_eq!(m.apps, vec!["omarchyvr-passthrough", "wayvr", "libmonado"]);
    }

    #[test]
    fn gyro_without_a_measured_bias_asks_for_a_rest() {
        let mut m = MonadoLog::default();
        m.feed(" WARN [bias_timeout] headset not still for 200 ms (0 restarts): tracking with the zero (no cache) gyro bias 0.000 0.000 0.000 dps; it is refined every time the headset rests for 2000 ms (put the headset still on the desk for 2 s to calibrate the gyro)");
        assert!(m.gyro_ready && m.gyro_unmeasured);
        assert_eq!(gyro_hint(&m), Some(GYRO_REST_HINT));
        m.feed(" WARN [bias_provisional] gyro bias provisional: 0.120 0.781 0.366 dps (gyro sd 0.210 0.190 0.230 dps: not a real rest, not cached); put the headset still on the desk for 2 s to calibrate the gyro");
        assert!(m.gyro_unmeasured, "a provisional bias is not a measured one");
        m.feed(" INFO [bias_finish] gyro bias refined at rest: 0.118 0.775 0.370 dps (moved 0.006; 1 rests this session, 2 windows rejected as not a real rest)");
        assert!(!m.gyro_unmeasured);
        assert_eq!(gyro_hint(&m), None);
        let mut c = MonadoLog::default();
        c.feed(" WARN [bias_timeout] headset not still for 200 ms (0 restarts): tracking with the cached gyro bias 0.087 0.684 0.474 dps; it is refined every time the headset rests for 2000 ms");
        assert!(c.gyro_ready && !c.gyro_unmeasured, "a cached bias needs no hint");
        assert_eq!(gyro_hint(&MonadoLog::default()), None, "not started: no hint");
    }

    #[test]
    fn drift_hold_line() {
        let line = "6DoF drift: head tracking slid 12 cm without movement in 10 s; held (session total 150 cm)";
        for l in [line.to_string(), format!(" INFO [slam_poll_locked] {line}")] {
            let mut m = MonadoLog::default();
            m.feed(&l);
            let s = &m.sixdof;
            assert_eq!((s.drift_last_cm, s.drift_last_secs, s.drift_total_cm), (Some(12.0), Some(10.0), Some(150.0)), "{l}");
            assert_eq!(s.recent.len(), 1);
        }
        let mut m = MonadoLog::default();
        assert_eq!(m.sixdof.drift_total_cm, None);
        m.feed("6DoF drift: head tracking slid 3 cm without movement in 10 s; held (session total 7 cm)");
        m.feed("6DoF drift: head tracking slid 9 cm without movement in 10 s; held (session total 16 cm)");
        assert_eq!((m.sixdof.drift_last_cm, m.sixdof.drift_total_cm), (Some(9.0), Some(16.0)));
    }

    #[test]
    fn lost_reset_and_reacquired() {
        let m = parse(concat!(
            " INFO [setup_slam] 6DoF: SLAM tracker up, head origin (0.049 -0.015 0.030) m in the IMU frame; 3DoF until it is trusted\n",
            " INFO [slam_poll_locked] 6DoF: tracking (acquisition 1)\n",
            " WARN [slam_poll_locked] 6DoF: lost (tracker drifting while still); 3DoF at the last position\n",
            " WARN [slam_poll_locked] 6DoF: resetting the tracker (tracker drifting while still; reset 1)\n",
        ));
        assert_eq!(m.sixdof.state, TrackState::Lost);
        assert_eq!(m.sixdof.losses, 1);
        assert_eq!(m.sixdof.resets, 1);
        assert_eq!(m.sixdof.reason, "tracker drifting while still");
        let mut m2 = m.clone();
        m2.feed(" INFO [slam_poll_locked] 6DoF: tracking (re-aligned) (acquisition 2)");
        assert_eq!(m2.sixdof.state, TrackState::Tracking);
        assert_eq!(m2.sixdof.acquisitions, 2);
        assert_eq!(m2.sixdof.recent.len(), 5);
        m2.feed(" INFO [update_stats] slam: 900 frame sets, 36000 IMU samples (0 held from the tracker in 0 camera gaps), 0 untimed frames skipped; lost (3DoF, position held); 2 acquired, 2 lost, tracker drifting while still");
        assert_eq!(m2.sixdof.state, TrackState::Lost);
        assert_eq!((m2.sixdof.acquisitions, m2.sixdof.losses), (2, 2));
        assert_eq!(m2.sixdof.reason, "tracker drifting while still");
    }

    #[test]
    fn still_rezero_is_a_reset_not_a_loss() {
        let m = parse(concat!(
            " INFO [slam_poll_locked] 6DoF: tracking (acquisition 1)\n",
            " INFO [slam_poll_locked] 6DoF: tracker re-zeroed while still (position held, no dropout; reset 3)\n",
            " INFO [update_stats] slam: 900 frame sets, 36000 IMU samples (0 held from the tracker in 0 camera gaps), 0 untimed frames skipped; 6DoF tracking (re-zeroing while still, position held); 1 acquired, 0 lost, re-zeroed while still\n",
        ));
        assert_eq!(m.sixdof.state, TrackState::Tracking);
        assert_eq!((m.sixdof.acquisitions, m.sixdof.losses, m.sixdof.resets), (1, 0, 3));
        assert_eq!(m.sixdof.reason, "re-zeroed while still");
    }

    #[test]
    fn low_light_lines_of_the_camera_auto_exposure() {
        let mut m = parse("PIMAX_AE_EXTENDED: armed, normal 4108 us / gain 215, extended up to 10410 us / gain 449 (gain first, exposure last)\n");
        assert_eq!((m.camera.ae_stage, m.camera.light_low), (0, false));
        m.feed("PIMAX_AE_EXTENDED: range extended to stage 1 (caps 4108 us / gain 449)");
        assert_eq!(m.camera.ae_stage, 1);
        m.feed("PIMAX_AE_EXTENDED: range extended to stage 2 (caps 10410 us / gain 449)");
        assert_eq!(m.camera.ae_stage, 2);
        m.feed(&format!("PIMAX_AE_LIGHT_LOW: {LIGHT_LOW_TEXT}"));
        assert!(m.camera.light_low);
        m.feed("PIMAX_AE_LIGHT_LOW: cleared, the cameras see enough light again");
        assert!(!m.camera.light_low);
        m.feed("PIMAX_AE_LIGHT_LOW: Tracking cameras see too little infrared light");
        m.feed("PIMAX_AE_EXTENDED: back to the normal range (caps 4108 us / gain 215)");
        assert_eq!((m.camera.ae_stage, m.camera.light_low), (0, false));
    }

    #[test]
    fn build_without_camera_code_is_flagged() {
        let m = parse(" WARN [pimax_camera_create] PIMAX_HANDS/PIMAX_CAM/PIMAX_SLAM set, but Monado was built without -DPIMAX_CAM=ON\n");
        assert!(m.sixdof.no_camera_code);
        assert_eq!(m.sixdof.state, TrackState::Off);
    }

    #[test]
    fn optical_controller_fixes() {
        let m = parse(concat!(
            " INFO [pimax_ctrl_create] Pimax controllers: optical 6DoF on (PIMAX_CTRL_TRACK=1): poses from /run/user/1000/pimax-ctrltrack.pose\n",
            " INFO [optical_pose] optical fix: head-frame position (-0.040 -0.106 -0.303) m, 15 LEDs on 3 cameras, 0.86 px\n",
            " INFO [optical_pose] optical fix lost: rest pose\n",
            " INFO [optical_pose] optical fix: head-frame position (0.117 -0.102 -0.323) m, 18 LEDs on 3 cameras, 0.96 px\n",
        ));
        assert!(m.ctrl.optical && m.ctrl.tracking);
        assert_eq!((m.ctrl.fixes, m.ctrl.fix_lost), (2, 1));
        assert!(m.ctrl.last_fix.as_deref().unwrap().contains("18 LEDs"));
        assert_eq!(
            ctrltrack_stats("x\nlive: 125 LED frames, left 100 %, right 95 %, prior tracks 244/250\n   left: (0 0 0) m\n").as_deref(),
            Some("125 LED frames, left 100 %, right 95 %, prior tracks 244/250")
        );
    }

    #[test]
    fn follower_reads_only_new_bytes_and_restarts_on_a_new_file() {
        let mut mm = FakeMachine::new();
        let p = "/tmp/omarchyvr-session/monado.log";
        let first = " INFO [slam_poll_locked] 6DoF: tracking (acquisition 1)\n INFO [x] partial";
        mm.file(p, first);
        mm.set_meta(p, FileMeta { len: first.len() as u64, ino: 1, ..Default::default() });
        let mut f = LogFollower::new(Path::new(p));
        assert!(f.poll(&mm));
        assert_eq!(f.log.lines, 1, "the unterminated line waits for its newline");
        assert_eq!(f.log.sixdof.state, TrackState::Tracking);
        // append
        let more = " INFO [x] partial\n WARN [slam_poll_locked] 6DoF: lost (no landmarks); 3DoF at the last position\n";
        let text = format!(" INFO [slam_poll_locked] 6DoF: tracking (acquisition 1)\n{more}");
        mm.file(p, &text);
        mm.set_meta(p, FileMeta { len: text.len() as u64, ino: 1, ..Default::default() });
        assert!(f.poll(&mm));
        assert_eq!(f.log.sixdof.state, TrackState::Lost);
        assert!(!f.poll(&mm), "nothing new");
        // a new session: new inode, short file
        mm.file(p, " INFO [x] Monado starting up\n");
        mm.set_meta(p, FileMeta { len: 29, ino: 2, ..Default::default() });
        assert!(f.poll(&mm));
        assert_eq!(f.log.sixdof.state, TrackState::Off);
        assert_eq!(f.log.lines, 1);
    }
}
