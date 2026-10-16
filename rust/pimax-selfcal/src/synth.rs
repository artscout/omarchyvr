//! Synthetic scenes: a textured room rendered through any calibration, so that a known calibration error can be
//! injected and the pipeline checked end to end (tests, `pimax-selfcal selftest`).
//!
//! The room is a box with a smaller box standing in it (near structure next to far walls, with occlusion
//! edges). Surfaces carry a smooth 3-D texture, a sum of sinusoids with wavelengths of 12 cm and more, which is
//! band-limited in the images at room distances. Each tile is rendered pixel by pixel through the camera's own
//! fisheye model and pose, so nothing in the rendering shares code with the rectifier or the matcher beyond
//! `pimax_cam::kb4`.

use crate::scene::Scene;
use pimax_cam::calib::{DeviceCalib, RigConvention};
use pimax_cam::frame::{TILE_H, TILE_W};
use pimax_cam::geom::{self, Pose, Vec3};

fn lcg(s: &mut u64) -> f64 {
    *s = s
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    ((*s >> 11) as f64) / ((1u64 << 53) as f64)
}

/// A textured box room with one box inside (world coordinates, metres).
#[derive(Clone, Debug)]
pub struct Room {
    /// Half extents of the room, centred on the origin.
    pub half: Vec3,
    /// The box inside: minimum and maximum corner.
    pub inner: (Vec3, Vec3),
    waves: Vec<(Vec3, f64, f64)>,
}

impl Room {
    /// A 6 x 4 x 5 m room with a 0.7 m box about a metre in front of the origin; texture from `seed`.
    pub fn new(seed: u64) -> Room {
        let mut s = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
        let waves = (0..28)
            .map(|_| {
                // random direction, wavelength 0.12 .. 0.7 m
                let dir =
                    geom::normalize([lcg(&mut s) - 0.5, lcg(&mut s) - 0.5, lcg(&mut s) - 0.5]);
                let wavelength = 0.12 * (0.7f64 / 0.12).powf(lcg(&mut s));
                let k = std::f64::consts::TAU / wavelength;
                (
                    geom::scale(dir, k),
                    lcg(&mut s) * std::f64::consts::TAU,
                    10.0 + 25.0 * lcg(&mut s),
                )
            })
            .collect();
        Room {
            half: [3.0, 2.0, 2.5],
            inner: ([-0.9, -0.2, 0.9], [-0.2, 0.5, 1.6]),
            waves,
        }
    }

    /// First surface point along the ray `o + t d` (`o` inside the room, outside the inner box).
    pub fn hit(&self, o: Vec3, d: Vec3) -> Vec3 {
        // room walls, from inside
        let mut t_wall = f64::INFINITY;
        for ax in 0..3 {
            if d[ax].abs() > 1e-12 {
                let t = ((if d[ax] > 0.0 {
                    self.half[ax]
                } else {
                    -self.half[ax]
                }) - o[ax])
                    / d[ax];
                if t > 0.0 && t < t_wall {
                    t_wall = t;
                }
            }
        }
        // inner box, from outside (slab test)
        let (lo, hi) = self.inner;
        let (mut t0, mut t1) = (0.0f64, t_wall);
        for ax in 0..3 {
            if d[ax].abs() < 1e-12 {
                if o[ax] < lo[ax] || o[ax] > hi[ax] {
                    t0 = f64::INFINITY;
                }
                continue;
            }
            let (a, b) = ((lo[ax] - o[ax]) / d[ax], (hi[ax] - o[ax]) / d[ax]);
            t0 = t0.max(a.min(b));
            t1 = t1.min(a.max(b));
        }
        let t = if t0 < t1 && t0 > 0.0 { t0 } else { t_wall };
        geom::add(o, geom::scale(d, t))
    }

    /// Grey level (about 30..230) of the surface at `p`.
    pub fn shade(&self, p: Vec3) -> f64 {
        let mut v = 128.0;
        for &(k, ph, amp) in &self.waves {
            v += amp * (geom::dot(k, p) + ph).sin() / 3.5;
        }
        v.clamp(0.0, 255.0)
    }
}

/// Render the four tiles of `cal` in `room` with the rig's camera 0 at `world_from_cam0`. `noise` is the
/// standard deviation of the added sensor noise (grey levels); `seed` makes it reproducible.
pub fn render(
    cal: &DeviceCalib,
    room: &Room,
    world_from_cam0: &Pose,
    noise: f64,
    seed: u64,
    name: &str,
) -> Scene {
    let mut tiles: Vec<Vec<u8>> = vec![Vec::new(); cal.cameras.len()];
    std::thread::scope(|sc| {
        for (cam, out) in tiles.iter_mut().enumerate() {
            sc.spawn(move || {
                let k = &cal.cameras[cam].intr;
                let world_from_cam = world_from_cam0
                    .compose(&cal.cam_from_cam0(cam, RigConvention::CamFromCam0).inverse());
                let mut s = seed
                    .wrapping_add(cam as u64 * 7919)
                    .wrapping_mul(0x2545_f491_4f6c_dd1d)
                    | 1;
                let mut img = vec![0u8; TILE_W * TILE_H];
                for v in 0..TILE_H {
                    for u in 0..TILE_W {
                        let Some(ray) = k.unproject(u as f64, v as f64) else {
                            continue;
                        };
                        let d = geom::mat_vec(&world_from_cam.r, ray);
                        let p = room.hit(world_from_cam.t, d);
                        // sum of four uniforms: close to a normal deviate with unit variance
                        let g = (lcg(&mut s) + lcg(&mut s) + lcg(&mut s) + lcg(&mut s) - 2.0)
                            * 1.7320508;
                        img[v * TILE_W + u] =
                            (room.shade(p) + noise * g + 0.5).clamp(0.0, 255.0) as u8;
                    }
                }
                *out = img;
            });
        }
    });
    Scene {
        name: name.to_string(),
        tiles,
    }
}

/// Rig poses (camera 0 in the room) that give the cameras different views, near and far structure.
pub fn default_poses() -> Vec<Pose> {
    vec![
        Pose {
            r: geom::rodrigues([0.25, 0.1, 0.0]),
            t: [0.0, 0.0, -0.6],
        },
        Pose {
            r: geom::rodrigues([0.1, -0.9, 0.2]),
            t: [0.9, 0.2, 0.3],
        },
        Pose {
            r: geom::rodrigues([-0.2, 2.2, 0.1]),
            t: [-1.2, -0.3, -0.2],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rays_hit_walls_and_the_inner_box() {
        let room = Room::new(1);
        // straight ahead from the origin: the inner box spans x -0.9..-0.2, so +z misses it and hits the wall
        let p = room.hit([0.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        assert!((p[2] - 2.5).abs() < 1e-12);
        // towards the box centre: front face at z = 0.9
        let d = geom::normalize([-0.55, 0.15, 1.25]);
        let p = room.hit([0.0, 0.0, 0.0], d);
        assert!(
            (p[2] - 0.9).abs() < 1e-9 && p[0] < -0.2 && p[0] > -0.9,
            "{p:?}"
        );
        // every direction hits something inside the room bounds
        let mut s = 9u64;
        for _ in 0..200 {
            let d = geom::normalize([lcg(&mut s) - 0.5, lcg(&mut s) - 0.5, lcg(&mut s) - 0.5]);
            let p = room.hit([0.3, -0.1, -0.4], d);
            for ax in 0..3 {
                assert!(p[ax].abs() <= room.half[ax] + 1e-9);
            }
        }
        // texture: varied, not clipped
        let vals: Vec<f64> = (0..500)
            .map(|i| room.shade([i as f64 * 0.013, 0.2, 2.5]))
            .collect();
        let (lo, hi) = vals
            .iter()
            .fold((255.0f64, 0.0f64), |a, &v| (a.0.min(v), a.1.max(v)));
        assert!(lo > 5.0 && hi < 250.0 && hi - lo > 60.0, "{lo} {hi}");
    }

    #[test]
    fn rendered_tiles_look_like_camera_tiles() {
        let cal = DeviceCalib::parse(include_str!(
            "../../pimax-cam/tests/fixtures/headset-calibration.xml"
        ))
        .unwrap();
        let sc = render(
            &cal,
            &Room::new(2),
            &default_poses()[0],
            1.5,
            4,
            "synthetic",
        );
        assert_eq!(sc.tiles.len(), 4);
        for t in &sc.tiles {
            assert_eq!(t.len(), TILE_W * TILE_H);
            // corners outside the lens model are black, the centre is textured
            assert_eq!(t[0], 0);
            let centre: Vec<u8> = (200..280)
                .flat_map(|y| t[y * TILE_W + 280..y * TILE_W + 360].to_vec())
                .collect();
            let (lo, hi) = centre
                .iter()
                .fold((255u8, 0u8), |a, &v| (a.0.min(v), a.1.max(v)));
            assert!(hi - lo > 40, "flat centre {lo}..{hi}");
        }
        // deterministic
        let again = render(
            &cal,
            &Room::new(2),
            &default_poses()[0],
            1.5,
            4,
            "synthetic",
        );
        assert_eq!(sc.tiles, again.tiles);
        assert_eq!(default_poses().len(), 3);
    }
}
