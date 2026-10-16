//! `pimax-roommap setup fast|full|show` and `pimax-roommap auto` (docs/ROOM-SETUP.md).
//!
//! * `setup fast`: in the middle of the play space, look around (the saved room is recognised, or a quick 30 s map is
//!   made), the floor from the map (a controller on the floor if the map has none), forward by trigger or by holding
//!   still, the play area from the free space the map shows around you (2 x 2 m when it shows none).
//! * `setup full`: walk the room (60 s map), the floor confirmed with a controller on it, the boundary traced with a
//!   trigger held (or a pinch), centre and forward.
//! * Both talk to the user IN THE HEADSET (the `pimax-hud` panel, an `XR_EXTX_overlay` session over WayVR; its input
//!   gives the triggers, the aim poses and the head) and print every step for the app. The result is
//!   `<map>/room-setup.toml` and the room put in place right away (offset with floor + centre + forward, STAGE bounds).
//! * `auto` (started by `pimax vr start` / `pimax desk start` when the room switch is on): wait for Monado's head
//!   ring, recognise the room (retrying), apply it; plain-words state in `$XDG_RUNTIME_DIR/pimax-room.status`.

use crate::{flag, map_dir, num, opt, unix_now, R};
use pimax_anchor::world::{self, HeadRing};
use pimax_cam::geom::{self, Pose};
use pimax_hud::{HudHandle, Line};
use pimax_room::map::RoomMap;
use pimax_roommap::floor::{self, StoredFloor};
use pimax_roommap::setup::{self, Guide, HandSample, RoomSetup, Sample, SetupMode, P2};
use pimax_roommap::store::RelocResult;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const HELP: &str = "\
pimax-roommap setup fast|full [--name NAME] [--label TEXT] [--no-hud] [--map-seconds S]
    guided room setup in the headset (docs/ROOM-SETUP.md): fast = look around in the middle (~30 s), automatic floor
    and play area, forward by trigger or holding still; full = map walk (60 s), floor touch, traced boundary, forward.
    Saves NAME/room-setup.toml and puts the room in place (needs Monado with 6DoF and the anchor hook).
pimax-roommap setup show [--name NAME]
pimax-roommap auto [--name NAME] [--wait S] [--status FILE]
    at a session start: wait for Monado's head ring, recognise the room (retries for --wait s, default 120),
    apply floor + centre + forward; state in $XDG_RUNTIME_DIR/pimax-room.status";

/// `$XDG_RUNTIME_DIR/pimax-room.status` (read by the app through `pimax_tools::vr::room`).
pub fn status_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp")).join("pimax-room.status")
}

/// `$XDG_RUNTIME_DIR/pimax-stage-bounds` (Monado, marker PIMAX_STAGE_BOUNDS).
pub fn stage_bounds_path() -> PathBuf {
    if let Some(p) = std::env::var_os("PIMAX_STAGE_BOUNDS_FILE").filter(|v| !v.is_empty()) {
        return PathBuf::from(p);
    }
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp")).join("pimax-stage-bounds")
}

/// The stage bounds file text.
pub fn stage_bounds_text(pid: u32, play_area: [f64; 2]) -> String {
    format!("pimax-stage-bounds 1\npid {pid}\nsize {:.3} {:.3}\n", play_area[0], play_area[1])
}

/// Write the stage bounds for Monado `pid` (atomic).
pub fn write_stage_bounds(pid: u32, play_area: [f64; 2]) -> std::io::Result<PathBuf> {
    let p = stage_bounds_path();
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, stage_bounds_text(pid, play_area))?;
    std::fs::rename(&tmp, &p)?;
    Ok(p)
}

fn write_status(path: &Path, state: &str, room: &str, pid: u32, text: &str) {
    let t = format!(
        "# pimax-roommap auto / setup (docs/ROOM-SETUP.md)\nstate = \"{state}\"\nroom = \"{room}\"\npid = {pid}\ntext = \"{}\"\nwhen = {}\n",
        text.replace(['"', '\n'], " "),
        unix_now()
    );
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, t).and_then(|_| std::fs::rename(&tmp, path)).is_err() {
        eprintln!("cannot write {}", path.display());
    }
    println!("room: {text}");
}

/// The app's owner file next to the offset (`roommap <pid>`): the Room page shows who placed the world.
fn write_owner(pid: u32) {
    let p = world::offset_path().with_file_name("pimax-anchor.offset.owner");
    let _ = std::fs::write(p, format!("roommap {pid}\n"));
}

/// The AprilTag holds this Monado session's offset (the app's one-writer rule): the room map must not overwrite it.
fn tag_holds(pid: u32) -> bool {
    let p = world::offset_path().with_file_name("pimax-anchor.offset.owner");
    std::fs::read_to_string(p).ok().is_some_and(|t| {
        let mut w = t.split_whitespace();
        w.next() == Some("anchor") && w.next().and_then(|x| x.parse::<u32>().ok()) == Some(pid)
    }) && world::read_offset(&world::offset_path()).is_some_and(|(p, _)| p == pid)
}

fn alive(pid: u32) -> bool {
    pid != 0 && Path::new(&format!("/proc/{pid}")).exists()
}

/// The running Monado's pid when its head-pose feed is live (written by a running monado-service, fresh).
fn ring_pid() -> Option<u32> {
    world::feed_now().live_pid()
}

/// The ring's writer pid, whatever its state.
fn ring_owner() -> Option<u32> {
    HeadRing::read(&world::ring_path()).map(|r| r.pid)
}

/// Ok while the head-pose feed is the live one of Monado `pid`; else why not, in plain words.
fn feed_ok(pid: u32) -> R<()> {
    let f = world::feed_now();
    match f.live_pid() {
        Some(p) if p == pid => Ok(()),
        _ => Err(format!("{}; nothing saved", f.plain())),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// In-headset guide

/// Room-frame conversion of STAGE samples: stage = (0, oh, 0) + offset * slam, room = room_from_slam * slam.
struct Conv {
    oh: f64,
    inv_off: Pose,
    a: Pose,
}

impl Conv {
    fn load(dir: &Path, pid: u32) -> Option<Conv> {
        let r = RelocResult::load(&dir.join("last-reloc.toml")).ok().filter(|r| r.pid == pid && pid != 0)?;
        let off = world::read_offset(&world::offset_path()).filter(|(p, _)| *p == pid).map(|(_, o)| o).unwrap_or(Pose { r: geom::rot_x(0.0), t: [0.0; 3] });
        Some(Conv { oh: floor::origin_height(pid), inv_off: off.inverse(), a: r.align.pose() })
    }

    fn pos(&self, p: [f32; 3]) -> [f64; 3] {
        self.a.apply(self.inv_off.apply([p[0] as f64, p[1] as f64 - self.oh, p[2] as f64]))
    }

    fn dir(&self, d: [f32; 3]) -> [f64; 3] {
        geom::mat_vec(&self.a.r, geom::mat_vec(&self.inv_off.r, [d[0] as f64, d[1] as f64, d[2] as f64]))
    }
}

/// Wrap a sentence into lines of at most `w` characters (the HUD's 5x7 font shrinks long lines).
pub fn wrap(text: &str, w: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        if !cur.is_empty() && cur.len() + 1 + word.len() > w {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

struct HudGuide {
    hud: Option<HudHandle>,
    t0: Instant,
    dir: PathBuf,
    pid: u32,
    conv: Option<(Instant, Option<Conv>)>,
    last: (String, Instant),
}

impl HudGuide {
    fn show(&self, title: &str, text: &str, progress: Option<f32>) {
        if let Some(h) = &self.hud {
            let mut lines = vec![Line::new(title).color(pimax_hud::screen::CYAN)];
            lines.extend(wrap(text, 34).into_iter().map(|l| Line::new(l).scale(5)));
            h.set_styled(lines);
            h.set_progress(progress);
        }
    }

    /// A blocking step (mapping, recognising) with a countdown bar on the panel.
    fn during<T>(&self, title: &str, text: &str, seconds: f64, f: impl FnOnce() -> T) -> T {
        self.show(title, text, Some(0.0));
        println!("room setup: {title}: {text}");
        let stop = Arc::new(AtomicBool::new(false));
        let ticker = self.hud.clone().map(|h| {
            let stop = stop.clone();
            let t0 = Instant::now();
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    h.set_progress(Some((t0.elapsed().as_secs_f64() / seconds).min(1.0) as f32));
                    std::thread::sleep(Duration::from_millis(250));
                }
            })
        });
        let r = f();
        stop.store(true, Ordering::Relaxed);
        if let Some(t) = ticker {
            let _ = t.join();
        }
        if let Some(h) = &self.hud {
            h.set_progress(None);
        }
        r
    }

    fn restart_clock(&mut self) {
        self.t0 = Instant::now();
    }
}

impl Guide for HudGuide {
    fn say(&mut self, title: &str, text: &str, progress: Option<f32>) {
        self.show(title, text, progress);
        // print each new prompt; a running counter at most once a second
        let key = format!("{title}: {text}");
        if key != self.last.0 && (self.last.1.elapsed() >= Duration::from_secs(1) || !self.last.0.starts_with(title)) {
            println!("room setup: {key}");
            self.last = (key, Instant::now());
        }
    }

    fn sample(&mut self) -> Sample {
        std::thread::sleep(Duration::from_millis(20));
        let fresh = self.conv.as_ref().is_some_and(|(t, _)| t.elapsed() < Duration::from_millis(500));
        if !fresh {
            self.conv = Some((Instant::now(), Conv::load(&self.dir, self.pid)));
        }
        let mut s = Sample { t: self.t0.elapsed().as_secs_f64(), ..Default::default() };
        let Some((_, Some(c))) = &self.conv else { return s };
        let inp = self.hud.as_ref().map(|h| h.input()).unwrap_or_default();
        if inp.samples == 0 {
            // no panel input: the head from Monado's head ring (forward by holding still still works)
            if let Some(r) = HeadRing::read(&world::ring_path()).filter(|r| r.pid == self.pid) {
                if let Some((_, p, _)) = r.samples.iter().rev().find(|x| x.2) {
                    let room = c.a.compose(p);
                    let f = geom::mat_vec(&room.r, [0.0, 0.0, -1.0]);
                    s.head = Some((room.t, setup::heading(f[0], f[2])));
                }
            }
            return s;
        }
        if let (Some(p), Some(d)) = (inp.head_pos, inp.head_dir) {
            let dr = c.dir(d);
            s.head = Some((c.pos(p), setup::heading(dr[0], dr[2])));
        }
        for i in 0..2 {
            let x = inp.hands[i];
            s.hands[i] = HandSample { pos: x.pos.map(|p| c.pos(p)), value: x.value as f64, active: x.active };
        }
        s
    }
}

/// Start the panel on its own thread (OpenXR frame calls stay there): an overlay over WayVR, or a main session when
/// no main app runs. None (printed) when there is no OpenXR runtime: the setup then runs with the app's text only.
fn start_hud() -> Option<HudHandle> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mk = |overlay: bool| {
            pimax_hud::Hud::new(pimax_hud::Opts {
                placement: pimax_hud::Placement::HeadLocked,
                distance: 1.3,
                overlay,
                app_name: "pimax-room-setup".into(),
                input: true,
            })
        };
        let mut hud = match mk(true).or_else(|e| {
            eprintln!("in-headset panel as an overlay: {e}; trying a main session");
            mk(false)
        }) {
            Ok(h) => h,
            Err(e) => {
                let _ = tx.send(Err(e));
                return;
            }
        };
        let _ = tx.send(Ok(hud.handle()));
        loop {
            match hud.frame() {
                Ok(pimax_hud::FrameResult::Exit) => break,
                Err(e) => {
                    eprintln!("in-headset panel: {e}");
                    break;
                }
                _ => {}
            }
        }
    });
    match rx.recv_timeout(Duration::from_secs(20)) {
        Ok(Ok(h)) => Some(h),
        Ok(Err(e)) => {
            println!("note: no in-headset panel ({e}): follow the steps here");
            None
        }
        Err(_) => {
            println!("note: the in-headset panel did not start in 20 s: follow the steps here");
            None
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------

/// Wait until the headset is worn: Monado's head pose moves (looking around) within `seconds`. The setup used to
/// start mapping the moment the button was pressed, while the headset could still be on the desk. Ok(false) on
/// a timeout (the setup then goes on: the user may be sitting very still); Err when the head-pose feed is not the
/// running Monado's or stopped (more than 2 s), in plain words, instead of waiting for movement that cannot show.
fn wait_for_wear(g: &mut HudGuide, seconds: f64) -> R<bool> {
    let t0 = Instant::now();
    let mut poses: Vec<(Instant, Pose)> = Vec::new();
    let mut said = false;
    let mut checked = Instant::now();
    let mut bad_since: Option<Instant> = None;
    while t0.elapsed().as_secs_f64() < seconds {
        if checked.elapsed() >= Duration::from_millis(500) {
            checked = Instant::now();
            match feed_ok(g.pid) {
                Ok(()) => bad_since = None,
                Err(e) if bad_since.get_or_insert_with(Instant::now).elapsed() > Duration::from_secs(2) => return Err(e),
                Err(_) => {}
            }
        }
        if let Some(r) = HeadRing::read(&world::ring_path()).filter(|r| r.pid == g.pid) {
            if let Some((_, p, _)) = r.samples.iter().rev().find(|x| x.2) {
                poses.push((Instant::now(), *p));
            }
        }
        poses.retain(|(t, _)| t.elapsed() < Duration::from_secs(3));
        let ps: Vec<Pose> = poses.iter().map(|x| x.1).collect();
        if pimax_roommap::light::moved(&ps, 8.0, 0.15) {
            return Ok(true);
        }
        if !said {
            g.say("Room setup", "Put the headset on and look around: the room setup starts when you move your head.", None);
            said = true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    println!("room setup: no head movement in {seconds:.0} s: starting anyway");
    Ok(false)
}

/// Before mapping: is it bright enough? While it is too dark, ask for light (up to `seconds`). Ok(advice) of the
/// last measurement; a camera problem only skips the check.
fn check_light(g: &mut HudGuide, title: &str, seconds: f64) -> Option<pimax_roommap::light::Advice> {
    use pimax_roommap::light::{self, Advice};
    let rig = match pimax_cam::calib::DeviceCalib::load_default(None)
        .map_err(|e| e.to_string())
        .and_then(|(c, _)| pimax_room::stereo::Rig::from_calib(&c, &[0, 1]))
    {
        Ok(r) => r,
        Err(e) => {
            println!("light check skipped: {e}");
            return None;
        }
    };
    let need = pimax_roommap::pipeline::build_params(0.2, 15.0).kf_min_points;
    let o = light::MeasureOpts { every: 4, ..Default::default() };
    let t0 = Instant::now();
    loop {
        let qs = match light::measure_source("broker", 2.0, &o, &rig, &pimax_room::orb::OrbParams::default()) {
            Ok(q) if !q.is_empty() => q,
            Ok(_) => return None,
            Err(e) => {
                println!("light check skipped: {e}");
                return None;
            }
        };
        let r = light::summarise(&qs, need);
        let q = light::FrameQuality { mean: r.mean, stereo: r.stereo, ..Default::default() };
        let a = light::advice(&q, need);
        println!(
            "light check: brightness {:.0}/255 (exposure {} gain {}), {} stereo points per view (a view needs {need}): {a:?}",
            r.mean, r.exposure, r.gain, r.stereo
        );
        if a != Advice::TooDark || t0.elapsed().as_secs_f64() >= seconds {
            return Some(a);
        }
        g.say(title, &format!("{} ({} points in view, brightness {:.0} of 255.)", light::advice_text(a), r.stereo, r.mean), None);
    }
}

/// Map with the live count of views and points (and what to do) on the panel; a dark or plain room keeps mapping
/// up to 30 s longer until 8 views are in.
fn map_with_feedback(g: &HudGuide, title: &str, text: &str, args: Vec<String>, map_s: f64, append: bool) -> R<()> {
    let mut args = args;
    if append {
        args.push("--append".into());
    } else {
        args.extend(s(&["--min-keyframes", "8", "--extend", "30"]));
    }
    let mut last = Instant::now() - Duration::from_secs(5);
    let hud = g.hud.clone();
    let title_s = title.to_string();
    g.during(title, text, map_s + 5.0, || {
        crate::cmd_build_with(
            args,
            Some(&mut |p: &pimax_roommap::light::MapProgress| {
                if last.elapsed() < Duration::from_millis(700) {
                    return;
                }
                last = Instant::now();
                let t = pimax_roommap::light::progress_text(p);
                println!("room setup: {title_s}: {t}");
                if let Some(h) = &hud {
                    let mut lines = vec![Line::new(&title_s).color(pimax_hud::screen::CYAN)];
                    lines.extend(wrap(&t, 34).into_iter().map(|l| Line::new(l).scale(5)));
                    h.set_styled(lines);
                }
            }),
        )
    })
}

/// Keep the map that is about to be replaced (map.bin.prev): a failed recognition at night must not lose the
/// daylight map for good.
fn backup_map(dir: &Path) {
    let m = dir.join("map.bin");
    if m.exists() {
        let _ = std::fs::copy(&m, dir.join("map.bin.prev"));
    }
}

fn save_fix_from_current(dir: &Path, pid: u32, what: &str) -> R<RelocResult> {
    // a map built in this session: its room frame is the app world when it was built (the offset then)
    let a = pimax_roommap::live::current_offset(pid);
    let r = RelocResult {
        map: dir.display().to_string(),
        source: what.into(),
        align: a,
        frames: 0,
        inliers: 0,
        rms_deg: 0.0,
        sigma_deg: 0.0,
        sigma_m: 0.0,
        fix_after_s: 0.0,
        pid,
    };
    r.save(&dir.join("last-reloc.toml")).map_err(|e| e.to_string())?;
    Ok(r)
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

/// The floor: kept if stored, else from the map points, else a controller on the floor. Ok(description).
fn ensure_floor(g: &mut HudGuide, dir: &Path, name: &str, confirm_touch: bool) -> R<String> {
    if StoredFloor::load(dir).is_err() {
        let _ = crate::floorcmd::cmd_floor(s(&["detect", "--save", "--name", name]));
    }
    let have = StoredFloor::load(dir).ok();
    if have.is_some() && !confirm_touch {
        return Ok("floor from the map".into());
    }
    let why = if have.is_some() { "Confirm the floor" } else { "Floor" };
    let text = "Lay the RIGHT controller flat on the floor in front of you and let go. Keep looking at it for 2 seconds.";
    let r = g.during(why, text, 25.0, || crate::floorcmd::cmd_floor(s(&["touch", "--hand", "right", "--save", "--seconds", "25", "--name", name])));
    match (r, have) {
        (Ok(()), _) if StoredFloor::load(dir).is_ok() => Ok("floor from the controller on the floor".into()),
        (_, Some(_)) => {
            g.say(why, "No controller seen on the floor: keeping the floor found in the map.", None);
            std::thread::sleep(Duration::from_secs(2));
            Ok("floor from the map (the controller touch did not work)".into())
        }
        (Err(e), None) => Err(format!("no floor: the map shows none and the controller touch failed ({e}). Look at the floor while mapping, or turn on controller tracking.")),
        (Ok(()), None) => Err("no floor stored (the controller touch found no room fix)".into()),
    }
}

#[allow(clippy::too_many_arguments)]
fn finish(g: &mut HudGuide, dir: &Path, name: &str, mode: SetupMode, label: &str, centre: P2, yaw: f64, head_y: f64, boundary: Vec<P2>, res: &RelocResult) -> R<RoomSetup> {
    let floor_y = StoredFloor::load(dir).map(|f| f.y).unwrap_or(f64::NAN);
    let mut st = RoomSetup {
        label: label.to_string(),
        mode,
        centre,
        yaw,
        boundary,
        play_area: [0.0; 2],
        head_m: head_y - floor_y,
        when: unix_now(),
    };
    if !setup::inside(&st.boundary, st.centre) {
        // stood outside the traced area: the middle of the boundary instead (forward kept)
        let n = st.boundary.len() as f64;
        st.centre = st.boundary.iter().fold([0.0, 0.0], |a, p| [a[0] + p[0] / n, a[1] + p[1] / n]);
        g.say("Centre", "You stood outside the traced area: the middle of the area is the centre.", None);
        std::thread::sleep(Duration::from_secs(2));
    }
    st.play_area = setup::play_area(&st.boundary_setup());
    if st.play_area[0] < 0.3 || st.play_area[1] < 0.3 {
        return Err(format!("the play area around the centre is too small ({:.1} x {:.1} m): run the setup again", st.play_area[0], st.play_area[1]));
    }
    st.save(dir).map_err(|e| e.to_string())?;
    println!("saved {}: {}", dir.join(setup::FILE).display(), st.describe());
    crate::write_offset(dir, res, Some(res.pid), false, false)?;
    write_owner(res.pid);
    write_status(&status_path(), "ok", name, res.pid, &format!("Room \"{name}\" set up and in place: {}", st.describe()));
    Ok(st)
}

fn cmd_setup_run(mode: SetupMode, mut v: Vec<String>) -> R<()> {
    let dir = map_dir(&mut v)?;
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "home".into());
    let label = opt(&mut v, "--label")?.unwrap_or_else(|| name.clone());
    let no_hud = flag(&mut v, "--no-hud");
    let map_s: f64 = num(&mut v, "--map-seconds", if mode == SetupMode::Fast { 30.0 } else { 60.0 })?;
    crate::done(&v)?;
    let feed = world::feed_now();
    let pid = feed.live_pid().ok_or_else(|| format!("cannot start the room setup: {}", feed.plain()))?;
    if tag_holds(pid) {
        return Err("the AprilTag placed the world in this session: press Clear offset on the Room page first".into());
    }
    if !no_hud {
        // the panel goes to the running Monado, never to the system runtime (SteamVR) (pimax_hud::runtime)
        if let Some(j) = pimax_hud::runtime::use_running_monado() {
            println!("in-headset panel: OpenXR runtime {}", j.display());
        }
    }
    let hud = if no_hud { None } else { start_hud() };
    // the panel must not have disturbed the feed (2026-10-16: it went to SteamVR, whose driver took the ring over)
    if let Err(e) = feed_ok(pid) {
        write_status(&status_path(), "failed", &name, pid, &format!("Room setup stopped: {e}"));
        return Err(e);
    }
    let mut g = HudGuide { hud, t0: Instant::now(), dir: dir.clone(), pid, conv: None, last: (String::new(), Instant::now()) };
    let title = if mode == SetupMode::Fast { "Fast room setup" } else { "Room setup" };
    let r = run_steps(&mut g, mode, &dir, &name, &label, map_s, title);
    match &r {
        Ok(st) => {
            g.say("Room setup done", &format!("Play area {:.1} x {:.1} m. The floor, the centre and forward are saved; they come back at every start.", st.play_area[0], st.play_area[1]), None);
        }
        Err(e) => {
            g.say("Room setup stopped", e, None);
            write_status(&status_path(), "failed", &name, pid, &format!("Room setup stopped: {e}"));
        }
    }
    std::thread::sleep(Duration::from_secs(5));
    if let Some(h) = &g.hud {
        h.request_exit();
        std::thread::sleep(Duration::from_millis(500));
    }
    r.map(|_| ())
}

fn run_steps(g: &mut HudGuide, mode: SetupMode, dir: &Path, name: &str, label: &str, map_s: f64, title: &str) -> R<RoomSetup> {
    let pid = g.pid;
    let map_args = |secs: f64| s(&["--source", "broker", "--seconds", &format!("{secs:.0}"), "--name", name]);
    // 1. the room: recognised (fast, map saved) or mapped (full, or fast without a usable map)
    let mut have_fix = false;
    wait_for_wear(g, 60.0)?;
    check_light(g, title, 30.0);
    feed_ok(pid)?;
    if mode == SetupMode::Fast && dir.join("map.bin").exists() {
        let r = g.during(title, "Stand in the middle of your play space. Look around slowly: left, right, behind you, and at the floor.", 25.0, || {
            crate::cmd_relocalize(s(&["--source", "broker", "--seconds", "25", "--name", name]))
        });
        match r {
            Ok(()) => {
                have_fix = true;
                // learn how the room looks now (other light, moved things): this session's views are added to the
                // map in the recognised room frame, so the next recognition in this light is quicker
                let r = map_with_feedback(g, title, "Room recognised. Keep looking around for a few seconds.", map_args(12.0), 12.0, true);
                if let Err(e) = r {
                    println!("map not updated: {e}");
                }
            }
            Err(e) => {
                // a lost feed is not an unknown room: stop instead of mapping without poses
                feed_ok(pid)?;
                println!("room not recognised ({e}): mapping it anew");
                g.say(title, "This room does not match the saved map: making a new one.", None);
                std::thread::sleep(Duration::from_secs(2));
            }
        }
    }
    if !have_fix {
        let text = if mode == SetupMode::Fast {
            "Stand in the middle of your play space. Look around slowly: left, right, behind you, and at the floor."
        } else {
            "Walk slowly through the room. Look at every wall, the furniture and the floor around you. End where you started."
        };
        backup_map(dir);
        map_with_feedback(g, if mode == SetupMode::Fast { title } else { "1/4 Map the room" }, text, map_args(map_s), map_s + 30.0, false)?;
        save_fix_from_current(dir, pid, "room setup (map built in this session)")?;
    }
    let res = RelocResult::load(&dir.join("last-reloc.toml"))?;
    if res.pid != pid {
        return Err("the room fix belongs to another Monado session".into());
    }
    // 2. the floor
    g.say(if mode == SetupMode::Fast { title } else { "2/4 Floor" }, "Finding the floor...", None);
    let floor_note = ensure_floor(g, dir, name, mode == SetupMode::Full)?;
    println!("{floor_note}");
    let floor_y = StoredFloor::load(dir).map(|f| f.y).map_err(|e| format!("no floor: {e}"))?;
    // 3. boundary (full) and 4. centre / forward
    let boundary_room: Option<Vec<P2>> = if mode == SetupMode::Full {
        g.restart_clock();
        let tr = setup::trace_boundary(g, 240.0)?;
        Some(setup::clean_boundary(&tr, 0.05)?)
    } else {
        None
    };
    g.restart_clock();
    let (centre, yaw, head_y) = setup::confirm_forward(g, 120.0)?;
    let boundary = match boundary_room {
        Some(b) => b,
        None => {
            // the free space around the user from the map's points (furniture, walls), clipped to what the map saw
            let map = RoomMap::load(dir)?;
            let (pts, _) = floor::map_points(&map, 4.5);
            let (obs, ext) = setup::obstacles_from_points(&pts, floor_y, centre, yaw);
            setup::free_area(&obs, ext, 2.5).into_iter().map(|p| setup::to_room(centre, yaw, p)).collect()
        }
    };
    finish(g, dir, name, mode, label, centre, yaw, head_y, boundary, &res)
}

fn cmd_setup_show(mut v: Vec<String>) -> R<()> {
    let dir = map_dir(&mut v)?;
    crate::done(&v)?;
    let st = RoomSetup::load(&dir)?;
    println!("{}: \"{}\" {}", dir.display(), st.label, st.describe());
    println!(
        "centre ({:+.2}, {:+.2}) m, forward {:.1} deg in the map, {} boundary points, head {:.2} m above the floor at setup",
        st.centre[0],
        st.centre[1],
        st.yaw.to_degrees(),
        st.boundary.len(),
        st.head_m
    );
    Ok(())
}

pub fn cmd_setup(mut v: Vec<String>) -> R<()> {
    if v.is_empty() {
        return Err(format!("setup needs fast, full or show:\n{HELP}"));
    }
    match v.remove(0).as_str() {
        "fast" => cmd_setup_run(SetupMode::Fast, v),
        "full" => cmd_setup_run(SetupMode::Full, v),
        "show" => cmd_setup_show(v),
        x => Err(format!("unknown setup command {x:?}\n{HELP}")),
    }
}

/// `auto`: put the saved room in place at a session start.
pub fn cmd_auto(mut v: Vec<String>) -> R<()> {
    let dir = map_dir(&mut v)?;
    let wait: f64 = num(&mut v, "--wait", 120.0)?;
    let status = opt(&mut v, "--status")?.map(PathBuf::from).unwrap_or_else(status_path);
    crate::done(&v)?;
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if !dir.join("map.bin").exists() {
        write_status(&status, "off", &name, 0, &format!("No room \"{name}\" saved yet: run Fast setup on the Room page."));
        return Ok(());
    }
    let t0 = Instant::now();
    // the session's Monado: a fresh head ring (the hook recreates it at every start)
    let pid = loop {
        if let Some(p) = ring_pid() {
            break p;
        }
        if t0.elapsed().as_secs_f64() > wait {
            write_status(&status, "failed", &name, 0, "Room not put in place: Monado runs without the room anchor hook or 6DoF.");
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(1));
    };
    write_status(&status, "searching", &name, pid, &format!("Looking for the room \"{name}\": look around for a few seconds."));
    let mut tries = 0;
    loop {
        if !alive(pid) || ring_owner() != Some(pid) {
            write_status(&status, "off", &name, pid, "The session ended before the room was recognised.");
            return Ok(());
        }
        if tag_holds(pid) {
            write_status(&status, "skipped", &name, pid, "The AprilTag placed the world in this session: the room map leaves it alone.");
            return Ok(());
        }
        tries += 1;
        match crate::cmd_relocalize(s(&["--source", "broker", "--seconds", "15", "--name", &name, "--apply"])) {
            Ok(()) => {
                write_owner(pid);
                let extra = match RoomSetup::load(&dir) {
                    Ok(st) => format!(" (floor, centre and forward; {})", st.describe()),
                    Err(_) => " (floor only: no room setup yet, run Fast setup for the play area and forward)".into(),
                };
                write_status(&status, "ok", &name, pid, &format!("Room \"{name}\" in place after {:.0} s{extra}.", t0.elapsed().as_secs_f64()));
                return Ok(());
            }
            Err(e) => {
                println!("try {tries}: {e}");
                if t0.elapsed().as_secs_f64() > wait {
                    write_status(
                        &status,
                        "failed",
                        &name,
                        pid,
                        &format!("Room \"{name}\" not recognised: the world starts where the headset is. Look around the room and press Recognise room, or run Fast setup again (lights, furniture moved?)."),
                    );
                    return Ok(());
                }
                write_status(&status, "searching", &name, pid, &format!("Still looking for the room \"{name}\" (try {tries}): look around slowly."));
                std::thread::sleep(Duration::from_secs(3));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_and_files() {
        assert_eq!(wrap("one two three four", 9), vec!["one two", "three", "four"]);
        assert_eq!(wrap("", 9), Vec::<String>::new());
        assert_eq!(stage_bounds_text(42, [3.04, 2.0]), "pimax-stage-bounds 1\npid 42\nsize 3.040 2.000\n");
    }

    #[test]
    fn conversion_from_stage_to_the_room() {
        // room_from_slam A, offset O (with floor shift), origin height 1.6: a slam point seen in STAGE comes back
        let a = pimax_room::session::Alignment { yaw: 0.4, t: [1.0, 0.2, -0.5] };
        let o = pimax_room::session::Alignment { yaw: -0.3, t: [0.1, -1.4, 0.7] };
        let c = Conv { oh: 1.6, inv_off: o.pose().inverse(), a: a.pose() };
        let slam = [0.3, 0.1, -0.8];
        let st = o.pose().apply(slam);
        let stage = [st[0] as f32, (st[1] + 1.6) as f32, st[2] as f32];
        let room = c.pos(stage);
        let want = a.pose().apply(slam);
        assert!(geom::norm(geom::sub(room, want)) < 1e-5, "{room:?} {want:?}");
    }
}
