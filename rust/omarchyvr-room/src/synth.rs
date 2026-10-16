//! A synthetic textured room rendered through the real KB4 fisheye cameras of the factory calibration: test
//! data for the whole room-memory pipeline (features, stereo keyframes, relocalisation) without a headset.
//!
//! The room is a box (floor at y = 0, ceiling at 2.6 m) with a few boxes inside (furniture: parallax and
//! occlusion), textured with multi-scale value noise plus sharp-edged "posters" so FAST finds corners.

use pimax_cam::geom::{self, Pose, Vec3};
use pimax_cam::kb4::Kb4;

/// Unit camera rays of 2x2 sub-pixel samples per pixel (KB4 unprojection done once). Averaging the samples
/// stands in for optics and pixel integration (single samples alias sharp edges differently in every view).
#[derive(Clone, Debug)]
pub struct RayTable {
    /// Width.
    pub w: usize,
    /// Height.
    pub h: usize,
    /// 4 rays per pixel (zero vector = outside the model).
    pub rays: Vec<[[f32; 3]; 4]>,
}

impl RayTable {
    /// Build for one camera.
    pub fn new(intr: &Kb4) -> RayTable {
        let (w, h) = (intr.width as usize, intr.height as usize);
        let mut rays = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let mut r = [[0f32; 3]; 4];
                // OpenCV / KB4 convention: pixel centres at integer coordinates.
                for (k, (dx, dy)) in [(-0.25, -0.25), (0.25, -0.25), (-0.25, 0.25), (0.25, 0.25)]
                    .into_iter()
                    .enumerate()
                {
                    if let Some(v) = intr.unproject(x as f64 + dx, y as f64 + dy) {
                        r[k] = [v[0] as f32, v[1] as f32, v[2] as f32];
                    }
                }
                rays.push(r);
            }
        }
        RayTable { w, h, rays }
    }
}

/// Axis-aligned box.
#[derive(Clone, Copy, Debug)]
pub struct Aabb {
    /// Min corner.
    pub lo: Vec3,
    /// Max corner.
    pub hi: Vec3,
}

/// The room.
#[derive(Clone, Debug)]
pub struct Room {
    /// Outer walls (viewed from inside).
    pub shell: Aabb,
    /// Furniture (viewed from outside).
    pub boxes: Vec<Aabb>,
    /// Texture seed.
    pub seed: u64,
    /// Brightness gain (lighting change between sessions).
    pub gain: f64,
}

impl Default for Room {
    fn default() -> Self {
        Room {
            shell: Aabb {
                lo: [-3.0, 0.0, -3.5],
                hi: [3.0, 2.6, 3.5],
            },
            boxes: vec![
                Aabb {
                    lo: [-2.9, 0.0, -3.4],
                    hi: [-1.7, 0.75, -2.6],
                },
                Aabb {
                    lo: [1.2, 0.0, -1.0],
                    hi: [2.0, 1.1, 0.2],
                },
                Aabb {
                    lo: [-0.6, 0.0, 2.0],
                    hi: [0.7, 0.45, 3.3],
                },
                Aabb {
                    lo: [-2.5, 0.0, 0.5],
                    hi: [-2.0, 1.8, 1.5],
                },
            ],
            seed: 42,
            gain: 1.0,
        }
    }
}

fn hash(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    x ^ (x >> 33)
}

fn lattice(seed: u64, i: i64, j: i64) -> f64 {
    let h = hash(
        seed ^ (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ (j as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F),
    );
    (h >> 11) as f64 / (1u64 << 53) as f64
}

fn value_noise(seed: u64, u: f64, v: f64) -> f64 {
    let (i, j) = (u.floor() as i64, v.floor() as i64);
    let (fu, fv) = (u - u.floor(), v - v.floor());
    let s = |t: f64| t * t * (3.0 - 2.0 * t);
    let (a, b) = (lattice(seed, i, j), lattice(seed, i + 1, j));
    let (c, d) = (lattice(seed, i, j + 1), lattice(seed, i + 1, j + 1));
    let x0 = a + (b - a) * s(fu);
    let x1 = c + (d - c) * s(fu);
    x0 + (x1 - x0) * s(fv)
}

/// Texture value (0..255) of a surface point; `face` keys the pattern per surface.
fn texture(seed: u64, face: u64, u: f64, v: f64) -> f64 {
    let s = seed ^ face.wrapping_mul(0x1000_0000_01b3);
    let mut t = 0.0;
    let mut amp = 0.5;
    let mut f = 1.5;
    for o in 0..4 {
        t += amp * value_noise(s.wrapping_add(o), u * f, v * f);
        amp *= 0.5;
        f *= 2.3;
    }
    let mut val = 40.0 + 150.0 * t;
    // Posters: random rectangles with sharp edges on a 0.6 m grid.
    let (ci, cj) = ((u / 0.6).floor() as i64, (v / 0.6).floor() as i64);
    if lattice(s ^ 0xABCD, ci, cj) < 0.45 {
        let (fu, fv) = (u / 0.6 - ci as f64, v / 0.6 - cj as f64);
        let w = 0.2 + 0.25 * lattice(s ^ 0x1111, ci, cj);
        let h = 0.2 + 0.25 * lattice(s ^ 0x2222, ci, cj);
        let (ou, ov) = (
            0.1 + 0.3 * lattice(s ^ 0x3333, ci, cj),
            0.1 + 0.3 * lattice(s ^ 0x4444, ci, cj),
        );
        if fu > ou && fu < ou + w && fv > ov && fv < ov + h {
            let dark = lattice(s ^ 0x5555, ci, cj) < 0.5;
            val = if dark { 25.0 } else { 225.0 };
            // A checker inside some posters.
            if lattice(s ^ 0x6666, ci, cj) < 0.4
                && ((fu * 20.0).floor() as i64 + (fv * 20.0).floor() as i64) % 2 == 0
            {
                val = 255.0 - val;
            }
        }
    }
    val
}

/// Ray-box intersection: (t, face index 0..5 = -x,+x,-y,+y,-z,+z) of the first hit beyond `t_min`.
fn hit_box(o: Vec3, d: Vec3, b: &Aabb, inside: bool) -> Option<(f64, usize)> {
    let mut best: Option<(f64, usize)> = None;
    for axis in 0..3 {
        if d[axis].abs() < 1e-12 {
            continue;
        }
        for (k, plane) in [b.lo[axis], b.hi[axis]].into_iter().enumerate() {
            let t = (plane - o[axis]) / d[axis];
            if t <= 1e-6 {
                continue;
            }
            let p = geom::add(o, geom::scale(d, t));
            let ok =
                (0..3).all(|a| a == axis || (p[a] >= b.lo[a] - 1e-9 && p[a] <= b.hi[a] + 1e-9));
            // From inside the shell we see the far planes; from outside a box the near ones (first hit).
            if ok && best.is_none_or(|(bt, _)| t < bt) {
                let _ = inside;
                best = Some((t, axis * 2 + k));
            }
        }
    }
    best
}

impl Room {
    /// First surface hit by a ray: (distance, texture value).
    pub fn trace(&self, o: Vec3, d: Vec3) -> Option<(f64, f64)> {
        let mut best: Option<(f64, u64, Vec3, usize)> = None;
        if let Some((t, f)) = hit_box(o, d, &self.shell, true) {
            best = Some((t, f as u64, geom::add(o, geom::scale(d, t)), f));
        }
        for (bi, b) in self.boxes.iter().enumerate() {
            if let Some((t, f)) = hit_box(o, d, b, false) {
                if best.is_none_or(|(bt, ..)| t < bt) {
                    best = Some((
                        t,
                        100 + bi as u64 * 6 + f as u64,
                        geom::add(o, geom::scale(d, t)),
                        f,
                    ));
                }
            }
        }
        let (t, key, p, f) = best?;
        let axis = f / 2;
        let (u, v) = match axis {
            0 => (p[2], p[1]),
            1 => (p[0], p[2]),
            _ => (p[0], p[1]),
        };
        Some((
            t,
            (texture(self.seed, key, u, v) * self.gain).clamp(0.0, 255.0),
        ))
    }

    /// Render one camera (640x480 grey) at `room_from_cam` (OpenCV camera axes).
    pub fn render(&self, intr: &Kb4, room_from_cam: &Pose) -> Vec<u8> {
        self.render_rays(&RayTable::new(intr), room_from_cam)
    }

    /// Render with a precomputed ray table (fast for many frames of one camera).
    pub fn render_rays(&self, rays: &RayTable, room_from_cam: &Pose) -> Vec<u8> {
        let mut img = vec![0u8; rays.w * rays.h];
        for (i, r4) in rays.rays.iter().enumerate() {
            let (mut sum, mut n) = (0.0, 0);
            for r in r4 {
                if r[2] == 0.0 && r[0] == 0.0 && r[1] == 0.0 {
                    continue;
                }
                let d = geom::normalize(geom::mat_vec(
                    &room_from_cam.r,
                    [r[0] as f64, r[1] as f64, r[2] as f64],
                ));
                if let Some((_, v)) = self.trace(room_from_cam.t, d) {
                    sum += v;
                    n += 1;
                }
            }
            if n > 0 {
                img[i] = (sum / n as f64).round() as u8;
            }
        }
        img
    }

    /// Distance along a camera ray (for checking triangulation).
    pub fn depth(&self, room_from_cam: &Pose, ray_cam: Vec3) -> Option<f64> {
        let d = geom::normalize(geom::mat_vec(&room_from_cam.r, ray_cam));
        self.trace(room_from_cam.t, d).map(|(t, _)| t)
    }
}
