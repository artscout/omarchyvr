//! Results: the summary (terminal text, head-locked lines, the notes stored in the calibration file) and the JSON
//! file. Accuracy is always the *validation* error (targets that were not used for the fit).

use crate::layout::{Region, Target};
use crate::procedure::{eye_name, region_errors, Procedure};
use pimax_eye::calib::{stats, Stats};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Seconds since the Unix epoch.
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `YYYYMMDD-HHMMSS` (UTC).
pub fn stamp(unix: u64) -> String {
    pimax_lenstest::gaze::stamp(unix)
}

/// Write with a `.bak-<stamp>` copy of an existing file; returns the backup path.
pub fn write_with_backup(path: &Path, text: &str) -> Result<Option<PathBuf>, String> {
    pimax_lenstest::gaze::write_with_backup(path, text, &stamp(unix_now()))
}

/// `~/.local/share/omarchyvr/eye-calibration/` (or `.` without `$HOME`).
pub fn results_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
        .join(".local/share/omarchyvr/eye-calibration")
}

/// The summary in three forms.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Summary {
    /// Terminal text.
    pub text: String,
    /// Head-locked lines (upper case, short).
    pub hud: Vec<String>,
    /// Comment lines for the calibration file.
    pub notes: Vec<String>,
    /// Combined validation error per target.
    pub accuracy: Stats,
    /// Same targets, raw gaze.
    pub raw: Stats,
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    if v.is_empty() {
        0.0
    } else {
        v[v.len() / 2]
    }
}

/// Build the summary of a procedure.
pub fn summary(p: &Procedure) -> Summary {
    let mut s = Summary::default();
    if let Some(cal) = &p.calibration {
        if p.val_res.is_empty() && p.rec_res.is_empty() {
            s.hud.push("CALIBRATED, NOT VALIDATED".into());
        }
        let _ = cal;
    }
    if p.val_res.is_empty() {
        if !p.rec_res.is_empty() {
            s.hud.push("RECENTRE MEASURED".into());
            s.text = "recentre measured".into();
        } else if p.calibration.is_none() {
            s.hud.push("NO CALIBRATION".into());
        }
        return s;
    }
    let comb: Vec<f64> = p.val_res.iter().filter_map(|v| v.eval.comb_err_deg).collect();
    let raw: Vec<f64> = p.val_res.iter().filter_map(|v| v.raw_eval.comb_err_deg).collect();
    let pooled: Vec<f64> = p.val_res.iter().flat_map(|v| v.eval.sample_errs.iter().copied()).collect();
    let prec: Vec<f64> = p.val_res.iter().filter(|v| v.eval.comb_err_deg.is_some()).map(|v| v.eval.comb_rms_s2s_deg).collect();
    let sd: Vec<f64> = p.val_res.iter().filter(|v| v.eval.comb_err_deg.is_some()).map(|v| v.eval.comb_sd_deg).collect();
    let loss = p.val_res.iter().map(|v| v.eval.loss).sum::<f64>() / p.val_res.len() as f64;
    s.accuracy = stats(&comb);
    s.raw = stats(&raw);
    let ps = stats(&pooled);
    let regions = region_errors(p);
    let reg = |r: Region| {
        regions
            .iter()
            .find(|x| x.0 == r)
            .filter(|x| !x.1.is_empty())
            .map_or("-".to_string(), |x| format!("{:.2}", stats(&x.1).mean))
    };
    let per_eye = |e: usize| {
        let v: Vec<f64> = p.val_res.iter().filter_map(|x| x.eval.eye_err_deg[e]).collect();
        if v.is_empty() {
            "-".to_string()
        } else {
            format!("{:.2}", stats(&v).mean)
        }
    };
    let a = s.accuracy;
    let _ = writeln!(
        s.text,
        "validation ({} of {} targets): accuracy mean {:.2} median {:.2} p95 {:.2} max {:.2} deg (raw gaze on the same targets: mean {:.2} deg)",
        comb.len(),
        p.val_res.len(),
        a.mean,
        a.median,
        a.p95,
        a.max,
        s.raw.mean
    );
    let _ = writeln!(
        s.text,
        "  per sample (accuracy + noise): mean {:.2} median {:.2} deg; precision RMS-S2S {:.2} deg, SD {:.2} deg; data loss {:.0}%",
        ps.mean,
        ps.median,
        median(prec.clone()),
        median(sd.clone()),
        100.0 * loss
    );
    let _ = writeln!(
        s.text,
        "  regions: centre (<=7 deg) {} | mid (7-12) {} | outer (>12) {} deg; per eye: left {} right {} deg",
        reg(Region::Centre),
        reg(Region::Mid),
        reg(Region::Outer),
        per_eye(0),
        per_eye(1)
    );
    for (e, f) in p.fits.iter().enumerate() {
        if let Some(f) = f {
            if let Some(b) = f.selection.best() {
                let _ = writeln!(
                    s.text,
                    "  {} eye: {}{} model on {} targets, calibration leave-one-out {:.2} deg (raw {:.2})",
                    eye_name(e),
                    b.kind.name(),
                    if b.org.is_some() { " + origin" } else { "" },
                    f.points,
                    b.loo.mean,
                    f.selection.raw.mean
                );
            }
        }
    }
    let _ = writeln!(s.text, "  head-motion restarts: {}", p.head_restarts);
    if let Some(pr) = &p.pursuit_res {
        let _ = writeln!(
            s.text,
            "  smooth pursuit: mean {:.2} median {:.2} deg, lag {:.0} ms",
            pr.err.mean,
            pr.err.median,
            pr.lag_s * 1000.0
        );
    }
    s.hud.push(format!(
        "ACCURACY {:.2} DEG (MEDIAN {:.2}, 95% {:.2})",
        a.mean, a.median, a.p95
    ));
    s.hud.push(format!(
        "RAW WAS {:.1} DEG   PRECISION {:.2} DEG",
        s.raw.mean,
        median(prec)
    ));
    s.hud.push(format!(
        "CENTRE {}  MID {}  OUTER {}",
        reg(Region::Centre),
        reg(Region::Mid),
        reg(Region::Outer)
    ));
    s.hud.push(format!(
        "LEFT {}  RIGHT {}  LOST {:.0}%",
        per_eye(0),
        per_eye(1),
        100.0 * loss
    ));
    if let Some(pr) = &p.pursuit_res {
        s.hud.push(format!("PURSUIT {:.2} DEG", pr.err.mean));
    }
    s.notes.push(format!(
        "validated {}: accuracy mean {:.2} median {:.2} p95 {:.2} deg on {} targets (raw {:.2}), precision {:.2} deg",
        stamp(unix_now()),
        a.mean,
        a.median,
        a.p95,
        comb.len(),
        s.raw.mean,
        median(sd)
    ));
    s
}

fn jnum(x: f64) -> String {
    if x.is_finite() {
        format!("{x:.4}")
    } else {
        "null".into()
    }
}

fn jopt(x: Option<f64>) -> String {
    x.map_or("null".into(), jnum)
}

fn jstats(s: &Stats) -> String {
    format!(
        "{{\"n\": {}, \"mean\": {}, \"median\": {}, \"p95\": {}, \"max\": {}}}",
        s.n,
        jnum(s.mean),
        jnum(s.median),
        jnum(s.p95),
        jnum(s.max)
    )
}

fn jtarget(t: &Target) -> String {
    format!("\"az\": {}, \"el\": {}", jnum(t.az), jnum(t.el))
}

fn jstr(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            c if (c as u32) < 0x20 => {
                let _ = write!(o, "\\u{:04x}", c as u32);
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn jeye(w: &Option<crate::collect::EyeWindow>) -> String {
    match w {
        None => "null".into(),
        Some(w) => format!(
            "{{\"total\": {}, \"valid\": {}, \"used\": {}, \"fix_s\": {}, \"raw_u\": {:.6}, \"raw_v\": {:.6}, \"mad_deg\": {}, \"rms_s2s_deg\": {}, \"sd_deg\": {}}}",
            w.total,
            w.valid,
            w.used,
            jnum(w.fix_s),
            w.center.0,
            w.center.1,
            jnum(w.mad_deg),
            jnum(w.rms_s2s_deg),
            jnum(w.sd_deg)
        ),
    }
}

/// The results file.
pub fn json(p: &Procedure) -> String {
    let mut s = String::from("{\n");
    let _ = writeln!(s, "  \"tool\": \"pimax-eyecal\",");
    let _ = writeln!(s, "  \"time_unix\": {},", unix_now());
    let c = p.s.conv;
    let _ = writeln!(
        s,
        "  \"flip_xyz\": [{}, {}, {}],",
        c.flip_x, c.flip_y, c.flip_z
    );
    let ep = p.eye_positions();
    let _ = writeln!(
        s,
        "  \"eye_pos_m\": [[{}, {}, {}], [{}, {}, {}]],",
        jnum(ep[0][0]),
        jnum(ep[0][1]),
        jnum(ep[0][2]),
        jnum(ep[1][0]),
        jnum(ep[1][1]),
        jnum(ep[1][2])
    );
    let sum = summary(p);
    let _ = writeln!(s, "  \"summary\": {},", jstr(sum.text.trim_end()));
    let _ = writeln!(s, "  \"validation_accuracy_deg\": {},", jstats(&sum.accuracy));
    let _ = writeln!(s, "  \"validation_raw_deg\": {},", jstats(&sum.raw));
    let _ = writeln!(s, "  \"head_restarts\": {},", p.head_restarts);
    let fits: Vec<String> = p
        .fits
        .iter()
        .enumerate()
        .map(|(e, f)| match f {
            None => format!("{}: null", jstr(eye_name(e))),
            Some(f) => {
                let cands: Vec<String> = f
                    .selection
                    .candidates
                    .iter()
                    .map(|m| {
                        format!(
                            "{{\"model\": {}, \"origin\": {}, \"origin_u\": [{}], \"origin_v\": [{}], \"ridge_quadratic\": {}, \"loo_deg\": {}, \"train_deg\": {}, \"u\": [{}], \"v\": [{}]}}",
                            jstr(m.kind.name()),
                            m.org.is_some(),
                            m.org.map_or(String::new(), |o| o.0.map(|x| format!("{x:.6}")).join(", ")),
                            m.org.map_or(String::new(), |o| o.1.map(|x| format!("{x:.6}")).join(", ")),
                            jnum(m.ridge.quadratic),
                            jstats(&m.loo),
                            jstats(&m.train),
                            m.map.cx.map(|x| format!("{x:.6}")).join(", "),
                            m.map.cy.map(|x| format!("{x:.6}")).join(", ")
                        )
                    })
                    .collect();
                format!(
                    "{}: {{\"points\": {}, \"raw_deg\": {}, \"chosen\": {}, \"candidates\": [{}]}}",
                    jstr(eye_name(e)),
                    f.points,
                    jstats(&f.selection.raw),
                    f.selection
                        .best()
                        .map_or("null".into(), |b| jstr(b.kind.name())),
                    cands.join(", ")
                )
            }
        })
        .collect();
    let _ = writeln!(s, "  \"fits\": {{{}}},", fits.join(", "));
    let cal: Vec<String> = p
        .cal_res
        .iter()
        .map(|r| {
            format!(
                "    {{{}, \"ok\": {}, \"attempts\": {}, \"head_deg\": {}, \"left\": {}, \"right\": {}}}",
                jtarget(&r.target),
                r.ok,
                r.attempts,
                jnum(r.head_deg),
                jeye(&r.eyes[0]),
                jeye(&r.eyes[1])
            )
        })
        .collect();
    let _ = writeln!(s, "  \"calibration_targets\": [\n{}\n  ],", cal.join(",\n"));
    let val: Vec<String> = p
        .val_res
        .iter()
        .map(|v| {
            format!(
                "    {{{}, \"region\": {}, \"err_deg\": {}, \"raw_err_deg\": {}, \"left_err_deg\": {}, \"right_err_deg\": {}, \"sample_err_deg\": {}, \"rms_s2s_deg\": {}, \"sd_deg\": {}, \"loss\": {}, \"head_deg\": {}, \"left\": {}, \"right\": {}}}",
                jtarget(&v.point.target),
                jstr(Region::of(&v.point.target).name()),
                jopt(v.eval.comb_err_deg),
                jopt(v.raw_eval.comb_err_deg),
                jopt(v.eval.eye_err_deg[0]),
                jopt(v.eval.eye_err_deg[1]),
                jopt(v.eval.comb_sample_err_deg),
                jnum(v.eval.comb_rms_s2s_deg),
                jnum(v.eval.comb_sd_deg),
                jnum(v.eval.loss),
                jnum(v.point.head_deg),
                jeye(&v.point.eyes[0]),
                jeye(&v.point.eyes[1])
            )
        })
        .collect();
    let _ = writeln!(s, "  \"validation_targets\": [\n{}\n  ],", val.join(",\n"));
    let _ = writeln!(
        s,
        "  \"pursuit\": {},",
        p.pursuit_res.as_ref().map_or("null".into(), |r| format!(
            "{{\"lag_s\": {}, \"err_deg\": {}, \"used\": {}}}",
            jnum(r.lag_s),
            jstats(&r.err),
            jnum(r.used)
        ))
    );
    let _ = writeln!(
        s,
        "  \"service_err_deg\": [{}],",
        p.service_err.iter().map(|e| jopt(*e)).collect::<Vec<_>>().join(", ")
    );
    let _ = writeln!(
        s,
        "  \"calibration\": {}",
        p.calibration.as_ref().map_or("null".into(), |c| jstr(&c.to_toml()))
    );
    s.push_str("}\n");
    s
}
