//! `pimax-room`: room memory tools (offline prototype of map persistence + relocalisation).
//!
//! ```text
//! pimax-room build  DATASET TRACKING.csv OUT_DIR [--calib XML] [--every N]
//!                   session 1: keyframes from a EuRoC recording + its tracker trajectory -> OUT_DIR/map.bin
//! pimax-room reloc  MAP_DIR DATASET TRACKING.csv [--calib XML] [--every N] [--save]
//!                   session 2: relocalise a second recording in the saved room; time to the first fix and the
//!                   spread of room_from_slam over the recording (consistency = accuracy proxy)
//! pimax-room info   MAP_DIR
//! pimax-room anchor MAP_DIR list | set NAME X Y Z [YAW_DEG] | rm NAME
//! pimax-room demo                synthetic room, two sessions (self-check of the pipeline)
//! ```
//! TRACKING.csv: `ts,px,py,pz,qw,qx,qy,qz` of the IMU in the tracker world (Basalt: z up; converted to y up),
//! e.g. `tracking.csv` of `run_slam.sh replay`. DATASET: EuRoC root (mav0) or a recording directory.

use pimax_cam::calib::DeviceCalib;
use pimax_cam::geom::{self, Pose, Vec3};
use pimax_cam::slam::{mat_of, qmul, qnorm, qrot, quat_of, qyaw, yaw_of, Quat};
use pimax_room::map::{room_dir, RoomMap};
use pimax_room::orb::Image;
use pimax_room::pngread;
use pimax_room::session::{Alignment, Event, RoomSession, SessionParams};
use pimax_room::stereo::Rig;
use std::path::{Path, PathBuf};

type R<T> = Result<T, String>;

/// Basalt's world is z up; ours y up: rotate -90 deg about X (same as the driver's SLAM_TO_YUP).
const SLAM_TO_YUP: Quat = [
    -std::f64::consts::FRAC_1_SQRT_2,
    0.0,
    0.0,
    std::f64::consts::FRAC_1_SQRT_2,
];

fn opt(v: &mut Vec<String>, name: &str) -> R<Option<String>> {
    match v.iter().position(|a| a == name) {
        Some(i) if i + 1 < v.len() => {
            let x = v.remove(i + 1);
            v.remove(i);
            Ok(Some(x))
        }
        Some(_) => Err(format!("{name} needs a value")),
        None => Ok(None),
    }
}

fn flag(v: &mut Vec<String>, name: &str) -> bool {
    match v.iter().position(|a| a == name) {
        Some(i) => {
            v.remove(i);
            true
        }
        None => false,
    }
}

fn pos(v: &mut Vec<String>, what: &str) -> R<String> {
    if v.is_empty() || v[0].starts_with("--") {
        return Err(format!("missing {what}"));
    }
    Ok(v.remove(0))
}

fn calib(p: Option<String>) -> R<DeviceCalib> {
    let (c, used) =
        DeviceCalib::load_default(p.as_deref().map(Path::new)).map_err(|e| e.to_string())?;
    eprintln!("calibration {} ({})", used.display(), c.uid);
    Ok(c)
}

/// Tracker trajectory, converted to y up.
fn load_tracking(p: &str) -> R<Vec<(i64, Pose)>> {
    let t = std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))?;
    let mut v = Vec::new();
    for l in t.lines() {
        let l = l.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let f: Vec<f64> = l.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        if f.len() < 8 {
            continue;
        }
        let q = qmul(SLAM_TO_YUP, qnorm([f[5], f[6], f[7], f[4]]));
        let p = qrot(SLAM_TO_YUP, [f[1], f[2], f[3]]);
        v.push((f[0] as i64, Pose { r: mat_of(q), t: p }));
    }
    v.sort_by_key(|x| x.0);
    if v.is_empty() {
        return Err(format!("{p}: no poses"));
    }
    Ok(v)
}

/// Tracker pose at a frame time (nearest within 5 ms).
fn pose_at(tr: &[(i64, Pose)], ts: i64) -> Option<Pose> {
    let i = tr.partition_point(|x| x.0 < ts);
    let mut best: Option<(i64, Pose)> = None;
    for j in [i.saturating_sub(1), i] {
        if let Some(&(t, p)) = tr.get(j) {
            let d = (t - ts).abs();
            if d <= 5_000_000 && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, p));
            }
        }
    }
    best.map(|b| b.1)
}

fn dataset_root(d: &str) -> R<PathBuf> {
    let p = Path::new(d);
    if p.join("mav0").is_dir() {
        return Ok(p.to_path_buf());
    }
    let mut v: Vec<PathBuf> = std::fs::read_dir(p)
        .map_err(|e| format!("{d}: {e}"))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join("mav0").is_dir())
        .collect();
    v.sort();
    v.pop()
        .ok_or_else(|| format!("{d}: no EuRoC dataset (mav0)"))
}

fn frames(root: &Path) -> R<Vec<(i64, PathBuf, PathBuf)>> {
    let read = |c: usize| -> R<Vec<(i64, String)>> {
        let p = root.join(format!("mav0/cam{c}/data.csv"));
        let t = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        Ok(t.lines()
            .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
            .filter_map(|l| {
                let (a, b) = l.split_once(',')?;
                Some((a.trim().parse().ok()?, b.trim().to_string()))
            })
            .collect())
    };
    let (c0, c1) = (read(0)?, read(1)?);
    let m1: std::collections::HashMap<i64, String> = c1.into_iter().collect();
    Ok(c0
        .into_iter()
        .filter_map(|(t, f)| {
            let g = m1.get(&t)?;
            Some((
                t,
                root.join("mav0/cam0/data").join(f),
                root.join("mav0/cam1/data").join(g),
            ))
        })
        .collect())
}

/// Run a session over a recording; returns (session, per-frame alignments from single relocalisations).
fn run(
    mut s: RoomSession,
    ds: &str,
    tracking: &str,
    every: usize,
    verbose: bool,
) -> R<(RoomSession, Vec<(i64, Alignment)>)> {
    let root = dataset_root(ds)?;
    let tr = load_tracking(tracking)?;
    let fr = frames(&root)?;
    eprintln!(
        "{}: {} frame sets, {} tracker poses",
        root.display(),
        fr.len(),
        tr.len()
    );
    let mut fixes = Vec::new();
    let world = Alignment {
        yaw: 0.0,
        t: [0.0; 3],
    };
    for (k, (ts, a, b)) in fr.iter().enumerate() {
        if k % every.max(1) != 0 {
            continue;
        }
        let Some(sb) = pose_at(&tr, *ts) else {
            continue;
        };
        let (ia, ib) = (pngread::read(a)?, pngread::read(b)?);
        let views = [
            Image {
                w: ia.w,
                h: ia.h,
                stride: ia.w,
                data: &ia.data,
            },
            Image {
                w: ib.w,
                h: ib.h,
                stride: ib.w,
                data: &ib.data,
            },
        ];
        for e in s.on_frame(*ts, &views, &sb, &world, true) {
            match e {
                Event::Relocalized(al) | Event::Refined(al) => {
                    fixes.push((*ts, al));
                    if verbose {
                        println!(
                            "{:>8.2} s {:?}: yaw {:+.2} deg t ({:+.3} {:+.3} {:+.3})",
                            (*ts - fr[0].0) as f64 / 1e9,
                            if matches!(e, Event::Relocalized(_)) {
                                "relocalised"
                            } else {
                                "refined"
                            },
                            al.yaw.to_degrees(),
                            al.t[0],
                            al.t[1],
                            al.t[2]
                        );
                    }
                }
                Event::Keyframe(_) => {}
            }
        }
    }
    Ok((s, fixes))
}

fn cmd_build(mut v: Vec<String>) -> R<()> {
    let cal = calib(opt(&mut v, "--calib")?)?;
    let every: usize = opt(&mut v, "--every")?
        .map(|x| x.parse().unwrap_or(1))
        .unwrap_or(1);
    let (ds, tr, out) = (
        pos(&mut v, "DATASET")?,
        pos(&mut v, "TRACKING.csv")?,
        pos(&mut v, "OUT_DIR")?,
    );
    let rig = Rig::from_calib(&cal, &[0, 1])?;
    let s = RoomSession::new(
        RoomMap::new(&cal.uid, &[0, 1]),
        rig,
        SessionParams::default(),
    );
    let (s, _) = run(s, &ds, &tr, every, false)?;
    let st = s.stats;
    let map = s.finish();
    let p = map.save(Path::new(&out)).map_err(|e| e.to_string())?;
    println!(
        "{}: {} keyframes, {} points ({} keyframe inserts)",
        p.display(),
        map.keyframes.len(),
        map.points(),
        st.keyframes
    );
    Ok(())
}

fn cmd_reloc(mut v: Vec<String>) -> R<()> {
    let cal = calib(opt(&mut v, "--calib")?)?;
    let every: usize = opt(&mut v, "--every")?
        .map(|x| x.parse().unwrap_or(1))
        .unwrap_or(1);
    let save = flag(&mut v, "--save");
    let (mdir, ds, tr) = (
        pos(&mut v, "MAP_DIR")?,
        pos(&mut v, "DATASET")?,
        pos(&mut v, "TRACKING.csv")?,
    );
    let map = RoomMap::load(Path::new(&mdir))?;
    println!(
        "map: {} keyframes, {} points, {} sessions",
        map.keyframes.len(),
        map.points(),
        map.sessions
    );
    let rig = Rig::from_calib(&cal, &[0, 1])?;
    let p = SessionParams {
        refine_every_ns: 500_000_000,
        ..Default::default()
    };
    let (s, fixes) = run(RoomSession::new(map, rig, p), &ds, &tr, every, true)?;
    let st = s.stats;
    match st.first_fix_ns {
        Some(t) => println!(
            "first fix after {:.2} s; {} attempts, {} found, {} refinements, {} new keyframes",
            t as f64 / 1e9,
            st.attempts,
            st.found,
            st.refined,
            st.keyframes
        ),
        None => println!(
            "room NOT recognised ({} attempts, {} single relocalisations)",
            st.attempts, st.found
        ),
    }
    if fixes.len() >= 2 {
        let n = fixes.len() as f64;
        let mt = fixes
            .iter()
            .fold([0.0; 3], |a, f| geom::add(a, geom::scale(f.1.t, 1.0 / n)));
        let sd_t = (fixes
            .iter()
            .map(|f| geom::norm(geom::sub(f.1.t, mt)).powi(2))
            .sum::<f64>()
            / n)
            .sqrt();
        let y0 = fixes[0].1.yaw;
        let dy: Vec<f64> = fixes
            .iter()
            .map(|f| (f.1.yaw - y0).sin().atan2((f.1.yaw - y0).cos()))
            .collect();
        let my = dy.iter().sum::<f64>() / n;
        let sd_y = (dy.iter().map(|d| (d - my).powi(2)).sum::<f64>() / n).sqrt();
        println!(
            "room_from_slam over {} fixes: translation spread {:.1} mm RMS, yaw spread {:.2} deg RMS (tracker drift + relocalisation noise)",
            fixes.len(),
            sd_t * 1000.0,
            sd_y.to_degrees()
        );
    }
    if save {
        let map = s.finish();
        let p = map.save(Path::new(&mdir)).map_err(|e| e.to_string())?;
        println!("saved {} ({} keyframes)", p.display(), map.keyframes.len());
    }
    Ok(())
}

fn cmd_info(mut v: Vec<String>) -> R<()> {
    let d = pos(&mut v, "MAP_DIR")?;
    let m = RoomMap::load(Path::new(&d))?;
    let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
    for k in &m.keyframes {
        for i in 0..3 {
            lo[i] = lo[i].min(k.pose.t[i]);
            hi[i] = hi[i].max(k.pose.t[i]);
        }
    }
    println!(
        "device {} cams {:?}: {} sessions, {} keyframes, {} points; keyframe positions x {:.2}..{:.2} y {:.2}..{:.2} z {:.2}..{:.2} m",
        m.device_uid, m.cams, m.sessions, m.keyframes.len(), m.points(), lo[0], hi[0], lo[1], hi[1], lo[2], hi[2]
    );
    for a in &m.anchors {
        println!(
            "anchor {:<16} p ({:+.3} {:+.3} {:+.3}) yaw {:+.1} deg",
            a.name,
            a.p[0],
            a.p[1],
            a.p[2],
            yaw_of(a.q).to_degrees()
        );
    }
    Ok(())
}

fn cmd_anchor(mut v: Vec<String>) -> R<()> {
    let d = pos(&mut v, "MAP_DIR")?;
    let what = pos(&mut v, "list|set|rm")?;
    let mut m = RoomMap::load(Path::new(&d))?;
    match what.as_str() {
        "list" => {
            for a in &m.anchors {
                println!(
                    "{} {:.4} {:.4} {:.4} {:.2}",
                    a.name,
                    a.p[0],
                    a.p[1],
                    a.p[2],
                    yaw_of(a.q).to_degrees()
                );
            }
            return Ok(());
        }
        "set" => {
            let name = pos(&mut v, "NAME")?;
            let mut n = [0.0f64; 4];
            for (i, x) in n.iter_mut().enumerate() {
                if i == 3 && v.is_empty() {
                    break;
                }
                *x = pos(&mut v, "X Y Z")?.parse().map_err(|_| "bad number")?;
            }
            m.set_anchor(&name, [n[0], n[1], n[2]], qyaw(n[3].to_radians()));
        }
        "rm" => {
            let name = pos(&mut v, "NAME")?;
            if !m.remove_anchor(&name) {
                return Err(format!("no anchor {name:?}"));
            }
        }
        _ => return Err("anchor: list, set or rm".into()),
    }
    m.save(Path::new(&d)).map_err(|e| e.to_string())?;
    println!("{} anchors", m.anchors.len());
    Ok(())
}

fn cmd_demo() -> R<()> {
    use pimax_room::synth::{RayTable, Room};
    let cal = calib(None)?;
    let rig = Rig::from_calib(&cal, &[0, 1])?;
    let rays: Vec<RayTable> = rig.cams.iter().map(|c| RayTable::new(&c.intr)).collect();
    let render = |room: &Room, rb: &Pose| -> Vec<Vec<u8>> {
        rig.cams
            .iter()
            .zip(rays.iter())
            .map(|(c, r)| room.render_rays(r, &rb.compose(&c.body_from_cam)))
            .collect()
    };
    let walk = |n: usize, r: f64, ph: f64| -> Vec<Pose> {
        (0..n)
            .map(|i| {
                let a = ph + i as f64 / n as f64 * std::f64::consts::TAU;
                let q = qmul(qyaw(a + 0.5), [(-0.1f64).sin(), 0.0, 0.0, (-0.1f64).cos()]);
                Pose {
                    r: mat_of(q),
                    t: [r * a.sin(), 1.55, r * a.cos()],
                }
            })
            .collect()
    };
    let room = Room::default();
    let t0 = std::time::Instant::now();
    let mut s1 = RoomSession::new(
        RoomMap::new(&cal.uid, &[0, 1]),
        rig.clone(),
        SessionParams::default(),
    );
    let id = Alignment {
        yaw: 0.0,
        t: [0.0; 3],
    };
    for (k, rb) in walk(30, 0.9, 0.0).iter().enumerate() {
        let ims = render(&room, rb);
        let v = [
            Image {
                w: 640,
                h: 480,
                stride: 640,
                data: &ims[0],
            },
            Image {
                w: 640,
                h: 480,
                stride: 640,
                data: &ims[1],
            },
        ];
        s1.on_frame(k as i64 * 100_000_000, &v, rb, &id, true);
    }
    let map = s1.finish();
    println!(
        "session 1: {} keyframes, {} points ({:.1} s)",
        map.keyframes.len(),
        map.points(),
        t0.elapsed().as_secs_f64()
    );
    let slam_from_room = Pose {
        r: mat_of(qyaw(-0.8)),
        t: [-1.0, 0.0, 3.0],
    };
    let truth = slam_from_room.inverse();
    let mut s2 = RoomSession::new(map, rig.clone(), SessionParams::default());
    let dim = Room {
        gain: 0.6,
        ..Room::default()
    };
    for (k, rb) in walk(30, 0.6, 0.1).iter().enumerate().take(12) {
        let ims = render(&dim, rb);
        let v = [
            Image {
                w: 640,
                h: 480,
                stride: 640,
                data: &ims[0],
            },
            Image {
                w: 640,
                h: 480,
                stride: 640,
                data: &ims[1],
            },
        ];
        let sb = slam_from_room.compose(rb);
        for e in s2.on_frame(k as i64 * 250_000_000, &v, &sb, &id, true) {
            if let Event::Relocalized(a) | Event::Refined(a) = e {
                let est = a.pose();
                let dt = geom::norm(geom::sub(est.t, truth.t));
                let dy = (yaw_of(quat_of(&est.r)) - yaw_of(quat_of(&truth.r))).to_degrees();
                println!(
                    "session 2 frame {k}: room found, alignment error {:.1} mm, {:+.2} deg",
                    dt * 1000.0,
                    dy
                );
            }
        }
    }
    println!("{:?}", s2.stats);
    Ok(())
}

fn main() {
    let mut v: Vec<String> = std::env::args().skip(1).collect();
    if v.is_empty() || v[0] == "-h" || v[0] == "--help" {
        eprintln!(
            "{}",
            include_str!("main.rs")
                .lines()
                .take_while(|l| l.starts_with("//!"))
                .map(|l| l.trim_start_matches("//!").trim_start_matches(' '))
                .collect::<Vec<_>>()
                .join("\n")
        );
        eprintln!("default room directory: {}", room_dir("<name>").display());
        std::process::exit(2);
    }
    let cmd = v.remove(0);
    let r = match cmd.as_str() {
        "build" => cmd_build(v),
        "reloc" => cmd_reloc(v),
        "info" => cmd_info(v),
        "anchor" => cmd_anchor(v),
        "demo" => cmd_demo(),
        _ => Err(format!(
            "unknown command {cmd:?} (build, reloc, info, anchor, demo)"
        )),
    };
    if let Err(e) = r {
        eprintln!("pimax-room {cmd}: {e}");
        std::process::exit(1);
    }
}

#[allow(dead_code)]
fn unused(_: Vec3) {}
