//! Image quality for mapping: is the room bright and textured enough, and how many stereo points does a frame give?
//!
//! The room map needs [`crate::pipeline::build_params`]' `kf_min_points` stereo points in a frame set before it makes
//! a keyframe. In a dark room (night, one monitor lit) the tracking cameras run at the auto-exposure ceiling and the
//! stereo point count of a frame drops below that: no keyframe is ever made and the map fails. This module measures
//! it (mean brightness, ORB corners per camera, stereo points) for `omarchyvr-roommap light` and for the live feedback of
//! the room setup ("turn on the lights" / "look at the furniture"), with no head pose needed.

use pimax_cam::frame::{self, FrameKind, Meta, TILE_H, TILE_W};
use pimax_cam::source::{self, ReplayOptions};
use omarchyvr_room::orb::{Image, OrbParams};
use omarchyvr_room::pngread::Grey;
use omarchyvr_room::stereo::{features_with, triangulate, LowLight, Rig, StereoParams, LOW_LIGHT};
use std::time::Instant;

/// Quality of one frame set (cameras 0 and 1).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameQuality {
    /// Mean grey level of both tiles (0..255).
    pub mean: f64,
    /// ORB corners per camera.
    pub corners: [usize; 2],
    /// Stereo points (what a keyframe is made of).
    pub stereo: usize,
    /// Exposure and gain of the frame (metadata; 0 when unknown).
    pub exposure: u16,
    /// Gain code.
    pub gain: u16,
}

/// Measure one frame set (with the low-light stretch the map uses).
pub fn measure(rig: &Rig, imgs: [&Grey; 2], orb: &OrbParams) -> FrameQuality {
    measure_with(rig, imgs, orb, Some(LOW_LIGHT))
}

/// [`measure`] with a chosen low-light stretch (None: off).
pub fn measure_with(rig: &Rig, imgs: [&Grey; 2], orb: &OrbParams, low: Option<LowLight>) -> FrameQuality {
    let feats: Vec<_> = imgs
        .iter()
        .zip(rig.cams.iter())
        .map(|(g, c)| features_with(&Image { w: g.w, h: g.h, stride: g.w, data: &g.data }, c, orb, low))
        .collect();
    let stereo = triangulate(&feats[0], &feats[1], &rig.cams[0], &rig.cams[1], &StereoParams::default()).len();
    let n: usize = imgs.iter().map(|g| g.data.len()).sum();
    let sum: u64 = imgs.iter().flat_map(|g| g.data.iter()).map(|&v| v as u64).sum();
    FrameQuality {
        mean: sum as f64 / n.max(1) as f64,
        corners: [feats[0].kps.len(), feats[1].kps.len()],
        stereo,
        exposure: 0,
        gain: 0,
    }
}

/// What the user should do, from the recent stereo point counts (`need` = points per keyframe).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Advice {
    /// Enough points: keep looking around.
    Good,
    /// Few points and a dark image: more light.
    TooDark,
    /// Few points in a bright enough image: look at furniture / textured things, not at blank walls.
    TooPlain,
}

/// Mean grey level below which a frame set counts as dark.
pub const DARK_MEAN: f64 = 35.0;

/// Advice from a frame's quality.
pub fn advice(q: &FrameQuality, need: usize) -> Advice {
    if q.stereo >= need {
        Advice::Good
    } else if q.mean < DARK_MEAN {
        Advice::TooDark
    } else {
        Advice::TooPlain
    }
}

/// Plain words for the headset panel / voice.
pub fn advice_text(a: Advice) -> &'static str {
    match a {
        Advice::Good => "Good: keep looking around slowly.",
        Advice::TooDark => "Too dark for the cameras: turn on the room lights.",
        Advice::TooPlain => "Too little detail here: look at furniture, shelves or the desk, not a blank wall.",
    }
}

/// Summary of a run of frames.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LightReport {
    /// Frame sets measured.
    pub frames: usize,
    /// Median mean grey.
    pub mean: f64,
    /// Median corners (camera 0, camera 1).
    pub corners: [usize; 2],
    /// Median stereo points.
    pub stereo: usize,
    /// Frames with at least `need` stereo points.
    pub good: usize,
    /// Last exposure / gain seen.
    pub exposure: u16,
    /// Gain.
    pub gain: u16,
}

fn median<T: Copy + PartialOrd + Default>(mut v: Vec<T>) -> T {
    if v.is_empty() {
        return T::default();
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v[v.len() / 2]
}

/// Summarise measured frames.
pub fn summarise(qs: &[FrameQuality], need: usize) -> LightReport {
    LightReport {
        frames: qs.len(),
        mean: median(qs.iter().map(|q| q.mean).collect()),
        corners: [median(qs.iter().map(|q| q.corners[0]).collect()), median(qs.iter().map(|q| q.corners[1]).collect())],
        stereo: median(qs.iter().map(|q| q.stereo).collect()),
        good: qs.iter().filter(|q| q.stereo >= need).count(),
        exposure: qs.last().map(|q| q.exposure).unwrap_or(0),
        gain: qs.last().map(|q| q.gain).unwrap_or(0),
    }
}

/// Simulated darker room (benchmarks from a lit recording): grey * `scale` plus deterministic noise of +-`noise`
/// grey levels (the sensor noise of a high gain), rounded and clamped.
pub fn darken(g: &mut Grey, scale: f32, noise: f32, seed: u64) {
    let mut x = seed | 1;
    for v in g.data.iter_mut() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let u = (x >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0;
        *v = (*v as f32 * scale + u * noise).round().clamp(0.0, 255.0) as u8;
    }
}

/// Options of [`measure_source`].
#[derive(Clone, Copy, Debug)]
pub struct MeasureOpts {
    /// Every n-th tracking frame set.
    pub every: usize,
    /// Low-light stretch (None: off).
    pub low: Option<LowLight>,
    /// Simulated darkening (scale, noise), None = the frames as they are.
    pub darken: Option<(f32, f32)>,
}

impl Default for MeasureOpts {
    fn default() -> Self {
        MeasureOpts { every: 3, low: Some(LOW_LIGHT), darken: None }
    }
}

/// Measure every `every`-th tracking frame set of `spec` (`broker`, or a `.pxcam` recording) for `seconds`
/// (a recording: until its end). Read-only; no head pose needed.
pub fn measure_source(spec: &str, seconds: f64, o: &MeasureOpts, rig: &Rig, orb: &OrbParams) -> Result<Vec<FrameQuality>, String> {
    let mut src = source::open(spec, 0, ReplayOptions { looping: false, realtime: false })
        .map_err(|e| format!("cannot open source {spec:?}: {e}"))?;
    let t0 = Instant::now();
    let mut out = Vec::new();
    let mut k = 0usize;
    while t0.elapsed().as_secs_f64() < seconds {
        let fr = match src.next(500) {
            Ok(Some(fr)) => fr,
            Ok(None) => {
                if src.finished() {
                    break;
                }
                continue;
            }
            Err(e) => return Err(format!("source: {e}")),
        };
        let Some(m) = Meta::parse(&fr.data[..frame::META_LEN]) else { continue };
        if m.kind() != FrameKind::Tracking {
            continue;
        }
        k += 1;
        if !k.is_multiple_of(o.every.max(1)) {
            continue;
        }
        let rows = frame::tile_rows(Some(&m), fr.data, fr.stride);
        let mut imgs: [Grey; 2] = std::array::from_fn(|_| Grey { w: TILE_W, h: TILE_H, data: vec![0; TILE_W * TILE_H] });
        for (ci, g) in imgs.iter_mut().enumerate() {
            frame::copy_tile(fr.data, fr.stride, ci, rows.y[ci], &mut g.data, TILE_W);
        }
        if let Some((sc, nz)) = o.darken {
            for (i, g) in imgs.iter_mut().enumerate() {
                darken(g, sc, nz, (k as u64) << 1 | i as u64);
            }
        }
        let mut q = measure_with(rig, [&imgs[0], &imgs[1]], orb, o.low);
        q.exposure = m.exposure[0];
        q.gain = m.gain[0];
        out.push(q);
    }
    Ok(out)
}

/// Quality from features already extracted (no second ORB pass).
pub fn quality_of(rig: &Rig, feats: &[omarchyvr_room::stereo::Features], imgs: [&Grey; 2]) -> FrameQuality {
    let stereo = triangulate(&feats[0], &feats[1], &rig.cams[0], &rig.cams[1], &StereoParams::default()).len();
    let n: usize = imgs.iter().map(|g| g.data.len()).sum();
    let sum: u64 = imgs.iter().flat_map(|g| g.data.iter()).map(|&v| v as u64).sum();
    FrameQuality { mean: sum as f64 / n.max(1) as f64, corners: [feats[0].kps.len(), feats[1].kps.len()], stereo, exposure: 0, gain: 0 }
}

/// Live mapping progress (for the headset panel).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MapProgress {
    /// Seconds since the first frame.
    pub elapsed_s: f64,
    /// Keyframes made so far in this session.
    pub keyframes: usize,
    /// The latest measured frame set.
    pub quality: FrameQuality,
    /// What to tell the user.
    pub advice: Advice,
    /// The head has not moved enough yet for more keyframes (all views so far are mapped).
    pub move_more: bool,
}

/// One line for the panel: "12 views mapped, 85 points in view. Good: keep looking around slowly."
pub fn progress_text(p: &MapProgress) -> String {
    let what = if p.advice == Advice::Good && p.move_more {
        "Turn your head further: left, right, behind you and down at the floor."
    } else {
        advice_text(p.advice)
    };
    format!("{} views mapped, {} points in view. {what}", p.keyframes, p.quality.stereo)
}

/// Has the head moved (someone wearing the headset and looking around)? True when the poses span more than
/// `min_deg` of view direction or `min_m` of position. A headset lying on the desk gives a few hundredths of a degree.
pub fn moved(poses: &[pimax_cam::geom::Pose], min_deg: f64, min_m: f64) -> bool {
    use pimax_cam::geom;
    let Some(first) = poses.first() else { return false };
    let f0 = geom::mat_vec(&first.r, [0.0, 0.0, -1.0]);
    poses.iter().any(|p| {
        geom::angle_deg(f0, geom::mat_vec(&p.r, [0.0, 0.0, -1.0])) > min_deg || geom::norm(geom::sub(p.t, first.t)) > min_m
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pimax_cam::geom::{self, Pose};

    #[test]
    fn advice_and_text() {
        let q = |mean, stereo| FrameQuality { mean, stereo, ..Default::default() };
        assert_eq!(advice(&q(8.0, 0), 40), Advice::TooDark);
        assert_eq!(advice(&q(60.0, 10), 40), Advice::TooPlain);
        assert_eq!(advice(&q(8.0, 55), 40), Advice::Good);
        let p = MapProgress { elapsed_s: 3.0, keyframes: 4, quality: q(8.0, 0), advice: Advice::TooDark, move_more: false };
        assert_eq!(progress_text(&p), "4 views mapped, 0 points in view. Too dark for the cameras: turn on the room lights.");
        let p = MapProgress { advice: Advice::Good, move_more: true, ..p };
        assert!(progress_text(&p).contains("Turn your head further"));
        let r = summarise(&[q(8.0, 0), q(10.0, 50), q(12.0, 60)], 40);
        assert_eq!((r.frames, r.mean, r.stereo, r.good), (3, 10.0, 50, 2));
    }

    #[test]
    fn head_motion() {
        let at = |yaw: f64, x: f64| Pose { r: geom::rodrigues([0.0, yaw.to_radians(), 0.0]), t: [x, 1.6, 0.0] };
        assert!(!moved(&[at(0.0, 0.0), at(0.05, 0.001)], 8.0, 0.1));
        assert!(moved(&[at(0.0, 0.0), at(12.0, 0.0)], 8.0, 0.1));
        assert!(moved(&[at(0.0, 0.0), at(0.0, 0.2)], 8.0, 0.1));
        assert!(!moved(&[], 8.0, 0.1));
    }

    #[test]
    fn darken_is_deterministic_and_dims() {
        let mut a = Grey { w: 4, h: 1, data: vec![200, 100, 0, 255] };
        let mut b = a.clone();
        darken(&mut a, 0.25, 0.0, 1);
        darken(&mut b, 0.25, 0.0, 1);
        assert_eq!(a.data, vec![50, 25, 0, 64]);
        assert_eq!(a.data, b.data);
    }
}
