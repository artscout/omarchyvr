//! Camera rig from the factory calibration, per-camera ORB features with bearings, and stereo triangulation of
//! the lower tracking pair (11 cm baseline) into 3D points in the IMU (body) frame.

use crate::orb::{self, Descriptor, Image, Keypoint, OrbParams};
use pimax_cam::calib::DeviceCalib;
use pimax_cam::geom::{self, Pose, Vec3};
use pimax_cam::kb4::Kb4;

/// A virtual pinhole view of a fisheye camera: features are extracted on the undistorted image, so a point looks
/// alike wherever it falls in the fisheye. The tracking pair diverges by 54 deg (cam 0 looks 31 deg left and is
/// mounted upside down, cam 1 29 deg right, upright), so both cameras get the SAME virtual orientation: upright,
/// looking forward and down by the pair's mean pitch. Their views then overlap almost fully (stereo) and match
/// the map from any session. About +-54 x +-46 deg at the fisheye's centre resolution.
#[derive(Clone, Debug)]
pub struct Pinhole {
    /// Width.
    pub w: usize,
    /// Height.
    pub h: usize,
    /// Focal length (px).
    pub f: f64,
    /// Principal point.
    pub cx: f64,
    /// Principal point.
    pub cy: f64,
    /// Virtual camera axes (OpenCV) -> physical camera axes.
    pub cam_from_virt: geom::Mat3,
    /// Fisheye pixel for each pinhole pixel (NaN = outside).
    lut: Vec<(f32, f32)>,
}

impl Pinhole {
    /// Pinhole of the same size and centre focal length as the fisheye `k`, oriented by `cam_from_virt`.
    pub fn new(k: &Kb4, cam_from_virt: geom::Mat3) -> Pinhole {
        let (w, h) = (k.width as usize, k.height as usize);
        let (f, cx, cy) = (k.fx, (w as f64 - 1.0) / 2.0, (h as f64 - 1.0) / 2.0);
        let mut lut = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let ray = geom::mat_vec(
                    &cam_from_virt,
                    [(x as f64 - cx) / f, (y as f64 - cy) / f, 1.0],
                );
                lut.push(match k.project(ray) {
                    Some((u, v))
                        if u >= 0.0 && v >= 0.0 && u < w as f64 - 1.0 && v < h as f64 - 1.0 =>
                    {
                        (u as f32, v as f32)
                    }
                    _ => (f32::NAN, f32::NAN),
                });
            }
        }
        Pinhole {
            w,
            h,
            f,
            cx,
            cy,
            cam_from_virt,
            lut,
        }
    }

    /// Upright virtual view looking forward with `pitch_rad` (negative = down) in the body frame (OpenXR axes).
    pub fn body_from_virt(pitch_rad: f64) -> geom::Mat3 {
        let z = [0.0, pitch_rad.sin(), -pitch_rad.cos()]; // forward
        let x = [1.0, 0.0, 0.0]; // right
        let y = geom::cross(z, x); // down
        geom::from_cols(x, y, z)
    }

    /// Undistort a fisheye image (bilinear); pixels outside the fisheye are 0.
    pub fn undistort(&self, img: &Image) -> Vec<u8> {
        let mut out = vec![0u8; self.w * self.h];
        for (o, &(u, v)) in out.iter_mut().zip(self.lut.iter()) {
            if !u.is_finite() || u as usize + 1 >= img.w || v as usize + 1 >= img.h {
                continue;
            }
            let (x0, y0) = (u as usize, v as usize);
            let (fx, fy) = (u - x0 as f32, v - y0 as f32);
            let p = |x: usize, y: usize| img.data[y * img.stride + x] as f32;
            let top = p(x0, y0) * (1.0 - fx) + p(x0 + 1, y0) * fx;
            let bot = p(x0, y0 + 1) * (1.0 - fx) + p(x0 + 1, y0 + 1) * fx;
            *o = (top * (1.0 - fy) + bot * fy).round() as u8;
        }
        out
    }

    /// Is the pinhole pixel covered by the fisheye?
    pub fn covered(&self, x: f32, y: f32) -> bool {
        let (xi, yi) = (x.round() as isize, y.round() as isize);
        xi >= 0
            && yi >= 0
            && (xi as usize) < self.w
            && (yi as usize) < self.h
            && self.lut[yi as usize * self.w + xi as usize].0.is_finite()
    }

    /// Unit bearing (physical camera frame) of a pinhole pixel.
    pub fn bearing(&self, x: f64, y: f64) -> Vec3 {
        geom::normalize(geom::mat_vec(
            &self.cam_from_virt,
            [(x - self.cx) / self.f, (y - self.cy) / self.f, 1.0],
        ))
    }
}

/// One camera of the rig.
#[derive(Clone, Debug)]
pub struct RigCam {
    /// Factory camera id.
    pub id: usize,
    /// KB4 intrinsics.
    pub intr: Kb4,
    /// `body_from_cam` (body = IMU frame, OpenXR axes; camera = OpenCV axes).
    pub body_from_cam: Pose,
    /// Virtual pinhole the features are extracted on.
    pub pinhole: Pinhole,
}

/// The cameras used for the room map, in rig order.
#[derive(Clone, Debug)]
pub struct Rig {
    /// Cameras.
    pub cams: Vec<RigCam>,
}

impl Rig {
    /// Rig of factory cameras `ids` (the first two must be the stereo pair). Every camera gets a virtual view
    /// with the pair's mean pitch (see [`Pinhole`]).
    pub fn from_calib(cal: &DeviceCalib, ids: &[usize]) -> Result<Rig, String> {
        let mut poses = Vec::new();
        for &i in ids {
            let p = pimax_cam::slam::t_imu_cam(cal, i)
                .ok_or_else(|| format!("camera {i}: no camera-IMU extrinsic in the calibration"))?;
            poses.push(p);
        }
        let pitch = {
            let n = poses.len().clamp(1, 2);
            poses
                .iter()
                .take(n)
                .map(|p| {
                    let a = geom::col(&p.r, 2); // optical axis in the body frame
                    a[1].atan2((a[0] * a[0] + a[2] * a[2]).sqrt())
                })
                .sum::<f64>()
                / n as f64
        };
        let bv = Pinhole::body_from_virt(pitch);
        let mut cams = Vec::new();
        for (&i, body_from_cam) in ids.iter().zip(poses) {
            let intr = cal.cameras[i].intr.clone();
            let cam_from_virt = geom::mat_mul(&geom::transpose(&body_from_cam.r), &bv);
            cams.push(RigCam {
                id: i,
                pinhole: Pinhole::new(&intr, cam_from_virt),
                intr,
                body_from_cam,
            });
        }
        if cams.is_empty() {
            return Err("empty rig".into());
        }
        Ok(Rig { cams })
    }

    /// `body_from_cam` of every camera (PnP input).
    pub fn body_from_cams(&self) -> Vec<Pose> {
        self.cams.iter().map(|c| c.body_from_cam).collect()
    }
}

/// Features of one image.
#[derive(Clone, Debug, Default)]
pub struct Features {
    /// Keypoints (level-0 pixels).
    pub kps: Vec<Keypoint>,
    /// Descriptors.
    pub desc: Vec<Descriptor>,
    /// Unit bearing in the camera frame per keypoint.
    pub bearings: Vec<Vec3>,
    /// The undistorted (virtual pinhole) image the features come from (for sub-pixel stereo).
    pub img: Vec<u8>,
    /// Its width.
    pub w: usize,
}

/// ORB features of one fisheye camera image, extracted on its virtual pinhole image (keypoints are in pinhole
/// pixels; bearings in the camera frame). The mask keeps the 31x31 patches (radius ~22 px) inside the covered
/// area.
pub fn features(img: &Image, cam: &RigCam, p: &OrbParams) -> Features {
    features_with(img, cam, p, Some(LOW_LIGHT))
}

/// [`features`] with a chosen low-light stretch (None: off; for measurements).
pub fn features_with(img: &Image, cam: &RigCam, p: &OrbParams, low: Option<LowLight>) -> Features {
    let ph = &cam.pinhole;
    let mut und = ph.undistort(img);
    if let Some(l) = low {
        low_light_stretch(&mut und, ph.w, l);
    }
    let view = Image {
        w: ph.w,
        h: ph.h,
        stride: ph.w,
        data: &und,
    };
    let m = 24.0f32;
    let mask = |x: f32, y: f32| -> bool {
        ph.covered(x - m, y) && ph.covered(x + m, y) && ph.covered(x, y - m) && ph.covered(x, y + m)
    };
    let raw = orb::extract(&view, Some(&mask), p);
    let mut f = Features {
        w: ph.w,
        ..Default::default()
    };
    for (k, d) in raw {
        f.kps.push(k);
        f.desc.push(d);
        f.bearings.push(ph.bearing(k.x as f64, k.y as f64));
    }
    f.img = und;
    f
}

/// Low-light contrast stretch before ORB (see [`low_light_stretch`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LowLight {
    /// Frames whose 99th percentile grey is at or above this are left alone (lit rooms: unchanged features).
    pub p99_below: u8,
    /// The 99th percentile is stretched to this grey.
    pub target: u8,
    /// Largest gain.
    pub max_gain: f32,
    /// 3x3 box filter of a stretched frame first (the sensor noise at high gain would otherwise fill FAST's corner
    /// budget with noise).
    pub denoise: bool,
}

/// The default: only dark frames (p99 < 100) are stretched, up to 6x, p99 to 200.
pub const LOW_LIGHT: LowLight = LowLight { p99_below: 100, target: 200, max_gain: 6.0, denoise: true };

/// Night / dim room: the tracking cameras sit at their auto-exposure ceiling (exposure 4108, gain 215) and the image
/// mean falls to ~8/255; FAST's absolute thresholds (20, retry 7) then find almost no real corners and the stereo
/// pairs fall apart. A linear stretch of the dark frame (black level kept, the 99th percentile of the lit pixels to
/// `target`) gives FAST the contrast back. ORB descriptors are pixel-pair comparisons, so a monotonic stretch keeps
/// them comparable with keyframes made in daylight; the sub-pixel NCC is gain invariant. Pixels that are 0 (outside
/// the fisheye) do not count. Returns the gain applied (1 = unchanged).
pub fn low_light_stretch(img: &mut [u8], w: usize, p: LowLight) -> f32 {
    let mut hist = [0u32; 256];
    for &v in img.iter() {
        hist[v as usize] += 1;
    }
    let n: u32 = hist[1..].iter().sum();
    if n == 0 {
        return 1.0;
    }
    let want = (n as f64 * 0.99).ceil() as u32;
    let mut acc = 0u32;
    let mut p99 = 255usize;
    for (v, &c) in hist.iter().enumerate().skip(1) {
        acc += c;
        if acc >= want {
            p99 = v;
            break;
        }
    }
    if p99 >= p.p99_below as usize {
        return 1.0;
    }
    let g = (p.target as f32 / p99.max(1) as f32).min(p.max_gain);
    if g <= 1.0 {
        return 1.0;
    }
    if p.denoise && w >= 3 && img.len() >= 3 * w {
        box3(img, w, g);
    } else {
        let lut: Vec<u8> = (0..256).map(|v| (v as f32 * g).round().min(255.0) as u8).collect();
        for v in img.iter_mut() {
            *v = lut[*v as usize];
        }
    }
    g
}

/// 3x3 box filter times `g` (rounded, clamped); border pixels and pixels that are 0 (outside the fisheye) are only
/// scaled, so the black mask edge stays where it is.
fn box3(img: &mut [u8], w: usize, g: f32) {
    let h = img.len() / w;
    let src = img.to_vec();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let v = if src[i] == 0 || x == 0 || y == 0 || x + 1 == w || y + 1 == h {
                src[i] as f32
            } else {
                let mut s = 0u32;
                for yy in y - 1..=y + 1 {
                    for xx in x - 1..=x + 1 {
                        s += src[yy * w + xx] as u32;
                    }
                }
                s as f32 / 9.0
            };
            img[i] = (v * g).round().min(255.0) as u8;
        }
    }
}

fn sample(img: &[u8], w: usize, x: f64, y: f64) -> Option<f64> {
    let h = img.len() / w.max(1);
    if x < 0.0 || y < 0.0 || x >= (w - 1) as f64 || y >= (h - 1) as f64 {
        return None;
    }
    let (x0, y0) = (x as usize, y as usize);
    let (fx, fy) = (x - x0 as f64, y - y0 as f64);
    let p = |xx: usize, yy: usize| img[yy * w + xx] as f64;
    let top = p(x0, y0) * (1.0 - fx) + p(x0 + 1, y0) * fx;
    let bot = p(x0, y0 + 1) * (1.0 - fx) + p(x0 + 1, y0 + 1) * fx;
    Some(top * (1.0 - fy) + bot * fy)
}

/// Sub-pixel x of the point in image b matching the 9x9 patch at (xa, ya) in image a: NCC along the row
/// (epipolar lines are nearly horizontal in the common virtual orientation) within +-2 px of `xb`, parabola fit.
/// None if the best NCC is below 0.8 or at the search border.
pub fn refine_x(fa: &Features, fb: &Features, xa: f64, ya: f64, xb: f64, yb: f64) -> Option<f64> {
    const R: i32 = 4;
    let mut pa = Vec::with_capacity(81);
    for dy in -R..=R {
        for dx in -R..=R {
            pa.push(sample(&fa.img, fa.w, xa + dx as f64, ya + dy as f64)?);
        }
    }
    let ma = pa.iter().sum::<f64>() / pa.len() as f64;
    let va: f64 = pa.iter().map(|v| (v - ma).powi(2)).sum();
    if va < 1e-6 {
        return None;
    }
    let mut scores = Vec::new();
    for k in -8..=8 {
        let off = k as f64 * 0.25;
        let mut pb = Vec::with_capacity(81);
        for dy in -R..=R {
            for dx in -R..=R {
                pb.push(sample(&fb.img, fb.w, xb + off + dx as f64, yb + dy as f64)?);
            }
        }
        let mb = pb.iter().sum::<f64>() / pb.len() as f64;
        let vb: f64 = pb.iter().map(|v| (v - mb).powi(2)).sum();
        let cov: f64 = pa
            .iter()
            .zip(pb.iter())
            .map(|(a, b)| (a - ma) * (b - mb))
            .sum();
        scores.push(if vb < 1e-6 {
            -1.0
        } else {
            cov / (va * vb).sqrt()
        });
    }
    let (bi, &best) = scores
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())?;
    if best < 0.8 || bi == 0 || bi == scores.len() - 1 {
        return None;
    }
    let (l, c, r) = (scores[bi - 1], best, scores[bi + 1]);
    let den = l - 2.0 * c + r;
    let frac = if den.abs() > 1e-12 {
        0.5 * (l - r) / den
    } else {
        0.0
    };
    Some(xb + ((bi as f64 - 8.0) + frac.clamp(-0.5, 0.5)) * 0.25)
}

/// Stereo parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct StereoParams {
    /// Epipolar gate (deg).
    pub epi_deg: f64,
    /// Ray-intersection residual allowed per ray (deg).
    pub tri_deg: f64,
    /// Depth range (m, from the first camera).
    pub min_depth: f64,
    /// Max depth.
    pub max_depth: f64,
    /// Descriptor distance limit.
    pub max_dist: u32,
    /// Lowe ratio.
    pub ratio: f32,
}

impl Default for StereoParams {
    fn default() -> Self {
        StereoParams {
            epi_deg: 1.0,
            tri_deg: 0.5,
            min_depth: 0.25,
            max_depth: 8.0,
            // The epipolar gate leaves a handful of candidates, so the descriptor test can be loose: true stereo
            // pairs differ by ~47 bits (median) because the fisheye distorts the two views differently.
            max_dist: 85,
            ratio: 0.9,
        }
    }
}

/// A triangulated point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StereoPoint {
    /// Position in the body frame.
    pub p_body: Vec3,
    /// Descriptor (from camera a).
    pub desc: Descriptor,
    /// Depth along camera a's ray (m).
    pub depth: f64,
    /// Index of the feature in camera a.
    pub ia: usize,
}

/// Closest points of two rays `oa + s da`, `ob + u db`: (s, u).
fn ray_params(oa: Vec3, da: Vec3, ob: Vec3, db: Vec3) -> Option<(f64, f64)> {
    let w = geom::sub(oa, ob);
    let (a, b, c) = (geom::dot(da, da), geom::dot(da, db), geom::dot(db, db));
    let (d, e) = (geom::dot(da, w), geom::dot(db, w));
    let den = a * c - b * b;
    if den.abs() < 1e-12 {
        return None;
    }
    Some(((b * e - c * d) / den, (a * e - b * d) / den))
}

/// Triangulate matches between camera `a` and camera `b` of the rig.
pub fn triangulate(
    fa: &Features,
    fb: &Features,
    a: &RigCam,
    b: &RigCam,
    p: &StereoParams,
) -> Vec<StereoPoint> {
    let (oa, ob) = (a.body_from_cam.t, b.body_from_cam.t);
    let da: Vec<Vec3> = fa
        .bearings
        .iter()
        .map(|f| geom::mat_vec(&a.body_from_cam.r, *f))
        .collect();
    let db: Vec<Vec3> = fb
        .bearings
        .iter()
        .map(|f| geom::mat_vec(&b.body_from_cam.r, *f))
        .collect();
    let base = geom::sub(ob, oa);
    let normals: Vec<Vec3> = da
        .iter()
        .map(|d| geom::normalize(geom::cross(base, *d)))
        .collect();
    let sin_epi = p.epi_deg.to_radians().sin();
    let allow = |i: usize, j: usize| {
        geom::dot(normals[i], db[j]).abs() < sin_epi && geom::dot(da[i], db[j]) > 0.3
    };
    let m = orb::match_gated(&fa.desc, &fb.desc, p.max_dist, p.ratio, &allow);
    let cos_tri = p.tri_deg.to_radians().cos();
    let mut out = Vec::new();
    for (i, j, _) in m {
        // Sub-pixel disparity: ~1 px of keypoint error is ~10 % of depth at 3 m with this 11 cm baseline.
        let (ka, kb) = (fa.kps[i], fb.kps[j]);
        let Some(xb) = refine_x(fa, fb, ka.x as f64, ka.y as f64, kb.x as f64, kb.y as f64) else {
            continue;
        };
        let dbj = geom::mat_vec(&b.body_from_cam.r, b.pinhole.bearing(xb, kb.y as f64));
        let Some((s, u)) = ray_params(oa, da[i], ob, dbj) else {
            continue;
        };
        if s < p.min_depth || s > p.max_depth || u <= 0.0 {
            continue;
        }
        let x = geom::scale(
            geom::add(
                geom::add(oa, geom::scale(da[i], s)),
                geom::add(ob, geom::scale(dbj, u)),
            ),
            0.5,
        );
        let ra = geom::normalize(geom::sub(x, oa));
        let rb = geom::normalize(geom::sub(x, ob));
        if geom::dot(ra, da[i]) < cos_tri || geom::dot(rb, dbj) < cos_tri {
            continue;
        }
        out.push(StereoPoint {
            p_body: x,
            desc: fa.desc[i],
            depth: s,
            ia: i,
        });
    }
    out
}
