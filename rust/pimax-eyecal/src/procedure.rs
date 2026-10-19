//! The calibration / validation procedure as a time-driven state machine. No OpenXR, no USB: the caller feeds the
//! raw tracker samples, the head orientation and the wearer's commands, and draws the [`Scene`] it returns.
//!
//! # What the wearer does
//!
//! 1. Intro: keep the head still, look at the dot. The screen says whether both eyes are seen.
//! 2. Calibration: 9 (or 13) targets one after the other. Each target appears with a shrinking ring (it pulls the eye
//!    there), the procedure waits until the gaze is steady, records one second, and turns the dot green. If the head
//!    turns more than [`Settings::head_limit_deg`] meanwhile, the target says so and starts over.
//! 3. The fit (per eye, see `pimax_eye::calib`); a mirrored axis is detected and the tracker restarted with the right
//!    setting.
//! 4. Validation: 10 *other* targets measured the same way; their errors are the reported accuracy.
//! 5. Optional smooth pursuit: a dot moving on a circle.
//! 6. Results, live cursor, save (menu / Y), and the end-to-end check through the runtime.

use crate::collect::{analyze_eye, evaluate_window, truth_tangent, EyeWindow, RawSample, WindowEval};
use crate::layout::{
    calibration_targets, pimax_targets, pursuit_at, recenter_targets, service_targets, validation_targets, Region,
    Target,
    PURSUIT_PERIOD_S, TARGET_DIST_M,
};
use crate::report;
use pimax_eye::calib::{select_model, stats, tangent_angle_deg, Calibration, Combine, CombineState, EyeCal, EyePoint, ModelKind, Selection, Stats};
use pimax_eye::correction::tangent_of;
use pimax_eye::filter::{FilterParams, OneEuro};
use pimax_eye::fixation::{smooth, FixParams};
use pimax_eye::track::FrameConv;
use std::collections::VecDeque;

/// Settings of a run.
#[derive(Debug, Clone)]
pub struct Settings {
    /// 9 or 13 calibration targets (the grid pattern).
    pub cal_points: usize,
    /// Which calibration path.
    pub pattern: Pattern,
    /// Run the smooth-pursuit test after the validation.
    pub pursuit: bool,
    /// Device-to-head conversion of the raw tracker.
    pub conv: FrameConv,
    /// Largest head rotation during one target, degrees.
    pub head_limit_deg: f64,
    /// Largest head angular speed during one target, deg/s.
    pub head_speed_dps: f64,
    /// Attract animation, seconds.
    pub attract_s: f64,
    /// Earliest start of the recording after the target appeared, seconds.
    pub min_hold_s: f64,
    /// Latest start (recording starts even without a steady gaze), seconds.
    pub max_hold_s: f64,
    /// Recording per target, seconds.
    pub collect_s: f64,
    /// Full calibration, or recentre an existing one.
    pub mode: Mode,
    /// The calibration in use (recentre / comparison).
    pub existing: Option<Calibration>,
    /// Output filter written into the calibration.
    pub filter: FilterParams,
    /// Fixation detection.
    pub fix: FixParams,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            cal_points: 9,
            pattern: Pattern::Grid,
            pursuit: false,
            conv: FrameConv {
                flip_x: true,
                ..FrameConv::default()
            },
            head_limit_deg: 2.5,
            head_speed_dps: 25.0,
            attract_s: 0.7,
            min_hold_s: 0.45,
            max_hold_s: 2.0,
            collect_s: 1.0,
            mode: Mode::Full,
            existing: None,
            filter: FilterParams::default(),
            fix: FixParams::default(),
        }
    }
}

/// The calibration path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pattern {
    /// The 3x3 (or 13-point) grid on a dark background, a shrinking ring at each target.
    Grid,
    /// Pimax Play's path ([`pimax_targets`]): the dot travels between the targets, the second half on white; the
    /// fit also tries the pupil-size terms.
    Pimax,
}

impl Settings {
    /// The settings of the Pimax path: longer recording per target (Pimax holds each 3-5 s), a longer travel.
    pub fn pimax(mut self) -> Settings {
        self.pattern = Pattern::Pimax;
        self.attract_s = 1.0;
        self.collect_s = 2.0;
        self
    }
}

/// What the run does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Calibrate, validate, save.
    Full,
    /// Five targets: update the drift offsets of the existing calibration.
    Recenter,
}

/// Which list a target belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Set {
    /// Calibration targets.
    Calibration,
    /// Validation targets.
    Validation,
    /// Recentre targets.
    Recenter,
    /// Check through the runtime (combined gaze from OpenXR).
    Service,
}

/// Sub-stage of one target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Ring shrinking onto the target.
    Attract,
    /// Waiting for a steady gaze.
    Hold,
    /// Recording.
    Collect,
    /// Recorded, green flash.
    Accepted,
    /// The head moved: message, then start over.
    HeadMoved,
}

/// Where the procedure is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phase {
    /// Instructions, eye status.
    Intro,
    /// A target.
    Point {
        /// List.
        set: Set,
        /// Index in the list.
        i: usize,
        /// Sub-stage.
        stage: Stage,
        /// Time the stage started.
        t0: f64,
        /// Attempts at this target so far.
        attempt: u32,
    },
    /// Smooth pursuit.
    Pursuit {
        /// Start time.
        t0: f64,
    },
    /// Results.
    Results,
    /// Live cursor.
    Cursor,
    /// Waiting for the runtime's gaze (end-to-end check starting).
    ServiceWait {
        /// Start.
        t0: f64,
    },
    /// Cursor through the runtime.
    ServiceCursor,
}

/// What the wearer asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    /// Start / continue (space, A, trigger).
    Next,
    /// Start over (R, B).
    Redo,
    /// Save (Y, menu).
    Accept,
    /// Smooth pursuit test (P).
    Pursuit,
    /// Toggle the live cursor (C).
    Cursor,
    /// End-to-end check through the runtime (S).
    Service,
    /// The trigger: starts from the intro (in the Pimax path nothing else, so a squeeze never restarts a run).
    Start,
}

/// Things for the caller to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// A line for the terminal.
    Print(String),
    /// Write the results JSON.
    SaveResults(String),
    /// Write the calibration file (TOML text).
    SaveCalibration(String),
    /// Restart the raw tracker with this conversion (a mirrored axis was found).
    RestartTracker(FrameConv),
    /// Release the chip and read the gaze through the runtime.
    StartService,
    /// Back to the own tracker.
    StopService,
    /// Close the app (the Pimax path's "save and exit").
    Quit,
}

/// Shape of a mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkKind {
    /// Filled disc.
    Disc,
    /// Ring.
    Ring,
    /// Plus sign (two bars).
    Cross,
}

/// One thing to draw, head frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mark {
    /// Position, metres (head frame).
    pub pos: [f64; 3],
    /// Shape.
    pub kind: MarkKind,
    /// Radius (disc, ring) or half length (cross), degrees.
    pub radius_deg: f64,
    /// Ring line width / bar width, degrees.
    pub width_deg: f64,
    /// Colour (sRGB, 0..1).
    pub color: [f32; 3],
}

/// What to show this frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    /// Marks, drawn in order.
    pub marks: Vec<Mark>,
    /// Text lines (head-locked panel below the centre).
    pub hud: Vec<String>,
    /// Background colour (sRGB).
    pub background: [f32; 3],
}

/// Dark grey: keeps the pupils mid-sized and the white targets high in contrast.
pub const BACKGROUND: [f32; 3] = [0.10, 0.10, 0.11];
const WHITE: [f32; 3] = [1.0, 1.0, 1.0];
const GREEN: [f32; 3] = [0.2, 1.0, 0.35];
const AMBER: [f32; 3] = [1.0, 0.7, 0.1];
const RED: [f32; 3] = [1.0, 0.25, 0.2];
const GREY: [f32; 3] = [0.55, 0.55, 0.58];

/// One recorded target.
#[derive(Debug, Clone, PartialEq)]
pub struct PointRes {
    /// Target.
    pub target: Target,
    /// Per eye.
    pub eyes: [Option<EyeWindow>; 2],
    /// Samples recorded.
    pub samples: Vec<RawSample>,
    /// Largest head rotation during the target, degrees.
    pub head_deg: f64,
    /// Attempts used.
    pub attempts: u32,
    /// Usable (at least one eye with a fixation).
    pub ok: bool,
}

/// One validated target.
#[derive(Debug, Clone, PartialEq)]
pub struct ValRes {
    /// The recording.
    pub point: PointRes,
    /// Errors of the new calibration.
    pub eval: WindowEval,
    /// Errors of the raw gaze (same window, identity per-eye maps).
    pub raw_eval: WindowEval,
}

/// Fit of one eye.
#[derive(Debug, Clone, PartialEq)]
pub struct EyeFit {
    /// Model comparison.
    pub selection: Selection,
    /// Points used.
    pub points: usize,
}

/// Smooth-pursuit outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct PursuitRes {
    /// Lag giving the lowest error, seconds.
    pub lag_s: f64,
    /// Error at that lag over the pursuit samples (degrees).
    pub err: Stats,
    /// Fraction of samples used (no saccade, gaze valid).
    pub used: f64,
}

/// The head-frame mid point of the eyes.
fn mid(p: &[[f64; 3]; 2]) -> [f64; 3] {
    [0.5 * (p[0][0] + p[1][0]), 0.5 * (p[0][1] + p[1][1]), 0.5 * (p[0][2] + p[1][2])]
}

fn quat_angle_deg(a: [f64; 4], b: [f64; 4]) -> f64 {
    let d = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]).abs().min(1.0);
    2.0 * d.acos().to_degrees()
}

/// The procedure.
#[derive(Debug, Clone)]
pub struct Procedure {
    /// Settings.
    pub s: Settings,
    /// Current phase.
    pub phase: Phase,
    cal_t: Vec<Target>,
    val_t: Vec<Target>,
    eye_pos: [[f64; 3]; 2],
    window: Vec<RawSample>,
    recent: VecDeque<RawSample>,
    head_ref: Option<[f64; 4]>,
    head_prev: Option<(f64, [f64; 4])>,
    head_max: f64,
    head_speed: f64,
    /// Recorded calibration targets.
    pub cal_res: Vec<PointRes>,
    /// Recorded validation targets.
    pub val_res: Vec<ValRes>,
    /// Recentre recordings.
    pub rec_res: Vec<PointRes>,
    /// Per-eye fits.
    pub fits: [Option<EyeFit>; 2],
    /// The new calibration.
    pub calibration: Option<Calibration>,
    /// Smooth pursuit.
    pub pursuit_res: Option<PursuitRes>,
    pursuit_samples: Vec<RawSample>,
    /// End-to-end errors through the runtime, per service target (degrees).
    pub service_err: Vec<Option<f64>>,
    service_samples: Vec<(f64, [f64; 3])>,
    /// Head-motion restarts.
    pub head_restarts: u32,
    /// Saved to disk.
    pub saved: bool,
    status: String,
    cursor_state: (CombineState, OneEuro),
    cursor: Option<[f64; 3]>,
    raw_cursor: Option<[f64; 3]>,
    service_cursor: Option<[f64; 3]>,
    redo_queue: Vec<usize>,
    /// Rounds of re-measuring done in this calibration.
    redo_rounds: u32,
    in_redo: bool,
    axis_restarts: u32,
    now: f64,
}

/// A sample older than this is no longer "the current gaze".
const RECENT_S: f64 = 1.0;

impl Procedure {
    /// New procedure.
    pub fn new(s: Settings) -> Procedure {
        Procedure {
            cal_t: match s.pattern {
                Pattern::Grid => calibration_targets(s.cal_points),
                Pattern::Pimax => pimax_targets(),
            },
            val_t: validation_targets(),
            s,
            phase: Phase::Intro,
            eye_pos: [[-0.032, 0.0, 0.0], [0.032, 0.0, 0.0]],
            window: Vec::new(),
            recent: VecDeque::new(),
            head_ref: None,
            head_prev: None,
            head_max: 0.0,
            head_speed: 0.0,
            cal_res: Vec::new(),
            val_res: Vec::new(),
            rec_res: Vec::new(),
            fits: [None, None],
            calibration: None,
            pursuit_res: None,
            pursuit_samples: Vec::new(),
            service_err: Vec::new(),
            service_samples: Vec::new(),
            head_restarts: 0,
            saved: false,
            status: String::new(),
            cursor_state: (CombineState::default(), OneEuro::default()),
            cursor: None,
            raw_cursor: None,
            service_cursor: None,
            redo_queue: Vec::new(),
            redo_rounds: 0,
            in_redo: false,
            axis_restarts: 0,
            now: 0.0,
        }
    }

    fn list(&self, set: Set) -> &[Target] {
        match set {
            Set::Calibration => &self.cal_t,
            Set::Validation => &self.val_t,
            Set::Recenter | Set::Service => &[],
        }
    }

    fn target(&self, set: Set, i: usize) -> Target {
        match set {
            Set::Calibration => self.cal_t[i].clone(),
            Set::Validation => self.val_t[i].clone(),
            Set::Recenter => recenter_targets()[i].clone(),
            Set::Service => service_targets()[i].clone(),
        }
    }

    fn set_len(&self, set: Set) -> usize {
        match set {
            Set::Calibration | Set::Validation => self.list(set).len(),
            Set::Recenter => recenter_targets().len(),
            Set::Service => service_targets().len(),
        }
    }

    /// Per-eye tracking ratio over the last second.
    pub fn eye_ratio(&self) -> [f64; 2] {
        let n = self.recent.len().max(1) as f64;
        let c = |e: usize| self.recent.iter().filter(|s| s.eyes[e].is_some()).count() as f64 / n;
        [c(0), c(1)]
    }

    /// A short name of the current target state, for the raw log.
    pub fn tag(&self) -> String {
        match self.phase {
            Phase::Point { set, i, stage, .. } => {
                let t = self.target(set, i);
                format!("{set:?}:{i}:{}:{:?}", t.name(), stage)
            }
            Phase::Pursuit { t0 } => {
                let p = pursuit_at((self.now - t0 - 1.0).max(0.0));
                format!("Pursuit:{:.2},{:.2}", p.az, p.el)
            }
            other => format!("{other:?}").split_whitespace().next().unwrap_or("").to_string(),
        }
    }

    /// The command handler.
    pub fn command(&mut self, c: Cmd) -> Vec<Effect> {
        let mut fx = Vec::new();
        // The Pimax path has two buttons, as Pimax Play: at the start A calibrate, B exit; at the end A again, B save
        // and exit. Nothing else.
        let c = match (self.s.pattern, c, self.phase) {
            (Pattern::Pimax, Cmd::Next, Phase::Results | Phase::Cursor) => {
                let s = self.s.clone();
                *self = Procedure::new(s);
                fx.push(Effect::Print("calibrating again".into()));
                Cmd::Next // from the fresh intro: start at once
            }
            (Pattern::Pimax, Cmd::Redo, Phase::Results | Phase::Cursor) => {
                fx.extend(self.accept());
                fx.push(Effect::Quit);
                return fx;
            }
            (Pattern::Pimax, Cmd::Next | Cmd::Start, Phase::Intro) => Cmd::Next,
            (Pattern::Pimax, Cmd::Redo, Phase::Intro) => {
                fx.push(Effect::Quit);
                return fx;
            }
            (Pattern::Pimax, _, _) if !matches!(c, Cmd::Accept) => return fx,
            (Pattern::Grid, Cmd::Start, _) => Cmd::Next,
            _ => c,
        };
        match (c, self.phase) {
            (Cmd::Next, Phase::Intro) => {
                fx.push(Effect::Print(match self.s.mode {
                    Mode::Full => format!(
                        "calibration: {} targets, then {} validation targets; keep the head still",
                        self.cal_t.len(),
                        self.val_t.len()
                    ),
                    Mode::Recenter => "recentre: 5 targets".to_string(),
                }));
                let set = if self.s.mode == Mode::Recenter {
                    Set::Recenter
                } else {
                    Set::Calibration
                };
                self.start_point(set, 0, 1);
            }
            (Cmd::Redo, _) => {
                let s = self.s.clone();
                *self = Procedure::new(s);
                fx.push(Effect::Print("starting over".into()));
            }
            (Cmd::Accept, Phase::Results | Phase::Cursor | Phase::ServiceCursor) => fx.extend(self.accept()),
            (Cmd::Next | Cmd::Cursor, Phase::Results) if self.calibration.is_some() => self.phase = Phase::Cursor,
            (Cmd::Next | Cmd::Cursor, Phase::Cursor) => self.phase = Phase::Results,
            (Cmd::Pursuit, Phase::Results | Phase::Cursor) if self.calibration.is_some() => {
                self.pursuit_samples.clear();
                self.phase = Phase::Pursuit { t0: self.now };
            }
            (Cmd::Service, Phase::Results | Phase::Cursor) => {
                if self.saved {
                    self.phase = Phase::ServiceWait { t0: self.now };
                    self.service_err.clear();
                    fx.push(Effect::StartService);
                    fx.push(Effect::Print(
                        "end-to-end check: the chip is handed to the runtime, which applies the saved calibration"
                            .into(),
                    ));
                } else {
                    self.status = "SAVE FIRST (MENU / Y), THEN S".into();
                }
            }
            (Cmd::Next | Cmd::Service | Cmd::Cursor, Phase::ServiceCursor) => {
                self.phase = Phase::Results;
                fx.push(Effect::StopService);
            }
            _ => {}
        }
        fx
    }

    fn accept(&mut self) -> Vec<Effect> {
        let Some(c) = self.calibration.clone() else {
            self.status = "NOTHING TO SAVE".into();
            return vec![];
        };
        self.saved = true;
        self.status = "SAVED: NEW GAZE APPS USE IT. S: CHECK THROUGH THE RUNTIME".into();
        vec![Effect::SaveCalibration(c.to_toml())]
    }

    /// The index in the calibration targets of the result `k` (results of a target shown twice, as the Pimax path's
    /// centre, share one slot, so result and target indices differ).
    fn target_index(&self, k: usize) -> usize {
        self.cal_t.iter().position(|t| *t == self.cal_res[k].target).unwrap_or(k)
    }

    fn start_point(&mut self, set: Set, i: usize, attempt: u32) {
        self.window.clear();
        self.head_ref = None;
        self.head_max = 0.0;
        self.head_speed = 0.0;
        self.phase = Phase::Point {
            set,
            i,
            stage: Stage::Attract,
            t0: self.now,
            attempt,
        };
    }

    /// Advance to time `t` (seconds, the clock of the samples) with the new raw samples, the head orientation
    /// (LOCAL space quaternion x, y, z, w) and the eye rendering positions (VIEW space); `service` is the runtime's
    /// combined gaze direction (head frame) during the end-to-end check.
    pub fn frame(
        &mut self,
        t: f64,
        head: Option<[f64; 4]>,
        eye_pos: [[f64; 3]; 2],
        samples: &[RawSample],
        service: Option<[f64; 3]>,
    ) -> Vec<Effect> {
        self.now = t;
        if eye_pos.iter().all(|p| p.iter().all(|c| c.is_finite())) && (eye_pos[1][0] - eye_pos[0][0]).abs() > 0.01 {
            self.eye_pos = eye_pos;
        }
        for s in samples {
            self.recent.push_back(*s);
        }
        while self.recent.front().is_some_and(|s| s.t < t - RECENT_S) {
            self.recent.pop_front();
        }
        self.update_cursor(samples);
        self.service_cursor = service;
        // head motion
        if let Some(q) = head {
            if let Some((tp, qp)) = self.head_prev {
                let dt = t - tp;
                if dt > 1e-3 {
                    self.head_speed = self.head_speed.max(quat_angle_deg(q, qp) / dt);
                }
            }
            self.head_prev = Some((t, q));
            if let Some(r) = self.head_ref {
                self.head_max = self.head_max.max(quat_angle_deg(q, r));
            }
        }
        let mut fx = Vec::new();
        match self.phase {
            Phase::Point { set, i, stage, t0, attempt } => {
                if self.head_ref.is_none() {
                    self.head_ref = head;
                    self.head_speed = 0.0;
                }
                let moved = self.head_max > self.s.head_limit_deg || self.head_speed > self.s.head_speed_dps;
                let dt = t - t0;
                match stage {
                    Stage::Attract | Stage::Hold | Stage::Collect if moved && set != Set::Service => {
                        self.head_restarts += 1;
                        fx.push(Effect::Print(format!(
                            "target {} ({}): head moved {:.1} deg ({:.0} deg/s), starting the target again",
                            i + 1,
                            self.target(set, i).name(),
                            self.head_max,
                            self.head_speed
                        )));
                        self.phase = Phase::Point { set, i, stage: Stage::HeadMoved, t0: t, attempt };
                    }
                    Stage::Attract if dt >= self.s.attract_s => {
                        self.phase = Phase::Point { set, i, stage: Stage::Hold, t0, attempt };
                    }
                    Stage::Hold => {
                        if (dt >= self.s.min_hold_s && self.steady(t)) || dt >= self.s.max_hold_s {
                            self.window.clear();
                            self.phase = Phase::Point { set, i, stage: Stage::Collect, t0: t, attempt };
                        }
                    }
                    Stage::Collect => {
                        if set == Set::Service {
                            if let Some(d) = service {
                                self.service_samples.push((t, d));
                            }
                        } else {
                            self.window.extend_from_slice(samples);
                        }
                        if dt >= self.s.collect_s {
                            fx.extend(self.finish_point(set, i, attempt));
                        }
                    }
                    Stage::Accepted if dt >= 0.25 => fx.extend(self.next_point(set, i)),
                    Stage::HeadMoved if dt >= 1.2 => self.start_point(set, i, attempt),
                    _ => {}
                }
            }
            Phase::Pursuit { t0 } => {
                if t - t0 > 1.0 {
                    self.pursuit_samples.extend_from_slice(samples);
                }
                if t - t0 >= 1.0 + PURSUIT_PERIOD_S {
                    fx.extend(self.finish_pursuit(t0));
                }
            }
            Phase::ServiceWait { t0 } => {
                if service.is_some() {
                    self.service_samples.clear();
                    self.start_point(Set::Service, 0, 1);
                } else if t - t0 > 20.0 {
                    self.status = "NO GAZE FROM THE RUNTIME (PIMAX_EYE=1 SERVICE?)".into();
                    self.phase = Phase::ServiceCursor;
                }
            }
            _ => {}
        }
        fx
    }

    /// The gaze of the last 0.25 s is steady (smoothed track within 1.5 degrees, mostly valid).
    fn steady(&self, t: f64) -> bool {
        let last: Vec<&RawSample> = self.recent.iter().filter(|s| s.t >= t - 0.25).collect();
        if last.len() < 15 {
            return false;
        }
        let mut ok_eyes = 0;
        for e in 0..2 {
            let tan: Vec<Option<(f64, f64)>> =
                last.iter().map(|s| s.eyes[e].and_then(|x| tangent_of(x.dir))).collect();
            let valid = tan.iter().filter(|x| x.is_some()).count();
            if valid * 10 < tan.len() * 7 {
                continue;
            }
            let sm: Vec<(f64, f64)> = smooth(&tan, 3).into_iter().flatten().collect();
            let range = |f: &dyn Fn(&(f64, f64)) -> f64| {
                let (lo, hi) = sm.iter().map(f).fold((f64::MAX, f64::MIN), |a, x| (a.0.min(x), a.1.max(x)));
                hi - lo
            };
            let lim = 1.5f64.to_radians().tan();
            if range(&|p| p.0) <= lim && range(&|p| p.1) <= lim {
                ok_eyes += 1;
            }
        }
        ok_eyes > 0
    }

    fn finish_point(&mut self, set: Set, i: usize, attempt: u32) -> Vec<Effect> {
        let target = self.target(set, i);
        let mut fx = Vec::new();
        if set == Set::Service {
            let truth = target.dir();
            let errs: Vec<f64> = self
                .service_samples
                .iter()
                .map(|(_, d)| angle(*d, truth))
                .collect();
            let e = (!errs.is_empty()).then(|| median(errs.clone()));
            fx.push(Effect::Print(format!(
                "runtime check {}/{} ({}): {}",
                i + 1,
                service_targets().len(),
                target.name(),
                e.map_or("no gaze".to_string(), |e| format!("error {e:.2} deg (median of {} frames)", errs.len()))
            )));
            self.service_err.push(e);
            self.service_samples.clear();
            self.phase = Phase::Point { set, i, stage: Stage::Accepted, t0: self.now, attempt };
            return fx;
        }
        let w = std::mem::take(&mut self.window);
        let eyes = [analyze_eye(&w, 0, &self.s.fix), analyze_eye(&w, 1, &self.s.fix)];
        let ok = eyes.iter().any(|e| e.as_ref().is_some_and(|e| e.used >= 30));
        let res = PointRes {
            target: target.clone(),
            eyes,
            samples: w,
            head_deg: self.head_max,
            attempts: attempt,
            ok,
        };
        let line = |r: &PointRes| {
            let eye = |e: &Option<EyeWindow>| match e {
                Some(e) => format!("{} of {} used, noise {:.2} deg", e.used, e.total, e.rms_s2s_deg),
                None => "no fixation".to_string(),
            };
            format!(
                "{:?} {}/{} ({}): left {}; right {}; head {:.1} deg{}",
                set,
                i + 1,
                self.set_len(set),
                r.target.name(),
                eye(&r.eyes[0]),
                eye(&r.eyes[1]),
                r.head_deg,
                if r.ok { "" } else { "  [not usable]" }
            )
        };
        fx.push(Effect::Print(line(&res)));
        if !ok && attempt < 2 {
            fx.push(Effect::Print("  again (blink, no steady gaze?)".into()));
            self.start_point(set, i, attempt + 1);
            return fx;
        }
        match set {
            Set::Calibration => {
                if let Some(slot) = self.cal_res.iter_mut().find(|r| r.target == target) {
                    *slot = res;
                } else {
                    self.cal_res.push(res);
                }
            }
            Set::Validation => {
                let cal = self.calibration.clone();
                let raw_cal = Calibration::identity(self.s.conv, self.eye_pos[1][0] - self.eye_pos[0][0]);
                let mut raw_cal = raw_cal;
                for e in 0..2 {
                    raw_cal.eyes[e].origin_m = self.eye_pos[e];
                }
                if let Some(cal) = cal {
                    let eval = evaluate_window(&cal, &res.samples, &target, &self.s.fix);
                    let raw_eval = evaluate_window(&raw_cal, &res.samples, &target, &self.s.fix);
                    fx.push(Effect::Print(format!(
                        "  validation error: combined {} (raw {}), left {}, right {}",
                        fmt_opt(eval.comb_err_deg),
                        fmt_opt(raw_eval.comb_err_deg),
                        fmt_opt(eval.eye_err_deg[0]),
                        fmt_opt(eval.eye_err_deg[1])
                    )));
                    self.val_res.push(ValRes { point: res, eval, raw_eval });
                }
            }
            Set::Recenter => self.rec_res.push(res),
            Set::Service => {}
        }
        self.phase = Phase::Point { set, i, stage: Stage::Accepted, t0: self.now, attempt };
        fx
    }

    fn next_point(&mut self, set: Set, i: usize) -> Vec<Effect> {
        if set == Set::Calibration && self.in_redo {
            if !self.redo_queue.is_empty() {
                let j = self.redo_queue.remove(0);
                self.start_point(set, j, 2);
                return vec![];
            }
            self.in_redo = false;
            return self.finish_calibration();
        }
        if i + 1 < self.set_len(set) {
            self.start_point(set, i + 1, 1);
            return vec![];
        }
        match set {
            Set::Calibration => self.finish_calibration(),
            Set::Validation => self.finish_validation(),
            Set::Recenter => self.finish_recenter(),
            Set::Service => self.finish_service(),
        }
    }

    fn eye_points(&self, e: usize) -> Vec<(usize, EyePoint)> {
        self.cal_res
            .iter()
            .enumerate()
            .filter_map(|(k, r)| {
                let w = r.eyes[e].as_ref()?;
                let truth = truth_tangent(&r.target, self.eye_pos[e])?;
                Some((
                    k,
                    EyePoint {
                        raw: w.center,
                        truth,
                        weight: (w.used as f64 / 60.0).clamp(0.3, 1.0),
                    },
                ))
            })
            .collect()
    }

    fn finish_calibration(&mut self) -> Vec<Effect> {
        let mut fx = Vec::new();
        let mut eyes: Vec<Option<EyeCal>> = vec![None, None];
        let mut gains = [None, None];
        for e in 0..2 {
            // the sample-level fit (direction + reported origin); the point fit below is the fallback
            let opts = crate::refit::ExtractOpts {
                pupil: self.s.pattern == Pattern::Pimax,
                ..crate::refit::ExtractOpts::default()
            };
            let wins: Vec<(Target, &[RawSample])> =
                self.cal_res.iter().map(|r| (r.target.clone(), r.samples.as_slice())).collect();
            let data = crate::refit::eye_data(&wins, e, self.eye_pos[e], &opts, &self.s.fix);
            if let Some(fit) = crate::refit::fit_eye(&data, self.eye_pos[e], &opts) {
                let best = fit.selection.best().cloned();
                let Some(best) = best else { continue };
                if let Some(a) = fit.selection.candidates.iter().find(|m| m.kind == ModelKind::Affine) {
                    gains[e] = Some((a.map.cx[1], a.map.cy[2]));
                }
                let loo_med = best.loo.median.max(0.5);
                for (k, err) in &fit.loo_by_window {
                    if *err > (3.0 * loo_med).max(2.5) && self.cal_res[*k].attempts < 2 && !self.redo_queue.contains(&self.target_index(*k)) {
                        fx.push(Effect::Print(format!(
                            "{} eye: target {} is {:.1} deg off the fit of the others: measuring it again",
                            eye_name(e),
                            self.cal_res[*k].target.name(),
                            err
                        )));
                        self.redo_queue.push(self.target_index(*k));
                    }
                }
                fx.push(Effect::Print(format!(
                    "{} eye: raw error {:.2} deg -> {} model, leave-one-out {:.2} deg (median {:.2}, max {:.2})",
                    eye_name(e),
                    fit.selection.raw.mean,
                    crate::refit::model_name(&fit),
                    best.loo.mean,
                    best.loo.median,
                    best.loo.max
                )));
                self.fits[e] = Some(EyeFit {
                    points: fit.points,
                    selection: fit.selection,
                });
                eyes[e] = Some(fit.cal);
                continue;
            }
            let pts = self.eye_points(e);
            let only: Vec<EyePoint> = pts.iter().map(|p| p.1).collect();
            if only.len() < 4 {
                fx.push(Effect::Print(format!(
                    "{} eye: only {} usable targets, not calibrated",
                    eye_name(e),
                    only.len()
                )));
                continue;
            }
            let sel = select_model(&only);
            let Some(best) = sel.best().cloned() else {
                continue;
            };
            // the affine candidate tells the axis signs whatever model won
            if let Some(a) = sel.candidates.iter().find(|m| m.kind == ModelKind::Affine) {
                gains[e] = Some((a.map.cx[1], a.map.cy[2]));
            }
            // a target far off the fit of the others is re-measured once (the wearer looked elsewhere)
            let loo_med = best.loo.median.max(0.5);
            for (j, (k, p)) in pts.iter().enumerate() {
                let rest: Vec<EyePoint> = only
                    .iter()
                    .enumerate()
                    .filter(|(m, _)| *m != j)
                    .map(|(_, q)| *q)
                    .collect();
                let Some(m) = pimax_eye::calib::fit_eye(&rest, best.kind, best.ridge) else {
                    continue;
                };
                let err = pimax_eye::calib::point_error_deg(&m, p);
                if err > (3.0 * loo_med).max(2.5) && self.cal_res[*k].attempts < 2 && !self.redo_queue.contains(&self.target_index(*k)) {
                    fx.push(Effect::Print(format!(
                        "{} eye: target {} is {:.1} deg off the fit of the others: measuring it again",
                        eye_name(e),
                        self.cal_res[*k].target.name(),
                        err
                    )));
                    self.redo_queue.push(self.target_index(*k));
                }
            }
            fx.push(Effect::Print(format!(
                "{} eye: raw error {:.2} deg -> {} model, leave-one-out {:.2} deg (median {:.2}, max {:.2})",
                eye_name(e),
                sel.raw.mean,
                best.kind.name(),
                best.loo.mean,
                best.loo.median,
                best.loo.max
            )));
            let prec: Vec<f64> = self
                .cal_res
                .iter()
                .filter_map(|r| r.eyes[e].as_ref().map(|w| w.rms_s2s_deg))
                .collect();
            eyes[e] = Some(EyeCal {
                map: best.map.clone(),
                origin_m: self.eye_pos[e],
                accuracy_deg: best.loo.mean,
                precision_deg: if prec.is_empty() { 1.0 } else { median(prec) / std::f64::consts::SQRT_2 },
                drift: (0.0, 0.0),
                org: Default::default(),
            });
            self.fits[e] = Some(EyeFit { selection: sel, points: only.len() });
        }
        // at most two rounds of re-measuring, whatever the fit says (no loop if a target never fits)
        if self.redo_rounds >= 2 {
            self.redo_queue.clear();
        }
        if !self.redo_queue.is_empty() {
            self.redo_rounds += 1;
            let j = self.redo_queue.remove(0);
            self.in_redo = true;
            self.start_point(Set::Calibration, j, 2);
            return fx;
        }
        // mirrored axes: both eyes' gains negative
        let neg = |f: fn((f64, f64)) -> f64| gains.iter().flatten().count() == 2 && gains.iter().flatten().all(|g| f(*g) < 0.0);
        let (fx_neg, fy_neg) = (neg(|g| g.0), neg(|g| g.1));
        if (fx_neg || fy_neg) && self.axis_restarts == 0 {
            self.axis_restarts += 1;
            let mut c = self.s.conv;
            if fx_neg {
                c.flip_x = !c.flip_x;
            }
            if fy_neg {
                c.flip_y = !c.flip_y;
            }
            fx.push(Effect::Print(format!(
                "the gaze moves opposite to the targets on {}: restarting the tracker with PIMAX_EYE_FLIP_X={} FLIP_Y={} and calibrating again",
                match (fx_neg, fy_neg) {
                    (true, true) => "both axes",
                    (true, false) => "the horizontal axis",
                    _ => "the vertical axis",
                },
                u8::from(c.flip_x),
                u8::from(c.flip_y)
            )));
            self.s.conv = c;
            fx.push(Effect::RestartTracker(c));
            let s = self.s.clone();
            let restarts = self.axis_restarts;
            *self = Procedure::new(s);
            self.axis_restarts = restarts;
            self.status = "AXIS FIXED: PRESS A TO CALIBRATE AGAIN".into();
            return fx;
        }
        let (Some(l), Some(r)) = (eyes[0].clone(), eyes[1].clone()) else {
            // one eye only: both slots get that eye's map so the file is complete, the missing one never validates
            let one = eyes.iter().flatten().next().cloned();
            match one {
                Some(e) => {
                    let mut l = e.clone();
                    let mut r = e;
                    l.origin_m = self.eye_pos[0];
                    r.origin_m = self.eye_pos[1];
                    self.calibration = Some(self.make_calibration(l, r));
                    fx.push(Effect::Print("only one eye calibrated: check the headset fit".into()));
                    self.start_point(Set::Validation, 0, 1);
                }
                None => {
                    self.status = "CALIBRATION FAILED: NO EYE SEEN. CHECK THE FIT, B TO RETRY".into();
                    fx.push(Effect::Print("calibration failed: no eye had enough usable targets".into()));
                    self.phase = Phase::Results;
                }
            }
            return fx;
        };
        self.calibration = Some(self.make_calibration(l, r));
        self.start_point(Set::Validation, 0, 1);
        fx
    }

    fn make_calibration(&self, l: EyeCal, r: EyeCal) -> Calibration {
        Calibration {
            conv: self.s.conv,
            eyes: [l, r],
            combine: Combine::default(),
            filter: self.s.filter,
            created_unix: report::unix_now(),
            notes: Vec::new(),
        }
    }

    fn finish_validation(&mut self) -> Vec<Effect> {
        let mut fx = Vec::new();
        // per-eye validation errors become the combination weights
        if let Some(cal) = self.calibration.as_mut() {
            for e in 0..2 {
                let errs: Vec<f64> = self.val_res.iter().filter_map(|v| v.eval.eye_err_deg[e]).collect();
                if errs.len() >= 3 {
                    cal.eyes[e].accuracy_deg = stats(&errs).mean;
                }
            }
        }
        let sum = report::summary(self);
        if let Some(cal) = self.calibration.as_mut() {
            cal.notes = sum.notes.clone();
        }
        fx.push(Effect::Print(sum.text.clone()));
        fx.push(Effect::SaveResults(report::json(self)));
        if self.s.pursuit {
            self.pursuit_samples.clear();
            self.phase = Phase::Pursuit { t0: self.now };
        } else {
            self.phase = Phase::Results;
        }
        fx
    }

    fn finish_recenter(&mut self) -> Vec<Effect> {
        let mut fx = Vec::new();
        let Some(mut cal) = self.s.existing.clone() else {
            self.status = "NO CALIBRATION TO RECENTRE".into();
            self.phase = Phase::Results;
            return fx;
        };
        for e in 0..2 {
            let mut du = Vec::new();
            let mut dv = Vec::new();
            for r in &self.rec_res {
                let (Some(w), Some(truth)) = (r.eyes[e].as_ref(), truth_tangent(&r.target, cal.eyes[e].origin_m)) else {
                    continue;
                };
                let Some(g) = crate::collect::eye_center(&cal.eyes[e], w) else { continue };
                du.push(truth.0 - g.0);
                dv.push(truth.1 - g.1);
            }
            if du.len() >= 3 {
                let (mu, mv) = (median(du.clone()), median(dv.clone()));
                let before = (mu * mu + mv * mv).sqrt().atan().to_degrees();
                cal.eyes[e].drift.0 += mu;
                cal.eyes[e].drift.1 += mv;
                fx.push(Effect::Print(format!(
                    "{} eye: offset {:.2} deg removed ({} targets)",
                    eye_name(e),
                    before,
                    du.len()
                )));
            } else {
                fx.push(Effect::Print(format!("{} eye: too few targets, unchanged", eye_name(e))));
            }
        }
        cal.created_unix = report::unix_now();
        self.calibration = Some(cal);
        self.status = "MENU / Y: SAVE THE RECENTRED CALIBRATION".into();
        self.phase = Phase::Results;
        fx
    }

    fn finish_service(&mut self) -> Vec<Effect> {
        let errs: Vec<f64> = self.service_err.iter().flatten().copied().collect();
        let s = stats(&errs);
        self.status = if errs.is_empty() {
            "RUNTIME CHECK: NO GAZE".into()
        } else {
            format!("RUNTIME CHECK: MEAN {:.2} MAX {:.2} DEG", s.mean, s.max)
        };
        self.phase = Phase::ServiceCursor;
        vec![Effect::Print(format!(
            "end-to-end through the runtime: {} of {} targets, error mean {:.2} median {:.2} max {:.2} deg",
            errs.len(),
            self.service_err.len(),
            s.mean,
            s.median,
            s.max
        ))]
    }

    fn finish_pursuit(&mut self, t0: f64) -> Vec<Effect> {
        self.phase = Phase::Results;
        let Some(cal) = self.calibration.clone() else {
            return vec![];
        };
        let mut st = CombineState::default();
        let gaze: Vec<(f64, (f64, f64))> = self
            .pursuit_samples
            .iter()
            .enumerate()
            .filter_map(|(i, rs)| {
                let mut s = rs.to_sample(i as u64);
                cal.apply_sample(&mut s, &mut st);
                Some((rs.t, tangent_of(s.gaze?.direction.map(f64::from))?))
            })
            .collect();
        let start = t0 + 1.0;
        let m = mid(&self.eye_pos);
        let truth_at = |t: f64| {
            let p = pursuit_at((t - start).max(0.0));
            truth_tangent(&p, m)
        };
        // drop catch-up saccades: gaze speed far above the target's 4.5 deg/s
        let keep: Vec<bool> = (0..gaze.len())
            .map(|i| {
                let (a, b) = (i.saturating_sub(2), (i + 2).min(gaze.len().saturating_sub(1)));
                let dt = gaze[b].0 - gaze[a].0;
                dt <= 0.0 || tangent_angle_deg(gaze[a].1, gaze[b].1) / dt < 40.0
            })
            .collect();
        let mut best: Option<(f64, Stats)> = None;
        for k in 0..=25 {
            let lag = k as f64 * 0.01;
            let errs: Vec<f64> = gaze
                .iter()
                .zip(&keep)
                .filter(|(_, k)| **k)
                .filter_map(|((t, g), _)| truth_at(t - lag).map(|tr| tangent_angle_deg(*g, tr)))
                .collect();
            let s = stats(&errs);
            if best.as_ref().is_none_or(|b| s.mean < b.1.mean) {
                best = Some((lag, s));
            }
        }
        let used = if self.pursuit_samples.is_empty() {
            0.0
        } else {
            keep.iter().filter(|k| **k).count() as f64 / self.pursuit_samples.len() as f64
        };
        let Some((lag, err)) = best else { return vec![] };
        self.pursuit_res = Some(PursuitRes { lag_s: lag, err, used });
        vec![
            Effect::Print(format!(
                "smooth pursuit: error mean {:.2} median {:.2} p95 {:.2} deg at lag {:.0} ms ({:.0}% of samples)",
                err.mean,
                err.median,
                err.p95,
                lag * 1000.0,
                100.0 * used
            )),
            Effect::SaveResults(report::json(self)),
        ]
    }

    fn update_cursor(&mut self, samples: &[RawSample]) {
        let Some(last) = samples.last() else {
            if self.recent.back().is_none_or(|s| s.t < self.now - 0.3) {
                self.cursor = None;
                self.raw_cursor = None;
            }
            return;
        };
        // raw combined (identity maps), for the comparison
        let mut raw = Calibration::identity(self.s.conv, 0.064);
        for e in 0..2 {
            raw.eyes[e].origin_m = self.eye_pos[e];
        }
        let mut rs = CombineState::default();
        let mut s = last.to_sample(0);
        raw.apply_sample(&mut s, &mut rs);
        self.raw_cursor = s.gaze.map(|g| g.direction.map(f64::from));
        let Some(cal) = self.calibration.as_ref() else {
            self.cursor = None;
            return;
        };
        for rs in samples {
            let mut s = rs.to_sample(0);
            cal.apply_sample(&mut s, &mut self.cursor_state.0);
            pimax_eye::filter::apply_sample(&cal.filter, &mut self.cursor_state.1, &mut s);
            self.cursor = s.gaze.map(|g| g.direction.map(f64::from));
        }
    }

    /// The calibrated gaze cursor (head frame direction), when there is one.
    pub fn cursor(&self) -> Option<[f64; 3]> {
        self.cursor
    }

    /// What to draw now.
    pub fn scene(&self) -> Scene {
        let mut marks = Vec::new();
        let mut hud = Vec::new();
        let mut background = BACKGROUND;
        let at = |d: [f64; 3]| [d[0] * TARGET_DIST_M, d[1] * TARGET_DIST_M, d[2] * TARGET_DIST_M];
        let ratio = self.eye_ratio();
        let eye_status = format!(
            "EYES SEEN: LEFT {:.0}%  RIGHT {:.0}%",
            100.0 * ratio[0],
            100.0 * ratio[1]
        );
        match self.phase {
            Phase::Intro => {
                abc(&mut marks, at([0.0, 0.0, -1.0]), WHITE);
                hud.push(match self.s.mode {
                    Mode::Full => "EYE CALIBRATION".into(),
                    Mode::Recenter => "EYE RECENTRE".into(),
                });
                hud.push("KEEP YOUR HEAD STILL".into());
                hud.push("FOLLOW THE DOTS WITH YOUR EYES ONLY".into());
                hud.push(eye_status);
                if !self.status.is_empty() {
                    hud.push(self.status.clone());
                }
                hud.push(if self.s.pattern == Pattern::Pimax { "A: START   B: EXIT" } else { "A / SPACE: START" }.into());
            }
            Phase::Point { set, i, stage, t0, .. } if self.s.pattern == Pattern::Pimax && set == Set::Calibration => {
                // Pimax Play's look: a green dot that travels to the next place, black or white background
                let tg = self.target(set, i);
                let dt = self.now - t0;
                if tg.bright {
                    background = PIMAX_WHITE;
                }
                let dot = if tg.bright { PIMAX_GREEN_ON_WHITE } else { GREEN };
                let p = match (stage, i) {
                    (Stage::Attract, 1..) => {
                        let from = self.target(set, i - 1).dir();
                        let f = smoothstep((dt / self.s.attract_s).clamp(0.0, 1.0));
                        let to = tg.dir();
                        let d = [0, 1, 2].map(|k| from[k] + (to[k] - from[k]) * f);
                        let n = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                        d.map(|x| x / n * crate::layout::TARGET_DIST_M)
                    }
                    _ => tg.point(),
                };
                match stage {
                    Stage::HeadMoved => {
                        abc_on(&mut marks, p, AMBER, background);
                        hud.push("HEAD MOVED: KEEP IT STILL".into());
                    }
                    Stage::Accepted => abc_on(&mut marks, p, WHITE.map(|c| c * 0.6 + 0.2), background),
                    _ => abc_on(&mut marks, p, dot, background),
                }
            }
            Phase::Point { set, i, stage, t0, .. } => {
                let tg = self.target(set, i);
                let p = tg.point();
                let dt = self.now - t0;
                match stage {
                    Stage::Attract => {
                        let f = (dt / self.s.attract_s).clamp(0.0, 1.0);
                        marks.push(Mark {
                            pos: p,
                            kind: MarkKind::Ring,
                            radius_deg: 3.0 + (0.75 - 3.0) * f,
                            width_deg: 0.15,
                            color: GREY,
                        });
                        abc(&mut marks, p, WHITE);
                    }
                    Stage::Hold | Stage::Collect => abc(&mut marks, p, WHITE),
                    Stage::Accepted => abc(&mut marks, p, GREEN),
                    Stage::HeadMoved => {
                        abc(&mut marks, p, AMBER);
                        hud.push("HEAD MOVED: KEEP IT STILL".into());
                        hud.push("ONLY YOUR EYES FOLLOW THE DOT".into());
                    }
                }
            }
            Phase::Pursuit { t0 } => {
                let tg = pursuit_at((self.now - t0 - 1.0).max(0.0));
                abc(&mut marks, tg.point(), WHITE);
                if self.now - t0 < 1.0 {
                    hud.push("FOLLOW THE MOVING DOT".into());
                }
            }
            Phase::Results | Phase::Cursor if self.s.pattern == Pattern::Pimax => {
                hud.extend(report::summary(self).hud);
                hud.push(String::new());
                hud.push("A: CALIBRATE AGAIN".into());
                hud.push(if self.calibration.is_some() { "B: SAVE AND EXIT" } else { "B: EXIT" }.into());
            }
            Phase::Results | Phase::Cursor => {
                let sum = report::summary(self);
                hud.extend(sum.hud);
                if !self.status.is_empty() {
                    hud.push(self.status.clone());
                }
                if self.phase == Phase::Cursor {
                    for t in self.val_t.iter().chain(std::iter::once(&Target::new(0.0, 0.0))) {
                        marks.push(Mark {
                            pos: t.point(),
                            kind: MarkKind::Disc,
                            radius_deg: 0.25,
                            width_deg: 0.0,
                            color: GREY,
                        });
                    }
                    if let Some(d) = self.raw_cursor {
                        ring(&mut marks, at(d), 0.6, [0.4, 0.4, 0.45]);
                    }
                    if let Some(d) = self.cursor {
                        ring(&mut marks, at(d), 0.8, GREEN);
                    }
                    hud.push("GREEN: CALIBRATED  GREY: RAW".into());
                    hud.push("A: RESULTS  MENU/Y: SAVE  S: RUNTIME CHECK".into());
                } else {
                    hud.push(if self.calibration.is_some() {
                        "A: LIVE CURSOR  MENU/Y: SAVE  P: PURSUIT  B: REDO".into()
                    } else {
                        "B: START OVER".into()
                    });
                }
            }
            Phase::ServiceWait { .. } => {
                abc(&mut marks, at([0.0, 0.0, -1.0]), WHITE);
                hud.push("STARTING THE RUNTIME'S EYE TRACKING".into());
            }
            Phase::ServiceCursor => {
                for t in service_targets() {
                    marks.push(Mark {
                        pos: t.point(),
                        kind: MarkKind::Disc,
                        radius_deg: 0.25,
                        width_deg: 0.0,
                        color: GREY,
                    });
                }
                if let Some(d) = self.service_cursor {
                    ring(&mut marks, at(d), 0.8, [0.3, 0.8, 1.0]);
                }
                hud.push("BLUE: GAZE FROM THE RUNTIME (WHAT APPS GET)".into());
                if !self.status.is_empty() {
                    hud.push(self.status.clone());
                }
                hud.push("A: BACK".into());
            }
        }
        Scene { marks, hud, background }
    }

    /// Measuring now (the caller hides distractions).
    pub fn measuring(&self) -> bool {
        matches!(self.phase, Phase::Point { .. } | Phase::Pursuit { .. })
    }

    /// The eye rendering positions in use.
    pub fn eye_positions(&self) -> [[f64; 3]; 2] {
        self.eye_pos
    }

    /// Calibration targets.
    pub fn calibration_targets(&self) -> &[Target] {
        &self.cal_t
    }

    /// Validation targets.
    pub fn validation_targets(&self) -> &[Target] {
        &self.val_t
    }
}

/// Background of the white half of the Pimax path.
const PIMAX_WHITE: [f32; 3] = [0.92, 0.92, 0.92];
/// The green dot on white (the dark-background green is too light there).
const PIMAX_GREEN_ON_WHITE: [f32; 3] = [0.0, 0.55, 0.15];

fn smoothstep(f: f64) -> f64 {
    f * f * (3.0 - 2.0 * f)
}

/// The fixation target recommended by Thaler et al. (2013): a disc with a cross cut out and a dot in the middle.
fn abc(marks: &mut Vec<Mark>, p: [f64; 3], color: [f32; 3]) {
    abc_on(marks, p, color, BACKGROUND)
}

/// [`abc`] on another background (the cross is cut in the background colour).
fn abc_on(marks: &mut Vec<Mark>, p: [f64; 3], color: [f32; 3], background: [f32; 3]) {
    marks.push(Mark { pos: p, kind: MarkKind::Disc, radius_deg: 0.45, width_deg: 0.0, color });
    marks.push(Mark { pos: p, kind: MarkKind::Cross, radius_deg: 0.47, width_deg: 0.09, color: background });
    marks.push(Mark { pos: p, kind: MarkKind::Disc, radius_deg: 0.08, width_deg: 0.0, color: if color == GREEN { GREEN } else { RED } });
}

fn ring(marks: &mut Vec<Mark>, p: [f64; 3], r: f64, color: [f32; 3]) {
    marks.push(Mark { pos: p, kind: MarkKind::Ring, radius_deg: r, width_deg: 0.14, color });
}

fn angle(a: [f64; 3], b: [f64; 3]) -> f64 {
    let c = [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let s = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
    s.atan2(a[0] * b[0] + a[1] * b[1] + a[2] * b[2]).to_degrees()
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    if n == 0 {
        0.0
    } else if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

/// `left` / `right`.
pub fn eye_name(e: usize) -> &'static str {
    if e == 0 {
        "left"
    } else {
        "right"
    }
}

fn fmt_opt(x: Option<f64>) -> String {
    x.map_or("-".to_string(), |v| format!("{v:.2}"))
}

/// Regions of the validation targets with their combined errors.
pub fn region_errors(p: &Procedure) -> Vec<(Region, Vec<f64>)> {
    Region::ALL
        .iter()
        .map(|r| {
            (
                *r,
                p.val_res
                    .iter()
                    .filter(|v| Region::of(&v.point.target) == *r)
                    .filter_map(|v| v.eval.comb_err_deg)
                    .collect(),
            )
        })
        .collect()
}
