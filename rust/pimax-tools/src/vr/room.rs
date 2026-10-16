//! The room at every start, and SteamVR's room setup (docs/ROOM-SETUP.md).
//!
//! Settings: `~/.config/pimax/room.toml` (`auto = true|false`, `room = "home"`, `steamvr = true|false`; the app's
//! Room switches, `pimax vr room ...`). With `auto` on and the room saved (`~/.local/share/pimax/roommap/<room>/map.bin`):
//!
//! * every start gets Monado's room anchor hook (`PIMAX_ANCHOR=1`, [`auto_anchor`]), also on the SteamVR path (the
//!   driver settings file carries it);
//! * after the start, [`on_start`] runs `pimax-roommap auto --name <room>` detached (nice 15, one shot, at most two
//!   minutes): it recognises the room and applies floor + centre + forward; its plain-words state is
//!   `$XDG_RUNTIME_DIR/pimax-room.status` ([`Status`]); [`on_stop`] ends it by pid if it still runs;
//! * with `steamvr` on, [`write_chaperone`] (before SteamVR starts) writes SteamVR's room setup for driver_monado's
//!   tracking universe ([`UNIVERSE`]) into `~/.local/share/Steam/config/chaperone_info.vrchap`: the play area and
//!   the boundary walls of `room-setup.toml`, standing = identity (the room offset already makes the raw tracking
//!   space the setup frame: origin = play area centre on the floor, -Z = forward), seated = the head height at setup.
//!   Other universes stay as they are; the first write keeps SteamVR's own file as `chaperone_info.vrchap.pimax-backup`.
//!
//! Every step is optional: a failure is a line, VR goes on.
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The tracking universe of Monado's SteamVR driver: `Prop_CurrentUniverseId_Uint64 = 2` for the HMD and both
/// controllers (steamvr_drv/ovrd_driver.cpp, "a constant that's not 0 (invalid) or 1 (reserved for Oculus)").
pub const UNIVERSE: &str = "2";

/// SteamVR's collision wall height.
pub const WALL_HEIGHT_M: f64 = 2.43;

/// Settings file.
pub fn settings_path(home: &Path) -> PathBuf {
    home.join(".config/pimax/room.toml")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// Put the saved room in place at every Play and Work.
    pub auto: bool,
    /// The room (map directory name).
    pub room: String,
    /// Tell SteamVR (play area, boundary, floor) before it starts.
    pub steamvr: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { auto: true, room: "home".into(), steamvr: true }
    }
}

fn value<'a>(t: &'a str, key: &str) -> Option<&'a str> {
    t.lines().find_map(|l| {
        let l = l.split('#').next()?.trim();
        Some(l.strip_prefix(key)?.trim().strip_prefix('=')?.trim().trim_matches('"'))
    })
}

/// A room name the tools accept (letters, digits, `-`, `_`).
pub fn valid_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 40 && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn parse_settings(t: &str) -> Settings {
    let mut s = Settings::default();
    if matches!(value(t, "auto"), Some("false" | "off")) {
        s.auto = false;
    }
    if matches!(value(t, "steamvr"), Some("false" | "off")) {
        s.steamvr = false;
    }
    if let Some(r) = value(t, "room").filter(|r| valid_name(r)) {
        s.room = r.to_string();
    }
    s
}

pub fn load(home: &Path) -> Settings {
    std::fs::read_to_string(settings_path(home)).map(|t| parse_settings(&t)).unwrap_or_default()
}

pub fn save(home: &Path, s: &Settings) -> std::io::Result<()> {
    let p = settings_path(home);
    std::fs::create_dir_all(p.parent().unwrap())?;
    std::fs::write(
        p,
        format!("# Room at every start and SteamVR room setup (Pimax app, docs/ROOM-SETUP.md)\nauto = {}\nroom = \"{}\"\nsteamvr = {}\n", s.auto, s.room, s.steamvr),
    )
}

/// `~/.local/share/pimax/roommap` (or `PIMAX_ROOMMAP_DIR`).
pub fn maps_dir(home: &Path, env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    env("PIMAX_ROOMMAP_DIR").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join(".local/share/pimax/roommap"))
}

/// The saved room's directory, when its map exists.
pub fn room_dir(home: &Path, s: &Settings, env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let d = maps_dir(home, env).join(&s.room);
    d.join("map.bin").is_file().then_some(d)
}

/// Does the next start need Monado's room anchor hook for the room? (auto on and the room saved)
pub fn auto_anchor(env: &dyn Fn(&str) -> Option<String>) -> bool {
    let Some(home) = env("HOME").map(PathBuf::from) else { return false };
    let s = load(&home);
    s.auto && room_dir(&home, &s, env).is_some()
}

/// `pimax-roommap` next to the other tools.
pub fn roommap_bin(home: &Path) -> PathBuf {
    super::config::tools_dir(home).join("pimax-roommap")
}

fn pid_file(session_dir: &Path) -> PathBuf {
    session_dir.join("room-auto.pid")
}

/// After a successful start: `pimax-roommap auto --name <room>` detached (nice 15). A line for the user.
pub fn on_start(home: &Path, session_dir: &Path) -> Option<String> {
    let env = |k: &str| std::env::var(k).ok();
    let s = load(home);
    if !s.auto {
        return None;
    }
    room_dir(home, &s, &env)?;
    let bin = roommap_bin(home);
    if !bin.is_file() {
        return Some(format!("room: {} is not built: the room is not put in place", bin.display()));
    }
    let _ = std::fs::create_dir_all(session_dir);
    let log = std::fs::File::create(session_dir.join("room-auto.log")).ok();
    let mut c = Command::new("nice");
    c.args(["-n", "15"]).arg(&bin).args(["auto", "--name", &s.room]).stdin(Stdio::null());
    match log.and_then(|l| l.try_clone().ok().map(|l2| (l, l2))) {
        Some((a, b)) => {
            c.stdout(a).stderr(b);
        }
        None => {
            c.stdout(Stdio::null()).stderr(Stdio::null());
        }
    }
    match c.spawn() {
        Ok(ch) => {
            let _ = std::fs::write(pid_file(session_dir), format!("{}\n", ch.id()));
            Some(format!("room: looking for the room \"{}\" (floor, centre, forward)", s.room))
        }
        Err(e) => Some(format!("room: could not start {}: {e}", bin.display())),
    }
}

/// At stop: end a still running `pimax-roommap auto` by pid (SIGTERM; it only reads cameras and writes small files).
pub fn on_stop(session_dir: &Path) {
    let pf = pid_file(session_dir);
    let Some(pid) = std::fs::read_to_string(&pf).ok().and_then(|t| t.trim().parse::<i32>().ok()) else { return };
    let _ = std::fs::remove_file(&pf);
    // the pid is `nice`, which exec'd pimax-roommap: check the name before signalling
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
    if pid > 1 && comm.trim() == "pimax-roommap" {
        // SAFETY: kill(2) with a pid we started and whose name we just checked.
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
    }
}

/// `$XDG_RUNTIME_DIR/pimax-room.status` (written by `pimax-roommap auto` and `setup`).
pub fn status_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("pimax-room.status")
}

/// The room's state in this session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    /// `searching`, `ok`, `failed`, `skipped`, `off`.
    pub state: String,
    pub room: String,
    /// Monado (or vrserver) pid it belongs to.
    pub pid: u32,
    /// Plain words.
    pub text: String,
    /// Unix time.
    pub when: u64,
}

pub fn parse_status(t: &str) -> Option<Status> {
    let s = Status {
        state: value(t, "state")?.to_string(),
        room: value(t, "room").unwrap_or_default().to_string(),
        pid: value(t, "pid").and_then(|p| p.parse().ok()).unwrap_or(0),
        text: value(t, "text").unwrap_or_default().to_string(),
        when: value(t, "when").and_then(|p| p.parse().ok()).unwrap_or(0),
    };
    Some(s)
}

// ---------------------------------------------------------------------------------------------------------------
// SteamVR

/// What SteamVR needs from `room-setup.toml` (the full format: pimax-roommap `setup.rs`).
#[derive(Debug, Clone, PartialEq)]
pub struct SteamRoom {
    /// Play area (width x, depth z).
    pub play_area: [f64; 2],
    /// The boundary in the setup frame (centre at the origin, -Z forward).
    pub boundary: Vec<[f64; 2]>,
    /// Head height above the floor at setup (seated pose), NaN if unknown.
    pub head_m: f64,
}

/// Parse `room-setup.toml` into the setup frame (same transform as pimax-roommap's `setup::to_setup`).
pub fn parse_room_setup(t: &str) -> Option<SteamRoom> {
    let nums = |v: &str| -> Vec<f64> { v.split(['[', ']', ',']).filter_map(|x| x.trim().parse::<f64>().ok()).collect() };
    let c = nums(value(t, "centre")?);
    let yaw = value(t, "yaw_deg")?.parse::<f64>().ok()?.to_radians();
    let pa = nums(value(t, "play_area")?);
    let b = nums(value(t, "boundary")?);
    if c.len() != 2 || pa.len() != 2 || b.len() < 6 || !(pa[0] > 0.0 && pa[1] > 0.0) {
        return None;
    }
    let (s, co) = yaw.sin_cos();
    let boundary = b
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| {
            let (dx, dz) = (p[0] - c[0], p[1] - c[1]);
            [co * dx - s * dz, s * dx + co * dz]
        })
        .collect();
    let head_m = value(t, "head_m").and_then(|h| h.parse().ok()).unwrap_or(f64::NAN);
    Some(SteamRoom { play_area: [pa[0], pa[1]], boundary, head_m })
}

/// `~/.local/share/Steam/config/chaperone_info.vrchap`.
pub fn vrchap_path(home: &Path) -> PathBuf {
    home.join(".local/share/Steam/config/chaperone_info.vrchap")
}

fn r6(x: f64) -> serde_json::Value {
    serde_json::json!((x * 1e6).round() / 1e6)
}

/// The universe entry for `room` (SteamVR's own keys: collision_bounds, play_area, seated, standing, time, universeID).
pub fn universe_entry(room: &SteamRoom, universe: &str, time: &str) -> serde_json::Value {
    let n = room.boundary.len();
    let walls: Vec<serde_json::Value> = (0..n)
        .map(|i| {
            let (a, b) = (room.boundary[i], room.boundary[(i + 1) % n]);
            serde_json::json!([
                [r6(a[0]), 0, r6(a[1])],
                [r6(a[0]), WALL_HEIGHT_M, r6(a[1])],
                [r6(b[0]), WALL_HEIGHT_M, r6(b[1])],
                [r6(b[0]), 0, r6(b[1])]
            ])
        })
        .collect();
    let seated_y = if room.head_m.is_finite() && (0.5..=2.3).contains(&room.head_m) { room.head_m } else { 1.2 };
    serde_json::json!({
        "collision_bounds": walls,
        "play_area": [r6(room.play_area[0]), r6(room.play_area[1])],
        "seated": { "translation": [0.0, r6(seated_y), 0.0], "yaw": 0.0 },
        "standing": { "translation": [0.0, 0.0, 0.0], "yaw": 0.0 },
        "time": time,
        "universeID": universe
    })
}

/// The new file text: `existing` with the entry of `universe` replaced (or added), every other universe kept.
pub fn chaperone_text(existing: Option<&str>, room: &SteamRoom, universe: &str, time: &str) -> Result<String, String> {
    let mut v: serde_json::Value = match existing.map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => serde_json::from_str(t).map_err(|e| format!("SteamVR's chaperone_info.vrchap is not JSON ({e}): left alone"))?,
        None => serde_json::json!({ "jsonid": "chaperone_info", "universes": [], "version": 5 }),
    };
    let o = v.as_object_mut().ok_or("chaperone_info.vrchap is not a JSON object: left alone")?;
    o.entry("jsonid").or_insert_with(|| serde_json::json!("chaperone_info"));
    o.entry("version").or_insert_with(|| serde_json::json!(5));
    let us = o.entry("universes").or_insert_with(|| serde_json::json!([]));
    let arr = us.as_array_mut().ok_or("chaperone_info.vrchap: universes is not a list: left alone")?;
    let entry = universe_entry(room, universe, time);
    let same = |u: &serde_json::Value| u.get("universeID").and_then(|x| x.as_str()) == Some(universe);
    match arr.iter_mut().find(|u| same(u)) {
        Some(u) => *u = entry,
        None => arr.push(entry),
    }
    Ok(format!("{}\n", serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?))
}

/// SteamVR's time format: "Thu Oct 15 17:28:11 2026" (local time).
pub fn steam_time(unix: i64) -> String {
    let t: libc::time_t = unix as libc::time_t;
    // SAFETY: localtime_r and strftime with valid buffers.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return String::new();
        }
        let mut buf = [0u8; 64];
        let n = libc::strftime(buf.as_mut_ptr() as *mut libc::c_char, buf.len(), c"%a %b %d %H:%M:%S %Y".as_ptr(), &tm);
        String::from_utf8_lossy(&buf[..n]).into_owned()
    }
}

/// Before SteamVR starts (it reads the file at start and rewrites it itself): our room for driver_monado's universe.
/// None when the switch is off or there is no room setup; Some(line) otherwise. Only while SteamVR is stopped.
pub fn write_chaperone(home: &Path, env: &dyn Fn(&str) -> Option<String>, now_unix: i64) -> Option<String> {
    let s = load(home);
    if !s.steamvr || !s.auto {
        return None;
    }
    let dir = room_dir(home, &s, env)?;
    let text = std::fs::read_to_string(dir.join("room-setup.toml")).ok()?;
    let Some(room) = parse_room_setup(&text) else {
        return Some(format!("room: {}/room-setup.toml is not readable: SteamVR keeps its own room setup", dir.display()));
    };
    let p = vrchap_path(home);
    let old = std::fs::read_to_string(&p).ok();
    let new = match chaperone_text(old.as_deref(), &room, UNIVERSE, &steam_time(now_unix)) {
        Ok(t) => t,
        Err(e) => return Some(format!("room: {e}")),
    };
    // same content apart from the time: leave the file alone
    let strip = |t: &str| t.lines().filter(|l| !l.trim_start().starts_with("\"time\"")).collect::<Vec<_>>().join("\n");
    if old.as_deref().is_some_and(|o| strip(o) == strip(&new)) {
        return None;
    }
    let backup = p.with_file_name("chaperone_info.vrchap.pimax-backup");
    if let Some(o) = &old {
        if !backup.exists() {
            if let Err(e) = std::fs::write(&backup, o) {
                return Some(format!("room: cannot back up {} ({e}): SteamVR keeps its own room setup", p.display()));
            }
        }
    }
    let tmp = p.with_file_name(format!(".chaperone_info.vrchap.pimax-{}", std::process::id()));
    let r = std::fs::create_dir_all(p.parent().unwrap()).and_then(|_| std::fs::write(&tmp, &new)).and_then(|_| std::fs::rename(&tmp, &p));
    Some(match r {
        Ok(()) => format!(
            "room: SteamVR room setup written (play area {:.1} x {:.1} m, {} walls, floor and centre from the room \"{}\")",
            room.play_area[0],
            room.play_area[1],
            room.boundary.len(),
            s.room
        ),
        Err(e) => format!("room: cannot write {} ({e}): SteamVR keeps its own room setup", p.display()),
    })
}

/// `pimax vr room [status|on|off|steamvr on|off|name NAME]` (the app's Room switches).
pub fn cli(what: &[String], home: &Path, runtime_dir: &Path) -> Result<(), String> {
    let mut s = load(home);
    match (what.first().map(String::as_str).unwrap_or("status"), what.get(1).map(String::as_str)) {
        (w @ ("on" | "off"), None) => {
            s.auto = w == "on";
            save(home, &s).map_err(|e| e.to_string())?;
            println!("Room at every start: {w}");
        }
        ("steamvr", Some(w @ ("on" | "off"))) => {
            s.steamvr = w == "on";
            save(home, &s).map_err(|e| e.to_string())?;
            println!("Tell SteamVR the room: {w}");
        }
        ("name", Some(n)) if valid_name(n) => {
            s.room = n.to_string();
            save(home, &s).map_err(|e| e.to_string())?;
            println!("Room: {n}");
        }
        ("name", _) => return Err("name: letters, digits, - and _ (at most 40)".into()),
        ("status", None) => {
            let env = |k: &str| std::env::var(k).ok();
            println!("Room at every start: {} (room \"{}\")", if s.auto { "on" } else { "off" }, s.room);
            println!("Tell SteamVR the room: {}", if s.steamvr { "on" } else { "off" });
            match room_dir(home, &s, &env) {
                Some(d) => println!(
                    "saved: {}{}",
                    d.display(),
                    if d.join("room-setup.toml").is_file() { " (with room setup)" } else { " (map only: run Fast setup)" }
                ),
                None => println!("no saved room \"{}\"", s.room),
            }
            if let Some(st) = std::fs::read_to_string(status_path(runtime_dir)).ok().and_then(|t| parse_status(&t)) {
                println!("this session: {} ({})", st.text, st.state);
            }
        }
        _ => return Err("room: status | on | off | steamvr on|off | name NAME".into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SETUP: &str = "label = \"home\"\nmode = \"full\"\ncentre = [1.0000, 2.0000]\nyaw_deg = 90.0000\nplay_area = [2.000, 1.500]\nhead_m = 1.6500\nwhen = 1\nboundary = [[0.0000, 1.0000], [2.0000, 1.0000], [2.0000, 3.0000], [0.0000, 3.0000]]\n";

    #[test]
    fn settings_round_trip_and_defaults() {
        assert_eq!(parse_settings(""), Settings::default());
        let s = parse_settings("auto = false\nroom = \"office\"\nsteamvr = off\n");
        assert_eq!(s, Settings { auto: false, room: "office".into(), steamvr: false });
        assert_eq!(parse_settings("room = \"a b\"\n").room, "home", "bad names keep the default");
        let home = crate::vr::test_dir("room-settings");
        save(&home, &s).unwrap();
        assert_eq!(load(&home), s);
        // auto anchor: needs auto on and the map
        let h = home.display().to_string();
        let env = move |k: &str| (k == "HOME").then(|| h.clone());
        save(&home, &Settings::default()).unwrap();
        assert!(!auto_anchor(&env), "no map yet");
        std::fs::create_dir_all(home.join(".local/share/pimax/roommap/home")).unwrap();
        std::fs::write(home.join(".local/share/pimax/roommap/home/map.bin"), b"x").unwrap();
        assert!(auto_anchor(&env));
        save(&home, &Settings { auto: false, ..Default::default() }).unwrap();
        assert!(!auto_anchor(&env));
    }

    #[test]
    fn setup_frame_matches_pimax_roommap() {
        // centre (1, 2), forward rotated 90 deg: the room point (2, 2) (1 m along +x from the centre) is 1 m
        // behind (+z) in the setup frame, the same numbers as pimax-roommap's setup::to_setup
        let r = parse_room_setup(SETUP).unwrap();
        assert_eq!(r.play_area, [2.0, 1.5]);
        assert_eq!(r.boundary.len(), 4);
        let near = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9;
        // (0, 1) - (1, 2) = (-1, -1); yaw 90: x = c dx - s dz = 1, z = s dx + c dz = -1
        assert!(near(r.boundary[0], [1.0, -1.0]), "{:?}", r.boundary[0]);
        assert!((r.head_m - 1.65).abs() < 1e-12);
        assert!(parse_room_setup("centre = [0, 0]\n").is_none());
    }

    #[test]
    fn vrchap_keeps_other_universes_and_replaces_ours() {
        let theirs = r#"{
   "jsonid" : "chaperone_info",
   "universes" : [
      { "collision_bounds" : [], "play_area" : [ 1.0, 1.0 ], "standing" : { "translation" : [ 0.02, -1.17, -0.1 ], "yaw" : 2.6 }, "time" : "x", "universeID" : "2" },
      { "collision_bounds" : [], "play_area" : [ 3.0, 3.0 ], "standing" : { "translation" : [ 0, 0, 0 ], "yaw" : 0 }, "time" : "y", "universeID" : "1234567" }
   ],
   "version" : 5
}"#;
        let room = parse_room_setup(SETUP).unwrap();
        let t = chaperone_text(Some(theirs), &room, UNIVERSE, "Thu Oct 15 18:00:00 2026").unwrap();
        let v: serde_json::Value = serde_json::from_str(&t).unwrap();
        let us = v["universes"].as_array().unwrap();
        assert_eq!(us.len(), 2);
        let ours = us.iter().find(|u| u["universeID"] == "2").unwrap();
        let other = us.iter().find(|u| u["universeID"] == "1234567").unwrap();
        assert_eq!(other["play_area"], serde_json::json!([3.0, 3.0]), "the other universe is untouched");
        assert_eq!(other["time"], "y");
        assert_eq!(ours["play_area"], serde_json::json!([2.0, 1.5]));
        assert_eq!(ours["standing"]["translation"], serde_json::json!([0.0, 0.0, 0.0]));
        assert_eq!(ours["seated"]["translation"][1], serde_json::json!(1.65));
        let walls = ours["collision_bounds"].as_array().unwrap();
        assert_eq!(walls.len(), 4);
        // a wall: bottom, top of the first corner, top, bottom of the next; 2.43 m high
        assert_eq!(walls[0][0][1], serde_json::json!(0));
        assert_eq!(walls[0][1][1], serde_json::json!(2.43));
        assert_eq!(walls[0][0][0], walls[0][1][0]);
        assert_eq!(walls[0][2][0], walls[1][0][0]);
        assert_eq!(v["version"], 5);
        assert_eq!(v["jsonid"], "chaperone_info");
        // no file yet: a new one with only ours
        let t = chaperone_text(None, &room, UNIVERSE, "t").unwrap();
        let v: serde_json::Value = serde_json::from_str(&t).unwrap();
        assert_eq!(v["universes"].as_array().unwrap().len(), 1);
        // a broken file is left alone
        assert!(chaperone_text(Some("{ not json"), &room, UNIVERSE, "t").unwrap_err().contains("left alone"));
    }

    #[test]
    fn write_chaperone_backs_up_once_and_only_with_a_setup() {
        let home = crate::vr::test_dir("room-vrchap");
        let h = home.display().to_string();
        let env = move |k: &str| (k == "HOME").then(|| h.clone());
        let dir = home.join(".local/share/pimax/roommap/home");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("map.bin"), b"x").unwrap();
        assert_eq!(write_chaperone(&home, &env, 0), None, "map without a setup: SteamVR keeps its own");
        std::fs::write(dir.join("room-setup.toml"), SETUP).unwrap();
        let p = vrchap_path(&home);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let original = "{\"jsonid\":\"chaperone_info\",\"universes\":[{\"universeID\":\"77\",\"play_area\":[1,1]}],\"version\":5}";
        std::fs::write(&p, original).unwrap();
        let l = write_chaperone(&home, &env, 1_792_000_000).unwrap();
        assert!(l.contains("2.0 x 1.5 m"), "{l}");
        let backup = p.with_file_name("chaperone_info.vrchap.pimax-backup");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(v["universes"].as_array().unwrap().len(), 2);
        // again: nothing changed apart from the time, the file and the backup stay
        assert_eq!(write_chaperone(&home, &env, 1_792_000_100), None);
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);
        // switch off: nothing
        save(&home, &Settings { steamvr: false, ..Default::default() }).unwrap();
        assert_eq!(write_chaperone(&home, &env, 0), None);
    }

    #[test]
    fn status_and_time() {
        let s = parse_status("state = \"ok\"\nroom = \"home\"\npid = 42\ntext = \"Room in place\"\nwhen = 7\n").unwrap();
        assert_eq!(s, Status { state: "ok".into(), room: "home".into(), pid: 42, text: "Room in place".into(), when: 7 });
        assert!(parse_status("room = \"x\"\n").is_none());
        let t = steam_time(1_792_000_000);
        assert_eq!(t.split_whitespace().count(), 5, "{t}");
        assert!(t.ends_with("2026"), "{t}");
    }
}
