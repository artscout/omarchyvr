//! Calibration hypotheses checked against real frames with the epipolar test of [`crate::rectify`].

use crate::calib::{DeviceCalib, RigConvention};
use crate::frame::{TILE_H, TILE_W};
use crate::rectify::{epipolar_check, EpipolarReport, MatchParams, RectifyParams, StereoRectifier};

/// One way of reading the calibration for a camera pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hypothesis {
    /// Calibration camera shown on the left of the rectified pair.
    pub cam_a: usize,
    /// Calibration camera on the right.
    pub cam_b: usize,
    /// Tile that holds camera a's picture.
    pub tile_a: usize,
    /// Tile that holds camera b's picture.
    pub tile_b: usize,
    /// Rig convention.
    pub conv: RigConvention,
    /// Use the two extra XML coefficients as k5, k6.
    pub extra_radial: bool,
}

impl Hypothesis {
    /// Default reading: tile i = camera i, convention 2, KB4.
    pub fn default_for(a: usize, b: usize) -> Hypothesis {
        Hypothesis {
            cam_a: a,
            cam_b: b,
            tile_a: a,
            tile_b: b,
            conv: RigConvention::CamFromCam0,
            extra_radial: false,
        }
    }

    /// Short label.
    pub fn label(&self) -> String {
        format!(
            "cams {}-{} tiles {}-{} {} {}",
            self.cam_a,
            self.cam_b,
            self.tile_a,
            self.tile_b,
            match self.conv {
                RigConvention::CamFromCam0 => "conv2(X_i=R*X_0+t)",
                RigConvention::Cam0FromCam => "conv1(X_0=R*X_i+t)",
            },
            if self.extra_radial { "kb6" } else { "kb4" }
        )
    }
}

/// Result of one hypothesis on one frame.
pub struct Outcome {
    /// What was tested.
    pub hyp: Hypothesis,
    /// Matches.
    pub report: EpipolarReport,
    /// Rectified left image.
    pub left: Vec<u8>,
    /// Rectified right image.
    pub right: Vec<u8>,
    /// Rectifier (for masks).
    pub rect: StereoRectifier,
}

/// Rectify the two tiles under `h` and run the epipolar check.
pub fn evaluate(
    cal: &DeviceCalib,
    tiles: &[Vec<u8>],
    h: &Hypothesis,
    rp: RectifyParams,
    mp: &MatchParams,
) -> Outcome {
    let pick = |i: usize| {
        let k = cal.cameras[i].intr.clone();
        if h.extra_radial {
            k.with_extra_radial()
        } else {
            k
        }
    };
    let (ka, kb) = (pick(h.cam_a), pick(h.cam_b));
    let pose = cal.stereo(h.cam_a, h.cam_b, h.conv);
    let rect = StereoRectifier::new(&ka, &kb, &pose, rp);
    let left = rect.remap(0, &tiles[h.tile_a], TILE_W, TILE_H);
    let right = rect.remap(1, &tiles[h.tile_b], TILE_W, TILE_H);
    let report = epipolar_check(&left, &right, &rect.valid_both(), rp.width, rp.height, mp);
    Outcome {
        hyp: *h,
        report,
        left,
        right,
        rect,
    }
}

/// Side-by-side RGB image of a rectified pair with a horizontal line every `every` rows (red) for viewing.
pub fn pair_with_lines(left: &[u8], right: &[u8], w: usize, h: usize, every: usize) -> Vec<u8> {
    let (l, r) = (crate::png::stretch(left), crate::png::stretch(right));
    let ow = 2 * w;
    let mut out = vec![0u8; ow * h * 3];
    for y in 0..h {
        for x in 0..ow {
            let v = if x < w {
                l[y * w + x]
            } else {
                r[y * w + x - w]
            };
            let o = (y * ow + x) * 3;
            if every > 0 && y % every == 0 {
                out[o] = 255;
                out[o + 1] = v / 3;
                out[o + 2] = v / 3;
            } else {
                out[o] = v;
                out[o + 1] = v;
                out[o + 2] = v;
            }
        }
    }
    out
}

/// Score of a hypothesis: fraction of matches within 1 px, scaled down when there are fewer than 30 matches.
pub fn score(r: &EpipolarReport) -> f64 {
    r.frac_within(1.0) * (r.matches.len() as f64 / 30.0).min(1.0)
}

/// Verdict over the hypotheses evaluated for one camera pair: the best one, and whether it is the default reading
/// (tiles = cameras, convention 2) with a convincing epipolar fit (>= 20 matches, >= 60 % within 1 px).
pub fn verdict(rows: &[(Hypothesis, EpipolarReport)]) -> Option<(Hypothesis, bool, String)> {
    let (best, rep) = rows
        .iter()
        .max_by(|a, b| score(&a.1).partial_cmp(&score(&b.1)).unwrap())?;
    let default = best.tile_a == best.cam_a
        && best.tile_b == best.cam_b
        && best.conv == RigConvention::CamFromCam0;
    let convincing = rep.matches.len() >= 20 && rep.frac_within(1.0) >= 0.6;
    let ok = default && convincing;
    let text = format!(
        "pair {}-{}: best {} ({} matches, {:.0}% within 1 px, median dy {:+.2}) -> {}",
        best.cam_a,
        best.cam_b,
        best.label(),
        rep.matches.len(),
        rep.frac_within(1.0) * 100.0,
        rep.median_dy(),
        if ok {
            "PASS: factory calibration conventions confirmed"
        } else if !convincing {
            "INCONCLUSIVE: too few or poor matches (light? texture? overlap?)"
        } else {
            "CHECK: a non-default reading fits better"
        }
    );
    Some((*best, ok, text))
}

/// Red-cyan anaglyph (left in red, right in green+blue).
pub fn anaglyph(left: &[u8], right: &[u8]) -> Vec<u8> {
    let (l, r) = (crate::png::stretch(left), crate::png::stretch(right));
    l.iter()
        .zip(r.iter())
        .flat_map(|(&a, &b)| [a, b, b])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom;

    fn rep(n: usize, dy: f64) -> EpipolarReport {
        EpipolarReport {
            candidates: n,
            matches: (0..n)
                .map(|i| crate::rectify::Match {
                    x: i,
                    y: 0,
                    disp: 5.0,
                    dy,
                    score: 0.9,
                })
                .collect(),
        }
    }

    #[test]
    fn verdict_prefers_well_aligned_default() {
        let d = Hypothesis::default_for(0, 1);
        let c1 = Hypothesis {
            conv: RigConvention::Cam0FromCam,
            ..d
        };
        let (best, ok, text) = verdict(&[(c1, rep(40, 6.0)), (d, rep(40, 0.2))]).unwrap();
        assert_eq!(best, d);
        assert!(ok && text.contains("PASS"), "{text}");
        let (best, ok, text) = verdict(&[(c1, rep(40, 0.1)), (d, rep(40, 5.0))]).unwrap();
        assert_eq!(best, c1);
        assert!(!ok && text.contains("CHECK"), "{text}");
        let (_, ok, text) = verdict(&[(d, rep(5, 0.0))]).unwrap();
        assert!(!ok && text.contains("INCONCLUSIVE"), "{text}");
        assert!(verdict(&[]).is_none());
    }

    /// Render a synthetic scene (textured planes) into the four tiles through the factory calibration, then
    /// check that the right hypothesis wins and the wrong convention loses.
    #[test]
    fn synthetic_scene_prefers_true_convention() {
        let cal =
            DeviceCalib::parse(include_str!("../tests/fixtures/headset-calibration.xml")).unwrap();
        // scene: a box of textured walls 1.5 m around camera 0; texture from position hash
        let tex = |p: [f64; 3]| -> u8 {
            let q = [
                (p[0] * 8.0).floor() as i64,
                (p[1] * 8.0).floor() as i64,
                (p[2] * 8.0).floor() as i64,
            ];
            let mut hsh = (q[0].wrapping_mul(73_856_093)
                ^ q[1].wrapping_mul(19_349_663)
                ^ q[2].wrapping_mul(83_492_791)) as u64;
            hsh ^= hsh >> 13;
            hsh = hsh.wrapping_mul(0x5bd1_e995);
            (40 + (hsh >> 56) % 180) as u8
        };
        let render = |cam: usize| -> Vec<u8> {
            let k = &cal.cameras[cam].intr;
            let cam0_from_cam = cal.cam_from_cam0(cam, RigConvention::CamFromCam0).inverse();
            let mut img = vec![0u8; TILE_W * TILE_H];
            for v in 0..TILE_H {
                for u in 0..TILE_W {
                    let Some(ray) = k.unproject(u as f64, v as f64) else {
                        continue;
                    };
                    let d = geom::mat_vec(&cam0_from_cam.r, ray);
                    let o = cam0_from_cam.t;
                    // intersect the axis-aligned box |x|,|y|,|z| <= 1.5 from inside
                    let mut tmin = f64::INFINITY;
                    for ax in 0..3 {
                        if d[ax].abs() > 1e-9 {
                            let t = ((if d[ax] > 0.0 { 1.5 } else { -1.5 }) - o[ax]) / d[ax];
                            if t > 0.0 && t < tmin {
                                tmin = t;
                            }
                        }
                    }
                    img[v * TILE_W + u] = tex(geom::add(o, geom::scale(d, tmin)));
                }
            }
            img
        };
        let tiles: Vec<Vec<u8>> = (0..2).map(render).collect();
        let rp = RectifyParams {
            width: 320,
            height: 240,
            focal: 110.0,
        };
        let mp = MatchParams {
            half: 5,
            cell: 12,
            max_dy: 8,
            min_disp: -5,
            max_disp: 30,
            ..Default::default()
        };
        let good = evaluate(&cal, &tiles, &Hypothesis::default_for(0, 1), rp, &mp);
        assert!(good.report.matches.len() >= 8, "{}", good.report.summary());
        assert!(
            good.report.median_dy().abs() < 0.5 && good.report.frac_within(1.0) > 0.8,
            "{}",
            good.report.summary()
        );
        let bad = evaluate(
            &cal,
            &tiles,
            &Hypothesis {
                conv: RigConvention::Cam0FromCam,
                ..Hypothesis::default_for(0, 1)
            },
            rp,
            &mp,
        );
        assert!(
            bad.report.frac_within(1.0) < good.report.frac_within(1.0) - 0.2,
            "bad {} vs good {}",
            bad.report.summary(),
            good.report.summary()
        );
        assert_eq!(
            pair_with_lines(&good.left, &good.right, 320, 240, 20).len(),
            640 * 240 * 3
        );
        assert_eq!(anaglyph(&good.left, &good.right).len(), 320 * 240 * 3);
    }
}
