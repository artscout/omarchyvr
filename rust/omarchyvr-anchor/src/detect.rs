//! Tag detection in fisheye tiles and pose from KB4 corner observations.
//!
//! The AprilTag detector wants straight lines, so each 640x480 fisheye tile is resampled into a few virtual pinhole
//! views (centre and four side views). The corners found there are mapped back to fisheye pixels and the tag pose is
//! refined by Levenberg-Marquardt on the KB4 reprojection error, jointly over every camera that sees the tag.

use crate::tag::{self, Family};
use apriltag_sys as sys;
use pimax_cam::geom::{self, Mat3, Pose, Vec3};
use pimax_cam::kb4::Kb4;
use pimax_ctrltrack::solve::{self, Cam, Obs};

pub const VW: usize = 640;
pub const VH: usize = 480;
/// Focal length of the virtual pinhole views (px).
pub const VF: f64 = 230.0;
/// View directions (yaw deg about camera y, pitch deg about camera x).
const VIEWS: [(f64, f64); 5] = [(0.0, 0.0), (50.0, 0.0), (-50.0, 0.0), (0.0, 50.0), (0.0, -50.0)];

#[derive(Clone, Debug)]
pub struct Detection {
    pub id: i32,
    pub hamming: i32,
    pub margin: f32,
    /// Corners in fisheye tile pixels, in [`tag::CORNERS`] order.
    pub px: [(f64, f64); 4],
    /// Corners in the virtual view, and which view.
    pub virt: [(f64, f64); 4],
    pub view: usize,
}

/// Resampling maps of one camera.
pub struct CamRect {
    pub kb: Kb4,
    rv: Vec<Mat3>,
    maps: Vec<Vec<(f32, f32)>>,
}

impl CamRect {
    pub fn new(kb: &Kb4) -> CamRect {
        let mut rv = Vec::new();
        let mut maps = Vec::new();
        for &(yaw, pitch) in &VIEWS {
            let r = geom::mat_mul(
                &geom::rodrigues([0.0, yaw.to_radians(), 0.0]),
                &geom::rodrigues([pitch.to_radians(), 0.0, 0.0]),
            );
            let mut m = vec![(-1.0f32, -1.0f32); VW * VH];
            for v in 0..VH {
                for u in 0..VW {
                    let ray = [(u as f64 - 320.0) / VF, (v as f64 - 240.0) / VF, 1.0];
                    if let Some((su, sv)) = kb.project(geom::mat_vec(&r, ray)) {
                        m[v * VW + u] = (su as f32, sv as f32);
                    }
                }
            }
            rv.push(r);
            maps.push(m);
        }
        CamRect {
            kb: kb.clone(),
            rv,
            maps,
        }
    }

    fn remap(&self, view: usize, tile: &[u8]) -> Vec<u8> {
        let (w, h) = (self.kb.width as usize, self.kb.height as usize);
        let mut out = vec![128u8; VW * VH];
        for (o, &(su, sv)) in out.iter_mut().zip(&self.maps[view]) {
            if su < 0.0 || sv < 0.0 || su >= (w - 1) as f32 || sv >= (h - 1) as f32 {
                continue;
            }
            let (x, y) = (su as usize, sv as usize);
            let (fx, fy) = (su - x as f32, sv - y as f32);
            let a = tile[y * w + x] as f32;
            let b = tile[y * w + x + 1] as f32;
            let c = tile[(y + 1) * w + x] as f32;
            let d = tile[(y + 1) * w + x + 1] as f32;
            *o = ((a * (1.0 - fx) + b * fx) * (1.0 - fy) + (c * (1.0 - fx) + d * fx) * fy + 0.5) as u8;
        }
        out
    }

    fn virt_to_px(&self, view: usize, p: (f64, f64)) -> Option<(f64, f64)> {
        let ray = [(p.0 - 320.0) / VF, (p.1 - 240.0) / VF, 1.0];
        self.kb.project(geom::mat_vec(&self.rv[view], ray))
    }
}

pub struct Detector {
    td: *mut sys::apriltag_detector_t,
    _fam: Family,
}

unsafe impl Send for Detector {}

impl Detector {
    pub fn new(threads: i32) -> Detector {
        let fam = Family::tag36h11();
        unsafe {
            let td = sys::apriltag_detector_create();
            (*td).nthreads = threads;
            (*td).quad_decimate = 1.0;
            (*td).refine_edges = true;
            (*td).decode_sharpening = 0.25;
            sys::apriltag_detector_add_family_bits(td, fam.fam, 1);
            Detector { td, _fam: fam }
        }
    }

    fn detect_image(&self, img: &[u8]) -> Vec<(i32, i32, f32, [(f64, f64); 4])> {
        let mut out = Vec::new();
        unsafe {
            let im = sys::image_u8_create(VW as u32, VH as u32);
            let stride = (*im).stride as usize;
            for y in 0..VH {
                std::ptr::copy_nonoverlapping(img.as_ptr().add(y * VW), (*im).buf.add(y * stride), VW);
            }
            let dets = sys::apriltag_detector_detect(self.td, im);
            for i in 0..(*dets).size as usize {
                let d = *((*dets).data as *const *mut sys::apriltag_detection_t).add(i);
                let p = (*d).p;
                out.push((
                    (*d).id,
                    (*d).hamming,
                    (*d).decision_margin,
                    [(p[0][0], p[0][1]), (p[1][0], p[1][1]), (p[2][0], p[2][1]), (p[3][0], p[3][1])],
                ));
            }
            sys::apriltag_detections_destroy(dets);
            sys::image_u8_destroy(im);
        }
        out
    }

    /// All tags of the family in one fisheye tile (best view per id).
    pub fn detect(&self, rect: &CamRect, tile: &[u8]) -> Vec<Detection> {
        let mut best: Vec<Detection> = Vec::new();
        for view in 0..VIEWS.len() {
            let img = rect.remap(view, tile);
            for (id, hamming, margin, virt) in self.detect_image(&img) {
                let mut px = [(0.0, 0.0); 4];
                let mut ok = true;
                for k in 0..4 {
                    match rect.virt_to_px(view, virt[k]) {
                        Some(p) => px[k] = p,
                        None => ok = false,
                    }
                }
                if !ok {
                    continue;
                }
                let d = Detection {
                    id,
                    hamming,
                    margin,
                    px,
                    virt,
                    view,
                };
                match best.iter_mut().find(|b| b.id == id) {
                    Some(b) if b.margin >= margin => {}
                    Some(b) => *b = d,
                    None => best.push(d),
                }
            }
        }
        best
    }
}

impl Drop for Detector {
    fn drop(&mut self) {
        unsafe { sys::apriltag_detector_destroy(self.td) }
    }
}

fn solve_linear(a: &mut [[f64; 9]; 8]) -> Option<[f64; 8]> {
    for c in 0..8 {
        let p = (c..8).max_by(|&i, &j| a[i][c].abs().partial_cmp(&a[j][c].abs()).unwrap())?;
        if a[p][c].abs() < 1e-12 {
            return None;
        }
        a.swap(c, p);
        for r in 0..8 {
            if r != c {
                let f = a[r][c] / a[c][c];
                for k in c..9 {
                    a[r][k] -= f * a[c][k];
                }
            }
        }
    }
    let mut x = [0.0; 8];
    for i in 0..8 {
        x[i] = a[i][8] / a[i][i];
    }
    Some(x)
}

/// Initial `cam_from_tag` from the four corners of the virtual pinhole view (plane homography).
pub fn init_pose(d: &Detection, rv: &Mat3, size: f64) -> Option<Pose> {
    let mut a = [[0.0f64; 9]; 8];
    for k in 0..4 {
        let (xu, yu) = tag::CORNERS[k];
        let x = (d.virt[k].0 - 320.0) / VF;
        let y = (d.virt[k].1 - 240.0) / VF;
        a[2 * k] = [xu, yu, 1.0, 0.0, 0.0, 0.0, -x * xu, -x * yu, x];
        a[2 * k + 1] = [0.0, 0.0, 0.0, xu, yu, 1.0, -y * xu, -y * yu, y];
    }
    let h = solve_linear(&mut a)?;
    let mut c0 = [h[0], h[3], h[6]];
    let mut c1 = [h[1], h[4], h[7]];
    let mut c2 = [h[2], h[5], 1.0];
    if c2[2] < 0.0 {
        for c in [&mut c0, &mut c1, &mut c2] {
            *c = geom::scale(*c, -1.0);
        }
    }
    let n = (geom::norm(c0) + geom::norm(c1)) / 2.0;
    let nu = size / 2.0 / n;
    let r1 = geom::normalize(c0);
    let r3 = geom::normalize(geom::cross(r1, geom::normalize(c1)));
    let r2 = geom::cross(r3, r1);
    let virt_from_tag = Pose {
        r: geom::from_cols(r1, r2, r3),
        t: geom::scale(c2, nu),
    };
    // virtual view -> camera
    Some(Pose { r: *rv, t: [0.0; 3] }.compose(&virt_from_tag))
}

/// The other solution of the planar ambiguity: the tag normal reflected about the line of sight.
fn flipped(p: &Pose) -> Pose {
    let n = geom::mat_vec(&p.r, [0.0, 0.0, 1.0]);
    let u = geom::normalize(p.t);
    let n2 = geom::sub(geom::scale(u, 2.0 * geom::dot(n, u)), n);
    let ax = geom::cross(n, n2);
    let s = geom::norm(ax);
    if s < 1e-9 {
        return *p;
    }
    let ang = s.atan2(geom::dot(n, n2));
    let q = geom::rodrigues(geom::scale(ax, ang / s));
    Pose {
        r: geom::mat_mul(&q, &p.r),
        t: p.t,
    }
}

/// Result of a joint solve.
#[derive(Clone, Debug)]
pub struct Fix {
    pub body_from_tag: Pose,
    pub rms_px: f64,
    pub cams: Vec<usize>,
    /// Second-best planar solution (for one camera): rms of the alternative, if it is within 1.5x.
    pub ambiguous: bool,
}

/// Joint pose over every camera that saw the tag. `dets`: (camera index, detection). `rects` hold the view rotations.
pub fn solve(cams: &[Cam], rects: &[&CamRect], dets: &[(usize, Detection)], size: f64) -> Option<Fix> {
    if dets.is_empty() {
        return None;
    }
    let mut obs: Vec<Obs> = Vec::new();
    for (ci, d) in dets {
        for k in 0..4 {
            obs.push(Obs {
                cam: &cams[*ci],
                p: tag::corner(k, size),
                px: d.px[k],
            });
        }
    }
    let mut cands: Vec<(Pose, f64)> = Vec::new();
    for (ci, d) in dets {
        let Some(cft) = init_pose(d, &rects[*ci].rv[d.view], size) else {
            continue;
        };
        for init in [cft, flipped(&cft)] {
            let bft = cams[*ci].cam_from_body.inverse().compose(&init);
            let (p, rms) = solve::refine(bft, &obs, true, 60);
            if rms.is_finite() {
                cands.push((p, rms));
            }
        }
    }
    cands.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    let (p, rms) = *cands.first()?;
    // a visibly different pose with a similar error means the planar ambiguity is not resolved
    let ambiguous = cands.iter().skip(1).any(|(q, r)| {
        geom::norm(geom::sub(q.t, p.t)) > 0.01 && *r < rms * 1.5 + 0.2
    });
    Some(Fix {
        body_from_tag: p,
        rms_px: rms,
        cams: dets.iter().map(|d| d.0).collect(),
        ambiguous,
    })
}

pub fn pose_error(a: &Pose, b: &Pose) -> (f64, f64) {
    let dt = geom::norm(geom::sub(a.t, b.t));
    let rel = geom::mat_mul(&geom::transpose(&a.r), &b.r);
    let c = ((rel[0][0] + rel[1][1] + rel[2][2] - 1.0) / 2.0).clamp(-1.0, 1.0);
    (dt, c.acos().to_degrees())
}

#[allow(dead_code)]
fn _v(_: Vec3) {}
