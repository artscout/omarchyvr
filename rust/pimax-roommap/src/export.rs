//! `export` / `view`: the room map as JSON and as one self-contained HTML5 page (plain WebGL/canvas, no network).
//!
//! Points of all keyframes are merged on a 2 cm grid: position = mean, `n` = how many keyframe observations fell
//! in the cell (more = seen more often = more reliable). Keyframes carry the room-frame position and the
//! orientation quaternion (x y z w); the camera looks along its -Z axis. Room frame: y up, metres.

use crate::floor::StoredFloor;
use pimax_cam::slam::quat_of;
use pimax_room::map::RoomMap;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

/// Merge cell (m).
pub const CELL: f64 = 0.02;

fn num(v: f64) -> String {
    if v.is_finite() {
        let s = format!("{v:.3}");
        let s = s.trim_end_matches('0').trim_end_matches('.');
        if s.is_empty() || s == "-0" { "0".into() } else { s.into() }
    } else {
        "null".into()
    }
}

fn esc(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o += "\\\"",
            '\\' => o += "\\\\",
            '<' => o += "\\u003c",
            c if (c as u32) < 0x20 => {
                let _ = write!(o, "\\u{:04x}", c as u32);
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// Merged points: (x, y, z, observations).
pub fn merged_points(map: &RoomMap) -> Vec<(f64, f64, f64, u32)> {
    let mut cells: HashMap<(i64, i64, i64), ([f64; 3], u32)> = HashMap::new();
    let mut order = Vec::new();
    for k in &map.keyframes {
        for p in &k.points {
            if p.p.iter().any(|x| !x.is_finite()) {
                continue;
            }
            let key = (0..3).map(|i| (p.p[i] / CELL).floor() as i64).collect::<Vec<_>>();
            let key = (key[0], key[1], key[2]);
            let e = cells.entry(key).or_insert_with(|| {
                order.push(key);
                ([0.0; 3], 0)
            });
            for i in 0..3 {
                e.0[i] += p.p[i];
            }
            e.1 += 1;
        }
    }
    order
        .into_iter()
        .map(|k| {
            let (s, n) = cells[&k];
            let n_f = n as f64;
            (s[0] / n_f, s[1] / n_f, s[2] / n_f, n)
        })
        .collect()
}

/// The map directory as JSON text.
pub fn export_json(dir: &Path, name: &str) -> Result<String, String> {
    let map = RoomMap::load(dir)?;
    // no stored floor: detect one from the map for the picture only (nothing is saved)
    let floor = StoredFloor::load(dir).ok().or_else(|| {
        let (pts, heads) = crate::floor::map_points(&map, 3.5);
        let f = crate::floor::detect(&pts, &heads, &crate::floor::FloorParams::default()).ok()?;
        Some(StoredFloor {
            y: f.layer.y,
            source: crate::floor::FloorSource::Points,
            confidence: f.confidence,
            sigma_m: 0.0,
            inliers: 0,
            head_above_m: f.head_above_m,
            when: 0,
        })
    });
    Ok(map_json(&map, name, floor.as_ref(), std::fs::read_to_string(dir.join("roommap.toml")).ok().as_deref()))
}

/// JSON of a loaded map.
pub fn map_json(map: &RoomMap, name: &str, floor: Option<&StoredFloor>, meta_toml: Option<&str>) -> String {
    let pts = merged_points(map);
    let raw = map.points();
    let mut o = String::with_capacity(pts.len() * 28 + map.keyframes.len() * 90 + 512);
    o += "{\n\"format\":\"pimax-roommap-export\",\"version\":1,\"frame\":\"room, y up, metres\",\n";
    let _ = write!(
        o,
        "\"meta\":{{\"name\":{},\"device_uid\":{},\"sessions\":{},\"keyframes\":{},\"raw_observations\":{},\"points\":{},\"merge_cell_m\":{}",
        esc(name),
        esc(&map.device_uid),
        map.sessions,
        map.keyframes.len(),
        raw,
        pts.len(),
        num(CELL)
    );
    if let Some(t) = meta_toml {
        for l in t.lines() {
            if let Some((k, v)) = l.split_once('=') {
                let (k, v) = (k.trim(), v.trim().trim_matches('"'));
                if matches!(k, "source" | "created") {
                    let _ = write!(o, ",{}:{}", esc(k), esc(v));
                }
            }
        }
    }
    o += "},\n";
    match floor {
        Some(f) => {
            let _ = write!(
                o,
                "\"floor\":{{\"y\":{},\"source\":{},\"confidence\":{},\"sigma_m\":{}}},\n",
                num(f.y),
                esc(f.source.name()),
                num(f.confidence),
                num(f.sigma_m)
            );
        }
        None => o += "\"floor\":null,\n",
    }
    o += "\"points\":{\"columns\":[\"x\",\"y\",\"z\",\"n\"],\"data\":[";
    for (i, p) in pts.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        let _ = write!(o, "{},{},{},{}", num(p.0), num(p.1), num(p.2), p.3);
    }
    o += "]},\n\"keyframes\":[";
    for (i, k) in map.keyframes.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        let q = quat_of(&k.pose.r);
        let _ = write!(
            o,
            "\n{{\"id\":{},\"session\":{},\"p\":[{},{},{}],\"q\":[{},{},{},{}],\"points\":{}}}",
            k.id,
            k.session,
            num(k.pose.t[0]),
            num(k.pose.t[1]),
            num(k.pose.t[2]),
            num(q[0]),
            num(q[1]),
            num(q[2]),
            num(q[3]),
            k.points.len()
        );
    }
    o += "\n],\n\"anchors\":[";
    for (i, a) in map.anchors.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        let _ = write!(
            o,
            "{{\"name\":{},\"p\":[{},{},{}],\"q\":[{},{},{},{}]}}",
            esc(&a.name),
            num(a.p[0]),
            num(a.p[1]),
            num(a.p[2]),
            num(a.q[0]),
            num(a.q[1]),
            num(a.q[2]),
            num(a.q[3])
        );
    }
    o += "]\n}\n";
    o
}

/// The self-contained page for a JSON export.
pub fn html_page(json: &str, name: &str) -> String {
    let json = json.replace("</", "<\\/");
    include_str!("viewer.html").replace("__TITLE__", &esc(name).trim_matches('"').replace("\\u003c", "&lt;")).replace("/*__DATA__*/null", &json)
}
