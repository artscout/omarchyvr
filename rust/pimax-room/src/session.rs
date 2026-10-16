//! One tracking session against a room map: relocalise at the start (the room comes back where it was), add
//! keyframes where the map has no coverage yet, refine the alignment from time to time (drift), and hand the
//! map back for saving at the end.
//!
//! The driver feeds frame sets with the tracker's pose of the same frame and the supervisor's current
//! alignment; the session answers with alignment events which the supervisor applies (the head pose output
//! blends the change, so content slides into place instead of jumping).

use crate::map::{room_from_slam, tilt_of, RelocParams, RoomMap};
use crate::orb::{Image, OrbParams};
use crate::stereo::{features, triangulate, Features, Rig, StereoParams};
use pimax_cam::geom::{self, Pose, Vec3};
use pimax_cam::slam::{mat_of, quat_of, qyaw};

/// Session parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionParams {
    /// New keyframe when no keyframe is closer than this (m) with a similar view.
    pub kf_min_dist: f64,
    /// ... or the view direction differs by more than this (deg).
    pub kf_min_angle_deg: f64,
    /// Stereo points needed for a keyframe.
    pub kf_min_points: usize,
    /// Keyframe cap per map (oldest-session keyframes are kept; new ones stop).
    pub max_keyframes: usize,
    /// Keyframe spacing counts only this session's keyframes (a map update in another light: the same place gets
    /// a keyframe of the new session next to the old one, so relocalisation has both appearances).
    pub kf_per_session: bool,
    /// Interval between relocalisation attempts before the first success (ns).
    pub search_every_ns: i64,
    /// Interval between refinements after the first success (ns).
    pub refine_every_ns: i64,
    /// Candidates that must agree before the alignment is accepted.
    pub agree: usize,
    /// Agreement tolerance (m).
    pub agree_m: f64,
    /// Agreement tolerance (deg).
    pub agree_deg: f64,
    /// Refinements smaller than this are ignored (m) ...
    pub refine_min_m: f64,
    /// ... and (deg).
    pub refine_min_deg: f64,
    /// Refinements larger than this need `agree` consistent results too (m).
    pub refine_big_m: f64,
    /// Search radius around the predicted position when refining (m).
    pub refine_radius: f64,
    /// Features.
    pub orb: OrbParams,
    /// Stereo.
    pub stereo: StereoParams,
    /// Relocalisation.
    pub reloc: RelocParams,
}

impl Default for SessionParams {
    fn default() -> Self {
        SessionParams {
            kf_min_dist: 0.35,
            kf_min_angle_deg: 25.0,
            kf_min_points: 60,
            max_keyframes: 2000,
            kf_per_session: false,
            search_every_ns: 200_000_000,
            refine_every_ns: 2_000_000_000,
            agree: 2,
            agree_m: 0.05,
            agree_deg: 2.0,
            refine_min_m: 0.01,
            refine_min_deg: 0.3,
            refine_big_m: 0.3,
            refine_radius: 2.0,
            orb: OrbParams::default(),
            stereo: StereoParams::default(),
            reloc: RelocParams::default(),
        }
    }
}

/// Alignment `room_from_slam` (yaw rad, translation m).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Alignment {
    /// Yaw.
    pub yaw: f64,
    /// Translation.
    pub t: Vec3,
}

impl Alignment {
    /// As a pose.
    pub fn pose(&self) -> Pose {
        Pose {
            r: mat_of(qyaw(self.yaw)),
            t: self.t,
        }
    }

    fn close(&self, o: &Alignment, m: f64, deg: f64) -> bool {
        geom::norm(geom::sub(self.t, o.t)) <= m && wrap(self.yaw - o.yaw).abs().to_degrees() <= deg
    }
}

fn wrap(mut a: f64) -> f64 {
    while a > std::f64::consts::PI {
        a -= 2.0 * std::f64::consts::PI;
    }
    while a < -std::f64::consts::PI {
        a += 2.0 * std::f64::consts::PI;
    }
    a
}

/// What a frame changed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    /// The room was recognised: use this alignment (first time in the session).
    Relocalized(Alignment),
    /// The alignment was refined (drift correction).
    Refined(Alignment),
    /// A keyframe was added (id).
    Keyframe(u32),
}

/// Counters.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SessionStats {
    /// Relocalisation attempts.
    pub attempts: u64,
    /// Successful relocalisations (any).
    pub found: u64,
    /// Keyframes added.
    pub keyframes: u64,
    /// Refinements applied.
    pub refined: u64,
    /// Time of the first accepted relocalisation (ns since the first frame), if any.
    pub first_fix_ns: Option<i64>,
}

/// The session.
#[derive(Clone, Debug)]
pub struct RoomSession {
    /// The map (grows during the session).
    pub map: RoomMap,
    rig: Rig,
    /// Parameters.
    pub p: SessionParams,
    aligned: Option<Alignment>,
    candidates: Vec<Alignment>,
    last_try_ns: Option<i64>,
    first_ns: Option<i64>,
    had_map: bool,
    /// Counters.
    pub stats: SessionStats,
}

impl RoomSession {
    /// Start a session on `map` (empty = first visit: the session's own world becomes the room frame).
    pub fn new(map: RoomMap, rig: Rig, p: SessionParams) -> RoomSession {
        let had_map = !map.keyframes.is_empty();
        RoomSession {
            map,
            rig,
            p,
            aligned: None,
            candidates: Vec::new(),
            last_try_ns: None,
            first_ns: None,
            had_map,
            stats: SessionStats::default(),
        }
    }

    /// A session on a saved map whose room frame is already known (`room_from_slam`, e.g. from `relocalize`):
    /// keyframes are added from the first frame, no relocalisation runs.
    pub fn with_alignment(map: RoomMap, rig: Rig, p: SessionParams, room_from_slam: Alignment) -> RoomSession {
        let mut s = RoomSession::new(map, rig, p);
        s.aligned = Some(room_from_slam);
        s.had_map = false;
        s
    }

    /// Accepted alignment, if any.
    pub fn alignment(&self) -> Option<Alignment> {
        self.aligned
    }

    /// One frame set. `imgs[i]` = image of rig camera i; `slam_from_body` = tracker pose of this frame (y-up
    /// tracker world, IMU frame); `world_from_slam` = the supervisor's current alignment (the room frame of a
    /// first session); `tracking` = the supervisor trusts the tracker.
    pub fn on_frame(
        &mut self,
        ts_ns: i64,
        imgs: &[Image],
        slam_from_body: &Pose,
        world_from_slam: &Alignment,
        tracking: bool,
    ) -> Vec<Event> {
        let mut ev = Vec::new();
        self.first_ns.get_or_insert(ts_ns);
        if !tracking || imgs.len() < 2 || self.rig.cams.len() < 2 {
            return ev;
        }
        if !self.had_map && self.aligned.is_none() {
            // First visit: the room frame is this session's world.
            self.aligned = Some(*world_from_slam);
        }
        let due = |every: i64, last: Option<i64>| last.is_none_or(|l| ts_ns - l >= every);
        let searching = self.aligned.is_none();
        let want_reloc = self.had_map
            && if searching {
                due(self.p.search_every_ns, self.last_try_ns)
            } else {
                due(self.p.refine_every_ns, self.last_try_ns)
            };
        let room_from_body_now = self.aligned.map(|a| a.pose().compose(slam_from_body));
        let want_kf = room_from_body_now.is_some_and(|rb| {
            self.map.keyframes.len() < self.p.max_keyframes
                && self.map.wants_keyframe_in(
                    &rb,
                    self.p.kf_min_dist,
                    self.p.kf_min_angle_deg,
                    self.p.kf_per_session.then_some(self.map.sessions),
                )
        });
        if !want_reloc && !want_kf {
            return ev;
        }
        let feats: Vec<Features> = imgs
            .iter()
            .zip(self.rig.cams.iter())
            .map(|(im, c)| features(im, c, &self.p.orb))
            .collect();
        if want_reloc {
            self.last_try_ns = Some(ts_ns);
            self.stats.attempts += 1;
            let tilt = tilt_of(quat_of(&slam_from_body.r));
            let prior = room_from_body_now.map(|rb| (rb.t, self.p.refine_radius));
            if let Some(r) = self
                .map
                .relocalize(&feats, &self.rig, &tilt, prior, &self.p.reloc)
            {
                self.stats.found += 1;
                let (yaw, t) = room_from_slam(&r, slam_from_body);
                let cand = Alignment { yaw, t };
                self.candidates
                    .retain(|c| c.close(&cand, self.p.agree_m, self.p.agree_deg));
                self.candidates.push(cand);
                if self.candidates.len() >= self.p.agree.max(1) {
                    let n = self.candidates.len() as f64;
                    let mean_t = self
                        .candidates
                        .iter()
                        .fold([0.0; 3], |a, c| geom::add(a, geom::scale(c.t, 1.0 / n)));
                    let mean_yaw = cand.yaw
                        + self
                            .candidates
                            .iter()
                            .map(|c| wrap(c.yaw - cand.yaw))
                            .sum::<f64>()
                            / n;
                    let a = Alignment {
                        yaw: wrap(mean_yaw),
                        t: mean_t,
                    };
                    match self.aligned {
                        None => {
                            self.aligned = Some(a);
                            self.stats.first_fix_ns = Some(ts_ns - self.first_ns.unwrap_or(ts_ns));
                            ev.push(Event::Relocalized(a));
                        }
                        Some(cur) => {
                            let dm = geom::norm(geom::sub(a.t, cur.t));
                            let dd = wrap(a.yaw - cur.yaw).abs().to_degrees();
                            if dm >= self.p.refine_min_m || dd >= self.p.refine_min_deg {
                                self.aligned = Some(a);
                                self.stats.refined += 1;
                                ev.push(Event::Refined(a));
                            }
                        }
                    }
                    self.candidates.clear();
                } else if let Some(cur) = self.aligned {
                    // While aligned, a single small refinement is enough; a big one waits for agreement.
                    let dm = geom::norm(geom::sub(cand.t, cur.t));
                    if dm < self.p.refine_big_m && wrap(cand.yaw - cur.yaw).abs().to_degrees() < 5.0
                    {
                        let dd = wrap(cand.yaw - cur.yaw).abs().to_degrees();
                        if dm >= self.p.refine_min_m || dd >= self.p.refine_min_deg {
                            self.aligned = Some(cand);
                            self.stats.refined += 1;
                            ev.push(Event::Refined(cand));
                        }
                        self.candidates.clear();
                    }
                }
            }
        }
        // Keyframes only once the room frame is known (first visit: immediately).
        if let (true, Some(a)) = (want_kf, self.aligned) {
            let pts = triangulate(
                &feats[0],
                &feats[1],
                &self.rig.cams[0],
                &self.rig.cams[1],
                &self.p.stereo,
            );
            if pts.len() >= self.p.kf_min_points {
                let rb = a.pose().compose(slam_from_body);
                let id = self.map.add_keyframe(ts_ns, rb, &pts);
                self.stats.keyframes += 1;
                ev.push(Event::Keyframe(id));
            }
        }
        ev
    }

    /// End of the session: the grown map (save it).
    pub fn finish(mut self) -> RoomMap {
        self.map.sessions += 1;
        self.map
    }
}
