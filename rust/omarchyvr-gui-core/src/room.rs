//! The room: map it once (`omarchyvr-roommap build`), recognise it at the next start (`omarchyvr-roommap relocalize --apply`),
//! or use the printed AprilTag (`omarchyvr-anchor calibrate`, `apply`, `clear`). Both write Monado's world offset
//! (`$XDG_RUNTIME_DIR/pimax-anchor.offset`, read by the `desk` Monado build started with `--anchor`, i.e.
//! `PIMAX_ANCHOR=1`), so only one of them may write it per Monado session: the app records who wrote it next to the
//! offset (`omarchyvr-anchor.offset.owner`, `roommap <pid>` / `anchor <pid>`) and refuses the other one until the offset
//! is cleared. See analysis/room-map.md and analysis/anchor/README.md.
//!
//! The floor (analysis/floor.md): each map carries `floor.toml` (the floor height in the room frame, from the
//! mapping walk's stereo points, a controller laid on the floor, or typed in) and `preview.txt` (top-down points
//! for the Room page). Recognising the room then puts that floor on Monado's STAGE floor. The floor grid of
//! `omarchyvr-passthrough` follows `$XDG_RUNTIME_DIR/omarchyvr-floor-grid` (`1` / `0`).
use crate::cmd::Cmd;
use crate::facts::Live;
use crate::machine::Machine;
use crate::paths::Paths;
use std::path::{Path, PathBuf};

/// Who writes the offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Writer {
    /// The room map (`omarchyvr-roommap relocalize --apply`).
    RoomMap,
    /// The AprilTag (`omarchyvr-anchor apply`).
    Tag,
}

impl Writer {
    /// Name in the owner file.
    pub fn name(self) -> &'static str {
        match self {
            Writer::RoomMap => "roommap",
            Writer::Tag => "anchor",
        }
    }

    /// For people.
    pub fn title(self) -> &'static str {
        match self {
            Writer::RoomMap => "the room map",
            Writer::Tag => "the AprilTag",
        }
    }

    fn parse(s: &str) -> Option<Writer> {
        match s {
            "roommap" => Some(Writer::RoomMap),
            "anchor" => Some(Writer::Tag),
            _ => None,
        }
    }
}

/// Files and programs of the room tools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomPaths {
    /// Saved maps (`~/.local/share/omarchyvr/roommap/<name>/`).
    pub maps: PathBuf,
    /// The AprilTag calibration (`~/.config/omarchyvr/anchor.toml`).
    pub tag_conf: PathBuf,
    /// Monado's offset file.
    pub offset: PathBuf,
    /// Who wrote it (this app).
    pub owner: PathBuf,
    /// `omarchyvr-roommap`.
    pub roommap_bin: PathBuf,
    /// `omarchyvr-anchor`.
    pub anchor_bin: PathBuf,
    /// The printable tag (A4, 150 mm).
    pub tag_pdf: PathBuf,
    /// The floor grid switch of `omarchyvr-passthrough`.
    pub grid: PathBuf,
    /// Monado's head ring (`PIMAX_ANCHOR=1`): present = the hook runs.
    pub head_ring: PathBuf,
    /// The room's state in this session (`omarchyvr-roommap auto` / `setup`).
    pub status: PathBuf,
    /// The Room switches (`~/.config/omarchyvr/room.toml`, shared with `omarchyvr vr room`).
    pub prefs: PathBuf,
}

impl RoomPaths {
    /// From the app's paths.
    pub fn new(p: &Paths) -> RoomPaths {
        RoomPaths {
            maps: p.home.join(".local/share/omarchyvr/roommap"),
            tag_conf: p.config.join("anchor.toml"),
            offset: p.runtime_dir.join("pimax-anchor.offset"),
            owner: p.runtime_dir.join("omarchyvr-anchor.offset.owner"),
            roommap_bin: p.bin("omarchyvr-roommap"),
            anchor_bin: p.bin("omarchyvr-anchor"),
            tag_pdf: p.repo.join("analysis/anchor/tag36h11-id0-150mm.pdf"),
            grid: p.runtime_dir.join("omarchyvr-floor-grid"),
            head_ring: p.runtime_dir.join("pimax-anchor.head"),
            status: omarchyvr_tools::vr::room::status_path(&p.runtime_dir),
            prefs: omarchyvr_tools::vr::room::settings_path(&p.home),
        }
    }
}

/// What is on disk.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoomState {
    /// Saved maps.
    pub maps: Vec<String>,
    /// The AprilTag was calibrated.
    pub tag_calibrated: bool,
    /// The Monado pid of the offset file, if there is one.
    pub offset_pid: Option<u32>,
    /// Who wrote it, as recorded by the app (writer, Monado pid).
    pub owner: Option<(Writer, u32)>,
    /// The programs are built.
    pub tools: (bool, bool),
    /// The stored floor of each map that has one.
    pub floors: Vec<(String, FloorInfo)>,
    /// Top-down previews of the maps (`preview.txt`).
    pub previews: Vec<(String, Preview)>,
    /// The floor grid switch says on.
    pub grid_on: bool,
    /// The room fix of each map (`last-reloc.toml`): Monado pid.
    pub fixes: Vec<(String, u32)>,
    /// Where the head is now in a map's frame (map, x z y cm): the newest pose of Monado's head ring through the
    /// map's room fix of this Monado session.
    pub head: Option<(String, [i32; 3])>,
    /// The room setup of each map that has one (`room-setup.toml`).
    pub setups: Vec<(String, SetupInfo)>,
    /// The Room switches (room at every start, tell SteamVR, which room).
    pub prefs: omarchyvr_tools::vr::room::Settings,
    /// The room's state in the current (or last) session.
    pub status: Option<omarchyvr_tools::vr::room::Status>,
}

/// A map's room setup (`room-setup.toml`), in integers so the state stays comparable.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SetupInfo {
    pub label: String,
    /// `fast` or `full`.
    pub mode: String,
    /// Play area (cm): width, depth.
    pub play_cm: [i32; 2],
    /// Centre in the map (cm, x z).
    pub centre_cm: [i32; 2],
    /// Forward heading in the map (0.1 deg).
    pub yaw_decideg: i32,
    /// Boundary in the map (cm, x z).
    pub boundary_cm: Vec<[i32; 2]>,
    /// Unix time.
    pub when: u64,
}

impl SetupInfo {
    /// Area inside the boundary (m^2).
    pub fn area_m2(&self) -> f64 {
        let b = &self.boundary_cm;
        let n = b.len();
        if n < 3 {
            return 0.0;
        }
        let a: f64 = (0..n).map(|i| b[i][0] as f64 * b[(i + 1) % n][1] as f64 - b[(i + 1) % n][0] as f64 * b[i][1] as f64).sum();
        (a / 2.0).abs() / 1e4
    }

    /// For people: "play area 3.0 x 2.0 m (7.5 m^2), full setup".
    pub fn describe(&self) -> String {
        format!(
            "play area {:.1} x {:.1} m ({:.1} m\u{b2} inside the boundary), {} setup",
            self.play_cm[0] as f64 / 100.0,
            self.play_cm[1] as f64 / 100.0,
            self.area_m2(),
            self.mode
        )
    }
}

/// Parse `room-setup.toml` (omarchyvr-roommap `setup.rs`).
pub fn parse_setup(text: &str) -> Option<SetupInfo> {
    let mut f = SetupInfo::default();
    let nums = |v: &str| -> Vec<f64> { v.split(['[', ']', ',']).filter_map(|x| x.trim().parse::<f64>().ok()).collect() };
    let (mut c, mut yaw, mut pa) = (None, None, None);
    for l in text.lines() {
        let l = l.trim();
        if l.starts_with('#') {
            continue;
        }
        let Some((k, v)) = l.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        match k {
            "label" => f.label = v.trim_matches('"').to_string(),
            "mode" => f.mode = v.trim_matches('"').to_string(),
            "centre" => c = Some(nums(v)).filter(|n| n.len() == 2),
            "yaw_deg" => yaw = v.parse::<f64>().ok(),
            "play_area" => pa = Some(nums(v)).filter(|n| n.len() == 2),
            "when" => f.when = v.parse().unwrap_or(0),
            "boundary" => f.boundary_cm = nums(v).as_chunks::<2>().0.iter().map(|p| [(p[0] * 100.0).round() as i32, (p[1] * 100.0).round() as i32]).collect(),
            _ => {}
        }
    }
    let (c, pa) = (c?, pa?);
    f.centre_cm = [(c[0] * 100.0).round() as i32, (c[1] * 100.0).round() as i32];
    f.play_cm = [(pa[0] * 100.0).round() as i32, (pa[1] * 100.0).round() as i32];
    f.yaw_decideg = (yaw? * 10.0).round() as i32;
    (f.boundary_cm.len() >= 3).then_some(f)
}

/// Newest valid head position of Monado's head ring (`pimax-anchor.head`, see `omarchyvr_anchor::world`): (pid, xyz).
pub fn ring_newest(b: &[u8]) -> Option<(u32, [f64; 3])> {
    const HDR: usize = 16;
    const SLOT: usize = 40;
    const SLOTS: usize = 128;
    if b.len() < HDR + SLOTS * SLOT {
        return None;
    }
    let u32at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let f32at = |o: usize| f32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as f64;
    if u32at(0) != 0x434E_4150 || u32at(4) != 1 {
        return None;
    }
    let best = (0..SLOTS)
        .map(|i| HDR + i * SLOT)
        .filter(|&o| u32at(o + 36) & 1 == 1)
        .max_by_key(|&o| u64::from_le_bytes(b[o..o + 8].try_into().unwrap()))?;
    let p = [f32at(best + 8), f32at(best + 12), f32at(best + 16)];
    p.iter().all(|x| x.is_finite()).then_some((u32at(12), p))
}

/// A room fix: (Monado pid, yaw rad, translation m).
pub type Fix = (u32, f64, [f64; 3]);

/// `room_from_slam` of a `last-reloc.toml`.
pub fn parse_fix(text: &str) -> Option<Fix> {
    let (mut pid, mut yaw, mut t) = (None, None, None);
    for l in text.lines() {
        let l = l.split('#').next().unwrap_or("").trim();
        let Some((k, v)) = l.split_once('=') else { continue };
        let v = v.trim();
        match k.trim() {
            "pid" => pid = v.parse().ok(),
            "yaw_deg" => yaw = v.parse::<f64>().ok().map(f64::to_radians),
            "t" => {
                let x: Vec<f64> = v.trim_matches(|c| c == '[' || c == ']').split(',').filter_map(|x| x.trim().parse().ok()).collect();
                t = (x.len() == 3).then(|| [x[0], x[1], x[2]]);
            }
            _ => {}
        }
    }
    Some((pid?, yaw?, t?))
}

/// A tracker-world point in the room frame of a fix (yaw about +y, then the translation).
pub fn to_room(yaw: f64, t: [f64; 3], p: [f64; 3]) -> [f64; 3] {
    let (sn, c) = yaw.sin_cos();
    [c * p[0] + sn * p[2] + t[0], p[1] + t[1], -sn * p[0] + c * p[2] + t[2]]
}

/// A map's stored floor (`floor.toml`), in integers so the state stays comparable.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FloorInfo {
    /// Floor height in the room frame (mm).
    pub y_mm: i32,
    /// `points`, `touch`, `manual` or `standing`.
    pub source: String,
    /// Confidence (%).
    pub confidence: u8,
    /// Standard error (0.1 mm).
    pub sigma_tenth_mm: u32,
    /// Head height above it while mapping (mm), 0 if unknown.
    pub head_above_mm: i32,
}

impl FloorInfo {
    /// For people: "1.41 m below the head, from the mapping walk, 98 % sure".
    pub fn describe(&self) -> String {
        let how = match self.source.as_str() {
            "points" => "from the mapping walk",
            "touch" => "from a controller on the floor",
            "manual" => "typed in",
            "standing" => "from your standing height",
            _ => "stored",
        };
        let head = if self.head_above_mm > 0 { format!(", head {:.2} m above it while mapping", self.head_above_mm as f64 / 1000.0) } else { String::new() };
        format!("floor {:+.3} m in the map, {how}, {} % sure (+-{:.1} mm){head}", self.y_mm as f64 / 1000.0, self.confidence, self.sigma_tenth_mm as f64 / 10.0)
    }
}

/// Parse `floor.toml` (`floor_y = -0.42`, `source = "points"`, ...).
pub fn parse_floor(text: &str) -> Option<FloorInfo> {
    let mut f = FloorInfo::default();
    let mut y = None;
    for l in text.lines() {
        let l = l.split('#').next().unwrap_or("").trim();
        let Some((k, v)) = l.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim().trim_matches('"'));
        let num = || v.parse::<f64>().ok().filter(|x| x.is_finite());
        match k {
            "floor_y" => y = num(),
            "source" => f.source = v.to_string(),
            "confidence" => f.confidence = (num().unwrap_or(0.0).clamp(0.0, 1.0) * 100.0).round() as u8,
            "sigma_m" => f.sigma_tenth_mm = (num().unwrap_or(0.0).max(0.0) * 1e4).round() as u32,
            "head_above_m" => f.head_above_mm = (num().unwrap_or(0.0) * 1e3).round() as i32,
            _ => {}
        }
    }
    f.y_mm = (y? * 1e3).round() as i32;
    Some(f)
}

/// A map's top-down preview (`preview.txt`, cm): points and keyframes as (x, z, y).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Preview {
    /// Floor height (cm), if stored.
    pub floor_cm: Option<i32>,
    /// Keyframe (head) positions in walk order.
    pub heads: Vec<[i32; 3]>,
    /// Map points.
    pub points: Vec<[i32; 3]>,
}

impl Preview {
    /// Bounds in x and z (cm) over points and heads: (min x, max x, min z, max z).
    pub fn bounds(&self) -> Option<(i32, i32, i32, i32)> {
        let mut it = self.points.iter().chain(self.heads.iter());
        let f = it.next()?;
        Some(it.fold((f[0], f[0], f[1], f[1]), |b, p| (b.0.min(p[0]), b.1.max(p[0]), b.2.min(p[1]), b.3.max(p[1]))))
    }
}

/// Parse `preview.txt` (`floor Y`, `h X Z Y`, `p X Z Y`).
pub fn parse_preview(text: &str) -> Preview {
    let mut pv = Preview::default();
    for l in text.lines() {
        let mut w = l.split_whitespace();
        let tag = w.next();
        let n: Vec<f64> = w.filter_map(|x| x.parse().ok()).collect();
        match (tag, n.as_slice()) {
            (Some("floor"), [y]) => pv.floor_cm = Some(y.round() as i32),
            (Some("h"), [x, z, y]) => pv.heads.push([*x as i32, *z as i32, *y as i32]),
            (Some("p"), [x, z, y]) => pv.points.push([*x as i32, *z as i32, *y as i32]),
            _ => {}
        }
    }
    pv
}

/// A synthetic room for demo mode and screenshots (no real room ends up in the pictures): a 4 x 3.5 m floor, a
/// desk, two walls, a mapping walk around the middle.
pub fn demo_preview() -> Preview {
    let mut s = 12345u64;
    let mut r = |n: i32| {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((s >> 33) % n.max(1) as u64) as i32
    };
    let mut pts = Vec::new();
    for _ in 0..700 {
        pts.push([r(400) - 200, r(350) - 175, -142 + r(3) - 1]);
    }
    for _ in 0..500 {
        pts.push([r(140) - 20, r(70) - 170, -68 + r(3) - 1]);
    }
    for _ in 0..900 {
        let y = r(260) - 142;
        pts.push(if r(2) == 0 { [200, r(350) - 175, y] } else { [r(400) - 200, -175, y] });
    }
    let heads = (0..60)
        .map(|i| {
            let a = i as f64 / 60.0 * std::f64::consts::TAU;
            [(a.cos() * 90.0) as i32, (a.sin() * 70.0) as i32, (a * 3.0).sin() as i32 * 3]
        })
        .collect();
    Preview { floor_cm: Some(-142), heads, points: pts }
}

/// The Monado pid of an offset file (`pimax-anchor-offset 1`, `pid N`, `pose ...`).
pub fn parse_offset(text: &str) -> Option<u32> {
    let mut l = text.lines();
    if l.next()?.trim() != "pimax-anchor-offset 1" {
        return None;
    }
    l.find_map(|x| x.trim().strip_prefix("pid ").and_then(|p| p.trim().parse().ok()))
}

/// The owner file (`roommap 1234`).
pub fn parse_owner(text: &str) -> Option<(Writer, u32)> {
    let mut w = text.split_whitespace();
    let who = Writer::parse(w.next()?)?;
    Some((who, w.next()?.parse().ok()?))
}

/// Read the state (background).
pub fn collect(m: &dyn Machine, r: &RoomPaths) -> RoomState {
    let mut maps: Vec<String> = m.list(&r.maps).into_iter().filter(|n| m.exists(&r.maps.join(n).join("map.bin"))).collect();
    maps.sort();
    let fixes: Vec<(String, Fix)> =
        maps.iter().filter_map(|n| Some((n.clone(), parse_fix(&m.read_str(&r.maps.join(n).join("last-reloc.toml"))?)?))).collect();
    RoomState {
        tag_calibrated: m.exists(&r.tag_conf),
        offset_pid: m.read_str(&r.offset).and_then(|t| parse_offset(&t)),
        owner: m.read_str(&r.owner).and_then(|t| parse_owner(&t)),
        tools: (m.exists(&r.roommap_bin), m.exists(&r.anchor_bin)),
        floors: maps.iter().filter_map(|n| Some((n.clone(), parse_floor(&m.read_str(&r.maps.join(n).join("floor.toml"))?)?))).collect(),
        previews: maps.iter().filter_map(|n| Some((n.clone(), parse_preview(&m.read_str(&r.maps.join(n).join("preview.txt"))?)))).collect(),
        grid_on: m.read_str(&r.grid).is_some_and(|t| matches!(t.trim(), "1" | "on" | "true")),
        fixes: fixes.iter().map(|(n, f)| (n.clone(), f.0)).collect(),
        setups: maps.iter().filter_map(|n| Some((n.clone(), parse_setup(&m.read_str(&r.maps.join(n).join("room-setup.toml"))?)?))).collect(),
        prefs: m.read_str(&r.prefs).map(|t| omarchyvr_tools::vr::room::parse_settings(&t)).unwrap_or_default(),
        status: m.read_str(&r.status).and_then(|t| omarchyvr_tools::vr::room::parse_status(&t)),
        head: m.read(&r.head_ring).as_deref().and_then(ring_newest).and_then(|(pid, p)| {
            let (n, f) = fixes.iter().find(|(_, f)| f.0 == pid)?;
            let q = to_room(f.1, f.2, p);
            Some((n.clone(), [(q[0] * 100.0).round() as i32, (q[2] * 100.0).round() as i32, (q[1] * 100.0).round() as i32]))
        }),
        maps,
    }
}

/// Who holds the offset of a Monado session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Holder {
    /// No offset for this session.
    Free,
    /// Written through the app by this tool.
    App(Writer),
    /// Written outside the app (writer unknown).
    Outside,
}

/// Who holds the offset of this Monado session.
pub fn holder(st: &RoomState, monado: u32) -> Holder {
    match (st.offset_pid, st.owner) {
        (Some(p), Some((w, op))) if p == monado && op == monado => Holder::App(w),
        (Some(p), _) if p == monado => Holder::Outside,
        _ => Holder::Free,
    }
}

/// May `who` write the offset of this session? Only one of the room map and the tag per Monado session.
pub fn may_write(st: &RoomState, monado: u32, who: Writer) -> Result<(), String> {
    match holder(st, monado) {
        Holder::Free => Ok(()),
        Holder::App(w) if w == who => Ok(()),
        Holder::App(w) => Err(format!(
            "this session's room offset came from {}: only one of the room map and the AprilTag per session. Clear the offset first.",
            w.title()
        )),
        Holder::Outside => Err("this session already has a room offset written outside the app: clear it first".into()),
    }
}

/// What a room action needs from the running session; the reasons it cannot run now.
pub fn blockers(live: &Live, st: &RoomState, needs_map: Option<&str>) -> Vec<String> {
    let mut v = Vec::new();
    match &live.service {
        None => v.push("Start desk mode with 6DoF and the room anchor hook first (Room > anchor hook, then Start Desk).".into()),
        Some(s) => {
            if !s.sixdof {
                v.push("The session runs without 6DoF: the room tools need the camera broker and tracking.".into());
            } else if !live.alive("broker") {
                v.push("The camera broker is not running.".into());
            }
            if !s.anchor {
                v.push("Monado runs without the anchor hook (PIMAX_ANCHOR=1): restart desk mode with it on.".into());
            }
        }
    }
    if let Some(name) = needs_map {
        if !st.maps.iter().any(|m| m == name) {
            v.push(format!("No room map named \"{name}\" yet: map the room first."));
        }
    }
    v
}

/// `omarchyvr-roommap build --source broker --seconds N --name NAME`.
pub fn build_cmd(r: &RoomPaths, name: &str, seconds: u32) -> Cmd {
    Cmd::with_args(
        r.roommap_bin.display().to_string(),
        vec!["build".into(), "--source".into(), "broker".into(), "--seconds".into(), seconds.to_string(), "--name".into(), name.into()],
    )
}

/// `omarchyvr-roommap relocalize --source broker --name NAME --apply`.
pub fn relocalize_cmd(r: &RoomPaths, name: &str) -> Cmd {
    Cmd::with_args(
        r.roommap_bin.display().to_string(),
        vec!["relocalize".into(), "--source".into(), "broker".into(), "--name".into(), name.into(), "--apply".into()],
    )
}

/// `omarchyvr-roommap view --name NAME --open` (writes NAME/map.html and opens it with xdg-open).
pub fn view_cmd(r: &RoomPaths, name: &str) -> Cmd {
    Cmd::with_args(r.roommap_bin.display().to_string(), vec!["view".into(), "--name".into(), name.into(), "--open".into()])
}

/// `omarchyvr-anchor calibrate | apply | clear`.
pub fn anchor_cmd(r: &RoomPaths, what: &str) -> Cmd {
    Cmd::with_args(r.anchor_bin.display().to_string(), vec![what.to_string()])
}

/// A map name the tools accept (letters, digits, `-`, `_`).
pub fn valid_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 40 && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Record (or clear) who wrote the offset of a session.
pub fn write_owner(path: &Path, who: Option<(Writer, u32)>) -> std::io::Result<()> {
    match who {
        Some((w, pid)) => std::fs::write(path, format!("{} {pid}\n", w.name())),
        None => match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        },
    }
}

/// A floor action of the Room page.
#[derive(Debug, Clone, PartialEq)]
pub enum FloorAction {
    /// Find the floor in the map's points (offline; keeps a touch/manual floor the points do not contradict).
    Detect,
    /// A controller laid on the floor (live: ctrl-track and a room fix in this session).
    Touch(&'static str),
    /// Typed in: the floor height in the map (m).
    Set(f64),
    /// From the eye height standing upright (m; live, needs a room fix in this session).
    Standing(f64),
}

/// `omarchyvr-roommap floor detect --name NAME --save`.
pub fn floor_cmd(r: &RoomPaths, name: &str, a: &FloorAction) -> Cmd {
    let mut v: Vec<String> = vec!["floor".into()];
    match a {
        FloorAction::Detect => v.extend(["detect".into(), "--save".into()]),
        FloorAction::Touch(hand) => v.extend(["touch".into(), "--hand".into(), hand.to_string(), "--save".into()]),
        FloorAction::Set(y) => v.extend(["set".into(), "--y".into(), format!("{y:.3}")]),
        FloorAction::Standing(h) => v.extend(["set".into(), "--standing".into(), format!("{h:.3}")]),
    }
    v.extend(["--name".into(), name.into()]);
    Cmd::with_args(r.roommap_bin.display().to_string(), v)
}

/// Why a floor action cannot run now (empty: it can).
pub fn floor_blockers(live: &Live, st: &RoomState, name: &str, a: &FloorAction) -> Vec<String> {
    let mut v = Vec::new();
    if !st.tools.0 {
        v.push("omarchyvr-roommap is not built (cargo build --release -p omarchyvr-roommap).".into());
    }
    if !st.maps.iter().any(|m| m == name) {
        v.push(format!("No room map named \"{name}\" yet: map the room first."));
    }
    if let FloorAction::Set(y) = a {
        if !(-3.0..=1.0).contains(y) {
            v.push("The floor height in the map must be between -3 and +1 m.".into());
        }
    }
    if let FloorAction::Standing(h) = a {
        if !(1.0..=2.3).contains(h) {
            v.push("Eye height standing upright: 1.0-2.3 m.".into());
        }
    }
    if matches!(a, FloorAction::Touch(_) | FloorAction::Standing(_)) {
        match &live.service {
            None => v.push("Needs a running desk session with 6DoF and the anchor hook.".into()),
            Some(s) => {
                if !s.anchor {
                    v.push("Monado runs without the anchor hook (PIMAX_ANCHOR=1).".into());
                }
                if matches!(a, FloorAction::Touch(_)) && !s.ctrl_track {
                    v.push("Needs optical controller tracking (--ctrl-track).".into());
                }
                if !st.fixes.iter().any(|(m, p)| m == name && *p == s.pid) {
                    v.push("Recognise the room in this session first (the floor is stored in the map's frame).".into());
                }
            }
        }
    }
    v
}

impl crate::app::AppCore {
    /// Run a floor action (a job; its output goes to the Output panel).
    pub fn floor_action(&mut self, a: FloorAction) {
        let name = self.room.map_name.clone();
        let b = floor_blockers(&self.live, &self.room.state, &name, &a);
        if !b.is_empty() {
            self.message = b.join(" ");
            return;
        }
        let label = match &a {
            FloorAction::Detect => format!("Find the floor of \"{name}\""),
            FloorAction::Touch(h) => format!("Floor from the {h} controller (\"{name}\")"),
            FloorAction::Set(_) | FloorAction::Standing(_) => format!("Set the floor of \"{name}\""),
        };
        let mut steps = Vec::new();
        if let FloorAction::Touch(h) = &a {
            steps.push(crate::job::Step::Note(format!("Lay the {h} controller flat on the floor where the headset sees it, let go, keep looking at it for a second.")));
        }
        steps.push(crate::job::Step::Run(floor_cmd(&self.room_paths, &name, &a)));
        self.spawn(crate::job::JobSpec { kind: crate::job::JobKind::Other("room:floor".into()), label, steps });
    }

    /// Switch the floor grid of omarchyvr-passthrough (`$XDG_RUNTIME_DIR/omarchyvr-floor-grid`).
    pub fn set_floor_grid(&mut self, on: bool) {
        if self.demo {
            self.room.state.grid_on = on;
            self.message = format!("demo: would write {}", self.room_paths.grid.display());
            return;
        }
        let p = self.room_paths.grid.clone();
        self.task(None, move || {
            let r = std::fs::write(&p, if on { "1\n" } else { "0\n" });
            Box::new(move |c: &mut crate::app::AppCore| match r {
                Ok(()) => c.room.state.grid_on = on,
                Err(e) => c.message = format!("cannot write {}: {e}", p.display()),
            })
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{ServiceInfo, SessionProc};

    #[test]
    fn one_writer_per_session() {
        assert_eq!(parse_offset("pimax-anchor-offset 1\npid 42\npose 0 0 0 0 0 0 1\n"), Some(42));
        assert_eq!(parse_offset("something\npid 42\n"), None);
        assert_eq!(parse_owner("roommap 42\n"), Some((Writer::RoomMap, 42)));
        let mut st = RoomState::default();
        assert!(may_write(&st, 42, Writer::Tag).is_ok(), "no offset yet");
        st.offset_pid = Some(42);
        assert!(may_write(&st, 42, Writer::Tag).unwrap_err().contains("outside the app"));
        st.owner = Some((Writer::RoomMap, 42));
        assert!(may_write(&st, 42, Writer::RoomMap).is_ok(), "the same writer again");
        assert!(may_write(&st, 42, Writer::Tag).unwrap_err().contains("came from the room map"));
        assert!(may_write(&st, 43, Writer::Tag).is_ok(), "a new Monado session starts free");
    }

    #[test]
    fn blockers_and_commands() {
        let mut live = Live::default();
        let st = RoomState { maps: vec!["home".into()], ..Default::default() };
        assert_eq!(blockers(&live, &st, None).len(), 1);
        live.service = Some(ServiceInfo { pid: 1, age: None, build: None, hz: None, eye: false, sixdof: true, ctrl_track: false, broker: true, ipd: None, anchor: true });
        live.session.push(SessionProc { name: "broker", pid: Some(2), alive: true, age: None });
        assert!(blockers(&live, &st, Some("home")).is_empty());
        assert_eq!(blockers(&live, &st, Some("office")).len(), 1);
        let p = Paths::new(Path::new("/home/u"), Path::new("/home/u/omarchyvr"), Path::new("/tmp/s"), Path::new("/run/user/1000"));
        let r = RoomPaths::new(&p);
        assert_eq!(build_cmd(&r, "home", 90).display(), "/home/u/omarchyvr/rust/target/release/omarchyvr-roommap build --source broker --seconds 90 --name home");
        assert_eq!(relocalize_cmd(&r, "home").display(), "/home/u/omarchyvr/rust/target/release/omarchyvr-roommap relocalize --source broker --name home --apply");
        assert_eq!(anchor_cmd(&r, "clear").display(), "/home/u/omarchyvr/rust/target/release/omarchyvr-anchor clear");
        assert_eq!(r.offset, PathBuf::from("/run/user/1000/pimax-anchor.offset"));
        for c in [build_cmd(&r, "home", 90), relocalize_cmd(&r, "home"), anchor_cmd(&r, "apply")] {
            assert_eq!(crate::safety::check(&c), Ok(()), "{}", c.display());
        }
        assert!(valid_name("home-2") && !valid_name("a b") && !valid_name(""));
        for a in [FloorAction::Detect, FloorAction::Touch("left"), FloorAction::Set(-1.42), FloorAction::Standing(1.65)] {
            assert_eq!(crate::safety::check(&floor_cmd(&r, "home", &a)), Ok(()), "{a:?}");
        }
        assert_eq!(
            floor_cmd(&r, "home", &FloorAction::Touch("right")).display(),
            "/home/u/omarchyvr/rust/target/release/omarchyvr-roommap floor touch --hand right --save --name home"
        );
    }

    #[test]
    fn floor_files_and_blockers() {
        let f = parse_floor("# x\nfloor_y = -0.42070\nsource = \"points\"\nconfidence = 0.980\nsigma_m = 0.00080\nhead_above_m = 1.412\n").unwrap();
        assert_eq!(f, FloorInfo { y_mm: -421, source: "points".into(), confidence: 98, sigma_tenth_mm: 8, head_above_mm: 1412 });
        assert!(f.describe().contains("from the mapping walk"));
        assert_eq!(parse_floor("source = \"touch\"\n"), None);
        let pv = parse_preview("# c\nfloor -42.1\nh 0 0 0\nh 10 5 1\np -100 50 -42\np 200 -30 80\nbad line\n");
        assert_eq!(pv.floor_cm, Some(-42));
        assert_eq!((pv.heads.len(), pv.points.len()), (2, 2));
        assert_eq!(pv.bounds(), Some((-100, 200, -30, 50)));
        assert!(demo_preview().points.len() > 1000);
        let fx = parse_fix("map = \"x\"\nyaw_deg = 90.0\nt = [1.0, 0.5, 0.0]\npid = 42\n").unwrap();
        assert_eq!(fx.0, 42);
        let q = to_room(fx.1, fx.2, [1.0, 0.0, 0.0]);
        assert!((q[0] - 1.0).abs() < 1e-9 && (q[1] - 0.5).abs() < 1e-9 && (q[2] + 1.0).abs() < 1e-9, "{q:?}");
        let mut ring = vec![0u8; 16 + 128 * 40];
        ring[0..4].copy_from_slice(&0x434E_4150u32.to_le_bytes());
        ring[4..8].copy_from_slice(&1u32.to_le_bytes());
        ring[12..16].copy_from_slice(&42u32.to_le_bytes());
        for (i, (ts, x)) in [(5u64, 1.0f32), (9, 2.0), (7, 3.0)].iter().enumerate() {
            let o = 16 + i * 40;
            ring[o..o + 8].copy_from_slice(&ts.to_le_bytes());
            ring[o + 8..o + 12].copy_from_slice(&x.to_le_bytes());
            ring[o + 36..o + 40].copy_from_slice(&1u32.to_le_bytes());
        }
        assert_eq!(ring_newest(&ring), Some((42, [2.0, 0.0, 0.0])));
        let mut st = RoomState { maps: vec!["home".into()], tools: (true, true), ..Default::default() };
        let mut live = Live::default();
        assert!(floor_blockers(&live, &st, "home", &FloorAction::Detect).is_empty(), "offline");
        assert_eq!(floor_blockers(&live, &st, "office", &FloorAction::Detect).len(), 1);
        assert!(!floor_blockers(&live, &st, "home", &FloorAction::Touch("left")).is_empty(), "needs a session");
        live.service = Some(ServiceInfo { pid: 7, age: None, build: None, hz: None, eye: false, sixdof: true, ctrl_track: true, broker: true, ipd: None, anchor: true });
        assert_eq!(floor_blockers(&live, &st, "home", &FloorAction::Touch("left")).len(), 1, "no room fix yet");
        st.fixes = vec![("home".into(), 7)];
        assert!(floor_blockers(&live, &st, "home", &FloorAction::Touch("left")).is_empty());
        assert_eq!(floor_blockers(&live, &st, "home", &FloorAction::Standing(3.0)).len(), 1);
    }
}
