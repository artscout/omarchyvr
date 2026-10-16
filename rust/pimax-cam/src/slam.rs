//! 6DoF head tracking support (visual-inertial SLAM, Monado + Basalt through VIT):
//!
//! * [`t_imu_cam`]: camera extrinsic in the IMU frame the driver feeds to the tracker. That frame is the factory
//!   calibration's "body" frame (OpenXR head axes, x right, y up, z back), i.e. the raw IMU axes after the
//!   driver's `PIMAX_AXIS_MODE=1` mapping (x and z negated). Camera axes are OpenCV (x right, y down, z forward).
//! * [`ImuNoise`]: IMU noise model. The factory file has `IMUNoise` all 0.0, so we use conservative defaults or a
//!   file measured from a still recording (`pimax-slam imu-noise`, `~/.config/pimax/slam-imu.json`).
//! * [`basalt_calib_json`]: the calibration in Basalt's cereal JSON format (`camera_type` `kb4`), for offline runs
//!   (`basalt_vio`, `monado-cli slambatch` with a `SLAM_CONFIG`) and for every recording.
//! * [`Supervisor`]: decides when the tracker's poses are trustworthy (acquire after N consistent frames, lose on
//!   jumps / rotation disagreement with the gyro / staleness), keeps the output continuous across losses (held
//!   position + yaw-corrected 3DoF), and asks for a tracker reset after a long loss.
//!
//! Units: metres, nanoseconds, quaternions `[x, y, z, w]`.

use crate::calib::{DeviceCalib, RigConvention};
use crate::geom::{self, Mat3, Pose, Vec3};
use crate::json::{self, Value};
use std::path::{Path, PathBuf};

/// Tracking-exposure frame rate of each camera (the stream is 50 fps, alternating tracking and LED exposure).
pub const TRACKING_HZ: f64 = 25.0;
/// IMU sample rate (500 reports/s with two samples each).
pub const IMU_HZ: f64 = 1000.0;

/// Quaternion `[x, y, z, w]`.
pub type Quat = [f64; 4];

/// Identity quaternion.
pub const QUAT_ID: Quat = [0.0, 0.0, 0.0, 1.0];

/// Hamilton product `a * b` (apply `b` first).
pub fn qmul(a: Quat, b: Quat) -> Quat {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

/// Conjugate (inverse of a unit quaternion).
pub fn qconj(q: Quat) -> Quat {
    [-q[0], -q[1], -q[2], q[3]]
}

/// Normalise (identity for a zero quaternion).
pub fn qnorm(q: Quat) -> Quat {
    let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if n < 1e-12 || !n.is_finite() {
        QUAT_ID
    } else {
        [q[0] / n, q[1] / n, q[2] / n, q[3] / n]
    }
}

/// Rotate a vector.
pub fn qrot(q: Quat, v: Vec3) -> Vec3 {
    let u = [q[0], q[1], q[2]];
    let uv = geom::cross(u, v);
    let uuv = geom::cross(u, uv);
    geom::add(
        v,
        geom::add(geom::scale(uv, 2.0 * q[3]), geom::scale(uuv, 2.0)),
    )
}

/// Rotation angle in degrees.
pub fn qangle_deg(q: Quat) -> f64 {
    let q = qnorm(q);
    (2.0 * q[3].abs().min(1.0).acos()).to_degrees()
}

/// Rotation about +Y (up) by `rad`.
pub fn qyaw(rad: f64) -> Quat {
    [0.0, (rad / 2.0).sin(), 0.0, (rad / 2.0).cos()]
}

/// Yaw (rotation about +Y) part of `q`: the angle that takes forward (-Z) to `q`'s forward projected on the
/// floor. Gravity-aligned frames only differ by such a yaw.
pub fn yaw_of(q: Quat) -> f64 {
    let d = qrot(q, [0.0, 0.0, -1.0]);
    if d[0].abs() < 1e-9 && d[2].abs() < 1e-9 {
        // Looking straight up/down: use the up vector's tilt direction instead.
        let u = qrot(q, [0.0, 1.0, 0.0]);
        return (-u[0]).atan2(-u[2]);
    }
    (-d[0]).atan2(-d[2])
}

/// Matrix to quaternion (re-export for the C side).
pub fn quat_of(m: &Mat3) -> Quat {
    geom::quat_from_mat(m)
}

/// Quaternion to a row-major matrix.
pub fn mat_of(q: Quat) -> Mat3 {
    let c0 = qrot(q, [1.0, 0.0, 0.0]);
    let c1 = qrot(q, [0.0, 1.0, 0.0]);
    let c2 = qrot(q, [0.0, 0.0, 1.0]);
    geom::from_cols(c0, c1, c2)
}

/// `imu_from_cam_i` (= body_from_cam_i): maps camera-i OpenCV coordinates into the IMU frame the driver feeds.
pub fn t_imu_cam(cal: &DeviceCalib, i: usize) -> Option<Pose> {
    if i >= cal.cameras.len() {
        return None;
    }
    cal.body_from_cam(i, RigConvention::CamFromCam0)
}

/// IMU noise model in Basalt's terms (continuous-time densities: `*_noise_std` in unit/s/√Hz-style units as
/// Basalt expects them, `*_bias_std` bias random walk).
#[derive(Clone, Debug, PartialEq)]
pub struct ImuNoise {
    /// Accelerometer white noise, m/s²/√Hz.
    pub accel_noise_std: Vec3,
    /// Gyroscope white noise, rad/s/√Hz.
    pub gyro_noise_std: Vec3,
    /// Accelerometer bias random walk, m/s³/√Hz.
    pub accel_bias_std: Vec3,
    /// Gyroscope bias random walk, rad/s²/√Hz.
    pub gyro_bias_std: Vec3,
    /// Sample rate, Hz.
    pub rate_hz: f64,
    /// Where the numbers came from ("defaults" or a file path).
    pub source: String,
}

impl Default for ImuNoise {
    /// Pimax's own stationary densities (older Windows `device_calibration.xml` `IMUNoise`: gyro 0.001974,
    /// accel 0.081686; the active file has zeros). Our earlier datasheet-based 4e-4 / 4e-3 made Basalt trust a
    /// biased IMU over vision: on the 2026-10-14 walk (gyro bias seeded) the still drift went 10.3 -> 7.5 cm at
    /// 5 s and the loop error 6.8 -> 1.7 % with these (analysis/slam-drift.md). Bias random walks stay (larger
    /// ones made it worse). Replace with a measurement (`pimax-slam imu-noise`) if one ever beats these.
    fn default() -> Self {
        ImuNoise {
            accel_noise_std: [0.081686; 3],
            gyro_noise_std: [0.001974; 3],
            accel_bias_std: [1.0e-3; 3],
            gyro_bias_std: [1.0e-4; 3],
            rate_hz: IMU_HZ,
            source: "defaults".into(),
        }
    }
}

fn vec3_of(v: Option<&Value>, d: Vec3) -> Vec3 {
    match v {
        Some(Value::Num(x)) => [*x; 3],
        Some(v) => v.as_f64_array::<3>().unwrap_or(d),
        None => d,
    }
}

impl ImuNoise {
    /// Default file search: `$PIMAX_SLAM_IMU`, then `~/.config/pimax/slam-imu.json`.
    pub fn default_path() -> Option<PathBuf> {
        if let Some(v) = std::env::var_os("PIMAX_SLAM_IMU").filter(|v| !v.is_empty()) {
            return Some(PathBuf::from(v));
        }
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/pimax/slam-imu.json"))
    }

    /// Parse the JSON written by [`ImuNoise::to_json`] (missing keys keep the defaults; a scalar applies to all
    /// three axes).
    pub fn parse(text: &str) -> Result<ImuNoise, String> {
        let v = json::parse(text).map_err(|e| e.to_string())?;
        let d = ImuNoise::default();
        let n = ImuNoise {
            accel_noise_std: vec3_of(v.get("accel_noise_std"), d.accel_noise_std),
            gyro_noise_std: vec3_of(v.get("gyro_noise_std"), d.gyro_noise_std),
            accel_bias_std: vec3_of(v.get("accel_bias_std"), d.accel_bias_std),
            gyro_bias_std: vec3_of(v.get("gyro_bias_std"), d.gyro_bias_std),
            rate_hz: v
                .get("imu_update_rate")
                .and_then(Value::as_f64)
                .unwrap_or(d.rate_hz),
            source: String::new(),
        };
        let all = [
            n.accel_noise_std,
            n.gyro_noise_std,
            n.accel_bias_std,
            n.gyro_bias_std,
        ];
        if all.iter().flatten().any(|x| !x.is_finite() || *x <= 0.0)
            || n.rate_hz.is_nan()
            || n.rate_hz <= 0.0
        {
            return Err("IMU noise values must be finite and > 0".into());
        }
        Ok(n)
    }

    /// Load `path`, or the default file when `path` is `None`. `Ok(None)` = no file (use defaults).
    pub fn load(path: Option<&Path>) -> Result<Option<ImuNoise>, String> {
        let p = match path.map(Path::to_path_buf).or_else(ImuNoise::default_path) {
            Some(p) => p,
            None => return Ok(None),
        };
        if path.is_none() && !p.is_file() {
            return Ok(None);
        }
        let s = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        let mut n = ImuNoise::parse(&s).map_err(|e| format!("{}: {e}", p.display()))?;
        n.source = p.display().to_string();
        Ok(Some(n))
    }

    /// JSON (Basalt key names).
    pub fn to_json(&self, comment: &str) -> String {
        format!(
            "{{\n  \"comment\": {},\n  \"imu_update_rate\": {},\n  \"accel_noise_std\": {},\n  \"gyro_noise_std\": {},\n  \"accel_bias_std\": {},\n  \"gyro_bias_std\": {}\n}}\n",
            json::quote(comment),
            json::num(self.rate_hz),
            json::num_array(&self.accel_noise_std),
            json::num_array(&self.gyro_noise_std),
            json::num_array(&self.accel_bias_std),
            json::num_array(&self.gyro_bias_std)
        )
    }
}

/// Which cameras feed the tracker and how.
#[derive(Clone, Debug, PartialEq)]
pub struct SlamSetup {
    /// Factory camera ids in tracker order (cam0, cam1, ...).
    pub cams: Vec<usize>,
    /// Frame rate per camera.
    pub cam_hz: f64,
    /// Basalt `cam_time_offset_ns` (t_imu = t_cam + offset). The driver already shifts frame times by its own
    /// `PIMAX_SLAM_CAM_OFFSET_US`, so recordings carry 0 here.
    pub cam_time_offset_ns: i64,
    /// Gyro bias (rad/s, body frame) Basalt subtracts from every sample (`calib_gyro_bias`). Basalt starts its own
    /// bias estimate at zero; with the real ~0.85 deg/s and no landmarks yet (still headset, nothing in the stereo
    /// overlap beyond 0.33 m) the attitude tilts before vision can fix it and gravity leaks into position
    /// (analysis/slam-drift.md). Zero = none.
    pub gyro_bias: Vec3,
}

impl Default for SlamSetup {
    fn default() -> Self {
        SlamSetup {
            cams: vec![0, 1],
            cam_hz: TRACKING_HZ,
            cam_time_offset_ns: 0,
            gyro_bias: [0.0; 3],
        }
    }
}

/// The driver's gyro bias cache (`$XDG_CACHE_HOME/pimax/gyro-bias`, else `~/.cache/pimax/gyro-bias`).
pub fn gyro_bias_cache_path() -> Option<PathBuf> {
    if let Some(x) = std::env::var_os("XDG_CACHE_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(x).join("pimax/gyro-bias"));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache/pimax/gyro-bias"))
}

/// Parse the driver's cache text: three numbers, raw IMU axes, deg/s (`pimax_hmd.c` `bias_cache_save`). Returns
/// the bias in the body frame (raw x and z negated, [`t_imu_cam`]'s frame) in rad/s; `None` when malformed or
/// implausible (any axis above 5 deg/s).
pub fn parse_gyro_bias_cache(text: &str) -> Option<Vec3> {
    let v: Vec<f64> = text
        .split_whitespace()
        .map(|w| w.parse::<f64>())
        .collect::<Result<_, _>>()
        .ok()?;
    if v.len() != 3 || v.iter().any(|x| !x.is_finite() || x.abs() > 5.0) {
        return None;
    }
    Some([
        (-v[0]).to_radians(),
        v[1].to_radians(),
        (-v[2]).to_radians(),
    ])
}

/// The cached gyro bias (body frame, rad/s), if the driver has written one.
pub fn gyro_bias_cache() -> Option<Vec3> {
    let p = gyro_bias_cache_path()?;
    parse_gyro_bias_cache(&std::fs::read_to_string(p).ok()?)
}

/// Parse "0,1" / "0,1,2,3" (distinct ids < `count`).
pub fn parse_cams(s: &str, count: usize) -> Result<Vec<usize>, String> {
    let mut v = Vec::new();
    for p in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let i: usize = p
            .parse()
            .map_err(|_| format!("camera list {s:?}: bad id {p:?}"))?;
        if i >= count || v.contains(&i) {
            return Err(format!(
                "camera list {s:?}: ids must be distinct and < {count}"
            ));
        }
        v.push(i);
    }
    if v.is_empty() {
        return Err(format!("camera list {s:?} is empty"));
    }
    Ok(v)
}

fn pose_json(p: &Pose, indent: &str) -> String {
    let q = geom::quat_from_mat(&p.r);
    format!(
        "{indent}{{\n{indent}    \"px\": {},\n{indent}    \"py\": {},\n{indent}    \"pz\": {},\n{indent}    \"qx\": {},\n{indent}    \"qy\": {},\n{indent}    \"qz\": {},\n{indent}    \"qw\": {}\n{indent}}}",
        json::num(p.t[0]),
        json::num(p.t[1]),
        json::num(p.t[2]),
        json::num(q[0]),
        json::num(q[1]),
        json::num(q[2]),
        json::num(q[3])
    )
}

/// Basalt calibration (`"value0"` cereal document) for the selected cameras.
pub fn basalt_calib_json(
    cal: &DeviceCalib,
    setup: &SlamSetup,
    imu: &ImuNoise,
) -> Result<String, String> {
    let mut tic = Vec::new();
    let mut intr = Vec::new();
    let mut res = Vec::new();
    for &i in &setup.cams {
        let p = t_imu_cam(cal, i).ok_or_else(|| {
            format!("camera {i}: no camera-IMU extrinsic (ombc/tbc) in the calibration or bad id")
        })?;
        tic.push(pose_json(&p, "            "));
        let k = &cal.cameras[i].intr;
        intr.push(format!(
            "            {{\n                \"camera_type\": \"kb4\",\n                \"intrinsics\": {{\n                    \"fx\": {},\n                    \"fy\": {},\n                    \"cx\": {},\n                    \"cy\": {},\n                    \"k1\": {},\n                    \"k2\": {},\n                    \"k3\": {},\n                    \"k4\": {}\n                }}\n            }}",
            json::num(k.fx),
            json::num(k.fy),
            json::num(k.cx),
            json::num(k.cy),
            json::num(k.k[0]),
            json::num(k.k[1]),
            json::num(k.k[2]),
            json::num(k.k[3])
        ));
        res.push(format!("            [{}, {}]", k.width, k.height));
    }
    let z9 = json::num_array(&[0.0; 9]);
    let mut gb = [0.0; 12];
    gb[..3].copy_from_slice(&setup.gyro_bias);
    let z12 = json::num_array(&gb);
    Ok(format!(
        "{{\n    \"value0\": {{\n        \"T_imu_cam\": [\n{}\n        ],\n        \"intrinsics\": [\n{}\n        ],\n        \"resolution\": [\n{}\n        ],\n        \"calib_accel_bias\": {z9},\n        \"calib_gyro_bias\": {z12},\n        \"imu_update_rate\": {},\n        \"accel_noise_std\": {},\n        \"gyro_noise_std\": {},\n        \"accel_bias_std\": {},\n        \"gyro_bias_std\": {},\n        \"T_mocap_world\": {},\n        \"T_imu_marker\": {},\n        \"mocap_time_offset_ns\": 0,\n        \"mocap_to_imu_offset_ns\": 0,\n        \"cam_time_offset_ns\": {},\n        \"vignette\": []\n    }}\n}}\n",
        tic.join(",\n"),
        intr.join(",\n"),
        res.join(",\n"),
        json::num(imu.rate_hz),
        json::num_array(&imu.accel_noise_std),
        json::num_array(&imu.gyro_noise_std),
        json::num_array(&imu.accel_bias_std),
        json::num_array(&imu.gyro_bias_std),
        pose_json(&Pose::IDENTITY, "        ").trim_start(),
        pose_json(&Pose::IDENTITY, "        ").trim_start(),
        setup.cam_time_offset_ns
    ))
}

/// A short human/machine-readable description of a recording's sensor setup (`pimax-slam-info.json`).
pub fn info_json(
    cal: &DeviceCalib,
    setup: &SlamSetup,
    imu: &ImuNoise,
    extra: &[(&str, String)],
) -> String {
    let names: Vec<String> = setup
        .cams
        .iter()
        .map(|&i| json::quote(cal.cameras.get(i).map(|c| c.name.as_str()).unwrap_or("?")))
        .collect();
    let ids: Vec<f64> = setup.cams.iter().map(|&i| i as f64).collect();
    let mut s = format!(
        "{{\n  \"device_uid\": {},\n  \"factory_cams\": {},\n  \"cam_names\": [{}],\n  \"cam_hz\": {},\n  \"imu_frame\": \"factory body = OpenXR head axes (x right, y up, z back) = raw IMU with x,z negated\",\n  \"imu_noise_source\": {},\n  \"cam_time_offset_ns\": {},\n  \"gyro_bias_rad_s\": {}",
        json::quote(&cal.uid),
        json::num_array(&ids),
        names.join(", "),
        json::num(setup.cam_hz),
        json::quote(&imu.source),
        setup.cam_time_offset_ns,
        json::num_array(&setup.gyro_bias)
    );
    for (k, v) in extra {
        s.push_str(&format!(",\n  {}: {}", json::quote(k), json::quote(v)));
    }
    s.push_str("\n}\n");
    s
}

/// Write `pimax-basalt-calib.json` and `pimax-slam-info.json` into `dir` (created if needed).
pub fn write_basalt_files(
    dir: &Path,
    cal: &DeviceCalib,
    setup: &SlamSetup,
    imu: &ImuNoise,
    extra: &[(&str, String)],
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let calib = dir.join("pimax-basalt-calib.json");
    std::fs::write(&calib, basalt_calib_json(cal, setup, imu)?)
        .map_err(|e| format!("{}: {e}", calib.display()))?;
    let info = dir.join("pimax-slam-info.json");
    std::fs::write(&info, info_json(cal, setup, imu, extra))
        .map_err(|e| format!("{}: {e}", info.display()))?;
    Ok(calib)
}

/// Default head origin in the IMU frame: midway between the two lower tracking cameras, 5 cm behind them (same
/// estimate as the Mercury hand attachment, `PIMAX_HT_HEAD_OFFSET`).
pub fn default_head_in_imu(cal: &DeviceCalib) -> Vec3 {
    match (t_imu_cam(cal, 0), t_imu_cam(cal, 1)) {
        (Some(a), Some(b)) => {
            let m = geom::scale(geom::add(a.t, b.t), 0.5);
            [m[0], m[1], m[2] + 0.05]
        }
        _ => [0.0; 3],
    }
}

/*
 * Supervisor
 */

/// Supervisor thresholds.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SupervisorParams {
    /// Faster than this between two tracker poses = jump (m/s).
    pub max_speed_m_s: f64,
    /// Tracker relative rotation vs gyro (3DoF) relative rotation between two poses, degrees.
    pub max_rot_mismatch_deg: f64,
    /// Consecutive consistent poses needed to (re)acquire.
    pub acquire_poses: u32,
    /// Reserved, zero.
    pub reserved: u32,
    /// No new tracker pose for this long = lost (ns).
    pub stale_ns: u64,
    /// Lost for this long = ask for a tracker reset (ns, 0 = never).
    pub reset_after_ns: u64,
    /// At most one reset request per this interval (ns).
    pub reset_every_ns: u64,
}

impl Default for SupervisorParams {
    fn default() -> Self {
        SupervisorParams {
            max_speed_m_s: 4.0,
            max_rot_mismatch_deg: 8.0,
            acquire_poses: 12,
            reserved: 0,
            stale_ns: 350_000_000,
            reset_after_ns: 3_000_000_000,
            reset_every_ns: 6_000_000_000,
        }
    }
}

/// What the supervisor does while the IMU says the headset is perfectly still (lying on a desk or a cushion).
/// With nothing in the stereo overlap beyond Basalt's 0.33 m triangulation limit (cameras facing a cushion, a
/// wall, the desk) the tracker has no landmarks and integrates the IMU alone, so its position runs away
/// quadratically (6.7 m in 14 s in the 2026-10-14 walk, analysis/slam-drift.md).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StillParams {
    /// Hold the head position while still: the tracker's motion goes into the room alignment, not the output.
    pub hold: bool,
    /// The tracker moved this far (m) while still: reset it (a fresh start while still has the right zero
    /// velocity and attitude), so the walk that follows does not inherit a runaway velocity.
    pub reset_m: f64,
    /// At most one such reset per this interval (ns).
    pub reset_every_ns: u64,
    /// Re-zero instead of dropping out: the tracker is reset but the output stays exactly where it is (position
    /// held, orientation continuous through the gyro with the tracker's full attitude), no 6DoF loss is counted,
    /// and the fresh tracker world is aligned to the held pose silently. `false` = the old behaviour (a loss to
    /// 3DoF with a yaw-only fix, re-aligned as a new acquisition; `PIMAX_SLAM_STILL_REZERO=0`).
    pub rezero: bool,
    /// Consistent poses of the fresh tracker needed to take over after a re-zero (the headset lies still, so a
    /// short run is enough; a pickup during the gap would otherwise move the head with the position held).
    pub rezero_poses: u32,
}

impl Default for StillParams {
    fn default() -> Self {
        StillParams {
            hold: true,
            reset_m: 0.03,
            reset_every_ns: 1_000_000_000,
            rezero: true,
            rezero_poses: 3,
        }
    }
}

/// Tracking state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum TrackState {
    /// No usable 6DoF yet (3DoF output at the held position).
    Init = 0,
    /// Tracker poses are used.
    Tracking = 1,
    /// Lost after tracking (3DoF at the held position, yaw kept continuous).
    Lost = 2,
}

/// Event bits of one update.
pub mod events {
    /// Entered Tracking.
    pub const ACQUIRED: u32 = 1;
    /// Left Tracking.
    pub const LOST: u32 = 2;
    /// The caller should reset the tracker (`t_slam_reset`).
    pub const RESET_WANTED: u32 = 4;
    /// A recenter was applied.
    pub const RECENTERED: u32 = 8;
    /// Tracking came back in the same tracker world and the old room alignment was restored.
    pub const RESNAPPED: u32 = 16;
    /// The tracker drifted while the headset lay still and is re-zeroed (reset, output held; no loss).
    pub const REZEROED: u32 = 32;
}

/// Re-snap rule: when tracking returns, keep the room alignment from before the loss if the tracker world is
/// still the same one (no reset since, yaw consistent with the gyro, head position plausible).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Resnap {
    /// Largest yaw difference between the old alignment and the 3DoF-heading one (deg).
    pub yaw_deg: f64,
    /// The head may have moved at most `max(min_m, speed_m_s * lost time)` from the held position.
    pub speed_m_s: f64,
    /// See `speed_m_s`.
    pub min_m: f64,
}

/// One tracker pose: IMU frame in the tracker's world (already rotated to y-up), and the 3DoF filter's IMU
/// orientation at the same time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackSample {
    /// Tracker timestamp (monotonic ns).
    pub ts_ns: u64,
    /// Position of the IMU in the tracker world.
    pub pos: Vec3,
    /// Orientation of the IMU in the tracker world.
    pub q: Quat,
    /// 3DoF orientation of the IMU at `ts_ns`.
    pub q3dof: Quat,
}

/// The output the driver applies.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackOutput {
    /// State.
    pub state: TrackState,
    /// [`events`] bits raised by this update.
    pub events: u32,
    /// `world_from_slam` (yaw + translation) for tracker poses.
    pub world_from_slam: (Vec3, Quat),
    /// Correction applied to the 3DoF orientation while not tracking (yaw only after a loss; the full attitude
    /// of the tracker during a still re-zero, so the tilt does not change either).
    pub fix3dof: Quat,
    /// Head position while not tracking.
    pub held_pos: Vec3,
    /// Why the last loss/reject happened.
    pub reason: &'static str,
}

/// The state machine (see the module docs).
#[derive(Clone, Debug)]
pub struct Supervisor {
    p: SupervisorParams,
    head_in_imu: Vec3,
    state: TrackState,
    good_run: u32,
    last: Option<TrackSample>,
    last_new_at_ns: u64,
    w_t: Vec3,
    w_q: Quat,
    fix3dof: Quat,
    held_pos: Vec3,
    lost_since_ns: u64,
    last_reset_ns: u64,
    recenter: bool,
    reason: &'static str,
    resnap: Option<Resnap>,
    lost_w: Option<(Vec3, Quat)>,
    reset_since_loss: bool,
    still_p: StillParams,
    still: bool,
    /// While still and tracking: (tracker-world head position, world head position) at the start of the still
    /// period (or of the tracker world, after a reset).
    still_ref: Option<(Vec3, Vec3)>,
    last_still_reset_ns: u64,
    /// A still re-zero is in progress: the next acquisition is a silent re-alignment, not a new acquisition.
    rezeroing: bool,
    /// Tracker resets raised because it drifted while the headset was still (also counted in `resets`).
    pub still_resets: u64,
    /// Times Tracking was entered.
    pub acquisitions: u64,
    /// Times Tracking was left.
    pub losses: u64,
    /// Reset requests raised.
    pub resets: u64,
    /// Tracker poses rejected as inconsistent.
    pub rejects: u64,
}

impl Supervisor {
    /// New supervisor; `head_in_imu` = head origin in the IMU frame (m).
    pub fn new(p: SupervisorParams, head_in_imu: Vec3) -> Supervisor {
        Supervisor {
            p,
            head_in_imu,
            state: TrackState::Init,
            good_run: 0,
            last: None,
            last_new_at_ns: 0,
            w_t: [0.0; 3],
            w_q: QUAT_ID,
            fix3dof: QUAT_ID,
            held_pos: [0.0; 3],
            lost_since_ns: 0,
            last_reset_ns: 0,
            recenter: false,
            reason: "no tracker pose yet",
            resnap: None,
            lost_w: None,
            reset_since_loss: false,
            still_p: StillParams::default(),
            still: false,
            still_ref: None,
            last_still_reset_ns: 0,
            rezeroing: false,
            still_resets: 0,
            acquisitions: 0,
            losses: 0,
            resets: 0,
            rejects: 0,
        }
    }

    /// Enable (or disable with `None`) restoring the old room alignment after a loss in the same tracker world.
    pub fn set_resnap(&mut self, r: Option<Resnap>) {
        self.resnap = r;
    }

    /// Still-headset behaviour (see [`StillParams`]).
    pub fn set_still_params(&mut self, p: StillParams) {
        self.still_p = p;
    }

    /// The IMU says the headset is perfectly still (call before [`Supervisor::update`]).
    pub fn set_still(&mut self, still: bool) {
        self.still = still;
    }

    /// Current state.
    pub fn state(&self) -> TrackState {
        self.state
    }

    /// Head position in the world for a tracker pose (with the current alignment).
    pub fn head_world(&self, pos: Vec3, q: Quat) -> Vec3 {
        let head_slam = geom::add(pos, qrot(q, self.head_in_imu));
        geom::add(qrot(self.w_q, head_slam), self.w_t)
    }

    /// Put the head at the world origin (x, y, z = 0) without changing its heading; applied at the next update.
    pub fn request_recenter(&mut self) {
        self.recenter = true;
    }

    fn output(&self, events: u32) -> TrackOutput {
        TrackOutput {
            state: self.state,
            events,
            world_from_slam: (self.w_t, self.w_q),
            fix3dof: self.fix3dof,
            held_pos: self.held_pos,
            reason: self.reason,
        }
    }

    /// Check a new sample against the previous one; `Err(reason)` when inconsistent.
    fn consistent(&self, prev: &TrackSample, s: &TrackSample) -> Result<(), &'static str> {
        let vals = s.pos.iter().chain(s.q.iter()).chain(s.q3dof.iter());
        if vals.into_iter().any(|x| !x.is_finite()) {
            return Err("non-finite pose");
        }
        let dt = (s.ts_ns - prev.ts_ns) as f64 / 1e9;
        let speed = geom::norm(geom::sub(s.pos, prev.pos)) / dt.max(1e-3);
        if speed > self.p.max_speed_m_s {
            return Err("position jump");
        }
        let d_slam = qmul(qconj(qnorm(prev.q)), qnorm(s.q));
        let d_imu = qmul(qconj(qnorm(prev.q3dof)), qnorm(s.q3dof));
        if qangle_deg(qmul(qconj(d_slam), d_imu)) > self.p.max_rot_mismatch_deg {
            return Err("rotation disagrees with the gyro");
        }
        Ok(())
    }

    /// Align the tracker world so that this sample's head lands on the held position with the 3DoF heading, or,
    /// with re-snap on and the tracker world unchanged since the loss, restore the alignment from before the
    /// loss (returns true then).
    fn acquire(&mut self, s: &TrackSample, now_ns: u64) -> bool {
        let target_q = qmul(self.fix3dof, qnorm(s.q3dof));
        let new_q = qyaw(yaw_of(qmul(target_q, qconj(qnorm(s.q)))));
        let head_slam = geom::add(s.pos, qrot(qnorm(s.q), self.head_in_imu));
        let new_t = geom::sub(self.held_pos, qrot(new_q, head_slam));
        let mut restored = false;
        if let (Some(r), Some((old_t, old_q)), false) =
            (self.resnap, self.lost_w, self.reset_since_loss)
        {
            let mut dyaw = yaw_of(new_q) - yaw_of(old_q);
            while dyaw > std::f64::consts::PI {
                dyaw -= 2.0 * std::f64::consts::PI;
            }
            while dyaw < -std::f64::consts::PI {
                dyaw += 2.0 * std::f64::consts::PI;
            }
            let head_old = geom::add(qrot(old_q, head_slam), old_t);
            let lost_s = now_ns.saturating_sub(self.lost_since_ns) as f64 / 1e9;
            let allowed = r.min_m.max(r.speed_m_s * lost_s);
            if dyaw.abs().to_degrees() <= r.yaw_deg
                && geom::norm(geom::sub(head_old, self.held_pos)) <= allowed
            {
                self.w_t = old_t;
                self.w_q = old_q;
                restored = true;
            }
        }
        if !restored {
            self.w_q = new_q;
            self.w_t = new_t;
        }
        self.lost_w = None;
        self.state = TrackState::Tracking;
        self.acquisitions += 1;
        self.reason = if restored {
            "tracking (room restored)"
        } else if self.acquisitions > 1 {
            "tracking (re-aligned)"
        } else {
            "tracking"
        };
        restored
    }

    /// Leave Tracking: hold the last good head position, keep the heading continuous.
    fn lose(&mut self, now_ns: u64, reason: &'static str) {
        if self.state == TrackState::Tracking {
            self.lost_w = Some((self.w_t, self.w_q));
            self.reset_since_loss = false;
        }
        if let Some(l) = self.last {
            self.held_pos = self.head_world(l.pos, l.q);
            let world_q = qmul(self.w_q, qnorm(l.q));
            self.fix3dof = qyaw(yaw_of(qmul(world_q, qconj(qnorm(l.q3dof)))));
        }
        self.state = TrackState::Lost;
        self.lost_since_ns = now_ns;
        self.good_run = 0;
        self.losses += 1;
        self.reason = reason;
    }

    /// One update. `sample` = the tracker's newest pose (or `None`); repeated timestamps are ignored.
    pub fn update(&mut self, now_ns: u64, sample: Option<TrackSample>) -> TrackOutput {
        let mut ev = 0;
        if self.recenter {
            self.recenter = false;
            ev |= events::RECENTERED;
            match (self.state, self.last) {
                (TrackState::Tracking, Some(l)) => {
                    let head = self.head_world(l.pos, l.q);
                    self.w_t = geom::sub(self.w_t, head);
                }
                _ => self.held_pos = [0.0; 3],
            }
        }
        let fresh = match (sample, self.last) {
            (Some(s), Some(l)) => s.ts_ns > l.ts_ns,
            (Some(_), None) => true,
            _ => false,
        };
        if fresh {
            let s = sample.unwrap();
            self.last_new_at_ns = now_ns;
            let check = match self.last {
                Some(l) => self.consistent(&l, &s),
                None => Ok(()),
            };
            match (self.state, check) {
                (TrackState::Tracking, Ok(())) => {}
                (TrackState::Tracking, Err(r)) => {
                    self.rejects += 1;
                    self.lose(now_ns, r);
                    ev |= events::LOST;
                }
                (_, Ok(())) => {
                    self.good_run += 1;
                    let need = if self.rezeroing {
                        self.still_p.rezero_poses
                    } else {
                        self.p.acquire_poses
                    };
                    if self.good_run >= need.max(1) {
                        if self.rezeroing {
                            // Silent: the fresh tracker world takes over the held pose, nothing was lost.
                            self.rezeroing = false;
                            self.acquire(&s, now_ns);
                            self.acquisitions -= 1;
                            self.reason = "tracking (re-zeroed while still)";
                        } else {
                            if self.acquire(&s, now_ns) {
                                ev |= events::RESNAPPED;
                            }
                            ev |= events::ACQUIRED;
                        }
                    }
                }
                (_, Err(r)) => {
                    self.rejects += 1;
                    self.good_run = 0;
                    if !self.rezeroing {
                        self.reason = r;
                    }
                }
            }
            self.last = Some(s);
        } else if self.state == TrackState::Tracking
            && now_ns.saturating_sub(self.last_new_at_ns) > self.p.stale_ns
        {
            self.lose(now_ns, "no tracker pose (stale)");
            ev |= events::LOST;
        }
        if self.state == TrackState::Tracking && self.still && self.still_p.hold {
            ev |= self.hold_still(now_ns);
        } else {
            self.still_ref = None;
        }
        if self.state == TrackState::Lost
            && self.p.reset_after_ns > 0
            && now_ns.saturating_sub(self.lost_since_ns) > self.p.reset_after_ns
            && (self.last_reset_ns == 0
                || now_ns.saturating_sub(self.last_reset_ns) > self.p.reset_every_ns)
        {
            self.last_reset_ns = now_ns;
            ev |= self.request_reset();
        }
        self.output(ev)
    }

    /// A reset restarts the tracker's own world: require a fresh consistent run, no re-snap.
    fn request_reset(&mut self) -> u32 {
        self.resets += 1;
        self.last = None;
        self.good_run = 0;
        self.reset_since_loss = true;
        self.lost_w = None;
        self.still_ref = None;
        events::RESET_WANTED
    }

    /// The tracker drifted while the headset lies still: reset it without a dropout. The output keeps the pose
    /// it has now (the held still target, and the tracker's full world orientation carried on by the gyro, so
    /// neither position nor tilt moves at either end), and the fresh tracker world is aligned to that pose as
    /// soon as it gives a consistent run.
    fn rezero(&mut self, now_ns: u64) -> u32 {
        if let Some(l) = self.last {
            self.held_pos = self.head_world(l.pos, l.q);
            let world_q = qmul(self.w_q, qnorm(l.q));
            self.fix3dof = qnorm(qmul(world_q, qconj(qnorm(l.q3dof))));
        }
        self.state = TrackState::Lost;
        self.lost_since_ns = now_ns;
        self.good_run = 0;
        self.rezeroing = true;
        self.reason = "re-zeroed while still";
        events::REZEROED | self.request_reset()
    }

    /// Tracking while the IMU is still: keep the head where it was when the still period began (the tracker's
    /// own motion goes into `w_t`), and reset the tracker once it has drifted `reset_m`.
    fn hold_still(&mut self, now_ns: u64) -> u32 {
        let Some(l) = self.last else {
            return 0;
        };
        let head_slam = geom::add(l.pos, qrot(qnorm(l.q), self.head_in_imu));
        let Some((ref_slam, target)) = self.still_ref else {
            self.still_ref = Some((head_slam, self.head_world(l.pos, l.q)));
            return 0;
        };
        self.w_t = geom::sub(target, qrot(self.w_q, head_slam));
        let drift = geom::norm(geom::sub(head_slam, ref_slam));
        if drift > self.still_p.reset_m
            && (self.last_still_reset_ns == 0
                || now_ns.saturating_sub(self.last_still_reset_ns) > self.still_p.reset_every_ns)
        {
            self.last_still_reset_ns = now_ns;
            self.still_resets += 1;
            if self.still_p.rezero {
                return self.rezero(now_ns);
            }
            // Old behaviour: held at `target` (lose() takes the head position with the adjusted alignment),
            // 3DoF until the fresh tracker world is acquired again at that position.
            self.lose(now_ns, "tracker drifting while still");
            return events::LOST | self.request_reset();
        }
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cal() -> DeviceCalib {
        let p = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../analysis/headset-calibration.xml"
        );
        DeviceCalib::load(Path::new(p)).expect("factory calibration")
    }

    fn close3(a: Vec3, b: Vec3, eps: f64) -> bool {
        geom::norm(geom::sub(a, b)) < eps
    }

    #[test]
    #[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
    fn extrinsics_are_plausible() {
        let c = cal();
        let a = t_imu_cam(&c, 0).unwrap();
        let b = t_imu_cam(&c, 1).unwrap();
        // Lower tracking cameras: about 10 cm apart along body x, a few cm from the IMU, looking forward (-z).
        let base = geom::sub(b.t, a.t);
        assert!((geom::norm(base) - 0.112).abs() < 0.02, "baseline {base:?}");
        assert!(base[0].abs() > 0.08, "baseline mostly along x: {base:?}");
        for p in [a, b] {
            assert!(geom::norm(p.t) < 0.12);
            let fwd = geom::col(&p.r, 2);
            assert!(fwd[2] < -0.5, "optical axis looks forward: {fwd:?}");
            assert!(geom::orthonormality_error(&p.r) < 1e-9);
        }
    }

    #[test]
    #[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
    fn basalt_json_parses_and_roundtrips() {
        let c = cal();
        let setup = SlamSetup {
            cams: vec![0, 1, 2, 3],
            ..Default::default()
        };
        let s = basalt_calib_json(&c, &setup, &ImuNoise::default()).unwrap();
        let v = json::parse(&s).unwrap();
        let v0 = v.get("value0").unwrap();
        let t = v0.get("T_imu_cam").unwrap().as_array().unwrap();
        assert_eq!(t.len(), 4);
        let intr = v0.get("intrinsics").unwrap().as_array().unwrap();
        assert_eq!(intr[1].get("camera_type").unwrap().as_str(), Some("kb4"));
        let fx = intr[1]
            .get("intrinsics")
            .unwrap()
            .get("fx")
            .unwrap()
            .as_f64()
            .unwrap();
        assert!((fx - c.cameras[1].intr.fx).abs() < 1e-12);
        // T_imu_cam of cam 1 equals t_imu_cam().
        let q = [
            t[1].get("qx").unwrap().as_f64().unwrap(),
            t[1].get("qy").unwrap().as_f64().unwrap(),
            t[1].get("qz").unwrap().as_f64().unwrap(),
            t[1].get("qw").unwrap().as_f64().unwrap(),
        ];
        let p = t_imu_cam(&c, 1).unwrap();
        let m = mat_of(q);
        for (mr, pr) in m.iter().zip(p.r.iter()) {
            for (a, b) in mr.iter().zip(pr.iter()) {
                assert!((a - b).abs() < 1e-9);
            }
        }
        assert_eq!(v0.get("imu_update_rate").unwrap().as_f64(), Some(1000.0));
    }

    #[test]
    fn imu_noise_file() {
        let n = ImuNoise {
            gyro_noise_std: [1e-4, 2e-4, 3e-4],
            ..Default::default()
        };
        let back = ImuNoise::parse(&n.to_json("test")).unwrap();
        assert_eq!(back.gyro_noise_std, n.gyro_noise_std);
        let scalar = ImuNoise::parse("{\"accel_noise_std\": 0.01}").unwrap();
        assert_eq!(scalar.accel_noise_std, [0.01; 3]);
        assert!(ImuNoise::parse("{\"gyro_bias_std\": 0}").is_err());
    }

    #[test]
    fn cams_list() {
        assert_eq!(parse_cams("0,1", 4).unwrap(), vec![0, 1]);
        assert_eq!(parse_cams(" 0, 1,2,3 ", 4).unwrap(), vec![0, 1, 2, 3]);
        assert!(parse_cams("0,0", 4).is_err());
        assert!(parse_cams("4", 4).is_err());
        assert!(parse_cams("", 4).is_err());
    }

    #[test]
    fn yaw_extraction() {
        for deg in [-170.0f64, -45.0, 0.0, 30.0, 135.0] {
            let y = deg.to_radians();
            // yaw then some pitch: yaw_of must still return the yaw.
            let q = qmul(qyaw(y), [(0.2f64).sin(), 0.0, 0.0, (0.2f64).cos()]);
            assert!((yaw_of(q) - y).abs() < 1e-9, "{deg}");
        }
    }

    /// Simulated head walking a circle; the tracker world is yawed 70 deg and offset from ours.
    fn sim(t: f64, slam_yaw: f64, slam_off: Vec3) -> (Vec3, Quat, Quat) {
        let head_yaw = 0.5 * t;
        let q_world = qmul(qyaw(head_yaw), [(0.1f64).sin(), 0.0, 0.0, (0.1f64).cos()]);
        let pos_world = [
            0.5 * (0.3 * t).sin(),
            0.02 * t.sin(),
            0.5 * (0.3 * t).cos() - 0.5,
        ];
        let wq = qyaw(slam_yaw);
        let q = qmul(wq, q_world);
        let pos = geom::add(qrot(wq, pos_world), slam_off);
        (pos, q, q_world)
    }

    #[test]
    fn supervisor_acquires_aligns_and_survives_a_jump() {
        let head = [0.0, -0.02, 0.07];
        let mut s = Supervisor::new(SupervisorParams::default(), head);
        let (yaw, off) = (70f64.to_radians(), [3.0, -1.0, 2.0]);
        let mut t = 0.0;
        let mut acquired_at = None;
        for i in 0..40 {
            let (pos, q, q3) = sim(t, yaw, off);
            let ts = (1e9 + t * 1e9) as u64;
            let o = s.update(
                ts + 15_000_000,
                Some(TrackSample {
                    ts_ns: ts,
                    pos,
                    q,
                    q3dof: q3,
                }),
            );
            if o.events & events::ACQUIRED != 0 {
                acquired_at = Some(i);
                // Head lands at the origin (continuity with the 3DoF output at the origin).
                assert!(close3(s.head_world(pos, q), [0.0; 3], 1e-9));
            }
            t += 0.04;
        }
        assert_eq!(acquired_at, Some(11));
        assert_eq!(s.state(), TrackState::Tracking);
        // After acquisition the world orientation of the IMU equals the 3DoF one (yaw aligned).
        let (pos, q, q3) = sim(t, yaw, off);
        let o = s.update(
            (1e9 + t * 1e9) as u64,
            Some(TrackSample {
                ts_ns: (1e9 + t * 1e9) as u64,
                pos,
                q,
                q3dof: q3,
            }),
        );
        let wq = qmul(o.world_from_slam.1, q);
        assert!(qangle_deg(qmul(qconj(wq), q3)) < 1e-6);
        let before = s.head_world(pos, q);
        // A 1 m jump in one frame: lost, held at the last good head position.
        t += 0.04;
        let (mut pos2, q2, q32) = sim(t, yaw, off);
        pos2[0] += 1.0;
        let ts2 = (1e9 + t * 1e9) as u64;
        let o = s.update(
            ts2,
            Some(TrackSample {
                ts_ns: ts2,
                pos: pos2,
                q: q2,
                q3dof: q32,
            }),
        );
        assert_eq!(o.state, TrackState::Lost);
        assert!(o.events & events::LOST != 0);
        assert!(close3(o.held_pos, before, 1e-9));
        // Heading continuity: the corrected 3DoF orientation equals the last tracked world orientation.
        assert!(qangle_deg(qmul(qconj(qmul(o.fix3dof, q3)), wq)) < 1e-6);
        // The tracker comes back (new, shifted world): re-acquire at the held position.
        let off2 = [10.0, 0.0, 0.0];
        let mut reacq = false;
        for _ in 0..30 {
            t += 0.04;
            let (pos, q, q3) = sim(t, yaw + 0.5, off2);
            let ts = (1e9 + t * 1e9) as u64;
            let o = s.update(
                ts,
                Some(TrackSample {
                    ts_ns: ts,
                    pos,
                    q,
                    q3dof: q3,
                }),
            );
            if o.events & events::ACQUIRED != 0 {
                reacq = true;
                assert!(close3(s.head_world(pos, q), o.held_pos, 1e-9));
            }
        }
        assert!(reacq);
        assert_eq!(s.losses, 1);
        assert_eq!(s.acquisitions, 2);
    }

    #[test]
    #[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
    fn gyro_bias_cache_is_raw_dps() {
        // The driver writes raw IMU axes in deg/s; the body frame negates x and z.
        let b = parse_gyro_bias_cache("0.2315 0.7274 0.4080\n").unwrap();
        let want = [-0.2315f64, 0.7274, -0.4080].map(f64::to_radians);
        assert!(close3(b, want, 1e-12));
        assert!(parse_gyro_bias_cache("1 2").is_none());
        assert!(parse_gyro_bias_cache("9 0 0").is_none());
        let c = cal();
        let setup = SlamSetup {
            gyro_bias: b,
            ..Default::default()
        };
        let v = json::parse(&basalt_calib_json(&c, &setup, &ImuNoise::default()).unwrap()).unwrap();
        let g = v.get("value0").unwrap().get("calib_gyro_bias").unwrap().as_array().unwrap();
        assert_eq!(g.len(), 12);
        assert!((g[1].as_f64().unwrap() - b[1]).abs() < 1e-12);
        assert_eq!(g[3].as_f64(), Some(0.0));
    }

    /// Feed `n` tracker poses 40 ms apart, position `pos(i)`, identity rotation, IMU still flag `still`.
    fn feed(s: &mut Supervisor, t0: u64, n: u64, still: bool, pos: impl Fn(u64) -> Vec3) -> (u32, u64) {
        let mut ev = 0;
        let mut ts = t0;
        for i in 0..n {
            ts = t0 + i * 40_000_000;
            s.set_still(still);
            ev |= s
                .update(
                    ts,
                    Some(TrackSample {
                        ts_ns: ts,
                        pos: pos(i),
                        q: QUAT_ID,
                        q3dof: QUAT_ID,
                    }),
                )
                .events;
        }
        (ev, ts)
    }

    #[test]
    fn still_headset_holds_and_rezeroes_a_drifting_tracker_without_a_loss() {
        let p = SupervisorParams {
            acquire_poses: 3,
            ..Default::default()
        };
        let mut s = Supervisor::new(p, [0.0; 3]);
        // A tracker with no landmarks: constant acceleration 0.07 m/s^2 while the headset lies still, with a
        // 2 deg tilt between the tracker's and the 3DoF filter's attitude (as live: 1-3 deg).
        let tilt = qnorm([2f64.to_radians().sin() * 0.5, 0.0, 0.0, 1.0]);
        let drift = |t: f64| [0.5 * 0.07 * t * t, 0.0, 0.0];
        let mut ev = 0;
        let mut ts = 1_000_000_000;
        for i in 0..50u64 {
            ts = 1_000_000_000 + i * 40_000_000;
            s.set_still(true);
            let smp = TrackSample {
                ts_ns: ts,
                pos: drift(i as f64 * 0.04),
                q: tilt,
                q3dof: QUAT_ID,
            };
            ev |= s.update(ts, Some(smp)).events;
            if ev & events::RESET_WANTED != 0 {
                break; // the tracker restarts: its old world ends here
            }
        }
        assert!(
            ev & events::RESET_WANTED != 0,
            "drift > 3 cm while still must reset the tracker"
        );
        assert!(ev & events::REZEROED != 0);
        assert_eq!(ev & events::LOST, 0, "a re-zero is not a dropout");
        assert_eq!((s.still_resets, s.losses), (1, 0));
        // Output held at the acquisition position (origin), and the 3DoF fix carries the tracker's full
        // attitude (tilt included): fix * q3dof == world orientation of the tracker.
        let o = s.output(0);
        assert!(close3(o.held_pos, [0.0; 3], 1e-9));
        assert!(qangle_deg(qmul(qconj(qmul(o.fix3dof, QUAT_ID)), tilt)) < 1e-6);
        // New tracker world after the reset (another origin and yaw): silently re-aligned at the held position.
        let (ev, ts) = feed(&mut s, ts + 40_000_000, 10, true, |i| {
            geom::add([5.0, 0.0, 0.0], [0.0001 * i as f64, 0.0, 0.0])
        });
        assert_eq!(
            ev & (events::ACQUIRED | events::LOST),
            0,
            "no acquisition / loss is reported"
        );
        assert_eq!(s.state(), TrackState::Tracking);
        assert_eq!((s.acquisitions, s.losses), (1, 0));
        let l = s.last.unwrap();
        assert!(geom::norm(s.head_world(l.pos, l.q)) < 0.002);
        // Moving (not still): no hold, the output follows the tracker.
        let (ev, _) = feed(&mut s, ts + 40_000_000, 10, false, |i| {
            [5.0 + 0.05 * i as f64, 0.0, 0.0]
        });
        assert_eq!(ev & events::RESET_WANTED, 0);
        let l = s.last.unwrap();
        assert!(s.head_world(l.pos, l.q)[0] > 0.3);
    }

    #[test]
    fn still_rezero_off_is_the_old_dropout() {
        let p = SupervisorParams {
            acquire_poses: 3,
            ..Default::default()
        };
        let mut s = Supervisor::new(p, [0.0; 3]);
        s.set_still_params(StillParams {
            rezero: false,
            ..Default::default()
        });
        let drift = |t: f64| [0.5 * 0.07 * t * t, 0.0, 0.0];
        // 25 poses: up to the reset (3 cm from the hold start), the tracker restarts then.
        let (ev, ts) = feed(&mut s, 1_000_000_000, 25, true, |i| drift(i as f64 * 0.04));
        assert!(ev & events::LOST != 0 && ev & events::RESET_WANTED != 0);
        assert_eq!((s.still_resets, s.losses), (1, 1));
        let (ev, _) = feed(&mut s, ts + 40_000_000, 10, true, |i| {
            [5.0 + 0.0001 * i as f64, 0.0, 0.0]
        });
        assert!(ev & events::ACQUIRED != 0);
        assert_eq!(s.acquisitions, 2);
    }

    #[test]
    fn still_headset_with_a_good_tracker_is_left_alone() {
        let p = SupervisorParams {
            acquire_poses: 3,
            ..Default::default()
        };
        let mut s = Supervisor::new(p, [0.0; 3]);
        // Landmarks lock the pose: millimetre jitter only.
        let (ev, _) = feed(&mut s, 1_000_000_000, 200, true, |i| {
            [0.001 * ((i % 3) as f64 - 1.0), 0.0, 0.0]
        });
        assert_eq!(ev & events::RESET_WANTED, 0);
        assert_eq!(s.state(), TrackState::Tracking);
        let l = s.last.unwrap();
        assert!(geom::norm(s.head_world(l.pos, l.q)) < 1e-9, "held exactly");
    }

    #[test]
    fn supervisor_stale_and_reset() {
        let p = SupervisorParams {
            acquire_poses: 2,
            ..Default::default()
        };
        let mut s = Supervisor::new(p, [0.0; 3]);
        for i in 0..3u64 {
            let ts = 1_000_000_000 + i * 40_000_000;
            s.update(
                ts,
                Some(TrackSample {
                    ts_ns: ts,
                    pos: [0.0; 3],
                    q: QUAT_ID,
                    q3dof: QUAT_ID,
                }),
            );
        }
        assert_eq!(s.state(), TrackState::Tracking);
        let base = 1_080_000_000u64;
        // Same pose repeated (no new timestamp) for longer than stale_ns: lost.
        let o = s.update(base + 400_000_000, None);
        assert_eq!(o.state, TrackState::Lost);
        assert_eq!(o.reason, "no tracker pose (stale)");
        // Lost for > 3 s: one reset request, not repeated within 6 s.
        let o = s.update(base + 3_500_000_000, None);
        assert!(o.events & events::RESET_WANTED != 0);
        let o = s.update(base + 5_000_000_000, None);
        assert_eq!(o.events & events::RESET_WANTED, 0);
        let o = s.update(base + 9_600_000_000, None);
        assert!(o.events & events::RESET_WANTED != 0);
        assert_eq!(s.resets, 2);
    }

    #[test]
    fn supervisor_rejects_rotation_disagreement() {
        let p = SupervisorParams {
            acquire_poses: 3,
            ..Default::default()
        };
        let mut s = Supervisor::new(p, [0.0; 3]);
        // The tracker spins 20 deg per frame while the gyro says still: never acquires.
        for i in 0..20u64 {
            let ts = 1_000_000_000 + i * 40_000_000;
            let q = qyaw((20.0 * i as f64).to_radians());
            let o = s.update(
                ts,
                Some(TrackSample {
                    ts_ns: ts,
                    pos: [0.0; 3],
                    q,
                    q3dof: QUAT_ID,
                }),
            );
            assert_ne!(o.state, TrackState::Tracking);
        }
        assert!(s.rejects >= 18);
    }

    #[test]
    fn recenter_moves_head_to_origin() {
        let p = SupervisorParams {
            acquire_poses: 1,
            ..Default::default()
        };
        let mut s = Supervisor::new(p, [0.0; 3]);
        let mk = |i: u64, x: f64| TrackSample {
            ts_ns: 1_000_000_000 + i * 40_000_000,
            pos: [x, 0.0, 0.0],
            q: QUAT_ID,
            q3dof: QUAT_ID,
        };
        s.update(1_000_000_000, Some(mk(0, 0.0)));
        s.update(1_040_000_000, Some(mk(1, 0.05)));
        let p1 = s.head_world([0.05, 0.0, 0.0], QUAT_ID);
        assert!(geom::norm(p1) > 0.04);
        s.request_recenter();
        let o = s.update(1_080_000_000, Some(mk(2, 0.05)));
        assert!(o.events & events::RECENTERED != 0);
        assert!(close3(
            s.head_world([0.05, 0.0, 0.0], QUAT_ID),
            [0.0; 3],
            1e-9
        ));
    }
}
