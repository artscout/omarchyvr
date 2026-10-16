//! The per-LED-frame controller tracker shared by `live` (broker) and `lockrate` (offline replay of a `.pxcam`).
//!
//! Per LED frame: every hand slot with a recent fix is tracked from its prior (match within a pixel gate, joint
//! refine over all cameras); a slot that is not tracked triggers the brute-force search. On top of the original
//! tracker (`TrackCfg::baseline`), the default config adds (analysis/ctrl-lock-rate.md):
//! * a constant-velocity prediction of the prior (position and rotation from the last two fixes);
//! * a wider pixel gate when the controller moves fast or the prior is older than one frame;
//! * a constrained 3-4 LED track: with the orientation tied to the prediction (rotation prior in the LM), three
//!   or four matched LEDs give a position fix where the original needed five. Few-LED fixes are limited to a
//!   streak of `few_streak_max` frames without a 5+ LED fix, so a wrong lock cannot persist.

use crate::track::{claim, solve_frame, Found, LedFrame, Rig};
use pimax_cam::frame::{TILE_H, TILE_W};
use pimax_cam::geom::{self, Pose, Vec3};
use pimax_ctrltrack::assign::{Body, Cand, Slot};
use pimax_ctrltrack::solve::{self, CamObs, JointSolve, Obs};

#[derive(Clone, Copy, Debug)]
pub struct TrackCfg {
    /// Constant-velocity prediction of the prior.
    pub predict: bool,
    /// Second, wider prior gate (px) for fast motion or an older prior; 0 = off.
    pub wide_gate_px: f64,
    /// Fewest LEDs for a constrained (rotation-prior) track from a prior; 5 or more = off.
    pub few_leds: usize,
    /// Most consecutive few-LED fixes per slot before its prior is dropped.
    pub few_streak_max: u32,
    /// Rotation prior weight (px per rad) for few-LED tracks.
    pub rot_w: f64,
    /// Body model (reach, side, continuity, collapse); off = `--no-body`.
    pub body: bool,
}

impl TrackCfg {
    /// The tracker before 2026-10-15 (prior = last fix, 12 px gate, 5+ LEDs).
    pub fn baseline() -> Self {
        TrackCfg {
            predict: false,
            wide_gate_px: 0.0,
            few_leds: 5,
            few_streak_max: 0,
            rot_w: 0.0,
            body: true,
        }
    }
}

impl Default for TrackCfg {
    fn default() -> Self {
        TrackCfg {
            predict: true,
            wide_gate_px: 24.0,
            few_leds: 3,
            few_streak_max: 12,
            rot_w: 300.0,
            body: true,
        }
    }
}

#[derive(Clone, Copy)]
struct SlotState {
    model: usize,
    pose: Pose,
    ns: u64,
    /// (linear m/s, angular rad/s as a rotation vector in the body frame)
    vel: Option<(Vec3, Vec3)>,
    few_streak: u32,
}

#[derive(Default, Clone, Copy, Debug)]
pub struct Stats {
    pub led_frames: u64,
    pub hand: [u64; 2],
    pub few: [u64; 2],
    pub tries: u64,
    pub searches: u64,
    pub unassigned: u64,
    pub collapsed: u64,
}

pub struct Tracker {
    pub cfg: TrackCfg,
    prev: [Option<SlotState>; 2],
    last_seen: [Option<([f64; 3], u64)>; 2],
    last_fix_ns: u64,
    frame_no: u64,
    body: Body,
    pub stats: Stats,
    /// Slots to look for (left, right). A controller put down (Monado's PIMAX_CTRL_INHAND status) is neither tracked
    /// nor searched, and fixes assigned to it are dropped: less CPU, and its LEDs on the desk are never published.
    pub wanted: [bool; 2],
}

/// One tracked hand this frame.
pub struct Fix {
    pub slot: usize,
    pub found: Found,
    /// From the constrained few-LED path.
    pub few: bool,
}

const STALE_NS: u64 = 300_000_000;
const MAX_EXTRAP_S: f64 = 0.12;
const MAX_V: f64 = 3.0;
const MAX_W: f64 = 10.0;

fn cap(v: Vec3, m: f64) -> Vec3 {
    let n = geom::norm(v);
    if n > m {
        geom::scale(v, m / n)
    } else {
        v
    }
}

impl Tracker {
    pub fn new(cfg: TrackCfg) -> Self {
        Tracker {
            cfg,
            prev: [None, None],
            last_seen: [None, None],
            last_fix_ns: 0,
            frame_no: 0,
            body: Body::default(),
            stats: Stats::default(),
            wanted: [true, true],
        }
    }

    /// Last fix of a slot (body_from_model, frame ns).
    pub fn last(&self, slot: usize) -> Option<(usize, Pose, u64)> {
        self.prev[slot].map(|s| (s.model, s.pose, s.ns))
    }

    fn predicted(&self, st: &SlotState, frame_ns: u64) -> Pose {
        let dt = (frame_ns.saturating_sub(st.ns) as f64 * 1e-9).min(MAX_EXTRAP_S);
        match (self.cfg.predict, st.vel) {
            (true, Some((v, w))) => Pose {
                r: geom::mat_mul(&st.pose.r, &geom::rodrigues(geom::scale(w, dt))),
                t: geom::add(st.pose.t, geom::scale(v, dt)),
            },
            _ => st.pose,
        }
    }

    /// Track one slot from its prior. Returns the fix and whether it came from the few-LED path.
    fn track_slot(
        &self,
        rig: &Rig,
        obs: &[CamObs],
        used: &[Vec<bool>],
        st: &SlotState,
        frame_ns: u64,
    ) -> Option<(Found, bool)> {
        let prm = &rig.prm;
        let model = &rig.models[st.model];
        let pred = self.predicted(st, frame_ns);
        let dt = frame_ns.saturating_sub(st.ns) as f64 * 1e-9;
        let speed = st.vel.map(|(v, _)| geom::norm(v)).unwrap_or(0.0);
        let mut gates = vec![prm.prior_tol_px];
        if self.cfg.wide_gate_px > prm.prior_tol_px && (speed > 0.3 || dt > 0.06) {
            gates.push(self.cfg.wide_gate_px);
        }
        let mut starts = vec![pred];
        if self.cfg.predict && st.vel.is_some() {
            starts.push(st.pose);
        }
        let found = |j: JointSolve| Found {
            model: st.model,
            joint: j,
            alt: None,
            cam_spread_m: 0.0,
            single: 0,
        };
        let cos_face = prm.facing_deg.to_radians().cos();
        let mut best_few: Option<JointSolve> = None;
        for start in &starts {
            for &gate in &gates {
                let per_cam = match_all(start, rig, model, obs, used, cos_face, gate);
                let total: usize = per_cam.iter().map(|m| m.len()).sum();
                if total >= 4 {
                    // the original path: free 6DoF refine, rematch at the normal tolerance, 5+ LEDs
                    let (p1, _) =
                        solve::refine(*start, &obs_of(rig, model, obs, &per_cam), true, 25);
                    let m2 = match_all(&p1, rig, model, obs, used, cos_face, prm.tol_px);
                    let n2: usize = m2.iter().map(|m| m.len()).sum();
                    if n2 >= 4 {
                        let (p2, rms) = solve::refine(p1, &obs_of(rig, model, obs, &m2), true, 25);
                        if n2 >= 5 && rms <= prm.tol_px {
                            return Some((
                                found(JointSolve {
                                    body_from_model: p2,
                                    matches: m2,
                                    rms_px: rms,
                                }),
                                false,
                            ));
                        }
                    }
                }
                if self.cfg.few_leds < 5
                    && best_few.is_none()
                    && total >= self.cfg.few_leds
                    && st.few_streak < self.cfg.few_streak_max
                {
                    best_few = self.few_led(rig, model, obs, used, &pred, &per_cam, dt, cos_face);
                }
            }
        }
        best_few.map(|j| (found(j), true))
    }

    /// Constrained few-LED solve: refine with the rotation tied to the prediction, rematch at 1.5x the normal
    /// tolerance, accept 3-4 (or more, if the free solve failed) LEDs with a small residual near the prediction.
    #[allow(clippy::too_many_arguments)]
    fn few_led(
        &self,
        rig: &Rig,
        model: &pimax_ctrltrack::model::LedModel,
        obs: &[CamObs],
        used: &[Vec<bool>],
        pred: &Pose,
        per_cam: &[Vec<(usize, usize)>],
        dt: f64,
        cos_face: f64,
    ) -> Option<JointSolve> {
        let prm = &rig.prm;
        let prior = Some((pred.r, self.cfg.rot_w));
        let (p1, _) = solve::refine_with(*pred, &obs_of(rig, model, obs, per_cam), true, 20, prior);
        let m2 = match_all(&p1, rig, model, obs, used, cos_face, prm.tol_px * 1.5);
        let n2: usize = m2.iter().map(|m| m.len()).sum();
        if n2 < self.cfg.few_leds {
            return None;
        }
        let (p2, rms) = solve::refine_with(p1, &obs_of(rig, model, obs, &m2), true, 20, prior);
        let moved = geom::norm(geom::sub(p2.t, pred.t));
        let turned = geom::norm(solve::rotvec(&geom::mat_mul(
            &p2.r,
            &geom::transpose(&pred.r),
        )));
        // 3 LEDs leave little redundancy: demand a tighter fit
        let rms_max = if n2 == 3 { 1.2 } else { 1.8 };
        (rms <= rms_max && moved <= 0.03 + 0.75 * dt && turned <= 20f64.to_radians()).then_some(
            JointSolve {
                body_from_model: p2,
                matches: m2,
                rms_px: rms,
            },
        )
    }

    /// Process one LED frame (blobs per camera in `lf`, host time `frame_ns`).
    pub fn step(&mut self, rig: &Rig, lf: &LedFrame, frame_ns: u64) -> Vec<Fix> {
        self.frame_no += 1;
        self.stats.led_frames += 1;
        for p in self.prev.iter_mut() {
            if p.is_some_and(|s| frame_ns.saturating_sub(s.ns) > STALE_NS) {
                *p = None;
            }
        }
        let obs: Vec<CamObs> = rig
            .cams
            .iter()
            .zip(&lf.px)
            .map(|(c, px)| CamObs::new(c, px.clone()))
            .collect();
        let mut used: Vec<Vec<bool>> = obs.iter().map(|o| vec![false; o.px.len()]).collect();
        let mut tracked: Vec<(usize, Found, bool)> = Vec::new();
        for slot in 0..2 {
            if !self.wanted[slot] {
                self.prev[slot] = None;
                continue;
            }
            let Some(st) = self.prev[slot] else { continue };
            self.stats.tries += 1;
            if let Some((f, few)) = self.track_slot(rig, &obs, &used, &st, frame_ns) {
                claim(&mut used, &f.joint);
                tracked.push((slot, f, few));
            }
        }
        let body = &self.body;
        let last_seen = self.last_seen;
        if self.cfg.body {
            tracked.retain(|(s, f, _)| {
                let p = f.joint.body_from_model.t;
                let jump_ok = last_seen[*s].is_none_or(|(lp, ns)| {
                    let age = frame_ns.saturating_sub(ns) as f64 * 1e-9;
                    age >= body.memory_s
                        || geom::norm(geom::sub(p, lp)) <= 0.12 + body.max_speed * age
                });
                jump_ok && body.implausible(*s, p).is_none()
            });
            if tracked.len() == 2 {
                let now = [
                    tracked[0].1.joint.body_from_model.t,
                    tracked[1].1.joint.body_from_model.t,
                ];
                let last = [
                    last_seen[tracked[0].0].map(|x| x.0),
                    last_seen[tracked[1].0].map(|x| x.0),
                ];
                if let Some(k) = body.collapsed(now, last) {
                    tracked.remove(k);
                    self.stats.collapsed += 1;
                }
            }
        }
        if !tracked.is_empty() {
            self.last_fix_ns = frame_ns;
        }
        // something missing: brute-force search (about 10 ms over 4 threads); with nothing in view for 2 s, only
        // every third LED frame (CPU)
        let idle = tracked.is_empty() && frame_ns.saturating_sub(self.last_fix_ns) > 2_000_000_000;
        let mut searched = Vec::new();
        let missing = (0..2).any(|s| self.wanted[s] && !tracked.iter().any(|(t, _, _)| *t == s));
        if missing && (!idle || self.frame_no % 3 == 0) {
            let pre: Vec<Found> = tracked.iter().map(|(_, f, _)| f.clone()).collect();
            let got = solve_frame(
                lf,
                &rig.cams,
                &rig.models,
                &rig.triples,
                &rig.prm,
                None,
                &pre,
                true,
            );
            self.stats.searches += 1;
            let cands: Vec<Cand> = got
                .iter()
                .map(|g| Cand {
                    p: g.joint.body_from_model.t,
                    model: g.model,
                })
                .collect();
            let mut slots = [Slot::default(), Slot::default()];
            for (s, sl) in slots.iter_mut().enumerate() {
                sl.last =
                    self.last_seen[s].map(|(p, ns)| (p, frame_ns.saturating_sub(ns) as f64 * 1e-9));
                sl.tracked_now = tracked
                    .iter()
                    .find(|(t, _, _)| *t == s)
                    .map(|(_, f, _)| f.joint.body_from_model.t);
            }
            let assigned = if self.cfg.body {
                self.body.assign(&cands, &slots)
            } else {
                legacy_assign(&cands, &slots)
            };
            self.stats.unassigned += (got.len() - assigned.len()) as u64;
            for (ci, slot) in assigned {
                if !self.wanted[slot] {
                    continue;
                }
                searched.push(slot);
                tracked.push((slot, got[ci].clone(), false));
            }
        }
        let mut out = Vec::new();
        for (slot, f, few) in tracked {
            let p = f.joint.body_from_model;
            let old = self.prev[slot];
            let mut vel = None;
            if let Some(o) = old.filter(|o| !searched.contains(&slot) && o.model == f.model) {
                let dt = frame_ns.saturating_sub(o.ns) as f64 * 1e-9;
                if dt > 0.005 && dt < 0.13 {
                    let v = geom::scale(geom::sub(p.t, o.pose.t), 1.0 / dt);
                    let w = geom::scale(
                        solve::rotvec(&geom::mat_mul(&geom::transpose(&o.pose.r), &p.r)),
                        1.0 / dt,
                    );
                    let (v0, w0) = o.vel.unwrap_or((v, w));
                    // few-LED rotations follow the prior: keep (decay) the old rate rather than measure one
                    let w = if few {
                        geom::scale(w0, 0.5)
                    } else {
                        geom::scale(geom::add(w, w0), 0.5)
                    };
                    vel = Some((
                        cap(geom::scale(geom::add(v, v0), 0.5), MAX_V),
                        cap(w, MAX_W),
                    ));
                }
            }
            let few_streak = if few {
                old.map(|o| o.few_streak).unwrap_or(0) + 1
            } else {
                0
            };
            self.prev[slot] = Some(SlotState {
                model: f.model,
                pose: p,
                ns: frame_ns,
                vel,
                few_streak,
            });
            self.last_seen[slot] = Some((p.t, frame_ns));
            self.stats.hand[slot] += 1;
            if few {
                self.stats.few[slot] += 1;
            }
            out.push(Fix {
                slot,
                found: f,
                few,
            });
        }
        out
    }
}

/// Pixel matches of `model` at `pose` in every camera.
pub fn match_all(
    pose: &Pose,
    rig: &Rig,
    model: &pimax_ctrltrack::model::LedModel,
    obs: &[CamObs],
    used: &[Vec<bool>],
    cos_face: f64,
    tol_px: f64,
) -> Vec<Vec<(usize, usize)>> {
    rig.cams
        .iter()
        .enumerate()
        .map(|(ci, cam)| {
            let cfm = cam.cam_from_body.compose(pose);
            solve::match_pixels(&cfm, model, cam, &obs[ci], &used[ci], cos_face, tol_px)
        })
        .collect()
}

fn obs_of<'a>(
    rig: &'a Rig,
    model: &pimax_ctrltrack::model::LedModel,
    obs: &[CamObs],
    per_cam: &[Vec<(usize, usize)>],
) -> Vec<Obs<'a>> {
    let mut o = Vec::new();
    for (ci, m) in per_cam.iter().enumerate() {
        for &(l, b) in m {
            o.push(Obs {
                cam: &rig.cams[ci],
                p: model.leds[l].p,
                px: obs[ci].px[b],
            });
        }
    }
    o
}

/// Facing LEDs of `model` at `pose` that project inside each camera's image, and the blobs within `near_px` of
/// one of them (loss classification).
pub fn visibility(
    rig: &Rig,
    model_i: usize,
    pose: &Pose,
    px: &[Vec<(f64, f64)>],
    near_px: f64,
) -> (usize, usize) {
    let model = &rig.models[model_i];
    let cos_face = rig.prm.facing_deg.to_radians().cos();
    let (mut inview, mut near) = (0, 0);
    for (ci, cam) in rig.cams.iter().enumerate() {
        let cfm = cam.cam_from_body.compose(pose);
        let mut proj = Vec::new();
        for led in &model.leds {
            let pc = cfm.apply(led.p);
            if pc[2] <= 0.0 {
                continue;
            }
            let n = geom::mat_vec(&cfm.r, led.n);
            if geom::dot(n, geom::scale(geom::normalize(pc), -1.0)) < cos_face {
                continue;
            }
            if let Some((u, v)) = cam.intr.project(pc) {
                if u >= 0.0 && v >= 0.0 && u < TILE_W as f64 && v < TILE_H as f64 {
                    proj.push((u, v));
                }
            }
        }
        inview += proj.len();
        near += px[ci]
            .iter()
            .filter(|b| {
                proj.iter()
                    .any(|p| (p.0 - b.0).powi(2) + (p.1 - b.1).powi(2) <= near_px * near_px)
            })
            .count();
    }
    (inview, near)
}

/// The assignment before the body model (A/B: `--no-body`): two new poses by x, one into the only free slot, else
/// by side / shape label.
fn legacy_assign(cands: &[Cand], slots: &[Slot; 2]) -> Vec<(usize, usize)> {
    let free: Vec<usize> = (0..2).filter(|&s| slots[s].tracked_now.is_none()).collect();
    let mut idx: Vec<usize> = (0..cands.len()).collect();
    idx.sort_by(|&a, &b| cands[a].p[0].partial_cmp(&cands[b].p[0]).unwrap());
    if idx.len() == 2 && free.len() == 2 {
        return vec![(idx[0], 0), (idx[1], 1)];
    }
    let mut out = Vec::new();
    let mut free = free;
    for c in idx {
        let slot = match free.len() {
            0 => break,
            1 => free[0],
            _ => {
                let x = cands[c].p[0];
                if x < -0.08 {
                    0
                } else if x > 0.08 {
                    1
                } else {
                    cands[c].model
                }
            }
        };
        free.retain(|&s| s != slot);
        out.push((c, slot));
    }
    out
}
