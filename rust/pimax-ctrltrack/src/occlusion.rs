//! Masked replay: how wrong is the controller pose while the cameras do not see it?
//!
//! Ground truth comes for free from the session's own `PIMAX_CTRL_IMU_LOG` logs (`ctrl-imu-<hand>.csv`, written by the
//! fuser in Monado): while a controller is really visible, its optical fixes (9+ LEDs, accepted) are the truth. The
//! harness picks windows where the controller stays visible, hides the optical fixes for `D` seconds (0.5 / 1 / 2 s),
//! replays every gyro sample, every other fix and the session's display requests through a fresh [`Fuser`] and
//! compares what it would have shown with the hidden fixes:
//! * position and orientation error at the end of the gap (elapsed ~ D);
//! * the re-acquire: the largest step between two shown poses 11 ms apart once the fixes come back (what snaps).
//!
//! Each variant is a config change (`A/B`), so the same windows score every variant.

use pimax_hands::fuse::{
    calib_or_default, calib_path, qangle, qconj, qmul, qnorm, qrot, yaw_only, Calib, Config, Fuser,
    State, P,
};
use std::path::Path;

type V3 = [f64; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn norm(a: V3) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

#[derive(Clone, Copy, Debug)]
pub enum Ev {
    Imu {
        dev: u32,
        g: [i16; 3],
        a: Option<[i16; 3]>,
    },
    Fix {
        w: P,
        h: P,
        rms: f32,
        leds: u32,
        ok: bool,
    },
    Query {
        h: P,
    },
}

/// One continuous stretch of a log (a log can hold several sessions, or a hole of NUL bytes).
#[derive(Clone, Debug)]
pub struct Seg {
    pub hand: usize,
    pub name: String,
    /// Sorted by time (ns).
    pub evs: Vec<(u64, Ev)>,
}

fn pose(f: &[&str], k: usize) -> Option<P> {
    let v: Vec<f64> = f
        .get(k..k + 7)?
        .iter()
        .map(|x| x.trim().parse::<f64>())
        .collect::<Result<_, _>>()
        .ok()?;
    if v.iter().any(|x| !x.is_finite()) {
        return None;
    }
    Some(P {
        p: [v[0], v[1], v[2]],
        q: qnorm([v[3], v[4], v[5], v[6]]),
    })
}

/// Read a fuser log; unreadable lines are skipped; a new header or time running back by more than 1 s starts a
/// new segment.
pub fn load(path: &Path) -> Result<Vec<Seg>, String> {
    let raw = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let text = String::from_utf8_lossy(&raw);
    let name = path.display().to_string();
    let mut segs: Vec<Seg> = Vec::new();
    let mut hand = if text.contains("hand=right") { 1 } else { 0 };
    let mut cur: Vec<(u64, Ev)> = Vec::new();
    let mut last_imu = 0u64;
    let flush = |cur: &mut Vec<(u64, Ev)>, segs: &mut Vec<Seg>, hand: usize| {
        if cur.len() > 100 {
            let mut evs = std::mem::take(cur);
            evs.sort_by_key(|e| e.0);
            segs.push(Seg {
                hand,
                name: name.clone(),
                evs,
            });
        }
        cur.clear();
    };
    for l in text.lines() {
        if l.contains('\0') {
            continue;
        }
        if let Some(h) = l.strip_prefix("# pimax-ctrlfuse log v1 hand=") {
            flush(&mut cur, &mut segs, hand);
            hand = if h.trim() == "right" { 1 } else { 0 };
            last_imu = 0;
            continue;
        }
        let f: Vec<&str> = l.split(',').collect();
        let t = match f.get(1).and_then(|v| v.trim().parse::<u64>().ok()) {
            Some(t) => t,
            None => continue,
        };
        match f[0] {
            "I" if f.len() == 6 || f.len() == 9 => {
                let n: Vec<i64> = match f[2..].iter().map(|x| x.trim().parse::<i64>()).collect() {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if last_imu > 0 && t + 1_000_000_000 < last_imu {
                    flush(&mut cur, &mut segs, hand);
                }
                last_imu = t;
                let a = (n.len() == 7).then(|| [n[4] as i16, n[5] as i16, n[6] as i16]);
                cur.push((
                    t,
                    Ev::Imu {
                        dev: n[0] as u32,
                        g: [n[1] as i16, n[2] as i16, n[3] as i16],
                        a,
                    },
                ));
            }
            "O" if f.len() >= 19 => {
                let (Some(w), Some(h)) = (pose(&f, 2), pose(&f, 9)) else {
                    continue;
                };
                let rms: f32 = f[16].trim().parse().unwrap_or(9.0);
                let leds: u32 = f[17].trim().parse().unwrap_or(0);
                let ok = f[18].trim() == "ok";
                cur.push((
                    t,
                    Ev::Fix {
                        w,
                        h,
                        rms,
                        leds,
                        ok,
                    },
                ));
            }
            "P" if f.len() >= 17 => {
                if let Some(h) = pose(&f, 10) {
                    cur.push((t, Ev::Query { h }));
                }
            }
            _ => {}
        }
    }
    flush(&mut cur, &mut segs, hand);
    Ok(segs)
}

/// A fix good enough to be ground truth.
fn truth(e: &Ev) -> Option<(P, P)> {
    match *e {
        Ev::Fix {
            w,
            h,
            rms,
            leds,
            ok,
        } if ok && leds >= 9 && rms < 1.5 => Some((w, h)),
        _ => None,
    }
}

/// Mask windows of `d` s in a segment: (index of the last fix shown before the gap, t0).
pub fn windows(seg: &Seg, d: f64) -> Vec<u64> {
    let fixes: Vec<(u64, bool)> = seg
        .evs
        .iter()
        .filter_map(|(t, e)| match e {
            Ev::Fix { .. } => Some((*t, truth(e).is_some())),
            _ => None,
        })
        .collect();
    let dn = (d * 1e9) as u64;
    let mut out = Vec::new();
    let mut next_ok = 0u64;
    for i in 0..fixes.len() {
        let (t0, good) = fixes[i];
        if !good || t0 < next_ok {
            continue;
        }
        // steady before: 6 good fixes in the last 0.6 s
        let before = fixes[..=i]
            .iter()
            .rev()
            .take_while(|f| f.0 + 600_000_000 >= t0)
            .filter(|f| f.1)
            .count();
        if before < 6 {
            continue;
        }
        // visible all along: no gap over 200 ms until 250 ms after the mask, and truth at its end
        let mut prev = t0;
        let mut cont = true;
        let mut end_truth = false;
        for &(t, g) in &fixes[i + 1..] {
            if t > t0 + dn + 250_000_000 {
                break;
            }
            if t - prev > 200_000_000 {
                cont = false;
                break;
            }
            prev = t;
            if g && t <= t0 + dn && t + 120_000_000 > t0 + dn {
                end_truth = true;
            }
        }
        if cont && end_truth && prev + 200_000_000 >= t0 + dn + 250_000_000 {
            out.push(t0);
            // windows may overlap (each starts at its own fix); 0.3 s apart keeps them from being near copies
            next_ok = t0 + 300_000_000;
        }
    }
    out
}

#[derive(Clone, Debug, Default)]
pub struct WinResult {
    /// Error at the last hidden truth fix (m, deg) and its elapsed time (s).
    pub end_pos: f64,
    pub end_rot: f64,
    pub elapsed: f64,
    /// Largest step between shown poses 11 ms apart in the 400 ms after the gap (m, deg).
    pub step_pos: f64,
    pub step_rot: f64,
    /// The fuser showed nothing (rest pose) at the end of the gap.
    pub none: bool,
    /// The controller really moved during the gap (> 5 cm or > 15 deg between the last fix before it and the
    /// truth at its end).
    pub moving: bool,
}

fn head_at(seg: &Seg, t: u64) -> Option<P> {
    let i = seg.evs.partition_point(|e| e.0 <= t);
    let mut best: Option<(u64, P)> = None;
    for k in [i.wrapping_sub(1), i] {
        let mut j = k;
        // nearest head sample around the index
        for _ in 0..64 {
            if j >= seg.evs.len() {
                break;
            }
            let (tj, e) = seg.evs[j];
            let h = match e {
                Ev::Fix { h, .. } | Ev::Query { h } => Some(h),
                _ => None,
            };
            if let Some(h) = h {
                let d = tj.abs_diff(t);
                if best.is_none_or(|b| d < b.0) {
                    best = Some((d, h));
                }
                break;
            }
            if k == i {
                j += 1;
            } else {
                j = j.wrapping_sub(1);
            }
        }
    }
    best.filter(|b| b.0 < 60_000_000).map(|b| b.1)
}

/// Replay one window through a fresh fuser.
pub fn run_window(
    seg: &Seg,
    t0: u64,
    d: f64,
    cfg: Config,
    calib: Option<Calib>,
) -> Option<WinResult> {
    let (r, shown) = replay_window(seg, t0, d, cfg, calib, true);
    let (_, oracle) = replay_window(seg, t0, d, cfg, calib, false);
    let mut r = r?;
    // re-acquire: the shown motion minus what the same fuser shows without the gap (the controller's own motion)
    for k in 1..shown.len().min(oracle.len()) {
        let (Some(a0), Some(a1), Some(b0), Some(b1)) =
            (shown[k - 1], shown[k], oracle[k - 1], oracle[k])
        else {
            continue;
        };
        let ds = sub(sub(a1.p, a0.p), sub(b1.p, b0.p));
        r.step_pos = r.step_pos.max(norm(ds));
        let rs = qangle(a0.q, a1.q) - qangle(b0.q, b1.q);
        r.step_rot = r.step_rot.max(rs.abs().to_degrees());
    }
    Some(r)
}

/// One replay of a window; `mask` hides the fixes in the gap. Returns the result and the poses shown every 11 ms
/// for 400 ms from the end of the gap.
fn replay_window(
    seg: &Seg,
    t0: u64,
    d: f64,
    cfg: Config,
    calib: Option<Calib>,
    mask: bool,
) -> (Option<WinResult>, Vec<Option<P>>) {
    let dn = (d * 1e9) as u64;
    let start = t0.saturating_sub(4_000_000_000);
    let t_end = t0 + dn;
    let mut fu = Fuser::new(seg.hand, cfg, calib);
    let mut res = WinResult::default();
    let i0 = seg.evs.partition_point(|e| e.0 < start);
    let mut steps_t = t_end;
    let mut shown: Vec<Option<P>> = Vec::new();
    let w0 = seg
        .evs
        .iter()
        .find(|e| e.0 == t0)
        .and_then(|e| truth(&e.1))
        .map(|x| x.0);
    for &(t, e) in &seg.evs[i0..] {
        if t > t_end + 400_000_000 {
            break;
        }
        // display requests every 11 ms from the end of the gap (re-acquire steps)
        while steps_t <= t && steps_t <= t_end + 400_000_000 {
            shown.push(head_at(seg, steps_t).and_then(|h| {
                let (s, p) = fu.get(steps_t, &h);
                (s != State::None).then_some(p)
            }));
            steps_t += 11_000_000;
        }
        match e {
            Ev::Imu { dev, g, a } => fu.sample_acc(t, dev, g, a),
            Ev::Fix {
                w, h, rms, leds, ..
            } => {
                if t > t0 && t <= t_end {
                    // hidden: score what would be shown now (the last one in the gap is the result)
                    if let Some((w, h)) = truth(&e) {
                        let (s, p) = fu.get(t, &h);
                        res.none = s == State::None;
                        res.end_pos = norm(sub(p.p, w.p));
                        res.end_rot = qangle(p.q, w.q).to_degrees();
                        res.elapsed = (t - t0) as f64 * 1e-9;
                        res.moving = w0.is_some_and(|a| {
                            norm(sub(a.p, w.p)) > 0.05 || qangle(a.q, w.q).to_degrees() > 15.0
                        });
                    }
                    if mask {
                        continue;
                    }
                }
                fu.optical_with(t, w, &h, rms, leds, None);
            }
            Ev::Query { h } => {
                if t <= t_end {
                    let _ = fu.get(t, &h);
                }
            }
        }
    }
    ((res.elapsed > 0.0).then_some(res), shown)
}

pub struct Variant {
    pub name: String,
    pub cfg: Config,
    pub calib: Option<Calib>,
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * p).round() as usize]
}

/// Score every variant on every window; returns the report text.
pub fn report(segs: &[Seg], durations: &[f64], variants: &[Variant]) -> String {
    let mut s = String::new();
    for &d in durations {
        let wins: Vec<(usize, u64)> = segs
            .iter()
            .enumerate()
            .flat_map(|(k, sg)| windows(sg, d).into_iter().map(move |t| (k, t)))
            .collect();
        let per_hand = [0, 1].map(|h| wins.iter().filter(|w| segs[w.0].hand == h).count());
        s += &format!(
            "\ngap {d:.1} s: {} windows (left {}, right {})\n  {:<40} {:>16} {:>18} {:>16} {:>20}\n",
            wins.len(),
            per_hand[0],
            per_hand[1],
            "variant",
            "pos err p50/p90",
            "moving: pos p50/p90",
            "rot err p50/p90",
            "re-acquire snap p90"
        );
        for v in variants {
            let mut pe = Vec::new();
            let mut re = Vec::new();
            let mut sp = Vec::new();
            let mut sr = Vec::new();
            let mut none = 0;
            let mut pm = Vec::new();
            for &(k, t0) in &wins {
                if let Some(r) = run_window(&segs[k], t0, d, v.cfg, v.calib_for(segs[k].hand)) {
                    if r.none {
                        none += 1;
                    }
                    if r.moving {
                        pm.push(r.end_pos * 1000.0);
                    }
                    pe.push(r.end_pos * 1000.0);
                    re.push(r.end_rot);
                    sp.push(r.step_pos * 1000.0);
                    sr.push(r.step_rot);
                }
            }
            let nm = pm.len();
            s += &format!(
                "  {:<40} {:>6.0}/{:<5.0} mm {:>5.0}/{:<5.0} mm ({:>3}) {:>6.1}/{:<5.1} deg {:>6.0} mm/{:<4.1} deg{}\n",
                v.name,
                pct(&mut pe, 0.5),
                pct(&mut pe, 0.9),
                pct(&mut pm, 0.5),
                pct(&mut pm, 0.9),
                nm,
                pct(&mut re, 0.5),
                pct(&mut re, 0.9),
                pct(&mut sp, 0.9),
                pct(&mut sr, 0.9),
                if none > 0 { format!("  ({none} rest pose)") } else { String::new() }
            );
        }
    }
    s
}

impl Variant {
    fn calib_for(&self, _hand: usize) -> Option<Calib> {
        self.calib
    }
}

/// The variants compared by `pimax-ctrltrack occlusion`.
pub fn variants(hand: usize) -> Vec<Variant> {
    let calib = calib_or_default(&calib_path(), hand);
    let base = Config::default();
    vec![
        Variant {
            name: "no gyro (head-yaw hold, pre 10-14)".into(),
            cfg: Config {
                use_imu: Some(false),
                forearm: None,
                blend_mps: 0.0,
                blend_dps: 0.0,
                follow_yaw: 1.0,
                ..base
            },
            calib: None,
        },
        Variant {
            name: "before: gyro + head-yaw hold".into(),
            cfg: Config {
                forearm: None,
                blend_mps: 0.0,
                blend_dps: 0.0,
                follow_yaw: 1.0,
                ..base
            },
            calib,
        },
        Variant {
            name: "+ forearm (wrist/elbow) model".into(),
            cfg: Config {
                blend_mps: 0.0,
                blend_dps: 0.0,
                follow_yaw: 1.0,
                ..base
            },
            calib,
        },
        Variant {
            name: "+ re-acquire <= 0.5 m/s, 120 deg/s".into(),
            cfg: Config {
                follow_yaw: 1.0,
                ..base
            },
            calib,
        },
        Variant {
            name: "now: + hold follows 50 % of head yaw".into(),
            cfg: base,
            calib,
        },
        Variant {
            name: "  (test) follows 0 % of head yaw".into(),
            cfg: Config {
                follow_yaw: 0.0,
                ..base
            },
            calib,
        },
        Variant {
            name: "  (rejected) + accel carry 150 ms".into(),
            cfg: Config {
                dr_tau_s: 0.15,
                ..base
            },
            calib,
        },
    ]
}

/// Gravity check of the accelerometer axes: at truth fixes with the nearest accelerometer sample, the angle between
/// the specific force rotated into the world (optical orientation) and +y, for several axis hypotheses.
pub fn accel_check(segs: &[Seg], hand: usize, c: &Calib) -> String {
    let m = c.axis_rotation();
    let mut out = String::new();
    let hyps: [(&str, [[f64; 3]; 3]); 2] = [
        ("gyro axes", m),
        (
            "raw axes",
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        ),
    ];
    for (name, r) in hyps {
        let mut ang = Vec::new();
        let mut mag = Vec::new();
        for sg in segs.iter().filter(|s| s.hand == hand) {
            let mut last_a: Option<(u64, [f64; 3])> = None;
            for &(t, e) in &sg.evs {
                match e {
                    Ev::Imu { a: Some(a), .. } => {
                        last_a = Some((t, [a[0] as f64, a[1] as f64, a[2] as f64]))
                    }
                    _ => {
                        if let (Some((w, _)), Some((ta, a))) = (truth(&e), last_a) {
                            if t.abs_diff(ta) > 5_000_000 {
                                continue;
                            }
                            let v = [
                                r[0][0] * a[0] + r[0][1] * a[1] + r[0][2] * a[2],
                                r[1][0] * a[0] + r[1][1] * a[1] + r[1][2] * a[2],
                                r[2][0] * a[0] + r[2][1] * a[1] + r[2][2] * a[2],
                            ];
                            let wv = qrot(w.q, v);
                            let n = norm(wv);
                            mag.push(n / 1024.0);
                            ang.push((wv[1] / n).clamp(-1.0, 1.0).acos().to_degrees());
                        }
                    }
                }
            }
        }
        out += &format!(
            "{} {name}: angle to +y p10/p50/p90 {:.1}/{:.1}/{:.1} deg, |a| p50 {:.3} g ({} samples)\n",
            ["left", "right"][hand],
            pct(&mut ang.clone(), 0.1),
            pct(&mut ang.clone(), 0.5),
            pct(&mut ang, 0.9),
            pct(&mut mag, 0.5),
            mag.len()
        );
    }
    out
}

/// Least-squares forearm vector (aim frame, controller minus elbow) from tracked stretches: the elbow
/// `p - R f` should move least in the head-yaw frame over 0.3-1 s. Returns (f, pairs, residual before/after m).
pub fn fit_forearm(segs: &[Seg], hand: usize) -> Option<(V3, usize, f64, f64)> {
    let mut rows: Vec<([[f64; 3]; 3], V3)> = Vec::new();
    for sg in segs.iter().filter(|s| s.hand == hand) {
        let tr: Vec<(u64, P)> = sg
            .evs
            .iter()
            .filter_map(|(t, e)| {
                truth(e).map(|(w, h)| {
                    let yq = yaw_only(h.q);
                    let yi = qconj(yq);
                    (
                        *t,
                        P {
                            q: qnorm(qmul(yi, w.q)),
                            p: qrot(yi, sub(w.p, h.p)),
                        },
                    )
                })
            })
            .collect();
        for a in 0..tr.len() {
            for b in a + 1..tr.len() {
                let dt = (tr[b].0 - tr[a].0) as f64 * 1e-9;
                if dt < 0.3 {
                    continue;
                }
                if dt > 1.0 {
                    break;
                }
                // (R_a - R_b) f = p_a - p_b
                let col = |q: [f64; 4], k: usize| {
                    let mut e = [0.0; 3];
                    e[k] = 1.0;
                    qrot(q, e)
                };
                let mut m = [[0.0; 3]; 3];
                for k in 0..3 {
                    let ca = col(tr[a].1.q, k);
                    let cb = col(tr[b].1.q, k);
                    for r in 0..3 {
                        m[r][k] = ca[r] - cb[r];
                    }
                }
                rows.push((m, sub(tr[a].1.p, tr[b].1.p)));
                break; // one partner per fix
            }
        }
    }
    if rows.len() < 30 {
        return None;
    }
    let solve = |keep: &[bool]| -> Option<V3> {
        let mut ata = [[0.0; 3]; 3];
        let mut atb = [0.0; 3];
        for (i, (m, y)) in rows.iter().enumerate() {
            if !keep[i] {
                continue;
            }
            for r in 0..3 {
                for c in 0..3 {
                    for k in 0..3 {
                        ata[c][k] += m[r][c] * m[r][k];
                    }
                    atb[c] += m[r][c] * y[r];
                }
            }
        }
        let det = ata[0][0] * (ata[1][1] * ata[2][2] - ata[1][2] * ata[2][1])
            - ata[0][1] * (ata[1][0] * ata[2][2] - ata[1][2] * ata[2][0])
            + ata[0][2] * (ata[1][0] * ata[2][1] - ata[1][1] * ata[2][0]);
        if det.abs() < 1e-12 {
            return None;
        }
        // Cramer's rule
        let rep = |c: usize| {
            let mut a = ata;
            for r in 0..3 {
                a[r][c] = atb[r];
            }
            a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
                - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
                + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0])
        };
        Some([rep(0) / det, rep(1) / det, rep(2) / det])
    };
    let res = |f: V3| -> Vec<f64> {
        rows.iter()
            .map(|(m, y)| {
                let p = [
                    m[0][0] * f[0] + m[0][1] * f[1] + m[0][2] * f[2],
                    m[1][0] * f[0] + m[1][1] * f[1] + m[1][2] * f[2],
                    m[2][0] * f[0] + m[2][1] * f[1] + m[2][2] * f[2],
                ];
                norm(sub(*y, p))
            })
            .collect()
    };
    let mut keep = vec![true; rows.len()];
    let mut f = solve(&keep)?;
    for _ in 0..4 {
        let r = res(f);
        let mut s = r.clone();
        let thr = pct(&mut s, 0.85);
        for i in 0..rows.len() {
            keep[i] = r[i] <= thr;
        }
        f = solve(&keep)?;
    }
    let mut r0 = res([0.0; 3]);
    let mut r1 = res(f);
    Some((f, rows.len(), pct(&mut r0, 0.5), pct(&mut r1, 0.5)))
}

pub fn run(args: &[String]) -> Result<(), String> {
    let mut durs = vec![0.5, 1.0, 2.0];
    let mut files = Vec::new();
    let mut fit = false;
    let mut accel = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--gaps" => {
                i += 1;
                durs = args
                    .get(i)
                    .ok_or("--gaps needs a list")?
                    .split(',')
                    .filter_map(|v| v.parse().ok())
                    .collect();
            }
            "--fit-forearm" => fit = true,
            "--accel-check" => accel = true,
            f => files.push(f.to_string()),
        }
        i += 1;
    }
    if files.is_empty() {
        return Err(
            "usage: pimax-ctrltrack occlusion LOG.csv... [--gaps 0.5,1,2] [--fit-forearm]".into(),
        );
    }
    let mut segs = Vec::new();
    for f in &files {
        let s = load(Path::new(f))?;
        eprintln!(
            "{f}: {} segment(s), {} events",
            s.len(),
            s.iter().map(|s| s.evs.len()).sum::<usize>()
        );
        segs.extend(s);
    }
    if accel {
        for h in 0..2 {
            let Some(c) = calib_or_default(&calib_path(), h) else {
                continue;
            };
            print!("{}", accel_check(&segs, h, &c));
        }
        return Ok(());
    }
    if fit {
        for h in 0..2 {
            match fit_forearm(&segs, h) {
                Some((f, n, r0, r1)) => println!(
                    "{} forearm (aim frame, controller - elbow): ({:+.3} {:+.3} {:+.3}) m, |f| {:.3} m, {n} pairs; elbow motion p50 {:.0} -> {:.0} mm",
                    ["left", "right"][h],
                    f[0],
                    f[1],
                    f[2],
                    norm(f),
                    r0 * 1000.0,
                    r1 * 1000.0
                ),
                None => println!("{}: not enough tracked data", ["left", "right"][h]),
            }
        }
        return Ok(());
    }
    for h in 0..2 {
        let hs: Vec<Seg> = segs.iter().filter(|s| s.hand == h).cloned().collect();
        if hs.is_empty() {
            continue;
        }
        println!("== {} controller ==", ["left", "right"][h]);
        print!("{}", report(&hs, &durs, &variants(h)));
    }
    Ok(())
}
