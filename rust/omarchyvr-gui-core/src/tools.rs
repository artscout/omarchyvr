//! The Tools page: what each tool needs before it may start, and how it is started.
//!
//! * Eye view and IPD (`pimax-eye watch`, `pimax-eye ipd --watch`) need the eye chip: greyed out while desk mode runs
//!   with eye tracking (LESSONS row 3), with "restart desk mode without eye tracking" offered.
//! * Eye calibration and lens patterns are main OpenXR apps (row 4): they get their own Monado, started with
//!   `omarchyvr vr start` (the launcher's env builder, row 5) after any previous Monado has exited (row 1), and are stopped
//!   with `omarchyvr vr stop --grace 30`.
//! * The 6DoF walk test starts `omarchyvr desk start --6dof` (broker, Monado with SLAM, passthrough so you see where you
//!   walk), records the walk for replay (`PIMAX_SLAM_RECORD`), shows the 6DoF state live and is stopped like any
//!   session. `analysis/tools/run_slam_view.sh` does the same by hand, but starts Monado with a copied environment and
//!   stops it with `pkill -x`, so the app only shows that command to copy (rows 1 and 5).
//! * Controller recording follows analysis/controller-6dof.md section 6: camera free (row 2), no Monado (row 1), disk
//!   space (row 15); broker first, broker last, everything by PID.
//! * The hands test is a headless OpenXR client against the running session (it cannot take the display).
//! * A session start passes `--force` only with the user's OK, only when the WayVR Xid 31 faults are the only problem,
//!   and re-checks that right before the launcher runs ([`crate::guards::force_still_justified`]).
//! * TUI tools open in a terminal; device tools wait for the headset lock like every documented device command.
use crate::cmd::{quote, Cmd};
use crate::facts::{gib, Facts, Live};
use crate::guards;
use crate::job::{JobKind, JobSpec, Step};
use crate::kernel::Verdict;
use crate::launcher::{self, DeskToggles};
use crate::paths::Paths;
use crate::preflight::{launcher_side, Action};
use std::time::Duration;

/// The tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolId {
    /// `pimax-eye watch`.
    EyeView,
    /// `pimax-eye ipd --watch`.
    Ipd,
    /// `pimax-eye power-on`.
    EyePowerOn,
    /// `pimax-eyecal` in its own session.
    EyeCalibration,
    /// `pimax-lenstest` in its own session.
    LensTest,
    /// Desk mode with 6DoF, recorded, with the live 6DoF state.
    WalkTest,
    /// Camera + controller report recording.
    CtrlRecording,
    /// `xr-handtest` against the running session.
    HandsTest,
    /// `omarchyvr-voice`.
    VoiceTest,
    /// `vr_runtime_switch.sh` + launch options.
    SteamRuntime,
}

impl ToolId {
    /// All tools in page order.
    pub const ALL: [ToolId; 10] = [
        ToolId::EyeView,
        ToolId::Ipd,
        ToolId::EyePowerOn,
        ToolId::EyeCalibration,
        ToolId::LensTest,
        ToolId::WalkTest,
        ToolId::CtrlRecording,
        ToolId::HandsTest,
        ToolId::VoiceTest,
        ToolId::SteamRuntime,
    ];

    /// Title.
    pub fn title(self) -> &'static str {
        match self {
            ToolId::EyeView => "Eye view",
            ToolId::Ipd => "IPD (measure and set)",
            ToolId::EyePowerOn => "Power on eye module",
            ToolId::EyeCalibration => "Eye calibration",
            ToolId::LensTest => "Lens test patterns",
            ToolId::WalkTest => "6DoF walk test",
            ToolId::CtrlRecording => "Controller recording",
            ToolId::HandsTest => "Hands test",
            ToolId::VoiceTest => "Voice test",
            ToolId::SteamRuntime => "Steam runtime",
        }
    }

    /// Section.
    pub fn section(self) -> &'static str {
        match self {
            ToolId::EyeView | ToolId::Ipd | ToolId::EyePowerOn | ToolId::EyeCalibration => "Eye tracking",
            ToolId::LensTest => "Display and lens",
            ToolId::WalkTest | ToolId::CtrlRecording | ToolId::HandsTest => "Tracking",
            ToolId::VoiceTest => "Voice",
            ToolId::SteamRuntime => "Games",
        }
    }

    /// What it does.
    pub fn about(self) -> &'static str {
        match self {
            ToolId::EyeView => "Full-screen live view of both pupils and the BAD flags (pimax-eye watch), in a terminal. Needs the eye chip: not while desk mode runs with eye tracking.",
            ToolId::Ipd => "Measures the IPD from the eye tracker; 'a' sets the lens motor once to the measured value (approved command, 58-72 mm), +/- adjust, q quits and writes ~/.config/omarchyvr/ipd.env (passed to Monado on the next desk start).",
            ToolId::EyePowerOn => "After a headset power cycle the eye module stays off: sends the captured enable once (approved) and waits for 2104:0561.",
            ToolId::EyeCalibration => "pimax-eyecal in the headset: keep the head still, follow the dots with the eyes; Y saves ~/.config/omarchyvr/eye-calibration.toml. Starts its own VR session (eye build, PIMAX_EYE=1); stop it here afterwards.",
            ToolId::LensTest => "pimax-lenstest patterns in the headset (grid, rings, channels, align, swim, ...); keys in its terminal or the controllers. Starts its own VR session; stop it here afterwards.",
            ToolId::WalkTest => "Desk mode with 6DoF (camera broker, Monado with SLAM, passthrough so you see where you walk), recorded for replay. Stand still 5 s, walk slowly, end where you started, then Finish: the 6DoF result stays here.",
            ToolId::CtrlRecording => "analysis/controller-6dof.md section 6: camera recording from the broker plus every controller report, together (gyro scale, axes, time offset; replay set for phase 5).",
            ToolId::HandsTest => "xr-handtest (headless OpenXR client) against the running session: valid hand joints from the controllers.",
            ToolId::VoiceTest => "Live speech to text from the headset microphone (omarchyvr-voice), or the transcript of the running desk listener.",
            ToolId::SteamRuntime => "Switch the OpenVR/OpenXR runtime files between xrizer + Monado and SteamVR, and copy the Steam launch options. The app never launches games.",
        }
    }

    /// LESSONS rows the tool's checks implement.
    pub fn rows(self) -> &'static [u8] {
        match self {
            ToolId::EyeView | ToolId::Ipd => &[3, 11],
            ToolId::EyePowerOn => &[11],
            ToolId::EyeCalibration => &[1, 3, 4, 5, 9, 10, 18],
            ToolId::LensTest => &[1, 4, 5, 9, 10, 18],
            ToolId::WalkTest => &[1, 2, 5, 6, 7, 9, 10, 12, 13, 14, 15],
            ToolId::CtrlRecording => &[1, 2, 15],
            ToolId::HandsTest => &[4],
            ToolId::VoiceTest => &[19],
            ToolId::SteamRuntime => &[20],
        }
    }
}

/// Options set on the Tools page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOptions {
    /// Lens pattern (`grid`, `rings`, ...); empty = guided list.
    pub pattern: String,
    /// 90 Hz for the lens test.
    pub hz90: bool,
    /// Raw camera recording length during the walk.
    pub walk_secs: u32,
    /// Auto exposure for the walk test.
    pub walk_ae: bool,
    /// Raw camera recording (.pxcam) during the walk, from the broker.
    pub walk_record: bool,
    /// Record the walk for replay (EuRoC via PIMAX_SLAM_RECORD).
    pub walk_euroc: bool,
    /// Controller recording length.
    pub rec_secs: u32,
    /// Eye calibration points (9 or 13).
    pub eyecal_points: u8,
    /// Log the eye view to a CSV.
    pub eye_log: bool,
    /// The user's OK to start over WayVR Xid 31 faults (`--force`, LESSONS row 9); cleared after a start.
    pub consent_force: bool,
}

impl Default for ToolOptions {
    fn default() -> Self {
        ToolOptions {
            pattern: String::new(),
            hz90: false,
            walk_secs: 90,
            walk_ae: true,
            walk_record: false,
            walk_euroc: true,
            rec_secs: 90,
            eyecal_points: 9,
            eye_log: true,
            consent_force: false,
        }
    }
}

/// Patterns of pimax-lenstest.
pub const PATTERNS: [&str; 11] = ["grid", "rings", "channels", "fov-edge", "square100", "square25", "corners", "align", "swim", "flat", "gaze"];

/// Something that keeps a tool from starting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocker {
    /// Why.
    pub text: String,
    /// A button that resolves it.
    pub action: Option<Action>,
}

/// Can the tool start now?
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Availability {
    /// Reasons it cannot.
    pub blockers: Vec<Blocker>,
    /// Things to know (yellow).
    pub notes: Vec<String>,
    /// Starting it needs the user's OK over WayVR Xid 31 faults.
    pub needs_consent: bool,
}

impl Availability {
    /// No blockers (and consent given where needed).
    pub fn ready(&self) -> bool {
        self.blockers.is_empty()
    }

    fn block(&mut self, text: impl Into<String>, action: Option<Action>) {
        self.blockers.push(Blocker { text: text.into(), action });
    }
}

/// Raw camera data rate (LESSONS row 15: 45 s of all cameras is about 2.2 GB).
pub fn recording_bytes(secs: u32) -> u64 {
    (f64::from(secs) / 45.0 * 2.2e9) as u64
}

fn need_no_monado(a: &mut Availability, live: &Live) {
    if live.service.is_some() {
        a.block("A desk/VR session is running: this tool brings its own Monado (one at a time, LESSONS row 1).", Some(Action::StopSession));
    } else if !live.foreign_monado.is_empty() {
        a.block(format!("monado-service pid {:?} is running.", live.foreign_monado), Some(Action::StopForeignMonado));
    } else if live.desk_clients_alive() {
        a.block("Leftovers of the last session still run.", Some(Action::StopSession));
    }
}

fn need_camera_free(a: &mut Availability, live: &Live, f: &Facts) {
    let session: Vec<u32> = live.session.iter().filter(|s| s.alive).filter_map(|s| s.pid).collect();
    for h in &f.camera_holders {
        let act = if session.contains(&h.pid) {
            // the running session's own broker or passthrough: stop the session, never signal its parts
            Some(Action::StopSession)
        } else if h.comm != "monado-service" {
            Some(Action::InterruptPid { pid: h.pid, comm: h.comm.clone() })
        } else {
            None
        };
        a.block(format!("Camera busy (LESSONS row 2): {}", h.describe()), act);
    }
}

fn need_eye_free(a: &mut Availability, live: &Live, f: &Facts) {
    if !f.tobii {
        a.block("Eye tracker 2104:0561 not enumerated.", None);
    }
    if live.service.as_ref().is_some_and(|s| s.eye) {
        a.block("Desk mode runs with eye tracking: Monado holds the eye chip (LESSONS row 3).", Some(Action::WithoutEye));
    }
    for h in f.eye_holders.iter().filter(|h| h.comm != "monado-service") {
        a.block(format!("Eye chip busy: {}", h.describe()), Some(Action::InterruptPid { pid: h.pid, comm: h.comm.clone() }));
    }
}

/// Everything a start through the launcher needs (the checks `--force` would skip, row 10, row 9), and the consent
/// over WayVR Xid 31 faults.
fn need_launcher_start(a: &mut Availability, f: &Facts, o: &ToolOptions) {
    for (id, text) in launcher_side(f) {
        a.block(text, Some(Action::Setup(id.to_string())));
    }
    if let Verdict::WayvrOnly(ev) = f.kernel_verdict() {
        a.needs_consent = true;
        if !o.consent_force {
            a.block(format!("{} WayVR Xid 31 fault(s) this boot: tick 'start anyway' to pass --force (LESSONS row 9).", ev.len()), None);
        }
    }
    if let Some(e) = &f.kernel_error {
        a.notes.push(format!("Kernel log not readable: {e}"));
    }
}

/// `--force` for a session start: only with consent, only for WayVR Xid 31 alone, and only when nothing else blocks.
pub fn force_allowed(f: &Facts, o: &ToolOptions) -> bool {
    o.consent_force && matches!(f.kernel_verdict(), Verdict::WayvrOnly(_)) && launcher_side(f).is_empty()
}

fn need_bin(a: &mut Availability, f: &Facts, p: &Paths, name: &str) {
    if !f.has(&p.bin(name)) {
        a.block(format!("{} is not built (Setup > Rust tools).", p.bin(name).display()), Some(Action::Setup("rust-bins".into())));
    }
}

/// Evaluate one tool.
pub fn availability(id: ToolId, live: &Live, f: &Facts, o: &ToolOptions, p: &Paths) -> Availability {
    let mut a = Availability::default();
    let desk = f.build(&p.desk_build().dir);
    match id {
        ToolId::EyeView | ToolId::Ipd => {
            need_bin(&mut a, f, p, "pimax-eye");
            need_eye_free(&mut a, live, f);
            if id == ToolId::Ipd {
                a.notes.push("'a' moves the lens motor once to the measured IPD (approved command, 58-72 mm).".into());
            }
        }
        ToolId::EyePowerOn => {
            need_bin(&mut a, f, p, "pimax-eye");
            if !f.headset {
                a.block("Headset 34a4:0044 not on USB.", None);
            }
            if f.tobii {
                a.block("The eye chip is already enumerated: nothing to do.", None);
            }
        }
        ToolId::EyeCalibration => {
            need_bin(&mut a, f, p, "pimax-eyecal");
            need_no_monado(&mut a, live);
            need_eye_free(&mut a, live, f);
            need_launcher_start(&mut a, f, o);
            match desk {
                Some(b) if b.built() => {
                    if b.markers.is_some_and(|m| !m.eye) {
                        a.block("The desk build has no eye gaze code.", None);
                    }
                }
                _ => a.block("The desk Monado build is missing.", Some(Action::Setup("build-desk".into()))),
            }
        }
        ToolId::LensTest => {
            need_bin(&mut a, f, p, "pimax-lenstest");
            need_no_monado(&mut a, live);
            need_launcher_start(&mut a, f, o);
            if o.hz90 {
                a.notes.push("90 Hz is new (same start commands as Pimax Play on Windows): check the picture in the headset.".into());
            }
        }
        ToolId::WalkTest => {
            need_bin(&mut a, f, p, "pimax-cam");
            need_bin(&mut a, f, p, "omarchyvr-slam");
            need_bin(&mut a, f, p, "omarchyvr-passthrough");
            need_no_monado(&mut a, live);
            need_camera_free(&mut a, live, f);
            need_launcher_start(&mut a, f, o);
            if !f.caps.sixdof {
                a.block("The installed launcher has no --6dof (Setup > pimax launcher).", Some(Action::Setup("launcher".into())));
            }
            match desk {
                Some(b) if b.built() => {
                    if b.pimax_cam() != Some(true) {
                        a.block("The desk build has PIMAX_CAM=OFF: the walk would be 3DoF (LESSONS row 6).", Some(Action::Setup("pimax-cam-desk".into())));
                    }
                    if o.walk_ae && b.markers.is_some_and(|m| !m.imu_gate) {
                        a.block("Auto exposure without the Basalt IMU gate can freeze tracking (LESSONS row 12): rebuild, or switch AE off.", None);
                    }
                    if b.sys_nice == Some(false) {
                        a.notes.push("monado-service lacks cap_sys_nice: frames may stutter (LESSONS row 7).".into());
                    }
                }
                _ => a.block("The desk Monado build is missing.", Some(Action::Setup("build-desk".into()))),
            }
            if !f.has(&p.basalt_lib) {
                a.block("Basalt is missing (~/.local/lib/libbasalt.so).", Some(Action::Setup("basalt".into())));
            }
            if o.walk_record {
                let need = recording_bytes(o.walk_secs);
                if let Some(free) = f.free_home {
                    if free < need * 2 {
                        a.block(format!("Raw recording needs about {} and only {} is free (LESSONS row 15).", gib(need), gib(free)), None);
                    } else {
                        a.notes.push(format!("Raw recording: about {} ({} free).", gib(need), gib(free)));
                    }
                }
            }
            a.notes.push("Lit room, cameras free; stand still 5 s first (LESSONS row 13).".into());
        }
        ToolId::CtrlRecording => {
            need_bin(&mut a, f, p, "pimax-cam");
            need_no_monado(&mut a, live);
            need_camera_free(&mut a, live, f);
            if !f.headset {
                a.block("Headset not on USB.", None);
            }
            let need = recording_bytes(o.rec_secs);
            match f.free_home {
                Some(free) if free < need * 2 => {
                    a.block(format!("The recording needs about {} and only {} is free (LESSONS row 15).", gib(need), gib(free)), None)
                }
                Some(free) => a.notes.push(format!("About {} for {} s ({} free).", gib(need), o.rec_secs, gib(free))),
                None => {}
            }
        }
        ToolId::HandsTest => {
            need_bin(&mut a, f, p, "xr-handtest");
            if live.service.is_none() {
                a.block("Start desk mode (or a VR session) first: the test asks the running Monado.", None);
            }
            a.notes.push("Headless client: it does not take the display from the main app (LESSONS row 4).".into());
        }
        ToolId::VoiceTest => {
            need_bin(&mut a, f, p, "omarchyvr-voice");
            if !f.has(&p.voice_model) {
                a.block("Parakeet model missing (Setup > Voice models).", Some(Action::Setup("voice".into())));
            }
            if live.service.is_none() {
                a.notes.push("The headset microphone sends zeros while the display is off: start desk mode first (LESSONS row 19).".into());
            }
        }
        ToolId::SteamRuntime => {}
    }
    a
}

/// How a tool starts.
#[derive(Debug, Clone)]
pub enum Launch {
    /// A terminal with this script.
    Terminal {
        /// Window title.
        title: String,
        /// Shell script.
        script: String,
    },
    /// A background job (output in the app's log).
    Job(JobSpec),
    /// Start a session first (job), then (optionally) open a client in a terminal.
    Session {
        /// The start job.
        start: JobSpec,
        /// Window title of the client.
        title: String,
        /// Client script, if the session has a terminal client.
        script: Option<String>,
    },
}

/// `flock -o -w 10 <lock> <cmd>`: device tools wait for the headset lock (INVENTORY section 2) for at most 10 s.
fn locked(p: &Paths, cmd: &str) -> String {
    format!(
        "flock -o -w 10 {} {cmd} || echo '(no headset lock within 10 s, or the tool failed: see above)'",
        quote(&p.headset_lock.display().to_string())
    )
}

fn q(p: &std::path::Path) -> String {
    quote(&p.display().to_string())
}

/// Environment of a session start: the IPD of ipd.env (row 18), plus the given variables; the stale experiment
/// variables of the app's own environment are removed (row 5).
fn session_env(mut c: Cmd, f: &Facts, env: &[(&str, &str)]) -> Cmd {
    if let Some(mm) = f.ipd_mm {
        c = c.env("OXR_DEBUG_IPD_MM", format!("{mm:.1}"));
    }
    for (k, v) in env {
        c = c.env(k, *v);
    }
    c.env_remove = f.stale_env.iter().map(|(k, _)| k.clone()).collect();
    c
}

/// The steps of a session start: wait for any previous Monado (row 1), re-check the devices it needs (rows 2, 3),
/// re-check what `--force` would skip (row 9), then the launcher.
fn start_steps(start: Cmd, force: bool, cameras: bool, eye: bool, before: Vec<Step>) -> Vec<Step> {
    let mut steps = vec![Step::WaitNoMonado(Duration::from_secs(45))];
    steps.extend(before);
    if cameras || eye {
        steps.push(Step::Guard("devices still free", guards::devices_free(cameras, eye)));
    }
    if force {
        steps.push(Step::Guard("still only WayVR Xid 31", guards::force_still_justified()));
    }
    steps.push(Step::Run(start));
    steps
}

/// Build the launch of a tool. `stamp` names output files (`20261014-171500`).
pub fn launch(id: ToolId, live: &Live, f: &Facts, o: &ToolOptions, p: &Paths, stamp: &str) -> Launch {
    let pl = p.launcher(&|x| f.has(x));
    let desk = p.desk_build().dir.clone();
    let runtime = |b: &std::path::Path| b.join("openxr_monado-dev.json").display().to_string();
    let force = force_allowed(f, o);
    let vr_start = |hz: u32| {
        let mut args = launcher::vr_start_args(hz, &desk);
        if force {
            args.push("--force".into());
        }
        Cmd::with_args(pl.display().to_string(), args)
    };
    match id {
        ToolId::EyeView => {
            let log = if o.eye_log { format!(" --log {}", q(&p.home.join(format!("eye-watch-{stamp}.csv")))) } else { String::new() };
            Launch::Terminal { title: "Eye view".into(), script: locked(p, &format!("{} watch{log}", q(&p.bin("pimax-eye")))) }
        }
        ToolId::Ipd => Launch::Terminal {
            title: "IPD".into(),
            script: format!(
                "{}\necho; echo 'ipd.env:'; cat {} 2>/dev/null || echo '(not written: no value applied)'",
                locked(p, &format!("{} ipd --watch", q(&p.bin("pimax-eye")))),
                q(&p.ipd_env)
            ),
        },
        ToolId::EyePowerOn => Launch::Job(JobSpec {
            kind: JobKind::Other("eye power-on".into()),
            label: "Power on the eye module".into(),
            steps: vec![Step::Run(Cmd::with_args(
                "flock",
                vec!["-o".into(), "-w".into(), "10".into(), p.headset_lock.display().to_string(), p.bin("pimax-eye").display().to_string(), "power-on".into()],
            ))],
        }),
        ToolId::EyeCalibration => {
            let c = session_env(vr_start(72), f, &[("PIMAX_EYE", "1"), ("PIMAX_EYE_FLIP_X", "1"), ("PIMAX_CTRL_HANDS", "0")]);
            Launch::Session {
                start: JobSpec {
                    kind: JobKind::ToolSession("eye calibration".into()),
                    label: "Eye calibration: start a VR session (eye build)".into(),
                    steps: start_steps(c, force, false, true, vec![]),
                },
                title: "Eye calibration".into(),
                script: Some(format!(
                    "XR_RUNTIME_JSON={} IPC_IGNORE_VERSION=1 {} --points {}\necho 'When done: press Stop session in the app (omarchyvr vr stop --grace 30).'",
                    quote(&runtime(&desk)),
                    q(&p.bin("pimax-eyecal")),
                    o.eyecal_points
                )),
            }
        }
        ToolId::LensTest => {
            let pat = if o.pattern.is_empty() || !PATTERNS.contains(&o.pattern.as_str()) {
                String::new()
            } else {
                format!(" --pattern {}", o.pattern)
            };
            let c = session_env(vr_start(if o.hz90 { 90 } else { 72 }), f, &[]);
            Launch::Session {
                start: JobSpec {
                    kind: JobKind::ToolSession("lens test".into()),
                    label: "Lens test: start a VR session".into(),
                    steps: start_steps(c, force, false, false, vec![]),
                },
                title: "Lens test".into(),
                script: Some(format!(
                    "XR_RUNTIME_JSON={} IPC_IGNORE_VERSION=1 {}{pat}\necho 'When done: press Stop session in the app (omarchyvr vr stop --grace 30).'",
                    quote(&runtime(&desk)),
                    q(&p.bin("pimax-lenstest"))
                )),
            }
        }
        ToolId::WalkTest => {
            let t = DeskToggles { eye: false, sixdof: true, ctrl_track: false, hands: false, hands_roles: false, passthrough: true, voice: false, hz90: false, perf: true, anchor: false, preset: crate::launcher::Preset::None, eye_default_on: false, screens: 4, ..DeskToggles::default() };
            let rec = walk_dir(p, stamp);
            let mut env: Vec<(&str, String)> = vec![("PIMAX_CAM_AE", if o.walk_ae { "1" } else { "0" }.to_string())];
            let mut before = Vec::new();
            if o.walk_euroc {
                env.push(("PIMAX_SLAM_RECORD", rec.display().to_string()));
                before.push(Step::Run(Cmd::new("mkdir", &["-p", &rec.display().to_string()])));
            }
            let env_ref: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
            let c = session_env(Cmd::with_args(pl.display().to_string(), launcher::desk_start_args(&t, force)), f, &env_ref);
            let script = o.walk_record.then(|| {
                format!(
                    "{} record {} --source broker --seconds {} --zlib 1",
                    q(&p.bin("pimax-cam")),
                    q(&p.home.join(format!("walk-{stamp}.pxcam"))),
                    o.walk_secs
                )
            });
            Launch::Session {
                start: JobSpec {
                    kind: JobKind::ToolSession("walk test".into()),
                    label: "6DoF walk test: start desk mode with 6DoF".into(),
                    steps: start_steps(c, force, true, false, before),
                },
                title: "Walk recording".into(),
                script,
            }
        }
        ToolId::CtrlRecording => Launch::Terminal { title: "Controller recording".into(), script: ctrl_recording_script(p, &pl, o.rec_secs, stamp) },
        ToolId::HandsTest => {
            let build = live.service.as_ref().and_then(|s| s.build.clone()).unwrap_or(desk);
            Launch::Job(JobSpec {
                kind: JobKind::Other("hands test".into()),
                label: "Hands test (xr-handtest)".into(),
                steps: vec![Step::Run(
                    Cmd::new(p.bin("xr-handtest").display().to_string(), &["--source", "any", "--seconds", "10", "--wait-runtime", "5"])
                        .env("XR_RUNTIME_JSON", runtime(&build))
                        .env("IPC_IGNORE_VERSION", "1"),
                )],
            })
        }
        ToolId::VoiceTest => {
            let v = q(&p.bin("omarchyvr-voice"));
            let script = if live.alive("voice") { format!("{v} tail") } else { format!("{v} listen --socket none") };
            Launch::Terminal { title: "Voice test".into(), script }
        }
        ToolId::SteamRuntime => Launch::Job(runtime_job(p, f, "status")),
    }
}

/// Where a walk is recorded (EuRoC, like `run_slam.sh live`).
pub fn walk_dir(p: &Paths, stamp: &str) -> std::path::PathBuf {
    p.slam_recordings.join(format!("{stamp}-walk"))
}

/// The by-hand version of the walk test, shown to copy (the app does not run it: it starts Monado with a copied
/// environment and stops it with `pkill -x`, against LESSONS rows 1 and 5).
pub fn walk_script_line(p: &Paths, o: &ToolOptions) -> String {
    format!(
        "cd {} && M={} PIMAX_CAM_AE={} {} {}",
        q(&p.repo),
        q(&p.desk_build().dir),
        if o.walk_ae { 1 } else { 0 },
        q(&p.script("run_slam_view.sh")),
        o.walk_secs
    )
}

/// "Games run with": `omarchyvr vr games monado|steamvr|status --monado-build <desk build>` (`xrizer` = monado). It saves
/// the choice (~/.config/omarchyvr/games.toml) and hands openvrpaths.vrpath to that side: Monado = xrizer only, read-only;
/// SteamVR = SteamVR first, writable, Monado's SteamVR driver registered. It works without any backup, handles the
/// read-only file itself and never changes the OpenXR runtime (docs/STEAMVR.md).
pub fn runtime_job(p: &Paths, f: &Facts, what: &str) -> JobSpec {
    let what = if what == "xrizer" { "monado" } else { what };
    let pl = p.launcher(&|x| f.has(x));
    let run = Cmd::with_args(
        pl.display().to_string(),
        vec!["vr".into(), "games".into(), what.into(), "--monado-build".into(), p.desk_build().dir.display().to_string()],
    );
    JobSpec {
        kind: JobKind::Other(format!("runtime {what}")),
        label: match what {
            "monado" => "Games run with: Monado".into(),
            "steamvr" => "Games run with: SteamVR".into(),
            _ => "Games runtime".into(),
        },
        steps: vec![Step::Run(run)],
    }
}

/// "Sound in headset" / "Headset microphone" / "Test microphone" / "Test headset sound":
/// `omarchyvr vr audio on|off|mic-on|mic-off|mic-test|speaker-test`
/// (docs/AUDIO.md).
pub fn audio_job(p: &Paths, f: &Facts, what: &str) -> JobSpec {
    let pl = p.launcher(&|x| f.has(x));
    let label = match what {
        "mic-on" | "mic-off" => format!("Headset microphone: {}", &what[4..]),
        "mic-test" => "Test microphone (3 s recording, then played back)".into(),
        "speaker-test" => "Test headset sound (a short chime on the headset speaker)".into(),
        w => format!("Sound in headset: {w}"),
    };
    JobSpec {
        kind: JobKind::Other(format!("sound {what}")),
        label,
        steps: vec![Step::Run(Cmd::with_args(pl.display().to_string(), vec!["vr".into(), "audio".into(), what.into()]))],
    }
}

/// "Brightness": `omarchyvr vr display image standard|bright|brighter` / `backlight 20..100`.
pub fn display_job(p: &Paths, f: &Facts, args: &[&str]) -> JobSpec {
    let pl = p.launcher(&|x| f.has(x));
    let mut a = vec!["vr".to_string(), "display".to_string()];
    a.extend(args.iter().map(|s| s.to_string()));
    JobSpec {
        kind: JobKind::Other(format!("display {}", args.join(" "))),
        label: match args {
            ["image", v] => format!("Image brightness: {v}"),
            ["backlight", b] => format!("Backlight: {b} %"),
            _ => "Brightness".into(),
        },
        steps: vec![Step::Run(Cmd::with_args(pl.display().to_string(), a))],
    }
}

/// "Game mode" and "Compositor time budget": `omarchyvr vr gamemode on|off|restore` / `budget auto|6|8`.
pub fn gamemode_job(p: &Paths, f: &Facts, args: &[&str]) -> JobSpec {
    let pl = p.launcher(&|x| f.has(x));
    let mut a = vec!["vr".to_string(), "gamemode".to_string()];
    a.extend(args.iter().map(|s| s.to_string()));
    JobSpec {
        kind: JobKind::Other(format!("gamemode {}", args.join(" "))),
        label: match args {
            ["on"] => "Game mode: on".into(),
            ["off"] => "Game mode: off".into(),
            ["restore"] => "Game mode: starting again what was paused".into(),
            ["budget", b] => format!("Compositor time budget: {b}"),
            _ => "Game mode".into(),
        },
        steps: vec![Step::Run(Cmd::with_args(pl.display().to_string(), a))],
    }
}

/// "Set up Steam for VR games": `omarchyvr vr steam-setup on|off|status` (PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1 for
/// Steam itself, so no game needs launch options; restart Steam once).
pub fn steam_setup_job(p: &Paths, f: &Facts, what: &str) -> JobSpec {
    let pl = p.launcher(&|x| f.has(x));
    JobSpec {
        kind: JobKind::Other(format!("steam setup {what}")),
        label: "Set up Steam for VR games".into(),
        steps: vec![Step::Run(Cmd::with_args(pl.display().to_string(), vec!["vr".into(), "steam-setup".into(), what.into()]))],
    }
}

/// `omarchyvr vr steam-options --monado-build <desk>`: the launch options line (printed into the job log).
pub fn steam_options_job(p: &Paths, f: &Facts) -> JobSpec {
    let pl = p.launcher(&|x| f.has(x));
    JobSpec {
        kind: JobKind::Other("steam options".into()),
        label: "Steam launch options".into(),
        steps: vec![Step::Run(Cmd::with_args(
            pl.display().to_string(),
            vec!["vr".into(), "steam-options".into(), "--monado-build".into(), p.desk_build().dir.display().to_string()],
        ))],
    }
}

/// The launch options line itself (same text as `omarchyvr vr steam-options`, from the launcher library).
pub fn steam_options_line(p: &Paths) -> String {
    omarchyvr_tools::vr::config::steam_options(&p.desk_build().dir.join("openxr_monado-dev.json"))
}

/// The recording procedure of analysis/controller-6dof.md section 6 as one script: broker first, camera recording and
/// controller reports together, broker stopped last (by PID); the whole run holds the headset lock. No command
/// substitution, so every command is visible to the safety checks.
pub fn ctrl_recording_script(p: &Paths, launcher_bin: &std::path::Path, secs: u32, stamp: &str) -> String {
    let out = p.home.join(format!("ctrl-imu-{stamp}.pxcam"));
    let hid = p.home.join(format!("ctrl-imu-{stamp}-hid.log"));
    let inner = format!(
        r#"set -u
OUT={out}; HID={hid}; SOCK={sock}; CAM={cam}; PIMAX={pimax}; BLOG=/tmp/omarchyvr-ctrlrec-broker.log
B=; R=; C=
cleanup() {{ [ -n "$R" ] && kill -INT "$R" 2>/dev/null; [ -n "$C" ] && kill -INT "$C" 2>/dev/null; [ -n "$R" ] && wait "$R" 2>/dev/null; [ -n "$C" ] && wait "$C" 2>/dev/null; if [ -n "$B" ]; then kill -INT "$B" 2>/dev/null; wait "$B" 2>/dev/null; fi; R=; C=; B=; }}
trap 'echo interrupted; cleanup; exit 130' INT TERM
echo "free space in $HOME (about 2.2 GB per 45 s):"; df -h --output=avail "$HOME" | tail -1
rm -f "$SOCK"
"$CAM" broker >"$BLOG" 2>&1 & B=$!
i=0
while [ $i -lt 50 ] && [ ! -S "$SOCK" ] && kill -0 "$B" 2>/dev/null; do sleep 0.2; i=$((i + 1)); done
if [ ! -S "$SOCK" ]; then echo "camera broker did not come up:"; tail -3 "$BLOG"; cleanup; exit 1; fi
echo "camera broker up (pid $B)"
"$CAM" record "$OUT" --source broker --seconds {secs} & R=$!
"$PIMAX" controllers {csecs} --raw >"$HID" & C=$!
cat <<'TXT'
Recording {secs} s. In a lit room:
 1. press a button on each controller (both must print 'controller attached' in the HID log)
 2. hold both still in front of the chest for 5 s
 3. rotate each slowly about its three axes (roll, pitch, yaw), about 90 deg each way, 20 s
 4. fast wrist flicks for 10 s
 5. move the hands left/right/up/down, near (20 cm) and far, 20 s
 6. hold still for 5 s
TXT
wait "$R"; R=
wait "$C"; C=
kill -INT "$B" 2>/dev/null; wait "$B" 2>/dev/null; B=
echo "camera recording:"; du -h "$OUT"
echo "controller reports (lines):"; wc -l <"$HID"
echo "offline next: cd {rust} && cargo build --release -p pimax-ctrltrack && ./target/release/pimax-ctrltrack track $OUT --fit-extrinsics --csv /tmp/ctrl-imu-poses.csv"
"#,
        out = q(&out),
        hid = q(&hid),
        sock = q(&p.broker_socket()),
        cam = q(&p.bin("pimax-cam")),
        pimax = q(launcher_bin),
        rust = q(&p.rust),
        secs = secs,
        csecs = secs + 5,
    );
    format!("flock -o -w 10 {} bash -c {} || echo '(no headset lock within 10 s)'", q(&p.headset_lock), quote(&inner))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builds::MarkerCache;
    use crate::demo;
    use crate::facts::{collect_facts, collect_live};
    use crate::monlog::LogFollower;

    fn state(m: &crate::machine::FakeMachine, p: &Paths) -> (Live, Facts) {
        let f = collect_facts(m, p, &demo::opts(p), &MarkerCache::default(), None, true, true);
        let mut fl = LogFollower::new(&p.session_dir.join("monado.log"));
        (collect_live(m, p, &mut fl), f)
    }

    fn consent() -> ToolOptions {
        ToolOptions { consent_force: true, ..ToolOptions::default() }
    }

    fn step_names(steps: &[Step]) -> Vec<String> {
        steps
            .iter()
            .map(|s| match s {
                Step::Run(c) => format!("run {}", c.display()),
                Step::RunAllowFail(c) => format!("try {}", c.display()),
                Step::WaitNoMonado(_) => "wait".to_string(),
                Step::Guard(n, _) => format!("guard {n}"),
                Step::Note(n) => format!("note {n}"),
            })
            .collect()
    }

    #[test]
    fn eye_tools_are_greyed_out_while_desk_runs_with_eye_tracking() {
        let (m, p) = demo::running_machine();
        let (live, f) = state(&m, &p);
        for t in [ToolId::EyeView, ToolId::Ipd] {
            let a = availability(t, &live, &f, &ToolOptions::default(), &p);
            assert!(!a.ready());
            assert!(a.blockers.iter().any(|b| b.action == Some(Action::WithoutEye)), "{a:?}");
        }
        let (m, p) = demo::idle_machine();
        let (live, f) = state(&m, &p);
        assert!(availability(ToolId::EyeView, &live, &f, &ToolOptions::default(), &p).ready());
    }

    #[test]
    fn session_tools_need_no_monado_and_consent_over_wayvr_xids() {
        let (m, p) = demo::idle_machine();
        let (live, f) = state(&m, &p);
        let a = availability(ToolId::LensTest, &live, &f, &ToolOptions::default(), &p);
        assert!(a.needs_consent && !a.ready());
        assert!(availability(ToolId::LensTest, &live, &f, &consent(), &p).ready());
        match launch(ToolId::LensTest, &live, &f, &ToolOptions { pattern: "swim".into(), ..consent() }, &p, "s") {
            Launch::Session { start, script, .. } => {
                let n = step_names(&start.steps);
                assert_eq!(n[0], "wait", "row 1 first");
                assert_eq!(n[1], "guard still only WayVR Xid 31");
                assert_eq!(n[2], "run OXR_DEBUG_IPD_MM=65.9 /home/u/.local/bin/omarchyvr vr start --fps 72 --monado-build /home/u/src/monado-eye/build --force");
                let script = script.unwrap();
                assert!(script.contains("XR_RUNTIME_JSON=/home/u/src/monado-eye/build/openxr_monado-dev.json IPC_IGNORE_VERSION=1"));
                assert!(script.contains("pimax-lenstest --pattern swim"));
            }
            l => panic!("{l:?}"),
        }
        let (m, p) = demo::running_machine();
        let (live, f) = state(&m, &p);
        let a = availability(ToolId::EyeCalibration, &live, &f, &consent(), &p);
        assert!(a.blockers.iter().any(|b| b.action == Some(Action::StopSession)));
    }

    #[test]
    fn force_needs_every_launcher_check_to_pass() {
        // headset unplugged: --force would skip that launcher check, so the tool is blocked even with consent
        let (mut m, p) = demo::idle_machine();
        m.host.files.retain(|k, _| !k.starts_with("/sys/bus/usb/devices/1-5.2"));
        let (live, f) = state(&m, &p);
        let a = availability(ToolId::LensTest, &live, &f, &consent(), &p);
        assert!(a.blockers.iter().any(|b| b.action == Some(Action::Setup("headset".into()))), "{a:?}");
        assert!(!force_allowed(&f, &consent()));
        // a clean kernel log: no --force at all
        let (mut m, p) = demo::idle_machine();
        m.cmd("journalctl -k -b --no-pager -q", 0, "kernel: quiet\n");
        let (live, f) = state(&m, &p);
        assert!(!force_allowed(&f, &consent()));
        match launch(ToolId::LensTest, &live, &f, &consent(), &p, "s") {
            Launch::Session { start, .. } => {
                let n = step_names(&start.steps);
                assert!(!n.iter().any(|s| s.contains("--force") || s.contains("guard still")), "{n:?}");
            }
            l => panic!("{l:?}"),
        }
    }

    #[test]
    fn eye_calibration_session_uses_the_eye_build_env_and_checks_the_chip() {
        let (m, p) = demo::idle_machine();
        let (live, f) = state(&m, &p);
        let o = ToolOptions { eyecal_points: 13, ..consent() };
        match launch(ToolId::EyeCalibration, &live, &f, &o, &p, "s") {
            Launch::Session { start, script, .. } => {
                let n = step_names(&start.steps);
                assert_eq!(n[1], "guard devices still free");
                match start.steps.last().unwrap() {
                    Step::Run(c) => {
                        assert!(c.env.contains(&("PIMAX_EYE".into(), "1".into())));
                        assert!(c.env.contains(&("PIMAX_CTRL_HANDS".into(), "0".into())));
                        assert!(c.env.contains(&("OXR_DEBUG_IPD_MM".into(), "65.9".into())), "row 18");
                    }
                    s => panic!("{s:?}"),
                }
                assert!(script.unwrap().contains("pimax-eyecal --points 13"));
            }
            l => panic!("{l:?}"),
        }
    }

    #[test]
    fn walk_test_goes_through_the_launcher_and_records_for_replay() {
        let (m, p) = demo::idle_machine();
        let (live, f) = state(&m, &p);
        assert!(availability(ToolId::WalkTest, &live, &f, &consent(), &p).ready());
        match launch(ToolId::WalkTest, &live, &f, &ToolOptions { walk_record: true, ..consent() }, &p, "20261014-1700") {
            Launch::Session { start, script, .. } => {
                let n = step_names(&start.steps);
                assert_eq!(n[0], "wait");
                assert_eq!(n[1], "run mkdir -p /home/u/.local/share/omarchyvr/slam-recordings/20261014-1700-walk");
                assert_eq!(n[2], "guard devices still free");
                assert_eq!(n[3], "guard still only WayVR Xid 31");
                assert!(n[4].ends_with("/home/u/.local/bin/omarchyvr desk start --fps 72 --6dof --force"), "{}", n[4]);
                assert!(n[4].contains("PIMAX_CAM_AE=1") && n[4].contains("PIMAX_SLAM_RECORD=/home/u/.local/share/omarchyvr/slam-recordings/20261014-1700-walk"));
                assert_eq!(
                    script.unwrap(),
                    "/home/u/omarchyvr/rust/target/release/pimax-cam record /home/u/walk-20261014-1700.pxcam --source broker --seconds 90 --zlib 1"
                );
            }
            l => panic!("{l:?}"),
        }
        // the by-hand script is only shown, never run
        assert!(walk_script_line(&p, &ToolOptions::default()).ends_with("run_slam_view.sh 90"));
        // AE off, no recording
        match launch(ToolId::WalkTest, &live, &f, &ToolOptions { walk_ae: false, walk_euroc: false, ..consent() }, &p, "x") {
            Launch::Session { start, script, .. } => {
                let n = step_names(&start.steps);
                assert!(n.last().unwrap().contains("PIMAX_CAM_AE=0") && !n.last().unwrap().contains("PIMAX_SLAM_RECORD"));
                assert!(script.is_none());
            }
            l => panic!("{l:?}"),
        }
    }

    #[test]
    fn walk_test_and_recording_check_camera_disk_and_build() {
        let (mut m, p) = demo::idle_machine();
        m.process(4242, "omarchyvr-passt", "omarchyvr-passthrough run", &[], 10, &["/dev/video0"]);
        m.free.insert("/home/u".into(), 3 << 30);
        let (live, f) = state(&m, &p);
        let o = ToolOptions { walk_record: true, ..consent() };
        let a = availability(ToolId::WalkTest, &live, &f, &o, &p);
        assert!(a.blockers.iter().any(|b| b.text.contains("Camera busy")));
        assert!(a.blockers.iter().any(|b| b.text.contains("row 15")));
        let r = availability(ToolId::CtrlRecording, &live, &f, &o, &p);
        assert!(r.blockers.iter().any(|b| b.text.contains("row 15")));
    }

    #[test]
    fn the_running_sessions_camera_is_stopped_with_the_session_not_signalled() {
        let (m, p) = demo::running_machine();
        let (live, f) = state(&m, &p);
        let a = availability(ToolId::WalkTest, &live, &f, &consent(), &p);
        let cam: Vec<&Blocker> = a.blockers.iter().filter(|b| b.text.contains("Camera busy")).collect();
        assert_eq!(cam.len(), 1);
        assert_eq!(cam[0].action, Some(Action::StopSession));
        assert!(!a.blockers.iter().any(|b| matches!(b.action, Some(Action::InterruptPid { .. }))));
        assert!(!availability(ToolId::EyePowerOn, &live, &f, &ToolOptions::default(), &p).ready(), "chip present: nothing to do");
    }

    #[test]
    fn controller_recording_script_follows_the_procedure_and_is_safe() {
        let p = demo::paths();
        let s = ctrl_recording_script(&p, std::path::Path::new("/home/u/.local/bin/omarchyvr"), 90, "X");
        assert!(s.starts_with("flock -o -w 10 /tmp/omarchyvr-headset.lock bash -c"));
        let b = s.find("broker >").unwrap();
        let r = s.find("record \"$OUT\" --source broker --seconds 90").unwrap();
        let c = s.find("controllers 95 --raw").unwrap();
        let stop = s.rfind("kill -INT \"$B\"").unwrap();
        assert!(b < r && r < c && c < stop, "broker first, recording + reports, broker last");
        assert_eq!(crate::safety::check_visible(&s), Ok(()), "no command substitution (arithmetic $(( )) is fine)");
        assert!(!s.contains("wait $R $C"), "an empty wait would block on the broker");
        assert_eq!(crate::safety::check(&Cmd::shell(&s)), Ok(()));
    }

    #[test]
    fn every_tool_launch_passes_the_safety_rules() {
        for (m, p) in [demo::idle_machine(), demo::running_machine()] {
            let (live, f) = state(&m, &p);
            let o = ToolOptions { walk_record: true, pattern: "grid".into(), ..consent() };
            for t in ToolId::ALL {
                let check_steps = |steps: &[Step]| {
                    for s in steps {
                        if let Step::Run(c) | Step::RunAllowFail(c) = s {
                            assert_eq!(crate::safety::check(c), Ok(()), "{t:?}: {}", c.display());
                        }
                    }
                };
                match launch(t, &live, &f, &o, &p, "s") {
                    Launch::Terminal { script, .. } => assert_eq!(crate::safety::check(&Cmd::shell(&script)), Ok(()), "{t:?}"),
                    Launch::Job(j) => check_steps(&j.steps),
                    Launch::Session { start, script, .. } => {
                        check_steps(&start.steps);
                        if let Some(s) = script {
                            assert_eq!(crate::safety::check(&Cmd::shell(&s)), Ok(()), "{t:?}");
                        }
                    }
                }
            }
            for w in ["xrizer", "steamvr", "status"] {
                for s in runtime_job(&p, &f, w).steps.into_iter().chain(steam_setup_job(&p, &f, w).steps) {
                    if let Step::Run(c) | Step::RunAllowFail(c) = s {
                        assert_eq!(crate::safety::check(&c), Ok(()));
                    }
                }
            }
        }
        assert!(steam_options_line(&demo::paths()).ends_with("openxr_monado-dev.json %command%"));
    }

    #[test]
    fn games_switch_is_one_launcher_call_without_the_old_script() {
        let (m, p) = demo::idle_machine();
        let (_, f) = state(&m, &p);
        for (w, want) in [("xrizer", "vr games monado --monado-build"), ("steamvr", "vr games steamvr --monado-build"), ("status", "vr games status")] {
            let n = step_names(&runtime_job(&p, &f, w).steps);
            assert_eq!(n.len(), 1, "{n:?}");
            assert!(n[0].contains(want) && !n[0].contains("vr_runtime_switch.sh") && !n[0].contains("chmod"), "{n:?}");
        }
        let n = step_names(&steam_setup_job(&p, &f, "on").steps);
        assert!(n[0].ends_with("vr steam-setup on"), "{n:?}");
    }

    #[test]
    fn hands_and_voice_follow_the_running_session() {
        let (m, p) = demo::running_machine();
        let (live, f) = state(&m, &p);
        assert!(availability(ToolId::HandsTest, &live, &f, &ToolOptions::default(), &p).ready());
        match launch(ToolId::HandsTest, &live, &f, &ToolOptions::default(), &p, "s") {
            Launch::Job(j) => match &j.steps[0] {
                Step::Run(c) => assert!(c.env.contains(&("XR_RUNTIME_JSON".into(), "/home/u/src/monado-eye/build/openxr_monado-dev.json".into()))),
                s => panic!("{s:?}"),
            },
            l => panic!("{l:?}"),
        }
        let (m, p) = demo::idle_machine();
        let (live, f) = state(&m, &p);
        assert!(!availability(ToolId::HandsTest, &live, &f, &ToolOptions::default(), &p).ready());
        let v = availability(ToolId::VoiceTest, &live, &f, &ToolOptions::default(), &p);
        assert!(v.ready() && v.notes.iter().any(|n| n.contains("row 19")));
    }
}
