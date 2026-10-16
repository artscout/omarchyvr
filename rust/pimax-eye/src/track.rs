//! Gaze samples in the OpenXR head frame: device-to-head conversion, the combined gaze ray, interpolation.
//!
//! # Device frame (what is known and what is not)
//!
//! The EyeChip reports origins in millimetres and unit directions in one 3D frame. From the data: `+z` is the viewing
//! direction (both eyes sit at `z` about -20 mm, behind the sensor plane, and look toward `+z`), eye A has `x` about
//! +30 mm and eye B about -28 mm. Tobii's XR runtime reports in the Unity-style HMD frame (x right, y up, z forward,
//! left-handed), so the working assumption is that **device x points to the user's right and eye A is the right
//! eye**, y up. The sign of x and y cannot be verified offline (it needs a live test: look left, look up).
//!
//! OpenXR is right-handed: x right, y up, **-z forward**. The default conversion is therefore `(x, y, z) -> (x, y, -z)`
//! (flip z only), millimetres to metres, with the eye order `[left, right]` = `[B, A]`.
//!
//! Every part is configurable ([`FrameConv`], environment, see [`FrameConv::from_env`]):
//!
//! | variable | default | effect |
//! |---|---|---|
//! | `PIMAX_EYE_FLIP_X` | 0 | negate x; **also swaps which eye is left** (the eye with the larger converted x is right) |
//! | `PIMAX_EYE_FLIP_Y` | 0 | negate y |
//! | `PIMAX_EYE_FLIP_Z` | 1 | negate z (device +z forward to OpenXR -z forward); set 0 if the device z already points back |
//! | `PIMAX_EYE_OFFSET_MM` | 0,0,0 | added to the converted origins (head-frame position of the device origin) |
//! | `PIMAX_EYE_SWAP` | 0 | swap the left/right assignment after the above |

/// Device-to-head conversion.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameConv {
    /// Negate device x.
    pub flip_x: bool,
    /// Negate device y.
    pub flip_y: bool,
    /// Negate device z (default true: device forward is +z, OpenXR forward is -z).
    pub flip_z: bool,
    /// Offset added to converted origins, millimetres (OpenXR axes).
    pub offset_mm: [f64; 3],
    /// Swap the left/right assignment.
    pub swap_eyes: bool,
}

impl Default for FrameConv {
    fn default() -> Self {
        FrameConv {
            flip_x: false,
            flip_y: false,
            flip_z: true,
            offset_mm: [0.0; 3],
            swap_eyes: false,
        }
    }
}

fn env_flag(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(v) => matches!(v.trim(), "1" | "true" | "yes" | "on"),
        Err(_) => default,
    }
}

impl FrameConv {
    /// Defaults overridden by the `PIMAX_EYE_*` environment variables (table in the module docs).
    pub fn from_env() -> FrameConv {
        let d = FrameConv::default();
        let mut offset = d.offset_mm;
        if let Ok(v) = std::env::var("PIMAX_EYE_OFFSET_MM") {
            let p: Vec<f64> = v.split(',').filter_map(|s| s.trim().parse().ok()).collect();
            if p.len() == 3 {
                offset = [p[0], p[1], p[2]];
            }
        }
        FrameConv {
            flip_x: env_flag("PIMAX_EYE_FLIP_X", d.flip_x),
            flip_y: env_flag("PIMAX_EYE_FLIP_Y", d.flip_y),
            flip_z: env_flag("PIMAX_EYE_FLIP_Z", d.flip_z),
            offset_mm: offset,
            swap_eyes: env_flag("PIMAX_EYE_SWAP", d.swap_eyes),
        }
    }

    fn axes(&self, v: [f64; 3]) -> [f64; 3] {
        [
            if self.flip_x { -v[0] } else { v[0] },
            if self.flip_y { -v[1] } else { v[1] },
            if self.flip_z { -v[2] } else { v[2] },
        ]
    }

    /// Device origin (mm) to head-frame origin (mm).
    pub fn origin(&self, v: [f64; 3]) -> [f64; 3] {
        let o = self.axes(v);
        [
            o[0] + self.offset_mm[0],
            o[1] + self.offset_mm[1],
            o[2] + self.offset_mm[2],
        ]
    }

    /// Device direction to head-frame direction.
    pub fn direction(&self, v: [f64; 3]) -> [f64; 3] {
        self.axes(v)
    }

    /// Index (0 = left, 1 = right) of the head-frame eye that device eye A (`a = true`) or B (`false`) is.
    pub fn slot(&self, a: bool) -> usize {
        // eye A has positive device x: right when x is not flipped
        let a_is_right = !self.flip_x;
        usize::from(a == a_is_right) ^ usize::from(self.swap_eyes)
    }
}

/// One eye in the head frame (OpenXR axes, metres).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct EyeRay {
    /// Origin, metres.
    pub origin: [f32; 3],
    /// Unit direction.
    pub direction: [f32; 3],
    /// Origin is valid.
    pub origin_valid: bool,
    /// Direction is valid (the eye is tracked).
    pub direction_valid: bool,
    /// Pupil diameter in millimetres, negative when unknown.
    pub pupil_mm: f32,
    /// Pupil position in the eye-camera sensor area, 0..1 (-1 when unknown).
    pub pupil_pos: [f32; 2],
    /// `pupil_pos` is valid.
    pub pupil_pos_valid: bool,
    /// Eye openness, negative when the device does not provide it.
    pub openness: f32,
}

/// Combined gaze ray in the head frame.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Gaze {
    /// Midpoint of the contributing origins, metres.
    pub origin: [f32; 3],
    /// Normalised mean direction.
    pub direction: [f32; 3],
    /// Bit 0 left eye contributed, bit 1 right eye.
    pub eyes: u8,
}

/// One gaze sample with host-clock time.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Sample {
    /// Running number of samples since the tracker started.
    pub seq: u64,
    /// Device timestamp, microseconds.
    pub device_ts_us: i64,
    /// Device time mapped to `CLOCK_MONOTONIC`, nanoseconds ([`crate::clock::ClockMap`]).
    pub host_ts_ns: i64,
    /// Host time the USB transfer carrying the sample returned.
    pub arrival_ns: i64,
    /// Eyes, index 0 left and 1 right (after [`FrameConv::slot`]).
    pub eyes: [EyeRay; 2],
    /// Combined ray; `None` when no eye has a valid ray.
    pub gaze: Option<Gaze>,
    /// Interpupillary distance reported by the chip (field 34), millimetres; 0 when absent.
    pub ipd_mm: f32,
}

fn norm(v: [f64; 3]) -> Option<[f64; 3]> {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    (l > 1e-9 && l.is_finite()).then(|| [v[0] / l, v[1] / l, v[2] / l])
}

fn f32v(v: [f64; 3]) -> [f32; 3] {
    [v[0] as f32, v[1] as f32, v[2] as f32]
}

/// Convert a decoded device sample. `host_ts_ns` / `arrival_ns` come from the clock map.
pub fn convert(
    g: &crate::gaze::GazeSample,
    conv: &FrameConv,
    seq: u64,
    host_ts_ns: i64,
    arrival_ns: i64,
) -> Sample {
    let mut eyes = [EyeRay::default(); 2];
    let mut sum_o = [0.0f64; 3];
    let mut sum_d = [0.0f64; 3];
    let mut n = 0u32;
    let mut mask = 0u8;
    for (i, e) in g.eyes.iter().enumerate() {
        let slot = conv.slot(i == 0);
        let o_mm = conv.origin(e.origin_mm);
        let dir = norm(conv.direction(e.direction));
        let dir_valid = e.direction_valid && dir.is_some();
        eyes[slot] = EyeRay {
            origin: f32v([o_mm[0] / 1000.0, o_mm[1] / 1000.0, o_mm[2] / 1000.0]),
            direction: dir.map_or([0.0; 3], f32v),
            origin_valid: e.origin_valid,
            direction_valid: dir_valid,
            pupil_mm: e.pupil_diameter_mm as f32,
            pupil_pos: [e.pupil_pos[0] as f32, e.pupil_pos[1] as f32],
            pupil_pos_valid: e.pupil_pos_valid,
            openness: e.openness as f32,
        };
        if e.origin_valid && dir_valid {
            let d = dir.unwrap_or([0.0; 3]);
            for k in 0..3 {
                sum_o[k] += o_mm[k] / 1000.0;
                sum_d[k] += d[k];
            }
            n += 1;
            mask |= 1 << slot;
        }
    }
    let gaze = if n > 0 {
        let inv = 1.0 / f64::from(n);
        // opposite rays cancel: treat a zero-length mean as "no combined gaze"
        norm(sum_d).map(|d| Gaze {
            origin: f32v([sum_o[0] * inv, sum_o[1] * inv, sum_o[2] * inv]),
            direction: f32v(d),
            eyes: mask,
        })
    } else {
        None
    };
    Sample {
        seq,
        device_ts_us: g.timestamp_us,
        host_ts_ns,
        arrival_ns,
        eyes,
        gaze,
        ipd_mm: g.frame.field_34 as f32,
    }
}

/// The sample for host time `t_ns` from a history ordered by `host_ts_ns`: linear interpolation of the combined
/// ray between the two neighbours, clamped to the ends (no extrapolation). `None` for an empty history.
pub fn sample_at(history: &[Sample], t_ns: i64) -> Option<Sample> {
    let first = history.first()?;
    let last = history.last()?;
    if t_ns <= first.host_ts_ns {
        return Some(*first);
    }
    if t_ns >= last.host_ts_ns {
        return Some(*last);
    }
    let i = history.partition_point(|s| s.host_ts_ns <= t_ns);
    let (a, b) = (&history[i - 1], &history[i]);
    let f = (t_ns - a.host_ts_ns) as f32 / (b.host_ts_ns - a.host_ts_ns).max(1) as f32;
    let mut out = if f < 0.5 { *a } else { *b };
    out.host_ts_ns = t_ns;
    if let (Some(ga), Some(gb)) = (a.gaze, b.gaze) {
        let mix = |x: [f32; 3], y: [f32; 3]| {
            [
                x[0] + (y[0] - x[0]) * f,
                x[1] + (y[1] - x[1]) * f,
                x[2] + (y[2] - x[2]) * f,
            ]
        };
        let d = mix(ga.direction, gb.direction);
        if let Some(d) = norm([f64::from(d[0]), f64::from(d[1]), f64::from(d[2])]) {
            out.gaze = Some(Gaze {
                origin: mix(ga.origin, gb.origin),
                direction: f32v(d),
                eyes: ga.eyes & gb.eyes,
            });
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gaze::{Eye, GazeSample};

    fn eye(o: [f64; 3], d: [f64; 3], valid: bool) -> Eye {
        Eye {
            origin_mm: o,
            origin_valid: valid,
            direction: d,
            direction_valid: valid,
            pupil_diameter_mm: 3.0,
            ..Eye::default()
        }
    }

    /// The frame from the replay log: A (29.8, 4.1, -20.2) dir (0.326, -0.005, 0.945), B (-28.5, 2.6, -19.4) dir (0.033, -0.025, 0.999).
    fn replay_frame() -> GazeSample {
        GazeSample {
            timestamp_us: 1000,
            eyes: [
                eye([29.8, 4.1, -20.2], [0.326, -0.005, 0.945], true),
                eye([-28.5, 2.6, -19.4], [0.033, -0.025, 0.999], true),
            ],
            ..GazeSample::default()
        }
    }

    #[test]
    fn default_conversion_flips_z_only_and_a_is_right() {
        let s = convert(&replay_frame(), &FrameConv::default(), 0, 5, 6);
        // right eye (slot 1) is device eye A
        assert!((s.eyes[1].origin[0] - 0.0298).abs() < 1e-6);
        assert!((s.eyes[1].origin[2] - 0.0202).abs() < 1e-6);
        assert!(s.eyes[1].direction[2] < -0.9, "forward must be -z");
        assert!((s.eyes[0].origin[0] + 0.0285).abs() < 1e-6);
        let g = s.gaze.unwrap();
        assert_eq!(g.eyes, 3);
        let l = g.direction.iter().map(|c| c * c).sum::<f32>().sqrt();
        assert!((l - 1.0).abs() < 1e-5);
        assert!(g.direction[2] < -0.9);
        assert!((g.origin[0] - (0.0298 - 0.0285) / 2.0).abs() < 1e-6);
    }

    #[test]
    fn flip_x_swaps_the_eyes_and_negates_x() {
        let c = FrameConv {
            flip_x: true,
            ..FrameConv::default()
        };
        let s = convert(&replay_frame(), &c, 0, 0, 0);
        assert!(
            (s.eyes[0].origin[0] + 0.0298).abs() < 1e-6,
            "A is now left, x negated"
        );
        assert!(
            s.eyes[0].origin[0] < 0.0 && s.eyes[1].origin[0] > 0.0,
            "left has -x, right +x"
        );
        assert!((s.eyes[0].direction[0] + 0.326).abs() < 1e-3);
    }

    #[test]
    fn single_valid_eye_gives_its_own_ray() {
        let mut g = replay_frame();
        g.eyes[1] = eye([0.0; 3], [0.0; 3], false);
        let s = convert(&g, &FrameConv::default(), 0, 0, 0);
        let gz = s.gaze.unwrap();
        assert_eq!(gz.eyes, 0b10);
        assert!((gz.origin[0] - 0.0298).abs() < 1e-6);
        assert!(!s.eyes[0].direction_valid);
    }

    #[test]
    fn no_valid_eye_no_gaze() {
        let mut g = replay_frame();
        g.eyes[0] = eye([0.0; 3], [0.0; 3], false);
        g.eyes[1] = eye([0.0; 3], [0.0; 3], false);
        assert!(convert(&g, &FrameConv::default(), 0, 0, 0).gaze.is_none());
    }

    #[test]
    fn origin_only_eye_does_not_contribute() {
        let mut g = replay_frame();
        g.eyes[0].direction_valid = false;
        let s = convert(&g, &FrameConv::default(), 0, 0, 0);
        assert_eq!(s.gaze.unwrap().eyes, 0b01);
        assert!(s.eyes[1].origin_valid && !s.eyes[1].direction_valid);
    }

    #[test]
    fn sample_at_interpolates_and_clamps() {
        let mk = |t: i64, dx: f64| {
            let mut g = replay_frame();
            g.eyes[0].direction = [dx, 0.0, 1.0];
            g.eyes[1].direction = [dx, 0.0, 1.0];
            convert(&g, &FrameConv::default(), 0, t, t)
        };
        let h = [mk(1000, 0.0), mk(2000, 1.0)];
        assert_eq!(sample_at(&h, 0).unwrap().host_ts_ns, 1000);
        assert_eq!(sample_at(&h, 9999).unwrap().host_ts_ns, 2000);
        let m = sample_at(&h, 1500).unwrap();
        assert_eq!(m.host_ts_ns, 1500);
        let d = m.gaze.unwrap().direction;
        // halfway between straight ahead and 45 degrees
        assert!(d[0] > 0.2 && d[0] < 0.5, "{d:?}");
        assert!(sample_at(&[], 5).is_none());
    }
}
