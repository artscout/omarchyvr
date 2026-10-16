//! The room map: stereo keyframes (pose in the room frame + their triangulated ORB points in room
//! coordinates), saved per room, and relocalisation of a new frame against it (descriptor matching on the best
//! keyframes, then gravity-aware 4-DoF PnP over all rig cameras). Anchors are named poses in the same room
//! frame, so they come back where they were when a later session relocalises.
//!
//! Room frame: gravity aligned, y up (the frame the driver reports to Monado). A session's tracker world
//! ("slam" frame, also y up) is tied to it by `room_from_slam` = yaw + translation.

use crate::orb::{self, Descriptor};
use crate::pnp::{self, Corr, PnpParams};
use crate::stereo::{Features, Rig, StereoPoint};
use pimax_cam::geom::{self, Mat3, Pose, Vec3};
use pimax_cam::slam::{mat_of, qmul, quat_of, qyaw, yaw_of, Quat};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// File magic.
pub const MAGIC: &[u8; 4] = b"PXRM";
/// File format version.
pub const VERSION: u32 = 1;

/// One map point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MapPoint {
    /// Position in the room frame (m).
    pub p: Vec3,
    /// ORB descriptor.
    pub desc: Descriptor,
}

/// One keyframe.
#[derive(Clone, Debug, PartialEq)]
pub struct Keyframe {
    /// Id (unique in the map).
    pub id: u32,
    /// Capture time (ns, the session's clock; informative).
    pub ts_ns: i64,
    /// Session counter when it was added.
    pub session: u32,
    /// `room_from_body` at capture.
    pub pose: Pose,
    /// Points seen in this keyframe (room frame).
    pub points: Vec<MapPoint>,
}

/// A named pose in the room frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Anchor {
    /// Name.
    pub name: String,
    /// Position (m).
    pub p: Vec3,
    /// Orientation x y z w.
    pub q: Quat,
}

/// The map of one room.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RoomMap {
    /// Headset calibration uid the map was built with.
    pub device_uid: String,
    /// Factory camera ids of the rig.
    pub cams: Vec<u32>,
    /// Sessions merged into the map.
    pub sessions: u32,
    /// Keyframes.
    pub keyframes: Vec<Keyframe>,
    /// Anchors.
    pub anchors: Vec<Anchor>,
    next_id: u32,
}

/// Rotation of a gravity-aligned orientation without its yaw (roll/pitch only): `q = Ry(yaw_of(q)) * tilt`.
pub fn tilt_of(q: Quat) -> Mat3 {
    mat_of(qmul(qyaw(-yaw_of(q)), q))
}

/// Yaw-only part of a rotation matrix (gravity-aligned frames differ by a yaw).
pub fn yaw_of_mat(r: &Mat3) -> f64 {
    yaw_of(quat_of(r))
}

/// Relocalisation parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct RelocParams {
    /// Keyframes matched in full after ranking.
    pub top_k: usize,
    /// Query descriptors used for ranking.
    pub rank_sample: usize,
    /// Descriptor distance for ranking hits.
    pub rank_dist: u32,
    /// Matching distance / ratio.
    pub max_dist: u32,
    /// Lowe ratio.
    pub ratio: f32,
    /// PnP.
    pub pnp: PnpParams,
    /// Inliers needed.
    pub min_inliers: usize,
    /// Inlier fraction of the correspondences needed.
    pub min_inlier_frac: f64,
}

impl Default for RelocParams {
    fn default() -> Self {
        RelocParams {
            top_k: 3,
            rank_sample: 200,
            rank_dist: 45,
            max_dist: 60,
            ratio: 0.8,
            pnp: PnpParams {
                thresh_deg: 0.8,
                max_iters: 400,
                confidence: 0.999,
                min_inliers: 20,
                seed: 7,
            },
            min_inliers: 25,
            min_inlier_frac: 0.25,
        }
    }
}

/// A relocalisation.
#[derive(Clone, Debug, PartialEq)]
pub struct Reloc {
    /// `room_from_body` of the query.
    pub room_from_body: Pose,
    /// Inliers.
    pub inliers: usize,
    /// Correspondences tried.
    pub corrs: usize,
    /// RMS angular residual (deg).
    pub rms_deg: f64,
    /// Keyframes used.
    pub keyframes: Vec<u32>,
}

/// Session alignment from one relocalisation: `room_from_slam` (yaw, translation) given the tracker's pose of
/// the same frame (`slam_from_body`).
pub fn room_from_slam(r: &Reloc, slam_from_body: &Pose) -> (f64, Vec3) {
    let m = r.room_from_body.compose(&slam_from_body.inverse());
    let yaw = yaw_of_mat(&m.r);
    // Re-derive the translation for the yaw-only rotation so the body lands exactly on the relocalised position.
    let ry = mat_of(qyaw(yaw));
    let t = geom::sub(r.room_from_body.t, geom::mat_vec(&ry, slam_from_body.t));
    (yaw, t)
}

impl RoomMap {
    /// New, empty map.
    pub fn new(device_uid: &str, cams: &[usize]) -> RoomMap {
        RoomMap {
            device_uid: device_uid.to_string(),
            cams: cams.iter().map(|&c| c as u32).collect(),
            ..Default::default()
        }
    }

    /// Total map points.
    pub fn points(&self) -> usize {
        self.keyframes.iter().map(|k| k.points.len()).sum()
    }

    /// True when no keyframe is within `min_dist` metres *and* `min_angle_deg` of viewing direction.
    pub fn wants_keyframe(&self, room_from_body: &Pose, min_dist: f64, min_angle_deg: f64) -> bool {
        self.wants_keyframe_in(room_from_body, min_dist, min_angle_deg, None)
    }

    /// [`RoomMap::wants_keyframe`] counting only the keyframes of `session` (None: all keyframes).
    pub fn wants_keyframe_in(&self, room_from_body: &Pose, min_dist: f64, min_angle_deg: f64, session: Option<u32>) -> bool {
        let fwd = geom::mat_vec(&room_from_body.r, [0.0, 0.0, -1.0]);
        !self.keyframes.iter().filter(|k| session.is_none_or(|s| k.session == s)).any(|k| {
            let d = geom::norm(geom::sub(k.pose.t, room_from_body.t));
            let kf = geom::mat_vec(&k.pose.r, [0.0, 0.0, -1.0]);
            d < min_dist && geom::angle_deg(fwd, kf) < min_angle_deg
        })
    }

    /// Add a keyframe from stereo points (body frame) at `room_from_body`. Returns its id.
    pub fn add_keyframe(&mut self, ts_ns: i64, room_from_body: Pose, pts: &[StereoPoint]) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        let points = pts
            .iter()
            .map(|s| MapPoint {
                p: room_from_body.apply(s.p_body),
                desc: s.desc,
            })
            .collect();
        self.keyframes.push(Keyframe {
            id,
            ts_ns,
            session: self.sessions,
            pose: room_from_body,
            points,
        });
        id
    }

    /// Relocalise a frame: `query[i]` = features of rig camera i; `r_tilt` = the body's roll/pitch (from the
    /// tracker or the IMU); `prior` = (room position, radius) to restrict the keyframes searched.
    pub fn relocalize(
        &self,
        query: &[Features],
        rig: &Rig,
        r_tilt: &Mat3,
        prior: Option<(Vec3, f64)>,
        p: &RelocParams,
    ) -> Option<Reloc> {
        let cands: Vec<&Keyframe> = self
            .keyframes
            .iter()
            .filter(|k| prior.is_none_or(|(c, r)| geom::norm(geom::sub(k.pose.t, c)) <= r))
            .collect();
        if cands.is_empty() {
            return None;
        }
        // Rank keyframes by quick hits of a sample of query descriptors.
        let all_q: Vec<(usize, usize)> = query
            .iter()
            .enumerate()
            .flat_map(|(c, f)| (0..f.desc.len()).map(move |i| (c, i)))
            .collect();
        if all_q.is_empty() {
            return None;
        }
        let stride = (all_q.len() / p.rank_sample.max(1)).max(1);
        let sample: Vec<&Descriptor> = all_q
            .iter()
            .step_by(stride)
            .map(|&(c, i)| &query[c].desc[i])
            .collect();
        let mut scored: Vec<(usize, &Keyframe)> = cands
            .iter()
            .map(|k| {
                let hits = sample
                    .iter()
                    .filter(|q| {
                        k.points
                            .iter()
                            .any(|m| orb::hamming(q, &m.desc) <= p.rank_dist)
                    })
                    .count();
                (hits, *k)
            })
            .collect();
        scored.sort_by_key(|s| std::cmp::Reverse(s.0));
        let top: Vec<&Keyframe> = scored
            .iter()
            .take(p.top_k.max(1))
            .filter(|s| s.0 > 0)
            .map(|s| s.1)
            .collect();
        if top.is_empty() {
            return None;
        }
        // Full matching of every query camera against the pooled points of the top keyframes.
        let pts: Vec<&MapPoint> = top.iter().flat_map(|k| k.points.iter()).collect();
        let pdesc: Vec<Descriptor> = pts.iter().map(|m| m.desc).collect();
        let mut corrs = Vec::new();
        for (c, f) in query.iter().enumerate() {
            for (i, j, _) in orb::match_bf(&f.desc, &pdesc, p.max_dist, p.ratio) {
                corrs.push(Corr {
                    cam: c,
                    bearing: f.bearings[i],
                    point: pts[j].p,
                });
            }
        }
        if corrs.len() < p.min_inliers {
            return None;
        }
        let bfc = rig.body_from_cams();
        let res = pnp::ransac(&corrs, &bfc, r_tilt, &p.pnp)?;
        if res.inliers.len() < p.min_inliers
            || (res.inliers.len() as f64) < p.min_inlier_frac * corrs.len() as f64
        {
            return None;
        }
        Some(Reloc {
            room_from_body: res.pose.to_pose(r_tilt),
            inliers: res.inliers.len(),
            corrs: corrs.len(),
            rms_deg: res.rms_deg,
            keyframes: top.iter().map(|k| k.id).collect(),
        })
    }

    /// Set or replace an anchor.
    pub fn set_anchor(&mut self, name: &str, p: Vec3, q: Quat) {
        self.anchors.retain(|a| a.name != name);
        self.anchors.push(Anchor {
            name: name.to_string(),
            p,
            q,
        });
    }

    /// Remove an anchor; true if it existed.
    pub fn remove_anchor(&mut self, name: &str) -> bool {
        let n = self.anchors.len();
        self.anchors.retain(|a| a.name != name);
        n != self.anchors.len()
    }

    /// Binary encoding (little endian).
    pub fn encode(&self) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(MAGIC);
        put_u32(&mut b, VERSION);
        put_str(&mut b, &self.device_uid);
        put_u32(&mut b, self.cams.len() as u32);
        for &c in &self.cams {
            put_u32(&mut b, c);
        }
        put_u32(&mut b, self.sessions);
        put_u32(&mut b, self.next_id);
        put_u32(&mut b, self.keyframes.len() as u32);
        for k in &self.keyframes {
            put_u32(&mut b, k.id);
            b.extend_from_slice(&k.ts_ns.to_le_bytes());
            put_u32(&mut b, k.session);
            put_pose(&mut b, &k.pose);
            put_u32(&mut b, k.points.len() as u32);
            for m in &k.points {
                for x in m.p {
                    b.extend_from_slice(&(x as f32).to_le_bytes());
                }
                b.extend_from_slice(&m.desc);
            }
        }
        put_u32(&mut b, self.anchors.len() as u32);
        for a in &self.anchors {
            put_str(&mut b, &a.name);
            for x in a.p.iter().chain(a.q.iter()) {
                b.extend_from_slice(&x.to_le_bytes());
            }
        }
        b
    }

    /// Decode [`RoomMap::encode`] output.
    pub fn decode(data: &[u8]) -> Result<RoomMap, String> {
        let mut r = Cursor { d: data, i: 0 };
        if r.take(4)? != MAGIC {
            return Err("not a pimax room map (magic)".into());
        }
        let v = r.u32()?;
        if v != VERSION {
            return Err(format!("room map version {v}, this build reads {VERSION}"));
        }
        let mut m = RoomMap {
            device_uid: r.string()?,
            ..Default::default()
        };
        let nc = r.u32()? as usize;
        for _ in 0..nc.min(8) {
            m.cams.push(r.u32()?);
        }
        m.sessions = r.u32()?;
        m.next_id = r.u32()?;
        let nk = r.u32()? as usize;
        for _ in 0..nk {
            let id = r.u32()?;
            let ts_ns = i64::from_le_bytes(r.take(8)?.try_into().unwrap());
            let session = r.u32()?;
            let pose = r.pose()?;
            let np = r.u32()? as usize;
            let mut points = Vec::with_capacity(np.min(100_000));
            for _ in 0..np {
                let p = [r.f32()? as f64, r.f32()? as f64, r.f32()? as f64];
                let desc: Descriptor = r.take(32)?.try_into().unwrap();
                points.push(MapPoint { p, desc });
            }
            m.keyframes.push(Keyframe {
                id,
                ts_ns,
                session,
                pose,
                points,
            });
        }
        let na = r.u32()? as usize;
        for _ in 0..na {
            let name = r.string()?;
            let mut v = [0.0; 7];
            for x in v.iter_mut() {
                *x = r.f64()?;
            }
            m.anchors.push(Anchor {
                name,
                p: [v[0], v[1], v[2]],
                q: [v[3], v[4], v[5], v[6]],
            });
        }
        Ok(m)
    }

    /// Save to `dir/map.bin` (atomic rename) and a short `dir/room.json` summary.
    pub fn save(&self, dir: &Path) -> io::Result<PathBuf> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join("map.bin");
        let tmp = dir.join("map.bin.tmp");
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&self.encode())?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &path)?;
        let anchors: Vec<String> = self
            .anchors
            .iter()
            .map(|a| {
                format!(
                    "    {{\"name\": {}, \"p\": [{:.4}, {:.4}, {:.4}], \"q\": [{:.6}, {:.6}, {:.6}, {:.6}]}}",
                    pimax_cam::json::quote(&a.name),
                    a.p[0], a.p[1], a.p[2], a.q[0], a.q[1], a.q[2], a.q[3]
                )
            })
            .collect();
        let summary = format!(
            "{{\n  \"device_uid\": {},\n  \"cams\": {:?},\n  \"sessions\": {},\n  \"keyframes\": {},\n  \"points\": {},\n  \"anchors\": [\n{}\n  ]\n}}\n",
            pimax_cam::json::quote(&self.device_uid),
            self.cams,
            self.sessions,
            self.keyframes.len(),
            self.points(),
            anchors.join(",\n")
        );
        std::fs::write(dir.join("room.json"), summary)?;
        Ok(path)
    }

    /// Load `dir/map.bin` (or a file path).
    pub fn load(path: &Path) -> Result<RoomMap, String> {
        let p = if path.is_dir() {
            path.join("map.bin")
        } else {
            path.to_path_buf()
        };
        let mut d = Vec::new();
        std::fs::File::open(&p)
            .and_then(|mut f| f.read_to_end(&mut d))
            .map_err(|e| format!("{}: {e}", p.display()))?;
        RoomMap::decode(&d).map_err(|e| format!("{}: {e}", p.display()))
    }
}

/// Default room directory: `~/.local/share/pimax/rooms/<name>`.
pub fn room_dir(name: &str) -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".local/share/pimax/rooms").join(name)
}

fn put_u32(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&v.to_le_bytes());
}

fn put_str(b: &mut Vec<u8>, s: &str) {
    put_u32(b, s.len() as u32);
    b.extend_from_slice(s.as_bytes());
}

fn put_pose(b: &mut Vec<u8>, p: &Pose) {
    let q = quat_of(&p.r);
    for x in p.t.iter().chain(q.iter()) {
        b.extend_from_slice(&x.to_le_bytes());
    }
}

struct Cursor<'a> {
    d: &'a [u8],
    i: usize,
}

impl Cursor<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        if self.i + n > self.d.len() {
            return Err("truncated room map".into());
        }
        let s = &self.d[self.i..self.i + n];
        self.i += n;
        Ok(s)
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn f32(&mut self) -> Result<f32, String> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn f64(&mut self) -> Result<f64, String> {
        Ok(f64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn string(&mut self) -> Result<String, String> {
        let n = self.u32()? as usize;
        if n > 4096 {
            return Err("bad string length in room map".into());
        }
        Ok(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
    fn pose(&mut self) -> Result<Pose, String> {
        let mut v = [0.0; 7];
        for x in v.iter_mut() {
            *x = self.f64()?;
        }
        Ok(Pose {
            r: mat_of(pimax_cam::slam::qnorm([v[3], v[4], v[5], v[6]])),
            t: [v[0], v[1], v[2]],
        })
    }
}
