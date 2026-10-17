//! VR screens as Hyprland outputs (user design, 2026-10-15): `omarchyvr desk start --screens N` (1 to 4, default 4).
//!
//! * **Displays before VR.** Each extra VR screen is a Hyprland *headless* output (`PIMAX-LEFT`, `PIMAX-RIGHT`,
//!   `PIMAX-UP`, `PIMAX-UPLEFT`, `PIMAX-UPRIGHT`), created by `desk start` BEFORE Monado and WayVR start, and removed
//!   by `desk stop` after Monado has gone. Nothing creates or removes an output while a monado-service runs (that can disturb the leased DP-1):
//!   [`prepare`] and [`remove_all`] refuse then. Runtime calls only (`~/.config/hypr` is never edited):
//!   `hyprctl eval 'hl.monitor({ output = "PIMAX-LEFT", mode = "1920x1080@60", position = "-1920x0", scale = 1 })'`
//!   (Hyprland 0.56 Lua: a monitor rule for this run of Hyprland), then `hyprctl output create headless PIMAX-LEFT`.
//!   The outputs have the main desktop's size and sit left of, right of and above it in the Hyprland layout, so
//!   Hyprland's own "focus left/right/up" crosses to them too.
//! * **Idempotent.** Outputs that already exist with the right size are kept; stale ones (another count or size, a
//!   crashed session) are removed first; missing ones are created. A failure never stops the session: it starts with
//!   the outputs that exist (one screen at worst) and the reason is in `<session dir>/screens.txt` for the app.
//! * **Layout in VR.** WayVR names each screen after its output. `wayvrctl desk-screens` (WayVR eye-gaze branch) puts
//!   the main screen in the centre and the others left, right and above it, and shows or hides each.
//! * **Screens on demand.** [`watch`] (`omarchyvr desk screens watch`, a leaf process of the session) follows Hyprland's
//!   event socket: a VR screen is shown while its workspace has an app or it has the focus ("new window" focuses the
//!   next free screen), and hidden (still running) otherwise. Presets set a limit and pinned screens in the state file
//!   (`$XDG_RUNTIME_DIR/omarchyvr-screens.state`): cinema and work-1 show the main screen only, work-3 the main screen plus
//!   left and right, assistant and none every screen on demand. Voice ("new window", "move this left", "go to the
//!   left screen") lifts the limit again. The focused screen always shows. A preset never closes, kills or moves
//!   an app: a screen it hides keeps its apps.
//! * **Layout inside a screen** ([`layout_plan`], run by the watcher whenever apps open or move): one app alone fills
//!   its screen; apps of the same kind share a screen, tiled; an app of another kind goes to its own screen (the
//!   one with its kind, else the next free one). Kinds: browser, terminal, the Pimax app, every other class its own.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Output name prefix of the VR screens (never a real connector name).
pub const PREFIX: &str = "PIMAX-";
/// Most VR screens (main + left + right + up + up-left + up-right).
pub const MAX: u8 = 6;
/// `--screens` default: every screen, shown on demand.
pub const DEFAULT: u8 = 4;
/// The report of the last `desk start` in the session dir (read by the app's readiness line).
pub const REPORT_FILE: &str = "screens.txt";
/// pid file stem / ProcSpec name of the watcher.
pub const PROC_NAME: &str = "screens";

/// Where a screen sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Slot {
    Main,
    Left,
    Right,
    Up,
    /// Above the left screen.
    UpLeft,
    /// Above the right screen.
    UpRight,
}

/// The extra screens in the order they are filled ("new window": the next free one).
pub const EXTRA: [Slot; 5] = [Slot::Left, Slot::Right, Slot::Up, Slot::UpLeft, Slot::UpRight];

impl Slot {
    /// `main`, `left`, `right`, `up`, `up-left`, `up-right` (the `wayvrctl desk-screens` slot names).
    pub fn key(self) -> &'static str {
        match self {
            Slot::Main => "main",
            Slot::Left => "left",
            Slot::Right => "right",
            Slot::Up => "up",
            Slot::UpLeft => "up-left",
            Slot::UpRight => "up-right",
        }
    }

    pub fn parse(s: &str) -> Option<Slot> {
        match s.trim().to_ascii_lowercase().as_str() {
            "main" | "middle" | "center" | "centre" => Some(Slot::Main),
            "left" => Some(Slot::Left),
            "right" => Some(Slot::Right),
            "up" | "top" | "upper" | "above" => Some(Slot::Up),
            "up-left" | "upleft" | "up_left" | "top-left" => Some(Slot::UpLeft),
            "up-right" | "upright" | "up_right" | "top-right" => Some(Slot::UpRight),
            _ => None,
        }
    }

    /// The Hyprland output of an extra screen (None for the main one, which is the existing desktop output).
    pub fn output(self) -> Option<&'static str> {
        match self {
            Slot::Main => None,
            Slot::Left => Some("PIMAX-LEFT"),
            Slot::Right => Some("PIMAX-RIGHT"),
            Slot::Up => Some("PIMAX-UP"),
            Slot::UpLeft => Some("PIMAX-UPLEFT"),
            Slot::UpRight => Some("PIMAX-UPRIGHT"),
        }
    }

    /// The slot of a VR output name.
    pub fn of_output(name: &str) -> Option<Slot> {
        EXTRA.into_iter().find(|s| s.output() == Some(name))
    }
}

/// The extra slots of `count` screens (count 1 = none).
pub fn extra_slots(count: u8) -> &'static [Slot] {
    &EXTRA[..(count.clamp(1, MAX) - 1) as usize]
}

/// Parse `--screens N`.
pub fn parse_count(s: &str) -> Result<u8, String> {
    match s.trim().parse::<u8>() {
        Ok(n) if (1..=MAX).contains(&n) => Ok(n),
        _ => Err(format!("--screens must be 1 to {MAX}, not {s}")),
    }
}

// --------------------------------------------------------------------------------------------------- Hyprland data

/// A monitor as `hyprctl -j monitors all` reports it (the fields used here).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Mon {
    pub name: String,
    pub width: i64,
    pub height: i64,
    pub x: i64,
    pub y: i64,
    pub focused: bool,
    pub disabled: bool,
    /// Mirrors another output (`mirrorOf` is not "none").
    pub mirror: bool,
    pub active_ws: i64,
}

pub fn parse_monitors(json: &str) -> Result<Vec<Mon>, String> {
    let v: serde_json::Value = serde_json::from_str(json.trim()).map_err(|e| format!("hyprctl monitors: {e}"))?;
    let arr = v.as_array().ok_or("hyprctl monitors: not a list")?;
    Ok(arr
        .iter()
        .filter_map(|m| {
            let i = |k: &str| m.get(k).and_then(serde_json::Value::as_i64).unwrap_or(0);
            Some(Mon {
                name: m.get("name")?.as_str()?.to_string(),
                width: i("width"),
                height: i("height"),
                x: i("x"),
                y: i("y"),
                focused: m.get("focused").and_then(serde_json::Value::as_bool).unwrap_or(false),
                disabled: m.get("disabled").and_then(serde_json::Value::as_bool).unwrap_or(false),
                mirror: m.get("mirrorOf").and_then(serde_json::Value::as_str).is_some_and(|s| s != "none" && !s.is_empty()),
                active_ws: m.pointer("/activeWorkspace/id").and_then(serde_json::Value::as_i64).unwrap_or(0),
            })
        })
        .collect())
}

/// Workspace id -> number of apps, from `hyprctl -j clients`.
pub fn parse_client_counts(json: &str) -> Result<HashMap<i64, usize>, String> {
    let v: serde_json::Value = serde_json::from_str(json.trim()).map_err(|e| format!("hyprctl clients: {e}"))?;
    let mut m = HashMap::new();
    for c in v.as_array().ok_or("hyprctl clients: not a list")? {
        if let Some(ws) = c.pointer("/workspace/id").and_then(serde_json::Value::as_i64) {
            if ws > 0 {
                *m.entry(ws).or_insert(0) += 1;
            }
        }
    }
    Ok(m)
}

/// Is this one of our VR outputs (or a stale one with our prefix)?
pub fn is_ours(name: &str) -> bool {
    name.starts_with(PREFIX)
}

/// The main desktop output: enabled, not mirrored, not ours, with a size; the one at 0,0 first, then the focused one.
pub fn main_output(mons: &[Mon]) -> Option<&Mon> {
    let ok = |m: &&Mon| !m.disabled && !m.mirror && !is_ours(&m.name) && m.width > 0 && m.height > 0;
    mons.iter().filter(ok).find(|m| m.x == 0 && m.y == 0).or_else(|| mons.iter().filter(ok).find(|m| m.focused)).or_else(|| mons.iter().find(ok))
}

/// One output to have.
#[derive(Debug, Clone, PartialEq)]
pub struct Want {
    pub slot: Slot,
    pub name: &'static str,
    pub width: i64,
    pub height: i64,
    pub x: i64,
    pub y: i64,
}

impl Want {
    /// The runtime monitor rule (Lua for `hyprctl eval`).
    pub fn rule(&self) -> String {
        format!(
            "hl.monitor({{ output = \"{}\", mode = \"{}x{}@60\", position = \"{}x{}\", scale = 1 }})",
            self.name, self.width, self.height, self.x, self.y
        )
    }
}

/// The outputs `count` screens need, with the main output's size, around it.
pub fn wanted(count: u8, main: &Mon) -> Vec<Want> {
    extra_slots(count)
        .iter()
        .map(|&slot| {
            let (x, y) = match slot {
                Slot::Left => (main.x - main.width, main.y),
                Slot::Right => (main.x + main.width, main.y),
                Slot::UpLeft => (main.x - main.width, main.y - main.height),
                Slot::UpRight => (main.x + main.width, main.y - main.height),
                _ => (main.x, main.y - main.height),
            };
            Want { slot, name: slot.output().unwrap_or_default(), width: main.width, height: main.height, x, y }
        })
        .collect()
}

// ------------------------------------------------------------------------------------------------ hyprctl access

/// `hyprctl` (faked in tests).
pub trait Hypr {
    /// `hyprctl ARGS` -> stdout.
    fn run(&mut self, args: &[&str]) -> Result<String, String>;
    fn sleep(&mut self, d: Duration);
}

/// The real `hyprctl`.
pub struct RealHypr;

impl Hypr for RealHypr {
    fn run(&mut self, args: &[&str]) -> Result<String, String> {
        // PIMAX_HYPRCTL: a fake for the CLI tests (they must never reach the real compositor)
        let prog = std::env::var("PIMAX_HYPRCTL").ok().filter(|p| !p.is_empty()).unwrap_or_else(|| "hyprctl".into());
        run_timeout(&prog, args, Duration::from_secs(5))
    }
    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
}

/// Run a short command with a timeout; stdout on success.
pub fn run_timeout(prog: &str, args: &[&str], limit: Duration) -> Result<String, String> {
    use std::process::{Command, Stdio};
    let mut child = Command::new(prog)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{prog}: {e}"))?;
    let t0 = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if t0.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{prog} timed out"));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => return Err(e.to_string()),
        }
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!("{prog} {}: {}", args.first().unwrap_or(&""), String::from_utf8_lossy(&out.stderr).trim()))
    }
}

fn hypr_ok(out: &str) -> Result<(), String> {
    let l = out.to_lowercase();
    if l.contains("error") || l.contains("invalid") || l.contains("unknown") || l.contains("failed") {
        Err(out.trim().to_string())
    } else {
        Ok(())
    }
}

fn monitors(h: &mut dyn Hypr) -> Result<Vec<Mon>, String> {
    parse_monitors(&h.run(&["-j", "monitors", "all"])?)
}

/// What [`prepare`] / [`remove_all`] did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    /// The main desktop output.
    pub main: Option<String>,
    /// Our outputs that exist afterwards, in slot order.
    pub outputs: Vec<(Slot, String)>,
    pub created: Vec<String>,
    pub kept: Vec<String>,
    pub removed: Vec<String>,
    /// Floating apps put back on their screen after a re-created output (see [`stale_floating`]).
    pub replaced: Vec<String>,
    /// Why fewer screens than asked (the session still starts).
    pub error: Option<String>,
    /// Dry run: what would be done.
    pub steps: Vec<String>,
}

impl Report {
    /// Screens in VR (main + ours).
    pub fn screens(&self) -> usize {
        usize::from(self.main.is_some()) + self.outputs.len()
    }

    /// Lines for the session dir file and the start output.
    pub fn lines(&self) -> Vec<String> {
        let mut v = vec![format!(
            "screens: {} ({}{})",
            self.screens(),
            self.main.as_deref().unwrap_or("no main output"),
            self.outputs.iter().map(|(s, n)| format!(", {} = {n}", s.key())).collect::<String>()
        )];
        for (what, list) in [("created", &self.created), ("kept", &self.kept), ("removed", &self.removed), ("apps put back on their screen", &self.replaced)] {
            if !list.is_empty() {
                v.push(format!("{what}: {}", list.join(", ")));
            }
        }
        if let Some(e) = &self.error {
            v.push(format!("error: {e}"));
        }
        v
    }

    /// Parse [`Report::lines`] back (the app's readiness line).
    pub fn parse(text: &str) -> Report {
        let mut r = Report::default();
        for l in text.lines() {
            if let Some(rest) = l.strip_prefix("screens: ") {
                let inner = rest.split_once('(').map(|(_, b)| b.trim_end_matches(')')).unwrap_or("");
                let mut parts = inner.split(", ");
                r.main = parts.next().filter(|m| !m.is_empty() && *m != "no main output").map(str::to_string);
                for p in parts {
                    if let Some((k, n)) = p.split_once(" = ") {
                        if let Some(s) = Slot::parse(k) {
                            r.outputs.push((s, n.to_string()));
                        }
                    }
                }
            } else if let Some(e) = l.strip_prefix("error: ") {
                r.error = Some(e.to_string());
            } else if let Some(x) = l.strip_prefix("created: ") {
                r.created = x.split(", ").map(str::to_string).collect();
            }
        }
        r
    }
}

/// Make the outputs of `count` screens exist (idempotent): stale `PIMAX-*` outputs are removed, wanted ones that
/// exist with the right size kept, missing ones created. `session_running`: a monado-service runs, nothing changes.
/// Never fails: the error is in the report and the session starts with what exists.
pub fn prepare(count: u8, h: &mut dyn Hypr, dry_run: bool, session_running: bool) -> Report {
    let mut r = Report::default();
    let mons = match monitors(h) {
        Ok(m) => m,
        Err(e) => {
            r.error = Some(format!("cannot read the Hyprland outputs ({e}): one screen"));
            return r;
        }
    };
    let Some(main) = main_output(&mons).cloned() else {
        r.error = Some("no desktop output found in Hyprland: one screen".into());
        return r;
    };
    r.main = Some(main.name.clone());
    let want = wanted(count, &main);
    let ours: Vec<&Mon> = mons.iter().filter(|m| is_ours(&m.name)).collect();
    let fits = |m: &Mon| want.iter().any(|w| w.name == m.name && w.width == m.width && w.height == m.height);
    if session_running {
        r.outputs = present(&mons, &want);
        let stale: Vec<&str> = ours.iter().filter(|m| !fits(m)).map(|m| m.name.as_str()).collect();
        if r.outputs.len() + 1 < count as usize || !stale.is_empty() {
            r.error = Some("a VR session runs: the screens are only changed between sessions".into());
        }
        return r;
    }
    // 1. stale outputs: not wanted, or the wrong size
    for m in ours.iter().filter(|m| !fits(m)) {
        if dry_run {
            r.steps.push(format!("would remove stale output {} (hyprctl output remove {})", m.name, m.name));
            continue;
        }
        match h.run(&["output", "remove", &m.name]).and_then(|o| hypr_ok(&o)) {
            Ok(()) => r.removed.push(m.name.clone()),
            Err(e) => add_err(&mut r, format!("cannot remove stale output {}: {e}", m.name)),
        }
    }
    // 2. missing outputs: the monitor rule first (so the new output gets its place), then the output
    let mut created_any = false;
    for w in &want {
        if ours.iter().any(|m| m.name == w.name && fits(m)) {
            r.kept.push(w.name.to_string());
            continue;
        }
        if dry_run {
            r.steps.push(format!("would run hyprctl eval '{}'", w.rule()));
            r.steps.push(format!("would run hyprctl output create headless {}", w.name));
            continue;
        }
        if let Err(e) = h.run(&["eval", &w.rule()]).and_then(|o| hypr_ok(&o)) {
            add_err(&mut r, format!("monitor rule for {}: {e}", w.name));
        }
        match h.run(&["output", "create", "headless", w.name]).and_then(|o| hypr_ok(&o)) {
            Ok(()) => created_any = true,
            Err(e) => add_err(&mut r, format!("cannot create {}: {e}", w.name)),
        }
    }
    if dry_run {
        r.outputs = want.iter().map(|w| (w.slot, w.name.to_string())).collect();
        return r;
    }
    // 3. wait until they are there (Hyprland adds an output within a few frames)
    let mut now = mons;
    for i in 0..15 {
        if i > 0 || created_any {
            h.sleep(Duration::from_millis(200));
            now = monitors(h).unwrap_or_default();
        }
        if present(&now, &want).len() == want.len() {
            break;
        }
    }
    r.outputs = present(&now, &want);
    for w in &want {
        match now.iter().find(|m| m.name == w.name) {
            None => add_err(&mut r, format!("{} did not appear", w.name)),
            Some(m) => {
                if r.kept.iter().all(|k| k != w.name) {
                    r.created.push(w.name.to_string());
                }
                if (m.x, m.y) != (w.x, w.y) {
                    // placed elsewhere (a config reload dropped the rule?): one more try; a wrong place only
                    // changes which way "focus left/right" crosses, never what the VR shows
                    let _ = h.run(&["eval", &w.rule()]);
                }
            }
        }
    }
    if r.outputs.len() < want.len() {
        let e = r.error.take().unwrap_or_default();
        r.error = Some(format!("{} of {} screens ({e})", r.screens(), count));
    }
    // 4. self-check: every floating app on a VR screen must lie on that screen (lesson 67: a re-created output gets
    //    its old workspaces back, but their floating apps keep the main screen's coordinates and are never drawn)
    if !r.outputs.is_empty() {
        rehome_floating(h, &main, &mut r);
    }
    r
}

/// A floating app that lies outside the VR output its workspace is on, and how to put it back.
#[derive(Debug, Clone, PartialEq)]
pub struct Stale {
    pub address: String,
    pub output: String,
    /// `Some((dx, dy))`: shift by the output's offset from the main screen (the app keeps its place on the screen);
    /// `None`: centre it on its output (it was somewhere else entirely).
    pub shift: Option<(i64, i64)>,
}

impl Stale {
    /// The Hyprland 0.56 dispatch (by address: no focus change, nothing else moves).
    pub fn dispatch(&self) -> String {
        match self.shift {
            Some((dx, dy)) => format!("hl.dsp.window.move({{ x = {dx}, y = {dy}, relative = true, window = \"address:{}\" }})", self.address),
            None => format!("hl.dsp.window.center({{ window = \"address:{}\" }})", self.address),
        }
    }
}

/// Floating apps on our outputs' workspaces whose box does not touch their output (Hyprland 0.56 bug, lesson 67:
/// when an output is removed its workspaces go to the main screen and floating apps are shifted into the main
/// screen's area; when an output of that name is created again the workspaces come back to it, but the shift back
/// is computed before the new output has its position, so the apps keep main-screen coordinates and the new output
/// draws only its wallpaper). Tiled and maximized apps are laid out again by Hyprland and are fine.
/// `ws_mon`: workspace id -> output name (`hyprctl -j workspaces`); `clients`: `hyprctl -j clients`.
pub fn stale_floating(mons: &[Mon], main: &Mon, ws_mon: &HashMap<i64, String>, clients: &serde_json::Value) -> Vec<Stale> {
    let num = |c: &serde_json::Value, p: &str| c.pointer(p).and_then(serde_json::Value::as_i64);
    let overlaps = |(x, y, w, h): (i64, i64, i64, i64), m: &Mon| x < m.x + m.width && x + w > m.x && y < m.y + m.height && y + h > m.y;
    let mut v = Vec::new();
    for c in clients.as_array().map(Vec::as_slice).unwrap_or_default() {
        let flag = |k: &str| c.get(k).and_then(serde_json::Value::as_bool).unwrap_or(false);
        let Some(address) = c.get("address").and_then(serde_json::Value::as_str).filter(|a| address_ok(a)) else { continue };
        if !flag("floating") || flag("pinned") || !c.get("mapped").and_then(serde_json::Value::as_bool).unwrap_or(true) {
            continue;
        }
        if num(c, "/fullscreen").unwrap_or(0) != 0 {
            continue;
        }
        let Some(ws) = num(c, "/workspace/id").filter(|&w| w > 0) else { continue };
        let Some(m) = ws_mon.get(&ws).filter(|n| is_ours(n)).and_then(|n| mons.iter().find(|m| &m.name == n && !m.disabled)) else { continue };
        let (Some(x), Some(y), Some(w), Some(hgt)) = (num(c, "/at/0"), num(c, "/at/1"), num(c, "/size/0"), num(c, "/size/1")) else { continue };
        if m.width <= 0 || m.height <= 0 || overlaps((x, y, w.max(1), hgt.max(1)), m) {
            continue;
        }
        // where it was left: inside the main screen's area -> the same place on its own output
        let on_main = x >= main.x && x < main.x + main.width && y >= main.y && y < main.y + main.height;
        let shift = on_main.then(|| (m.x - main.x, m.y - main.y));
        v.push(Stale { address: address.to_string(), output: m.name.clone(), shift });
    }
    v
}

fn workspace_monitors(json: &str) -> Result<HashMap<i64, String>, String> {
    let v: serde_json::Value = serde_json::from_str(json.trim()).map_err(|e| format!("hyprctl workspaces: {e}"))?;
    Ok(v.as_array()
        .ok_or("hyprctl workspaces: not a list")?
        .iter()
        .filter_map(|w| Some((w.get("id")?.as_i64()?, w.get("monitor")?.as_str()?.to_string())))
        .collect())
}

/// Read the floating apps on our outputs (monitors, workspaces, clients) and the stale ones among them.
fn read_stale(h: &mut dyn Hypr, main: &Mon) -> Result<Vec<Stale>, String> {
    let mons = monitors(h)?;
    let ws_mon = workspace_monitors(&h.run(&["-j", "workspaces"])?)?;
    let clients: serde_json::Value = serde_json::from_str(h.run(&["-j", "clients"])?.trim()).map_err(|e| format!("hyprctl clients: {e}"))?;
    Ok(stale_floating(&mons, main, &ws_mon, &clients))
}

/// Put stale floating apps back on their VR output, then check again (the self-check after the outputs exist).
fn rehome_floating(h: &mut dyn Hypr, main: &Mon, r: &mut Report) {
    let stale = match read_stale(h, main) {
        Ok(s) => s,
        Err(e) => return add_err(r, format!("cannot check the apps on the VR screens: {e}")),
    };
    if stale.is_empty() {
        return;
    }
    for s in &stale {
        match h.run(&["dispatch", &s.dispatch()]).and_then(|o| hypr_ok(&o)) {
            Ok(()) => r.replaced.push(format!("{} ({})", s.address, s.output)),
            Err(e) => add_err(r, format!("cannot put app {} back on {}: {e}", s.address, s.output)),
        }
    }
    h.sleep(Duration::from_millis(100));
    match read_stale(h, main) {
        Ok(left) if !left.is_empty() => add_err(
            r,
            format!(
                "{} app(s) still outside their VR screen (not drawn): {}",
                left.len(),
                left.iter().map(|s| format!("{} on {}", s.address, s.output)).collect::<Vec<_>>().join(", ")
            ),
        ),
        Ok(_) => {}
        Err(e) => add_err(r, format!("cannot check the apps on the VR screens: {e}")),
    }
}

fn add_err(r: &mut Report, e: String) {
    r.error = Some(match r.error.take() {
        Some(p) => format!("{p}; {e}"),
        None => e,
    });
}

fn present(mons: &[Mon], want: &[Want]) -> Vec<(Slot, String)> {
    want.iter()
        .filter(|w| mons.iter().any(|m| m.name == w.name && !m.disabled))
        .map(|w| (w.slot, w.name.to_string()))
        .collect()
}

/// Remove every `PIMAX-*` output (after a session). Refuses while a monado-service runs.
pub fn remove_all(h: &mut dyn Hypr, dry_run: bool, session_running: bool) -> Report {
    let mut r = Report::default();
    let mons = match monitors(h) {
        Ok(m) => m,
        Err(e) => {
            r.error = Some(format!("cannot read the Hyprland outputs: {e}"));
            return r;
        }
    };
    r.main = main_output(&mons).map(|m| m.name.clone());
    let ours: Vec<String> = mons.iter().filter(|m| is_ours(&m.name)).map(|m| m.name.clone()).collect();
    if ours.is_empty() {
        return r;
    }
    if session_running {
        r.outputs = ours.iter().filter_map(|n| Slot::of_output(n).map(|s| (s, n.clone()))).collect();
        r.error = Some("a VR session runs: the VR screens are removed after it".into());
        return r;
    }
    for n in ours {
        if dry_run {
            r.steps.push(format!("would run hyprctl output remove {n}"));
            continue;
        }
        match h.run(&["output", "remove", &n]).and_then(|o| hypr_ok(&o)) {
            Ok(()) => r.removed.push(n),
            Err(e) => add_err(&mut r, format!("cannot remove {n}: {e}")),
        }
    }
    r
}

// ------------------------------------------------------------------------------------------ what VR shows (state)

/// `$XDG_RUNTIME_DIR/omarchyvr-screens.state`: written by presets and voice, read by the watcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShowState {
    /// Most screens shown, the main one included (1 = only the main screen).
    pub limit: u8,
    /// Shown even without an app (work-3: left and right).
    pub pinned: Vec<Slot>,
    /// Apps of different kinds never share a screen ([`layout_plan`]). OFF by default (user rule 2026-10-17: screens
    /// and apps move only at command time; the app's "Auto-arrange apps" switch turns it on for a session). On after
    /// "put everything in its own window", off after "put everything back together".
    pub sort: bool,
    /// Hidden by voice ("hide screen 3", "close everything"): off even with apps or the focus on it. The apps on it
    /// stay open. `show` / `more` clear it, `add SLOT` takes that slot out.
    pub hidden: Vec<Slot>,
}

impl Default for ShowState {
    fn default() -> Self {
        ShowState { limit: MAX, pinned: Vec::new(), sort: false, hidden: Vec::new() }
    }
}

impl ShowState {
    pub fn parse(text: &str) -> ShowState {
        let mut s = ShowState::default();
        for l in text.lines() {
            let Some((k, v)) = l.split_once('=') else { continue };
            match k.trim() {
                "limit" => s.limit = v.trim().parse::<u8>().ok().filter(|n| (1..=MAX).contains(n)).unwrap_or(MAX),
                "pinned" => s.pinned = v.split(',').filter_map(Slot::parse).filter(|s| *s != Slot::Main).collect(),
                "sort" => s.sort = matches!(v.trim(), "on" | "true" | "1" | "yes"),
                "hidden" => s.hidden = v.split(',').filter_map(Slot::parse).filter(|s| *s != Slot::Main).collect(),
                _ => {}
            }
        }
        s
    }

    pub fn render(&self) -> String {
        format!(
            "# VR screens shown (omarchyvr desk / omarchyvr-voice; read by `omarchyvr desk screens watch`)\nlimit = {}\npinned = {}\nsort = {}\nhidden = {}\n",
            self.limit,
            self.pinned.iter().map(|s| s.key()).collect::<Vec<_>>().join(","),
            if self.sort { "on" } else { "off" },
            self.hidden.iter().map(|s| s.key()).collect::<Vec<_>>().join(",")
        )
    }

    /// The state a preset sets: (limit, pinned). A preset only changes what is shown: a live switch keeps `sort`
    /// ([`ShowState::switch_to`]).
    pub fn for_preset(p: super::preset::Preset) -> ShowState {
        use super::preset::Preset;
        match p {
            Preset::Cinema | Preset::Work1 => ShowState { limit: 1, pinned: Vec::new(), sort: false, hidden: Vec::new() },
            Preset::Work3 => ShowState { limit: 3, pinned: vec![Slot::Left, Slot::Right], sort: false, hidden: Vec::new() },
            Preset::Assistant => ShowState::default(),
        }
    }

    /// A live preset switch from this state: the preset's limit and pinned screens, the sort setting kept.
    pub fn switch_to(&self, p: super::preset::Preset) -> ShowState {
        ShowState { sort: self.sort, ..ShowState::for_preset(p) }
    }
}

pub fn state_path(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    env("XDG_RUNTIME_DIR").filter(|p| !p.is_empty()).map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("omarchyvr-screens.state")
}

pub fn read_state(p: &Path) -> ShowState {
    std::fs::read_to_string(p).map(|t| ShowState::parse(&t)).unwrap_or_default()
}

/// Write atomically (temp + rename): the watcher never reads half a file.
pub fn write_state(p: &Path, s: &ShowState) -> std::io::Result<()> {
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, s.render())?;
    std::fs::rename(tmp, p)
}

/// Which screens VR shows now: the main one always; an extra one when pinned or when its workspace has an app, at
/// most `limit` in all (slot order); and the focused screen always, even over the limit (you never type into a
/// screen you cannot see: an app that opens there, or "go to the left screen", shows it). A hidden screen keeps its
/// apps. Returns (slot, output, shown) for every screen present.
pub fn visibility(mons: &[Mon], apps: &HashMap<i64, usize>, st: &ShowState) -> Vec<(Slot, String, bool)> {
    let Some(main) = main_output(mons) else { return Vec::new() };
    let mut v = vec![(Slot::Main, main.name.clone(), true)];
    let mut shown = 1u8;
    for slot in EXTRA {
        let Some(m) = mons.iter().find(|m| Some(m.name.as_str()) == slot.output() && !m.disabled) else { continue };
        let busy = apps.get(&m.active_ws).copied().unwrap_or(0) > 0;
        let want = st.pinned.contains(&slot) || busy || m.focused;
        // hidden by voice: off even with the focus on it (its apps stay open)
        let on = !st.hidden.contains(&slot) && ((want && shown < st.limit) || m.focused);
        if on {
            shown += 1;
        }
        v.push((slot, m.name.clone(), on));
    }
    v
}

/// `wayvrctl desk-screens` arguments.
pub fn desk_screens_args(vis: &[(Slot, String, bool)], place: Option<(f32, f32, f32)>, hide_others: bool) -> Vec<String> {
    let mut a = vec!["desk-screens".to_string()];
    if let Some((width, distance, curvature)) = place {
        a.push("--place".into());
        a.extend(["--width".into(), format!("{width}"), "--distance".into(), format!("{distance}"), "--curvature".into(), format!("{curvature}")]);
    }
    if hide_others {
        a.push("--hide-others".into());
    }
    for (slot, name, on) in vis {
        a.push(format!("{}={name}{}", slot.key(), if *on { "" } else { ":hide" }));
    }
    a
}

/// Our outputs that exist now (slot order).
pub fn vr_outputs(mons: &[Mon]) -> Vec<(Slot, String)> {
    EXTRA
        .into_iter()
        .filter_map(|s| mons.iter().find(|m| Some(m.name.as_str()) == s.output() && !m.disabled).map(|m| (s, m.name.clone())))
        .collect()
}

/// The visibility a preset gives the screens present now (None: no VR outputs of ours, or hyprctl failed).
pub fn preset_visibility(p: super::preset::Preset, h: &mut dyn Hypr) -> Option<Vec<(Slot, String, bool)>> {
    let mons = monitors(h).ok()?;
    if vr_outputs(&mons).is_empty() {
        return None;
    }
    let apps = h.run(&["-j", "clients"]).ok().and_then(|j| parse_client_counts(&j).ok()).unwrap_or_default();
    Some(visibility(&mons, &apps, &ShowState::for_preset(p)))
}

// ------------------------------------------------------------------- `omarchyvr desk screens show|more|add` (voice)

/// What the voice (or anyone) asks of the shown screens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShowCmd {
    /// `show N`: N screens (the main one + the first N-1 extras).
    Show(u32),
    /// `more K`: K more than are shown now.
    More(u32),
    /// `add SLOT`: that one screen as well.
    Add(Slot),
    /// `hide SLOT[,SLOT..]` (screen numbers 2-6 or slot names; `others` = every extra screen): those screens no
    /// longer show. Never closes or moves an app: its apps stay open on the hidden screen.
    Hide(Vec<Slot>),
}

/// A screen by number (docs/SCREENS.md: 1 main, 2 left, 3 right, 4 up, 5 up-left, 6 up-right) or slot name.
pub fn slot_of(s: &str) -> Option<Slot> {
    match s.trim().parse::<usize>() {
        Ok(1) => Some(Slot::Main),
        Ok(n) if (2..=MAX as usize).contains(&n) => EXTRA.get(n - 2).copied(),
        Ok(_) => None,
        Err(_) => Slot::parse(s),
    }
}

/// The screen number of a slot (1 main .. 6 up-right).
pub fn number_of(s: Slot) -> usize {
    EXTRA.iter().position(|x| *x == s).map(|i| i + 2).unwrap_or(1)
}

/// Parse `show N` / `more K` / `add SLOT`.
pub fn parse_show(sub: &str, arg: Option<&str>) -> Result<ShowCmd, String> {
    let arg = arg.map(str::trim).filter(|a| !a.is_empty());
    let num = |what: &str| -> Result<u32, String> {
        arg.and_then(|a| a.parse::<u32>().ok()).ok_or_else(|| format!("desk screens {what}: a number is needed, e.g. `desk screens {what} 3`"))
    };
    match sub {
        "show" => Ok(ShowCmd::Show(num("show")?)),
        "more" => Ok(ShowCmd::More(num("more")?)),
        "add" => {
            let a = arg.ok_or("desk screens add: a screen is needed: left, right, up, up-left or up-right")?;
            match Slot::parse(a) {
                Some(Slot::Main) => Err("The main screen is always shown".into()),
                Some(s) => Ok(ShowCmd::Add(s)),
                None => Err(format!("No screen called {a}: use left, right, up, up-left or up-right")),
            }
        }
        "hide" => {
            let a = arg.ok_or("desk screens hide: which screens? e.g. `desk screens hide 3,4` or `desk screens hide others`")?;
            if matches!(a, "others" | "all" | "everything") {
                return Ok(ShowCmd::Hide(EXTRA.to_vec()));
            }
            let mut v = Vec::new();
            for x in a.split([',', ' ']).filter(|x| !x.trim().is_empty()) {
                match slot_of(x) {
                    Some(Slot::Main) => return Err("The main screen is always shown".into()),
                    Some(s) if !v.contains(&s) => v.push(s),
                    Some(_) => {}
                    None => return Err(format!("No screen {x}: use 2 to {MAX} or left, right, up, up-left, up-right")),
                }
            }
            if v.is_empty() {
                return Err("desk screens hide: which screens?".into());
            }
            Ok(ShowCmd::Hide(v))
        }
        other => Err(format!("desk screens: unknown command {other}")),
    }
}

/// What a [`ShowCmd`] comes to (pure; [`plan_show`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShowPlan {
    /// The state to write (the `sort` setting is kept).
    pub state: ShowState,
    /// Outputs the session needs for this: the Screens setting is raised to at least this (never lowered).
    pub setting: u8,
    /// Needed screens that have no output (the session started with fewer): cannot be made while WayVR runs.
    pub missing: Vec<Slot>,
    /// The one plain line for the headset.
    pub message: String,
}

fn plural(n: usize) -> String {
    format!("{n} screen{}", if n == 1 { "" } else { "s" })
}

fn friendly(s: Slot) -> String {
    s.key().replace('-', " ")
}

/// Plan a show command. `present`: our outputs that exist now; `shown_now`: the extra screens shown now (the main
/// one always shows), counted by [`visibility`]; `old`: the current state.
pub fn plan_show(cmd: &ShowCmd, present: &[Slot], shown_now: &[Slot], old: &ShowState) -> Result<ShowPlan, String> {
    let max = MAX as u32;
    match cmd {
        ShowCmd::Hide(slots) => {
            let mut st = old.clone();
            st.pinned.retain(|s| !slots.contains(s));
            for s in slots {
                if !st.hidden.contains(s) {
                    st.hidden.push(*s);
                }
            }
            let all = EXTRA.iter().all(|s| slots.contains(s));
            if all {
                // "close everything": only the main screen
                st.limit = 1;
            }
            let was: Vec<Slot> = slots.iter().copied().filter(|s| shown_now.contains(s)).collect();
            let names = |v: &[Slot]| v.iter().map(|s| number_of(*s).to_string()).collect::<Vec<_>>().join(" and ");
            let message = if all {
                "Only the main screen now; the apps stay open".to_string()
            } else if was.is_empty() {
                format!("Screen {} is not shown", names(slots))
            } else {
                format!("Hid screen {}; its apps stay open", names(&was))
            };
            Ok(ShowPlan { state: st, setting: 0, missing: Vec::new(), message })
        }
        ShowCmd::Show(_) | ShowCmd::More(_) => {
            let asked = match cmd {
                ShowCmd::Show(n) => *n,
                ShowCmd::More(k) => 1 + shown_now.len() as u32 + k,
                ShowCmd::Add(_) | ShowCmd::Hide(_) => unreachable!(),
            };
            if asked == 0 {
                return Err(format!("Screens go from 1 to {MAX}"));
            }
            let n = asked.min(max) as u8;
            let needed = extra_slots(n);
            let pinned: Vec<Slot> = needed.iter().copied().filter(|s| present.contains(s)).collect();
            let missing: Vec<Slot> = needed.iter().copied().filter(|s| !present.contains(s)).collect();
            let capped = asked > max;
            let mut message = if capped { format!("That's the most: {MAX} screens") } else { format!("Showing {}", plural(n as usize)) };
            if !missing.is_empty() {
                message = format!("{n} screens need Screens = {n} in the Pimax app and a Work restart. Showing {}", plural(1 + pinned.len()));
                if capped {
                    message = format!("That's the most: {MAX} screens. {message}");
                }
            }
            Ok(ShowPlan { state: ShowState { limit: n, pinned, sort: old.sort, hidden: Vec::new() }, setting: n, missing, message })
        }
        ShowCmd::Add(slot) => {
            let Some(idx) = EXTRA.iter().position(|s| s == slot) else { return Err("The main screen is always shown".into()) };
            let setting = idx as u8 + 2;
            if !present.contains(slot) {
                return Ok(ShowPlan {
                    state: old.clone(),
                    setting,
                    missing: vec![*slot],
                    message: format!("The {} screen needs Screens = {setting} in the Pimax app and a Work restart", friendly(*slot)),
                });
            }
            let mut pinned = old.pinned.clone();
            if !pinned.contains(slot) {
                pinned.push(*slot);
            }
            let hidden: Vec<Slot> = old.hidden.iter().copied().filter(|s| s != slot).collect();
            let room = if shown_now.contains(slot) { 1 + shown_now.len() } else { 2 + shown_now.len() };
            let limit = (old.limit as usize).max(room).max(1 + pinned.len()).min(MAX as usize) as u8;
            Ok(ShowPlan {
                state: ShowState { limit, pinned, sort: old.sort, hidden },
                setting,
                missing: Vec::new(),
                message: format!("Showing the {} screen", friendly(*slot)),
            })
        }
    }
}

/// The recorded start arguments with `--screens` raised to at least `n` (never lowered; an absent flag means the
/// default, so nothing is added up to it).
pub fn argv_with_screens(args: &[String], n: u8) -> Vec<String> {
    let mut v = args.to_vec();
    let pos = v.iter().position(|a| a == "--screens" || a.starts_with("--screens="));
    match pos {
        Some(i) if v[i] == "--screens" => {
            let cur = v.get(i + 1).and_then(|x| x.parse::<u8>().ok()).unwrap_or(DEFAULT);
            if i + 1 < v.len() {
                v[i + 1] = cur.max(n).min(MAX).to_string();
            } else {
                v.push(cur.max(n).min(MAX).to_string());
            }
        }
        Some(i) => {
            let cur = v[i].split_once('=').and_then(|(_, x)| x.parse::<u8>().ok()).unwrap_or(DEFAULT);
            v[i] = format!("--screens={}", cur.max(n).min(MAX));
        }
        None if n > DEFAULT => {
            v.push("--screens".into());
            v.push(n.min(MAX).to_string());
        }
        None => {}
    }
    v
}

/// Where `show` runs and writes.
pub struct ShowEnv<'a> {
    pub state_file: &'a Path,
    pub session_dir: &'a Path,
    pub wayvrctl: &'a Path,
    /// A Work session (monado-service) runs.
    pub running: bool,
    pub dry_run: bool,
}

/// `omarchyvr desk screens show|more|add`: plan, write the show state, tell WayVR (`desk-screens`, which places a screen
/// that was not shown) and raise the recorded Screens setting so a restart keeps it. Returns the one plain line for the
/// headset. Never creates or removes an output while a session runs (WayVR only learns the outputs at its start), and
/// never closes, kills or moves an app.
pub fn run_show(cmd: &ShowCmd, h: &mut dyn Hypr, e: &ShowEnv) -> Result<String, String> {
    if !e.running {
        return Err("No Work session is running".into());
    }
    let mons = monitors(h).map_err(|err| format!("Cannot read the screens: {err}"))?;
    if main_output(&mons).is_none() {
        return Err("No desktop screen found".into());
    }
    let apps = h.run(&["-j", "clients"]).ok().and_then(|j| parse_client_counts(&j).ok()).unwrap_or_default();
    let old = read_state(e.state_file);
    let present: Vec<Slot> = vr_outputs(&mons).into_iter().map(|(s, _)| s).collect();
    let shown: Vec<Slot> = visibility(&mons, &apps, &old).into_iter().filter(|v| v.2 && v.0 != Slot::Main).map(|v| v.0).collect();
    let plan = plan_show(cmd, &present, &shown, &old)?;
    if e.dry_run {
        return Ok(format!("would write {:?}; {}", plan.state, plan.message));
    }
    write_state(e.state_file, &plan.state).map_err(|err| format!("Cannot save the screens: {err}"))?;
    if let Some((kind, args)) = super::supervise::read_argv(e.session_dir) {
        if kind == "desk" {
            let new = argv_with_screens(&args, plan.setting);
            if new != args {
                let _ = super::supervise::rewrite_argv(e.session_dir, &kind, &new);
            }
        }
    }
    sync_once(h, e.wayvrctl, &plan.state, None, true, false, &mut |_| {}).map_err(|err| format!("WayVR did not answer: {err}"))?;
    Ok(plan.message)
}

// ------------------------------------------------------------------------------ the layout inside a screen

/// The kind of an app (user decision 2026-10-15): apps of the same kind share a screen, tiled to fill it; apps of
/// different kinds never share one. Browsers, terminals and the Pimax app are kinds; every other class is a kind of
/// its own.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AppKind {
    Browser,
    Terminal,
    Pimax,
    /// Any other app: its (lower-case) class.
    Class(String),
}

impl AppKind {
    pub fn of(class: &str) -> AppKind {
        let c = class.to_lowercase();
        if ["chromium", "chrome", "brave", "firefox", "zen", "vivaldi", "librewolf", "floorp", "waterfox"].iter().any(|b| c.contains(b)) {
            AppKind::Browser
        } else if ["foot", "alacritty", "kitty", "ghostty", "wezterm"].iter().any(|t| c.contains(t)) {
            AppKind::Terminal
        } else if c == "pimax" || c == "omarchyvr" || c.starts_with("pimax-") || c.starts_with("omarchyvr-") || c.starts_with("pimax.") || c.starts_with("omarchyvr.") {
            AppKind::Pimax
        } else {
            AppKind::Class(c)
        }
    }
}

/// Short-lived helper windows (file pickers, password prompts, screen-share pickers): never moved, tiled or counted.
pub fn is_dialog(class: &str) -> bool {
    let c = class.to_lowercase();
    c.is_empty()
        || ["xdg-desktop-portal", "org.freedesktop.impl.portal", "polkit", "pinentry", "gcr-prompter", "share-picker", "zenity", "kdialog"]
            .iter()
            .any(|d| c.contains(d))
}

/// A window as `hyprctl -j clients` reports it (the fields the layout uses).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Win {
    pub address: String,
    pub class: String,
    pub ws: i64,
    pub floating: bool,
    /// 0 none, 1 maximized, 2 fullscreen.
    pub fullscreen: i64,
    pub pinned: bool,
    /// `focusHistoryID`: 0 = the focused window, larger = focused longer ago.
    pub focus: i64,
}

pub fn parse_windows(json: &str) -> Result<Vec<Win>, String> {
    let v: serde_json::Value = serde_json::from_str(json.trim()).map_err(|e| format!("hyprctl clients: {e}"))?;
    Ok(v.as_array()
        .ok_or("hyprctl clients: not a list")?
        .iter()
        .filter_map(|c| {
            Some(Win {
                address: c.get("address")?.as_str()?.to_string(),
                class: c.get("class").and_then(serde_json::Value::as_str).unwrap_or_default().to_string(),
                ws: c.pointer("/workspace/id").and_then(serde_json::Value::as_i64)?,
                floating: c.get("floating").and_then(serde_json::Value::as_bool).unwrap_or(false),
                fullscreen: c.get("fullscreen").and_then(serde_json::Value::as_i64).unwrap_or(0),
                pinned: c.get("pinned").and_then(serde_json::Value::as_bool).unwrap_or(false),
                focus: c.get("focusHistoryID").and_then(serde_json::Value::as_i64).unwrap_or(i64::MAX),
            })
        })
        .collect())
}

fn address_ok(a: &str) -> bool {
    a.len() > 2 && a.len() <= 20 && a.starts_with("0x") && a[2..].chars().all(|c| c.is_ascii_hexdigit())
}

/// One layout step.
#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    /// Send an app to another screen's workspace.
    Move { address: String, ws: i64 },
    /// Make a floating app tiled (it fills its screen, or its share of it).
    Tile(String),
    /// Maximize (true: alone on its screen) or un-maximize (false: it shares the screen) an app.
    Maximize(String, bool),
}

/// Where the apps on the screens were after the last layout: address -> workspace.
pub type Seen = HashMap<String, i64>;

/// The layout of the VR screens after apps opened or moved. `screens`: (slot, workspace) in slot order (main
/// first); `prev`: where the apps were last time. Only screens where an app arrived (opened, or moved there) or left
/// are acted on, so an app the user floats or resizes by hand stays as it is until something changes there.
/// * With `sort`, an app that arrives on a screen holding another kind goes to the screen that already has its kind,
///   else to the next free screen (left, right, up, main). Without a free screen it stays and shares the screen.
///   The kind already there stays; when every app on the screen is new, the kind with the most apps stays (a tie:
///   the one focused longest ago).
/// * On every screen whose apps changed: floating apps become tiled; one app alone is maximized (fills the screen);
///   with 2 or more (same kind) apps none is maximized, so Hyprland tiles them to fill the screen, however many.
///   A real fullscreen (a video) is left alone.
///
/// Returns the steps and the new [`Seen`].
pub fn layout_plan(screens: &[(Slot, i64)], wins: &[Win], prev: &Seen, sort: bool) -> (Vec<Act>, Seen) {
    let on_screen = |ws: i64| ws > 0 && screens.iter().any(|s| s.1 == ws);
    let apps: Vec<&Win> = wins.iter().filter(|w| on_screen(w.ws) && !w.pinned && !is_dialog(&w.class) && address_ok(&w.address)).collect();
    let kind: Vec<AppKind> = apps.iter().map(|w| AppKind::of(&w.class)).collect();
    let mut at: Vec<i64> = apps.iter().map(|w| w.ws).collect();
    let is_new = |i: usize| prev.get(&apps[i].address) != Some(&apps[i].ws);
    let holds = |t: i64, at: &[i64]| (0..at.len()).filter(|&j| at[j] == t).collect::<Vec<usize>>();
    let mut acts = Vec::new();
    if sort {
        for &(_, ws) in screens {
            let here = holds(ws, &at);
            if here.iter().all(|&i| kind[i] == kind[here[0]]) {
                continue;
            }
            let mut keep: Vec<&AppKind> = here.iter().filter(|&&i| !is_new(i)).map(|&i| &kind[i]).collect();
            if keep.is_empty() {
                let score = |k: &AppKind| {
                    let of_k: Vec<usize> = here.iter().copied().filter(|&i| kind[i] == *k).collect();
                    (of_k.len(), of_k.iter().map(|&i| apps[i].focus).max().unwrap_or(0))
                };
                keep = vec![here.iter().map(|&i| &kind[i]).max_by_key(|k| score(k)).unwrap_or(&kind[here[0]])];
            }
            for i in here {
                if keep.contains(&&kind[i]) {
                    continue;
                }
                let same = screens.iter().find(|s| {
                    let h = holds(s.1, &at);
                    s.1 != ws && !h.is_empty() && h.iter().all(|&j| kind[j] == kind[i])
                });
                let free = [Slot::Left, Slot::Right, Slot::Up, Slot::Main]
                    .iter()
                    .filter_map(|sl| screens.iter().find(|s| s.0 == *sl))
                    .find(|s| s.1 != ws && holds(s.1, &at).is_empty());
                if let Some(&(_, to)) = same.or(free) {
                    at[i] = to;
                    acts.push(Act::Move { address: apps[i].address.clone(), ws: to });
                }
            }
        }
    }
    for &(_, ws) in screens {
        let here = holds(ws, &at);
        let arrived = here.iter().any(|&i| prev.get(&apps[i].address) != Some(&ws));
        let left = prev.iter().any(|(a, &w)| w == ws && !here.iter().any(|&i| apps[i].address == *a));
        if !arrived && !left {
            continue;
        }
        for &i in &here {
            let w = apps[i];
            if w.floating {
                acts.push(Act::Tile(w.address.clone()));
            }
            if here.len() == 1 && w.fullscreen == 0 {
                acts.push(Act::Maximize(w.address.clone(), true));
            } else if here.len() > 1 && w.fullscreen == 1 {
                acts.push(Act::Maximize(w.address.clone(), false));
            }
        }
    }
    let seen = (0..apps.len()).map(|i| (apps[i].address.clone(), at[i])).collect();
    (acts, seen)
}

/// The Hyprland 0.56 dispatches of one step (a move focuses the app first, like the voice commands do).
pub fn act_dispatches(a: &Act, follow: bool) -> Vec<String> {
    match a {
        Act::Move { address, ws } => vec![
            format!("hl.dsp.focus({{ window = \"address:{address}\" }})"),
            format!("hl.dsp.window.move({{ workspace = \"{ws}\", follow = {follow} }})"),
        ],
        Act::Tile(address) => vec![format!("hl.dsp.window.float({{ action = \"unset\", window = \"address:{address}\" }})")],
        Act::Maximize(address, on) => vec![format!(
            "hl.dsp.window.fullscreen({{ mode = \"maximized\", action = \"{}\", window = \"address:{address}\" }})",
            if *on { "set" } else { "unset" }
        )],
    }
}

/// Run the steps. The focused app follows its own move; after moving other apps the focus goes back to it.
/// Returns how many steps failed.
pub fn apply_layout(h: &mut dyn Hypr, acts: &[Act], focused: Option<&str>, out: &mut dyn FnMut(&str)) -> usize {
    let mut failed = 0;
    let mut refocus = false;
    for a in acts {
        let follow = matches!(a, Act::Move { address, .. } if Some(address.as_str()) == focused);
        refocus |= matches!(a, Act::Move { .. }) && !follow;
        for d in act_dispatches(a, follow) {
            if let Err(e) = h.run(&["dispatch", &d]).and_then(|o| hypr_ok(&o)) {
                failed += 1;
                out(&format!("layout: {d}: {e}"));
                break;
            }
        }
    }
    if let (true, Some(f)) = (refocus, focused) {
        let _ = h.run(&["dispatch", &format!("hl.dsp.focus({{ window = \"address:{f}\" }})")]);
    }
    failed
}

/// The VR screens' workspaces: (slot, workspace), main first.
pub fn screen_workspaces(mons: &[Mon]) -> Vec<(Slot, i64)> {
    let Some(main) = main_output(mons) else { return Vec::new() };
    let mut v = vec![(Slot::Main, main.active_ws)];
    v.extend(EXTRA.into_iter().filter_map(|s| mons.iter().find(|m| Some(m.name.as_str()) == s.output() && !m.disabled).map(|m| (s, m.active_ws))));
    v
}

/// The apps on the screens a preset hides, "left (google-chrome), right (pimax)": they stay open there.
pub fn hidden_with_apps(vis: &[(Slot, String, bool)], mons: &[Mon], wins: &[Win]) -> Vec<String> {
    vis.iter()
        .filter(|v| !v.2)
        .filter_map(|(slot, name, _)| {
            let ws = mons.iter().find(|m| m.name == *name).map(|m| m.active_ws).filter(|w| *w > 0)?;
            let mut classes: Vec<&str> = wins.iter().filter(|w| w.ws == ws).map(|w| w.class.as_str()).collect();
            classes.sort_unstable();
            classes.dedup();
            (!classes.is_empty()).then(|| format!("{} ({})", slot.key(), classes.join(", ")))
        })
        .collect()
}

// ------------------------------------------------------------------------------------------------- the watcher

/// Hyprland events that can change which screens have apps or the focus (`socket2`, `EVENT>>DATA` lines).
pub fn relevant_event(line: &str) -> bool {
    let ev = line.split(">>").next().unwrap_or("");
    matches!(
        ev,
        "openwindow"
            | "closewindow"
            | "movewindow"
            | "movewindowv2"
            | "workspace"
            | "workspacev2"
            | "focusedmon"
            | "focusedmonv2"
            | "monitoradded"
            | "monitoraddedv2"
            | "monitorremoved"
            | "monitorremovedv2"
            | "createworkspace"
            | "createworkspacev2"
            | "destroyworkspace"
            | "destroyworkspacev2"
            | "moveworkspace"
            | "moveworkspacev2"
            | "changefloatingmode"
    )
}

/// Hyprland's event socket.
pub fn event_socket(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let sig = env("HYPRLAND_INSTANCE_SIGNATURE").filter(|s| !s.is_empty())?;
    let rt = env("XDG_RUNTIME_DIR").filter(|s| !s.is_empty())?;
    Some(PathBuf::from(rt).join("hypr").join(sig).join(".socket2.sock"))
}

/// One layout pass of the watcher ([`layout_plan`]): `seen` None = the first look, which only notes where the apps
/// are (nothing moves when the watcher starts). When an app was sent to another screen while a preset limits the
/// screens, the limit is lifted (`limit = 4`, like the voice screen commands) so the app does not vanish.
/// Never closes, kills or hides anything.
pub fn layout_once(h: &mut dyn Hypr, st: &ShowState, seen: &mut Option<Seen>, state_file: Option<&Path>, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let mons = monitors(h)?;
    let wins = parse_windows(&h.run(&["-j", "clients"])?)?;
    let screens = screen_workspaces(&mons);
    let Some(prev) = seen.as_ref() else {
        *seen = Some(layout_plan(&screens, &wins, &Seen::new(), false).1);
        return Ok(());
    };
    let (acts, next) = layout_plan(&screens, &wins, prev, st.sort);
    *seen = Some(next);
    if acts.is_empty() {
        return Ok(());
    }
    let focused = wins.iter().find(|w| w.focus == 0).map(|w| w.address.clone());
    let moved = acts.iter().filter(|a| matches!(a, Act::Move { .. })).count();
    let failed = apply_layout(h, &acts, focused.as_deref(), out);
    out(&format!("layout: {} step(s), {moved} app(s) to their own screen{}", acts.len(), if failed > 0 { format!(", {failed} failed") } else { String::new() }));
    if moved > 0 && st.limit < MAX {
        if let Some(p) = state_file {
            let _ = write_state(p, &ShowState { limit: MAX, ..st.clone() });
        }
    }
    Ok(())
}

/// Compute and, when it changed (or `force`), send the visibility to WayVR. Returns the visibility sent (or kept).
pub fn sync_once(
    h: &mut dyn Hypr,
    wayvrctl: &Path,
    st: &ShowState,
    last: Option<&Vec<(Slot, String, bool)>>,
    force: bool,
    dry_run: bool,
    out: &mut dyn FnMut(&str),
) -> Result<Vec<(Slot, String, bool)>, String> {
    let mons = monitors(h)?;
    let apps = parse_client_counts(&h.run(&["-j", "clients"])?)?;
    let vis = visibility(&mons, &apps, st);
    if vis.len() <= 1 {
        return Ok(vis); // one screen: nothing to show or hide
    }
    if !force && last == Some(&vis) {
        return Ok(vis);
    }
    let args = desk_screens_args(&vis, None, false);
    if dry_run {
        out(&format!("would run {} {}", wayvrctl.display(), args.join(" ")));
        return Ok(vis);
    }
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    run_timeout(&wayvrctl.to_string_lossy(), &a, Duration::from_secs(3))?;
    out(&format!("shown: {}", vis.iter().filter(|v| v.2).map(|v| v.0.key()).collect::<Vec<_>>().join(", ")));
    Ok(vis)
}

/// `omarchyvr desk screens watch`: follow Hyprland and keep WayVR's screens shown on demand until `stop()` or WayVR's
/// pid file goes away for good. Best effort: errors are logged and retried, the session is never touched.
pub fn watch(wayvrctl: &Path, wayvr_pid: &Path, env: &dyn Fn(&str) -> Option<String>, stop: &dyn Fn() -> bool) {
    use std::io::{BufRead, BufReader};
    use std::os::unix::net::UnixStream;
    use std::time::Instant;
    let state_file = state_path(env);
    let mut h = RealHypr;
    let mut last: Option<Vec<(Slot, String, bool)>> = None;
    let mut seen: Option<Seen> = None;
    let mut last_state = read_state(&state_file);
    let mut last_wayvr = std::fs::read_to_string(wayvr_pid).ok();
    let mut no_wayvr_since: Option<Instant> = None;
    let mut dirty = true;
    let mut force = true;
    let mut reader: Option<BufReader<UnixStream>> = None;
    let mut last_check = Instant::now();
    let log = |m: &str| eprintln!("omarchyvr desk screens: {m}");
    log(&format!("watching (WayVR control {}, state {})", wayvrctl.display(), state_file.display()));
    while !stop() {
        if reader.is_none() {
            if let Some(p) = event_socket(env) {
                if let Ok(s) = UnixStream::connect(&p) {
                    let _ = s.set_read_timeout(Some(Duration::from_millis(250)));
                    reader = Some(BufReader::new(s));
                    dirty = true;
                }
            }
        }
        let mut got = false;
        match reader.as_mut() {
            Some(r) => {
                let mut line = String::new();
                match r.read_line(&mut line) {
                    Ok(0) => reader = None, // Hyprland closed it
                    Ok(_) => got = relevant_event(line.trim()),
                    Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                    Err(_) => reader = None,
                }
            }
            None => std::thread::sleep(Duration::from_millis(500)),
        }
        if got {
            dirty = true;
            continue; // read the burst first; the sync runs once the socket is quiet for 250 ms
        }
        if last_check.elapsed() >= Duration::from_secs(1) || reader.is_none() {
            last_check = Instant::now();
            let st = read_state(&state_file);
            if st != last_state {
                last_state = st;
                dirty = true;
            }
            let w = std::fs::read_to_string(wayvr_pid).ok();
            match &w {
                None => {
                    let since = *no_wayvr_since.get_or_insert_with(Instant::now);
                    if since.elapsed() > Duration::from_secs(120) {
                        log("WayVR has been gone for 2 minutes: exiting");
                        return;
                    }
                }
                Some(_) => no_wayvr_since = None,
            }
            if w != last_wayvr {
                // WayVR restarted: send everything again
                last_wayvr = w;
                force = true;
                dirty = true;
            }
            if reader.is_none() {
                dirty = true; // no event socket: poll once a second
            }
        }
        if dirty {
            // the layout first (it may send an app to its own screen), then what VR shows
            if let Err(e) = layout_once(&mut h, &last_state, &mut seen, Some(&state_file), &mut |m| log(m)) {
                log(&format!("layout skipped: {e}"));
            }
            last_state = read_state(&state_file);
            match sync_once(&mut h, wayvrctl, &last_state, last.as_ref(), force, false, &mut |m| log(m)) {
                Ok(v) => {
                    last = Some(v);
                    dirty = false;
                    force = false;
                }
                Err(e) => {
                    log(&format!("sync failed (retrying): {e}"));
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        }
    }
}

// --------------------------------------------------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    pub fn mon(name: &str, w: i64, x: i64, y: i64, ws: i64) -> Mon {
        Mon { name: name.into(), width: w, height: w * 9 / 16, x, y, active_ws: ws, ..Mon::default() }
    }

    #[derive(Default)]
    struct Fake {
        mons: Vec<Mon>,
        calls: Vec<String>,
        fail_create: bool,
        rule_error: bool,
        /// `hyprctl -j clients` entries (address, workspace, at, size, floating, ...)
        wins: Vec<serde_json::Value>,
        /// workspace id -> output name
        ws_mon: Vec<(i64, String)>,
        /// a dispatch is accepted but the app does not move
        stuck: bool,
    }

    fn json(mons: &[Mon]) -> String {
        let v: Vec<serde_json::Value> = mons
            .iter()
            .map(|m| {
                serde_json::json!({"name": m.name, "width": m.width, "height": m.height, "x": m.x, "y": m.y, "focused": m.focused,
                    "disabled": m.disabled, "mirrorOf": if m.mirror { "2" } else { "none" }, "activeWorkspace": {"id": m.active_ws, "name": ""}})
            })
            .collect();
        serde_json::to_string(&v).unwrap()
    }

    impl Hypr for Fake {
        fn run(&mut self, args: &[&str]) -> Result<String, String> {
            self.calls.push(args.join(" "));
            match args {
                ["-j", "monitors", "all"] => Ok(json(&self.mons)),
                ["-j", "clients"] => Ok(serde_json::Value::Array(self.wins.clone()).to_string()),
                ["-j", "workspaces"] => {
                    Ok(serde_json::Value::Array(self.ws_mon.iter().map(|(id, m)| serde_json::json!({"id": id, "monitor": m})).collect()).to_string())
                }
                ["dispatch", d] if d.starts_with("hl.dsp.window.move({ x = ") => {
                    // relative move by address: shift the fake app
                    let num = |k: &str| d.split(k).nth(1).and_then(|r| r.split(',').next()).and_then(|v| v.trim().parse::<i64>().ok()).unwrap();
                    let (dx, dy) = (num("x = "), num("y = "));
                    let addr = d.split("address:").nth(1).unwrap().split('"').next().unwrap().to_string();
                    if !self.stuck {
                        for w in self.wins.iter_mut().filter(|w| w["address"] == addr.as_str()) {
                            let (x, y) = (w["at"][0].as_i64().unwrap(), w["at"][1].as_i64().unwrap());
                            w["at"] = serde_json::json!([x + dx, y + dy]);
                        }
                    }
                    Ok("ok".into())
                }
                ["dispatch", d] if d.starts_with("hl.dsp.window.center(") => {
                    let addr = d.split("address:").nth(1).unwrap().split('"').next().unwrap().to_string();
                    let (mons, ws_mon, stuck) = (self.mons.clone(), self.ws_mon.clone(), self.stuck);
                    for w in self.wins.iter_mut().filter(|w| w["address"] == addr.as_str() && !stuck) {
                        let ws = w["workspace"]["id"].as_i64().unwrap();
                        let out = &ws_mon.iter().find(|(i, _)| *i == ws).unwrap().1;
                        let m = mons.iter().find(|m| &m.name == out).unwrap();
                        let (sw, sh) = (w["size"][0].as_i64().unwrap(), w["size"][1].as_i64().unwrap());
                        w["at"] = serde_json::json!([m.x + (m.width - sw) / 2, m.y + (m.height - sh) / 2]);
                    }
                    Ok("ok".into())
                }
                ["eval", _] => Ok(if self.rule_error { "error: bad lua".into() } else { "ok".into() }),
                ["output", "create", "headless", n] => {
                    if self.fail_create {
                        return Ok("error: backend refused".into());
                    }
                    let (w, x, y) = match *n {
                        "PIMAX-LEFT" => (1920, -1920, 0),
                        "PIMAX-RIGHT" => (1920, 1920, 0),
                        _ => (1920, 0, -1080),
                    };
                    self.mons.push(mon(n, w, x, y, 10 + self.mons.len() as i64));
                    Ok("ok".into())
                }
                ["output", "remove", n] => {
                    self.mons.retain(|m| m.name != *n);
                    Ok("ok".into())
                }
                _ => Err(format!("unexpected {args:?}")),
            }
        }
        fn sleep(&mut self, _d: Duration) {}
    }

    fn desktop() -> Vec<Mon> {
        let mut hdmi = mon("HDMI-A-1", 1920, 0, 0, 0);
        hdmi.mirror = true;
        let mut dp = mon("DP-1", 0, -1, -1, 0);
        dp.disabled = true;
        vec![hdmi, dp, mon("SUNSHINE", 1920, 0, 0, 2)]
    }

    #[test]
    fn parses_real_hyprctl_monitors_and_finds_the_main_output() {
        let j = r#"[{"id":0,"name":"HDMI-A-1","width":1920,"height":1080,"x":0,"y":0,"activeWorkspace":{"id":0,"name":""},"focused":false,"disabled":false,"mirrorOf":"2"},
            {"name":"DP-1","width":0,"height":0,"x":-1,"y":-1,"activeWorkspace":{"id":0,"name":""},"disabled":true,"mirrorOf":"none"},
            {"name":"SUNSHINE","width":1920,"height":1080,"x":0,"y":0,"activeWorkspace":{"id":2,"name":"2"},"focused":true,"disabled":false,"mirrorOf":"none"}]"#;
        let m = parse_monitors(j).unwrap();
        assert_eq!(m.len(), 3);
        assert!(m[0].mirror && m[1].disabled && !m[2].mirror);
        assert_eq!(main_output(&m).unwrap().name, "SUNSHINE");
        assert_eq!(m[2].active_ws, 2);
        let c = parse_client_counts(r#"[{"workspace":{"id":2}},{"workspace":{"id":2}},{"workspace":{"id":-98}},{"workspace":{"id":5}}]"#).unwrap();
        assert_eq!((c[&2], c[&5], c.get(&-98)), (2, 1, None));
    }

    #[test]
    fn wanted_outputs_sit_around_the_main_one() {
        let main = mon("SUNSHINE", 1920, 0, 0, 2);
        assert!(wanted(1, &main).is_empty());
        let w = wanted(4, &main);
        assert_eq!(w.iter().map(|w| (w.name, w.x, w.y)).collect::<Vec<_>>(), [("PIMAX-LEFT", -1920, 0), ("PIMAX-RIGHT", 1920, 0), ("PIMAX-UP", 0, -1080)]);
        assert_eq!(w[0].rule(), r#"hl.monitor({ output = "PIMAX-LEFT", mode = "1920x1080@60", position = "-1920x0", scale = 1 })"#);
        assert_eq!(wanted(2, &main).len(), 1);
        assert_eq!(parse_count("3"), Ok(3));
        assert!(parse_count("0").is_err() && parse_count("7").is_err() && parse_count("x").is_err());
    }

    #[test]
    fn prepare_creates_rule_first_then_output_and_is_idempotent() {
        let mut h = Fake { mons: desktop(), ..Fake::default() };
        let r = prepare(4, &mut h, false, false);
        assert_eq!(r.error, None, "{r:?}");
        assert_eq!(r.created, ["PIMAX-LEFT", "PIMAX-RIGHT", "PIMAX-UP"]);
        assert_eq!(r.screens(), 4);
        let i_rule = h.calls.iter().position(|c| c.starts_with("eval") && c.contains("PIMAX-LEFT")).unwrap();
        let i_out = h.calls.iter().position(|c| c == "output create headless PIMAX-LEFT").unwrap();
        assert!(i_rule < i_out, "the rule goes first");
        // again: nothing created or removed
        h.calls.clear();
        let r = prepare(4, &mut h, false, false);
        assert!(r.created.is_empty() && r.removed.is_empty() && r.error.is_none(), "{r:?}");
        assert_eq!(r.kept.len(), 3);
        assert!(!h.calls.iter().any(|c| c.starts_with("output")), "{:?}", h.calls);
        // fewer screens: the extra ones go
        let r = prepare(2, &mut h, false, false);
        assert_eq!(r.removed, ["PIMAX-RIGHT", "PIMAX-UP"]);
        assert_eq!(r.outputs, [(Slot::Left, "PIMAX-LEFT".to_string())]);
        // one screen: all ours go
        let r = prepare(1, &mut h, false, false);
        assert_eq!(r.removed, ["PIMAX-LEFT"]);
        assert_eq!(r.screens(), 1);
    }

    #[test]
    fn stale_outputs_of_a_crashed_session_are_cleaned_or_resized() {
        let mut mons = desktop();
        mons.push(mon("PIMAX-LEFT", 1280, -1280, 0, 11)); // wrong size
        mons.push(mon("PIMAX-OLD", 1920, 4000, 0, 12)); // unknown name
        let mut h = Fake { mons, ..Fake::default() };
        let r = prepare(2, &mut h, false, false);
        assert_eq!(r.removed, ["PIMAX-LEFT", "PIMAX-OLD"]);
        assert_eq!(r.created, ["PIMAX-LEFT"]);
        assert!(h.mons.iter().any(|m| m.name == "PIMAX-LEFT" && m.width == 1920));
    }

    #[test]
    fn nothing_changes_while_a_session_runs() {
        let mut h = Fake { mons: desktop(), ..Fake::default() };
        let r = prepare(4, &mut h, false, true);
        assert!(r.error.as_deref().unwrap().contains("VR session runs"));
        assert!(!h.calls.iter().any(|c| c.starts_with("output") || c.starts_with("eval")));
        assert_eq!(r.screens(), 1);
        prepare(4, &mut h, false, false);
        h.calls.clear();
        let r = remove_all(&mut h, false, true);
        assert!(r.error.is_some() && r.removed.is_empty() && !h.calls.iter().any(|c| c.starts_with("output")));
        let r = remove_all(&mut h, false, false);
        assert_eq!(r.removed.len(), 3);
        assert!(remove_all(&mut h, false, false).removed.is_empty(), "idempotent");
    }

    #[test]
    fn a_failed_create_still_gives_one_screen_and_says_why() {
        let mut h = Fake { mons: desktop(), fail_create: true, ..Fake::default() };
        let r = prepare(4, &mut h, false, false);
        assert_eq!(r.screens(), 1);
        let e = r.error.unwrap();
        assert!(e.starts_with("1 of 4 screens") && e.contains("backend refused"), "{e}");
        let mut h = Fake { mons: Vec::new(), ..Fake::default() };
        assert!(prepare(4, &mut h, false, false).error.unwrap().contains("no desktop output"));
    }

    #[test]
    fn dry_run_lists_steps_and_touches_nothing() {
        let mut h = Fake { mons: desktop(), ..Fake::default() };
        let r = prepare(3, &mut h, true, false);
        assert_eq!(r.steps.len(), 4);
        assert!(r.steps[0].contains("hl.monitor") && r.steps[1].ends_with("headless PIMAX-LEFT"));
        assert_eq!(h.calls, ["-j monitors all"]);
    }

    /// A floating/tiled app as `hyprctl -j clients` reports it.
    fn client(address: &str, ws: i64, at: (i64, i64), floating: bool, fullscreen: i64) -> serde_json::Value {
        serde_json::json!({"address": address, "class": "foot", "workspace": {"id": ws, "name": ws.to_string()}, "at": [at.0, at.1],
            "size": [700, 500], "floating": floating, "fullscreen": fullscreen, "pinned": false, "mapped": true, "hidden": false})
    }

    /// The desktop after `desk stop` of an earlier session: the outputs are gone, their workspaces (11 left, 12 right,
    /// 13 up) went to SUNSHINE and Hyprland shifted the floating apps into SUNSHINE's area.
    fn after_an_earlier_session() -> Fake {
        let wins = vec![
            client("0x10", 13, (610, 290), true, 0),   // floating, was on the up screen: stale
            client("0x11", 12, (5000, 5000), true, 0), // floating, nowhere near: stale, centred
            client("0x12", 13, (610, -790), true, 0),  // floating, already on the up screen
            client("0x13", 11, (12, 40), false, 0),    // tiled: Hyprland lays it out again
            client("0x14", 13, (2, 30), true, 1),      // maximized: Hyprland sizes it again
            client("0x15", 2, (100, 100), true, 0),    // the main screen's own app
            client("0x16", 11, (620, 300), true, 0),   // floating, was on the left screen: stale
        ];
        let ws_mon = vec![(2, "SUNSHINE".into()), (11, "PIMAX-LEFT".into()), (12, "PIMAX-RIGHT".into()), (13, "PIMAX-UP".into())];
        Fake { mons: desktop(), wins, ws_mon, ..Fake::default() }
    }

    #[test]
    fn a_recreated_output_gets_its_floating_apps_back_and_is_checked_again() {
        let mut h = after_an_earlier_session();
        let r = prepare(4, &mut h, false, false);
        assert_eq!(r.error, None, "{r:?}");
        assert_eq!(r.replaced, ["0x10 (PIMAX-UP)", "0x11 (PIMAX-RIGHT)", "0x16 (PIMAX-LEFT)"]);
        // exact order after the outputs exist: read, put back by address (no focus change), read again
        let i = h.calls.iter().position(|c| c == "-j workspaces").unwrap();
        assert!(h.calls[..i].iter().any(|c| c == "output create headless PIMAX-UP"), "the check runs after the outputs exist");
        let tail: Vec<&str> = h.calls[i - 1..].iter().map(String::as_str).collect();
        assert_eq!(
            tail,
            [
                "-j monitors all",
                "-j workspaces",
                "-j clients",
                r#"dispatch hl.dsp.window.move({ x = 0, y = -1080, relative = true, window = "address:0x10" })"#,
                r#"dispatch hl.dsp.window.center({ window = "address:0x11" })"#,
                r#"dispatch hl.dsp.window.move({ x = -1920, y = 0, relative = true, window = "address:0x16" })"#,
                "-j monitors all",
                "-j workspaces",
                "-j clients",
            ]
        );
        assert!(!h.calls.iter().any(|c| ["hl.dsp.focus", "0x12", "0x13", "0x14", "0x15"].iter().any(|x| c.contains(x))), "{:?}", h.calls);
        assert_eq!(h.wins[0]["at"], serde_json::json!([610, -790]), "back where it was on the up screen");
        assert!(r.lines().iter().any(|l| l.starts_with("apps put back on their screen: 0x10 (PIMAX-UP)")), "{:?}", r.lines());
        // the next start (outputs kept): nothing is stale, nothing is dispatched
        h.calls.clear();
        let r = prepare(4, &mut h, false, false);
        assert!(r.replaced.is_empty() && r.error.is_none(), "{r:?}");
        assert!(!h.calls.iter().any(|c| c.starts_with("dispatch")), "{:?}", h.calls);
    }

    #[test]
    fn an_app_that_stays_off_its_screen_is_reported() {
        let mut h = Fake { stuck: true, ..after_an_earlier_session() };
        let r = prepare(4, &mut h, false, false);
        let e = r.error.clone().unwrap();
        assert!(e.contains("3 app(s) still outside their VR screen (not drawn): 0x10 on PIMAX-UP"), "{e}");
        assert_eq!(r.screens(), 4, "the screens themselves are fine");
        // a dry run and a running session never touch apps
        let mut h = after_an_earlier_session();
        prepare(4, &mut h, true, false);
        prepare(4, &mut h, false, true);
        assert!(!h.calls.iter().any(|c| c.starts_with("dispatch") || c == "-j clients"), "{:?}", h.calls);
    }

    #[test]
    fn stale_floating_apps_from_real_hyprctl_json() {
        // the repro of 2026-10-17 (lesson 67): ws 6 came back to the re-created up output with the app at 609,289
        let mut mons = desktop();
        mons.push(mon("PIMAX-UP", 1920, 0, -1080, 6));
        let main = mons[2].clone();
        let ws_mon: HashMap<i64, String> = [(6, "PIMAX-UP".to_string()), (5, "SUNSHINE".to_string())].into();
        let clients: serde_json::Value = serde_json::from_str(
            r#"[{"address":"0x6079994cc8c0","mapped":true,"hidden":false,"at":[609,289],"size":[700,500],"workspace":{"id":6,"name":"6"},"floating":true,"pinned":false,"monitor":5,"class":"foot","fullscreen":0},
                {"address":"0x6079994cc8d0","mapped":true,"hidden":false,"at":[609,-791],"size":[700,500],"workspace":{"id":6,"name":"6"},"floating":true,"pinned":false,"monitor":5,"class":"foot","fullscreen":0},
                {"address":"0x607999351940","mapped":true,"hidden":false,"at":[12,40],"size":[1896,1028],"workspace":{"id":5,"name":"5"},"floating":false,"pinned":false,"monitor":2,"class":"foot","fullscreen":0}]"#,
        )
        .unwrap();
        let s = stale_floating(&mons, &main, &ws_mon, &clients);
        assert_eq!(s, [Stale { address: "0x6079994cc8c0".into(), output: "PIMAX-UP".into(), shift: Some((0, -1080)) }]);
        assert_eq!(s[0].dispatch(), r#"hl.dsp.window.move({ x = 0, y = -1080, relative = true, window = "address:0x6079994cc8c0" })"#);
    }

    #[test]
    fn report_round_trips_for_the_app() {
        let mut h = Fake { mons: desktop(), ..Fake::default() };
        let r = prepare(4, &mut h, false, false);
        let back = Report::parse(&r.lines().join("\n"));
        assert_eq!((back.main.as_deref(), back.outputs.len(), back.error), (Some("SUNSHINE"), 3, None));
        let r = Report { main: Some("SUNSHINE".into()), error: Some("1 of 4 screens (x)".into()), ..Report::default() };
        assert_eq!(Report::parse(&r.lines().join("\n")).error.as_deref(), Some("1 of 4 screens (x)"));
    }

    #[test]
    fn screens_show_on_demand_within_the_limit() {
        let mut mons = desktop();
        mons.extend([mon("PIMAX-LEFT", 1920, -1920, 0, 11), mon("PIMAX-RIGHT", 1920, 1920, 0, 12), mon("PIMAX-UP", 1920, 0, -1080, 13)]);
        let apps: HashMap<i64, usize> = [(2, 3), (12, 1)].into();
        let st = ShowState::default();
        let v = visibility(&mons, &apps, &st);
        let on: Vec<_> = v.iter().map(|x| (x.0, x.2)).collect();
        assert_eq!(on, [(Slot::Main, true), (Slot::Left, false), (Slot::Right, true), (Slot::Up, false)]);
        // the focused empty screen ("new window") shows too
        mons[3].focused = true;
        assert!(visibility(&mons, &apps, &st)[1].2);
        // work-1 / cinema: the main screen only, plus the focused screen (you never type into a hidden screen)
        let one = ShowState::for_preset(super::super::preset::Preset::Work1);
        assert_eq!(visibility(&mons, &apps, &one).iter().map(|x| x.2).collect::<Vec<_>>(), [true, true, false, false]);
        mons[3].focused = false;
        assert_eq!(visibility(&mons, &apps, &one).iter().filter(|x| x.2).count(), 1);
        mons[3].focused = true;
        // work-3: left and right pinned, even empty; up never
        let w3 = ShowState::for_preset(super::super::preset::Preset::Work3);
        let mut busy_up = apps.clone();
        busy_up.insert(13, 2);
        let v = visibility(&mons, &busy_up, &w3);
        assert_eq!(v.iter().map(|x| x.2).collect::<Vec<_>>(), [true, true, true, false]);
        assert_eq!(
            desk_screens_args(&v, None, false),
            ["desk-screens", "main=SUNSHINE", "left=PIMAX-LEFT", "right=PIMAX-RIGHT", "up=PIMAX-UP:hide"]
        );
        assert_eq!(desk_screens_args(&v, Some((1.6, 2.0, 0.0)), true)[1..9], ["--place", "--width", "1.6", "--distance", "2", "--curvature", "0", "--hide-others"]);
        // no outputs of ours: only the main screen
        assert_eq!(visibility(&desktop(), &apps, &st).len(), 1);
    }

    #[test]
    fn state_file_round_trip_and_events() {
        let s = ShowState { limit: 3, pinned: vec![Slot::Left, Slot::Right], sort: true, hidden: Vec::new() };
        assert_eq!(ShowState::parse(&s.render()), s);
        assert_eq!(ShowState::parse("limit = 9\npinned = main,up,bogus\n"), ShowState { limit: MAX, pinned: vec![Slot::Up], sort: false, hidden: Vec::new() });
        let d = crate::vr::test_dir("screens-state");
        let p = d.join("omarchyvr-screens.state");
        assert_eq!(read_state(&p), ShowState::default());
        write_state(&p, &s).unwrap();
        assert_eq!(read_state(&p), s);
        assert!(relevant_event("openwindow>>80a6f50,2,foot,foot") && relevant_event("focusedmonv2>>PIMAX-LEFT,11"));
        assert!(!relevant_event("activewindow>>foot,~") && !relevant_event("garbage"));
        let env = |k: &str| match k {
            "HYPRLAND_INSTANCE_SIGNATURE" => Some("abc".to_string()),
            "XDG_RUNTIME_DIR" => Some("/run/user/1000".to_string()),
            _ => None,
        };
        assert_eq!(event_socket(&env).unwrap(), PathBuf::from("/run/user/1000/hypr/abc/.socket2.sock"));
        assert_eq!(Slot::of_output("PIMAX-UP"), Some(Slot::Up));
        assert_eq!(Slot::parse("middle"), Some(Slot::Main));
    }

    fn w(address: &str, class: &str, ws: i64, focus: i64) -> Win {
        Win { address: address.into(), class: class.into(), ws, floating: true, fullscreen: 0, pinned: false, focus }
    }

    /// main = workspace 2, left 11, right 12, up 13
    const SCR: [(Slot, i64); 4] = [(Slot::Main, 2), (Slot::Left, 11), (Slot::Right, 12), (Slot::Up, 13)];

    fn seen(v: &[(&str, i64)]) -> Seen {
        v.iter().map(|(a, w)| (a.to_string(), *w)).collect()
    }

    #[test]
    fn kinds_by_class() {
        assert_eq!(AppKind::of("google-chrome"), AppKind::Browser);
        assert_eq!(AppKind::of("chromium"), AppKind::Browser);
        assert_eq!(AppKind::of("firefox"), AppKind::Browser);
        for t in ["foot", "Alacritty", "kitty", "com.mitchellh.ghostty"] {
            assert_eq!(AppKind::of(t), AppKind::Terminal, "{t}");
        }
        assert_eq!(AppKind::of("pimax"), AppKind::Pimax);
        assert_eq!(AppKind::of("obsidian"), AppKind::Class("obsidian".into()));
        assert_ne!(AppKind::of("obsidian"), AppKind::of("org.gnome.Nautilus"), "other classes are each their own kind");
        assert!(is_dialog("xdg-desktop-portal-gtk") && is_dialog("") && !is_dialog("foot"));
    }

    #[test]
    fn layout_first_look_moves_nothing() {
        let wins = [w("0x1", "foot", 2, 0), w("0x2", "google-chrome", 2, 1)];
        let (acts, s) = layout_plan(&SCR, &wins, &Seen::new(), false);
        assert!(acts.iter().all(|a| !matches!(a, Act::Move { .. })));
        assert_eq!(s, seen(&[("0x1", 2), ("0x2", 2)]));
    }

    #[test]
    fn one_app_alone_fills_its_screen_and_same_kind_apps_tile() {
        // a terminal opens alone on the main screen: tiled + maximized
        let (acts, s) = layout_plan(&SCR, &[w("0x1", "foot", 2, 0)], &Seen::new(), true);
        assert_eq!(acts, [Act::Tile("0x1".into()), Act::Maximize("0x1".into(), true)]);
        // a second terminal joins: both tiled, no maximize (half and half)
        let mut a = w("0x1", "foot", 2, 1);
        a.floating = false;
        a.fullscreen = 1;
        let (acts, s) = layout_plan(&SCR, &[a.clone(), w("0x2", "kitty", 2, 0)], &s, true);
        assert_eq!(acts, [Act::Maximize("0x1".into(), false), Act::Tile("0x2".into())]);
        // a third and more (5 terminals): they stay together, tiled
        let mut five = vec![w("0x1", "foot", 2, 4), w("0x2", "foot", 2, 3)];
        five.extend((3..=5).map(|i| w(&format!("0x{i}"), "foot", 2, 5 - i)));
        let (acts, _) = layout_plan(&SCR, &five, &s, true);
        assert!(acts.iter().all(|a| matches!(a, Act::Tile(_))), "{acts:?}");
        // 2 Chromes on the left screen: half and half
        let (acts, _) = layout_plan(&SCR, &[w("0xc1", "google-chrome", 11, 0), w("0xc2", "google-chrome", 11, 1)], &Seen::new(), true);
        assert!(!acts.iter().any(|a| matches!(a, Act::Move { .. } | Act::Maximize(_, true))), "{acts:?}");
        // nothing changed: nothing to do (an app floated by hand stays floating)
        let (acts, _) = layout_plan(&SCR, &[w("0x1", "foot", 2, 0)], &seen(&[("0x1", 2)]), true);
        assert!(acts.is_empty());
    }

    #[test]
    fn a_new_app_of_another_kind_goes_to_the_next_free_screen() {
        let before = seen(&[("0x1", 2)]);
        // Chrome opens on the main screen where a terminal is: it goes left (the first free screen)
        let wins = [w("0x1", "foot", 2, 1), w("0x2", "google-chrome", 2, 0)];
        let (acts, s) = layout_plan(&SCR, &wins, &before, true);
        assert_eq!(acts[0], Act::Move { address: "0x2".into(), ws: 11 });
        assert!(acts.contains(&Act::Maximize("0x2".into(), true)), "alone on its new screen: {acts:?}");
        assert_eq!(s["0x2"], 11);
        // a second Chrome joins the first one on the left screen; the Pimax app takes the right screen
        let wins = [w("0x1", "foot", 2, 2), w("0x2", "google-chrome", 11, 1), w("0x3", "google-chrome", 2, 0), w("0x4", "pimax", 2, 3)];
        let (acts, _) = layout_plan(&SCR, &wins, &s, true);
        let moves: Vec<_> = acts.iter().filter(|a| matches!(a, Act::Move { .. })).collect();
        assert_eq!(moves, [&Act::Move { address: "0x3".into(), ws: 11 }, &Act::Move { address: "0x4".into(), ws: 12 }]);
        // "send this to the left screen" with a terminal onto Chrome's screen: it goes on to the next free one
        let s = seen(&[("0x1", 2), ("0x2", 11)]);
        let (acts, _) = layout_plan(&SCR, &[w("0x1", "foot", 11, 0), w("0x2", "google-chrome", 11, 1)], &s, true);
        assert_eq!(acts[0], Act::Move { address: "0x1".into(), ws: 12 }, "the next free screen in the order left, right, up, main");
    }

    #[test]
    fn layout_without_free_screens_or_sorting_keeps_apps_where_they_are() {
        // every screen busy: the newcomer shares, tiled
        let s = seen(&[("0x1", 2), ("0x2", 11), ("0x3", 12), ("0x4", 13)]);
        let wins = [w("0x1", "foot", 2, 1), w("0x2", "google-chrome", 11, 2), w("0x3", "pimax", 12, 3), w("0x4", "obsidian", 13, 4), w("0x5", "steam", 2, 0)];
        let (acts, _) = layout_plan(&SCR, &wins, &s, true);
        assert!(!acts.iter().any(|a| matches!(a, Act::Move { .. })), "{acts:?}");
        // "put everything back together" turned sorting off: kinds share the main screen, tiled
        let s = seen(&[("0x1", 2), ("0x2", 11)]);
        let (acts, _) = layout_plan(&SCR, &[w("0x1", "foot", 2, 0), w("0x2", "google-chrome", 2, 1)], &s, false);
        assert_eq!(acts, [Act::Tile("0x1".into()), Act::Tile("0x2".into())]);
        // a hidden workspace, a pinned window and a file picker are never touched
        let mut pin = w("0x7", "mpv", 2, 3);
        pin.pinned = true;
        let wins = [w("0x1", "foot", 2, 1), w("0x6", "steam", 7, 2), pin, w("0x8", "xdg-desktop-portal-gtk", 2, 0)];
        let (acts, _) = layout_plan(&SCR, &wins, &seen(&[("0x1", 2)]), true);
        assert!(acts.is_empty(), "{acts:?}");
    }

    #[test]
    fn when_an_app_leaves_the_last_one_fills_the_screen() {
        let mut a = w("0x1", "foot", 2, 0);
        a.floating = false;
        let (acts, _) = layout_plan(&SCR, &[a], &seen(&[("0x1", 2), ("0x2", 2)]), true);
        assert_eq!(acts, [Act::Maximize("0x1".into(), true)]);
    }

    #[test]
    fn layout_dispatches_keep_the_focus() {
        let mut h = Fake::default();
        let acts = [Act::Move { address: "0x2".into(), ws: 11 }, Act::Tile("0x2".into()), Act::Maximize("0x2".into(), true)];
        // dispatches are not in the fake: each one fails, and is reported, nothing panics
        let mut log = Vec::new();
        assert_eq!(apply_layout(&mut h, &acts, Some("0x1"), &mut |l| log.push(l.to_string())), 3);
        assert_eq!(
            act_dispatches(&acts[0], false),
            ["hl.dsp.focus({ window = \"address:0x2\" })", "hl.dsp.window.move({ workspace = \"11\", follow = false })"]
        );
        assert_eq!(act_dispatches(&acts[1], false), ["hl.dsp.window.float({ action = \"unset\", window = \"address:0x2\" })"]);
        assert_eq!(
            act_dispatches(&Act::Maximize("0x2".into(), false), false),
            ["hl.dsp.window.fullscreen({ mode = \"maximized\", action = \"unset\", window = \"address:0x2\" })"]
        );
        assert_eq!(h.calls.last().unwrap(), "dispatch hl.dsp.focus({ window = \"address:0x1\" })", "the focus comes back");
        assert!(!h.calls.iter().any(|c| c.contains("close") || c.contains("kill")));
    }

    #[test]
    fn the_focused_screen_shows_even_over_a_presets_limit() {
        let mut mons = desktop();
        mons.extend([mon("PIMAX-LEFT", 1920, -1920, 0, 11), mon("PIMAX-RIGHT", 1920, 1920, 0, 12)]);
        let apps: HashMap<i64, usize> = [(2, 1), (11, 1), (12, 1)].into();
        let cinema = ShowState::for_preset(super::super::preset::Preset::Cinema);
        let on = |m: &[Mon]| visibility(m, &apps, &cinema).iter().map(|x| x.2).collect::<Vec<_>>();
        assert_eq!(on(&mons), [true, false, false], "hidden screens keep their apps");
        mons[4].focused = true; // PIMAX-RIGHT
        assert_eq!(on(&mons), [true, false, true]);
        let vis = visibility(&mons, &apps, &cinema);
        let wins = [w("0x1", "foot", 2, 1), w("0x2", "google-chrome", 11, 0), w("0x3", "google-chrome", 11, 2)];
        assert_eq!(hidden_with_apps(&vis, &mons, &wins), ["left (google-chrome)"]);
        // the sort setting: written, read back, kept by a mode switch
        let off = ShowState { sort: false, ..ShowState::default() };
        assert_eq!(ShowState::parse(&off.render()), off);
        assert!(!ShowState::parse("limit = 1\n").sort, "off by default: apps hop screens only on a command");
        assert!(ShowState::parse("sort = on\n").sort);
        assert!(!off.switch_to(super::super::preset::Preset::Cinema).sort);
        assert_eq!(off.switch_to(super::super::preset::Preset::Cinema).limit, 1);
    }

    #[test]
    fn layout_once_first_looks_then_sorts_and_lifts_a_limit() {
        struct H(Vec<String>, String);
        impl Hypr for H {
            fn run(&mut self, args: &[&str]) -> Result<String, String> {
                self.0.push(args.join(" "));
                match args {
                    ["-j", "monitors", "all"] => {
                        let mut m = desktop();
                        m.extend([mon("PIMAX-LEFT", 1920, -1920, 0, 11), mon("PIMAX-RIGHT", 1920, 1920, 0, 12)]);
                        Ok(json(&m))
                    }
                    ["-j", "clients"] => Ok(self.1.clone()),
                    ["dispatch", _] => Ok("ok".into()),
                    _ => Err(format!("unexpected {args:?}")),
                }
            }
            fn sleep(&mut self, _d: Duration) {}
        }
        let c = |a: &str, class: &str, ws: i64, f: i64| serde_json::json!({"address": a, "class": class, "workspace": {"id": ws}, "floating": true, "fullscreen": 0, "focusHistoryID": f});
        let mut h = H(Vec::new(), serde_json::json!([c("0x1", "foot", 2, 0)]).to_string());
        let d = crate::vr::test_dir("screens-layout");
        let st_file = d.join("omarchyvr-screens.state");
        // the default and every preset leave the sorting OFF: no app hops screens by itself
        assert!(!ShowState::default().sort && !ShowState::for_preset(super::super::preset::Preset::Cinema).sort);
        let off = ShowState::for_preset(super::super::preset::Preset::Cinema);
        let mut h0 = H(Vec::new(), serde_json::json!([c("0x1", "foot", 2, 1), c("0x2", "google-chrome", 2, 0)]).to_string());
        let mut seen0 = Some(layout_plan(&screen_workspaces(&monitors(&mut h0).unwrap()), &parse_windows(&serde_json::json!([c("0x1", "foot", 2, 0)]).to_string()).unwrap(), &Seen::new(), false).1);
        layout_once(&mut h0, &off, &mut seen0, None, &mut |_| {}).unwrap();
        assert!(!h0.0.iter().any(|x| x.contains("workspace = \"11\"")), "sort off: chrome stays on the main screen: {:?}", h0.0);
        // the explicit command ("put everything in its own window") turns the sorting on
        let st = ShowState { sort: true, ..off };
        write_state(&st_file, &st).unwrap();
        let mut seen = None;
        layout_once(&mut h, &st, &mut seen, Some(&st_file), &mut |_| {}).unwrap();
        assert!(!h.0.iter().any(|c| c.starts_with("dispatch")), "first look: nothing moves");
        // Chrome opens (focused) on the main screen: it goes left, following the focus; cinema's limit is lifted
        h.1 = serde_json::json!([c("0x1", "foot", 2, 1), c("0x2", "google-chrome", 2, 0)]).to_string();
        layout_once(&mut h, &st, &mut seen, Some(&st_file), &mut |_| {}).unwrap();
        assert!(h.0.contains(&"dispatch hl.dsp.window.move({ workspace = \"11\", follow = true })".to_string()), "{:?}", h.0);
        assert_eq!(read_state(&st_file).limit, MAX);
    }
    // ---------------------------------------------------------------- six screens and `desk screens show|more|add`

    fn six() -> Vec<Mon> {
        let mut mons = desktop();
        mons.extend([
            mon("PIMAX-LEFT", 1920, -1920, 0, 11),
            mon("PIMAX-RIGHT", 1920, 1920, 0, 12),
            mon("PIMAX-UP", 1920, 0, -1080, 13),
            mon("PIMAX-UPLEFT", 1920, -1920, -1080, 14),
            mon("PIMAX-UPRIGHT", 1920, 1920, -1080, 15),
        ]);
        mons
    }

    #[test]
    fn six_slots_names_keys_and_positions() {
        assert_eq!(MAX, 6);
        assert_eq!(EXTRA, [Slot::Left, Slot::Right, Slot::Up, Slot::UpLeft, Slot::UpRight]);
        assert_eq!((Slot::UpLeft.key(), Slot::UpRight.key()), ("up-left", "up-right"));
        assert_eq!((Slot::UpLeft.output(), Slot::UpRight.output()), (Some("PIMAX-UPLEFT"), Some("PIMAX-UPRIGHT")));
        assert_eq!((Slot::parse("up-left"), Slot::parse("UP-RIGHT"), Slot::parse("upright")), (Some(Slot::UpLeft), Some(Slot::UpRight), Some(Slot::UpRight)));
        assert_eq!(Slot::of_output("PIMAX-UPRIGHT"), Some(Slot::UpRight));
        assert_eq!(extra_slots(6), EXTRA);
        assert_eq!(extra_slots(5), &EXTRA[..4]);
        assert_eq!(parse_count("6"), Ok(6));
        assert!(parse_count("7").is_err());
        let main = desktop().remove(0);
        let w = wanted(6, &main);
        assert_eq!(
            w.iter().map(|w| (w.name, w.x, w.y)).collect::<Vec<_>>(),
            [("PIMAX-LEFT", -1920, 0), ("PIMAX-RIGHT", 1920, 0), ("PIMAX-UP", 0, -1080), ("PIMAX-UPLEFT", -1920, -1080), ("PIMAX-UPRIGHT", 1920, -1080)]
        );
        assert_eq!(w[4].rule(), r#"hl.monitor({ output = "PIMAX-UPRIGHT", mode = "1920x1080@60", position = "1920x-1080", scale = 1 })"#);
        // the state file knows the new keys and the new limit
        let st = ShowState { limit: 6, pinned: vec![Slot::UpLeft, Slot::UpRight], sort: true, hidden: Vec::new() };
        assert_eq!(ShowState::parse(&st.render()), st);
        assert_eq!(ShowState::parse("limit = 6\n").limit, 6);
        assert_eq!(ShowState::parse("limit = 7\n").limit, MAX);
    }

    #[test]
    fn visibility_with_six_screens() {
        let mons = six();
        let apps: HashMap<i64, usize> = [(2, 1), (14, 1), (15, 2)].into();
        let v = visibility(&mons, &apps, &ShowState::default());
        assert_eq!(
            v.iter().map(|x| (x.0, x.2)).collect::<Vec<_>>(),
            [(Slot::Main, true), (Slot::Left, false), (Slot::Right, false), (Slot::Up, false), (Slot::UpLeft, true), (Slot::UpRight, true)]
        );
        // the limit counts the main screen: 2 = main + the first screen that wants to show
        let two = ShowState { limit: 2, ..ShowState::default() };
        assert_eq!(visibility(&mons, &apps, &two).iter().filter(|x| x.2).count(), 2);
        let all = ShowState { limit: 6, pinned: EXTRA.to_vec(), sort: true, hidden: Vec::new() };
        assert_eq!(visibility(&mons, &HashMap::new(), &all).iter().filter(|x| x.2).count(), 6);
        let args = desk_screens_args(&visibility(&mons, &apps, &ShowState::default()), None, false);
        assert_eq!(args[args.len() - 2..], ["up-left=PIMAX-UPLEFT", "up-right=PIMAX-UPRIGHT"]);
        assert_eq!(screen_workspaces(&mons).len(), 6);
    }

    fn plan(cmd: ShowCmd, present: &[Slot], shown: &[Slot], old: &ShowState) -> ShowPlan {
        plan_show(&cmd, present, shown, old).unwrap()
    }

    #[test]
    fn show_n_pins_the_first_extras_in_order() {
        let old = ShowState { limit: 2, pinned: vec![Slot::Up], sort: false, hidden: Vec::new() };
        let p = plan(ShowCmd::Show(4), &EXTRA, &[], &old);
        assert_eq!((p.state.limit, p.state.pinned.clone(), p.state.sort, p.setting), (4, vec![Slot::Left, Slot::Right, Slot::Up], false, 4));
        assert_eq!((p.message.as_str(), p.missing.is_empty()), ("Showing 4 screens", true));
        let p = plan(ShowCmd::Show(6), &EXTRA, &[], &old);
        assert_eq!((p.state.limit, p.state.pinned.len(), p.message.as_str()), (6, 5, "Showing 6 screens"));
        let p = plan(ShowCmd::Show(1), &EXTRA, &[Slot::Left], &old);
        assert_eq!((p.state.limit, p.state.pinned.len(), p.message.as_str()), (1, 0, "Showing 1 screen"));
        // above 6: the most
        let p = plan(ShowCmd::Show(9), &EXTRA, &[], &old);
        assert_eq!((p.state.limit, p.setting, p.message.as_str()), (6, 6, "That's the most: 6 screens"));
        assert!(plan_show(&ShowCmd::Show(0), &EXTRA, &[], &old).is_err());
    }

    #[test]
    fn show_more_counts_what_is_shown_now() {
        let old = ShowState::default();
        let p = plan(ShowCmd::More(2), &EXTRA, &[Slot::Left], &old); // main + left shown: 2 more -> 4
        assert_eq!((p.state.limit, p.state.pinned.clone()), (4, vec![Slot::Left, Slot::Right, Slot::Up]));
        let p = plan(ShowCmd::More(5), &EXTRA, &[Slot::Left, Slot::Right], &old);
        assert_eq!((p.state.limit, p.message.as_str()), (6, "That's the most: 6 screens"));
        let p = plan(ShowCmd::More(1), &EXTRA, &[], &old);
        assert_eq!((p.state.limit, p.state.pinned.clone(), p.message.as_str()), (2, vec![Slot::Left], "Showing 2 screens"));
    }

    #[test]
    fn missing_outputs_are_not_made_live_and_the_rest_shows() {
        let four = &EXTRA[..3];
        let p = plan(ShowCmd::Show(6), four, &[], &ShowState::default());
        assert_eq!(p.missing, [Slot::UpLeft, Slot::UpRight]);
        assert_eq!((p.state.limit, p.state.pinned.len(), p.setting), (6, 3, 6));
        assert_eq!(p.message, "6 screens need Screens = 6 in the Pimax app and a Work restart. Showing 4 screens");
        let p = plan(ShowCmd::Show(99), four, &[], &ShowState::default());
        assert_eq!(p.message, "That's the most: 6 screens. 6 screens need Screens = 6 in the Pimax app and a Work restart. Showing 4 screens");
        let p = plan(ShowCmd::Add(Slot::UpRight), four, &[], &ShowState::default());
        assert_eq!((p.missing.clone(), p.setting, p.state), (vec![Slot::UpRight], 6, ShowState::default()));
        assert_eq!(p.message, "The up right screen needs Screens = 6 in the Pimax app and a Work restart");
    }

    #[test]
    fn add_pins_one_screen_and_raises_the_limit() {
        let old = ShowState { limit: 3, pinned: vec![Slot::Left, Slot::Right], sort: true, hidden: Vec::new() };
        let p = plan(ShowCmd::Add(Slot::UpLeft), &EXTRA, &[Slot::Left, Slot::Right], &old);
        assert_eq!((p.state.limit, p.state.pinned.clone(), p.setting), (4, vec![Slot::Left, Slot::Right, Slot::UpLeft], 5));
        assert_eq!(p.message, "Showing the up left screen");
        // already shown: nothing more is needed
        let p = plan(ShowCmd::Add(Slot::Left), &EXTRA, &[Slot::Left, Slot::Right], &old);
        assert_eq!((p.state.limit, p.state.pinned.len()), (3, 2));
        // a limit of 1 (cinema) is lifted just enough
        let p = plan(ShowCmd::Add(Slot::Up), &EXTRA, &[], &ShowState { limit: 1, pinned: vec![], sort: true, hidden: Vec::new() });
        // hide (live 2026-10-16 "remove screen number four and three", "close everything"): no app closes
        assert_eq!(parse_show("hide", Some("4,3")), Ok(ShowCmd::Hide(vec![Slot::Up, Slot::Right])));
        assert_eq!(parse_show("hide", Some("others")), Ok(ShowCmd::Hide(EXTRA.to_vec())));
        assert!(parse_show("hide", Some("1")).is_err() && parse_show("hide", Some("9")).is_err());
        let old4 = ShowState { limit: 4, pinned: vec![Slot::Left, Slot::Right, Slot::Up], sort: true, hidden: Vec::new() };
        let h = plan(ShowCmd::Hide(vec![Slot::Up, Slot::Right]), &EXTRA, &[Slot::Left, Slot::Right, Slot::Up], &old4);
        assert_eq!(h.state.pinned, vec![Slot::Left]);
        assert_eq!(h.state.hidden, vec![Slot::Up, Slot::Right]);
        assert_eq!(h.message, "Hid screen 4 and 3; its apps stay open");
        let back = plan(ShowCmd::Add(Slot::Up), &EXTRA, &[Slot::Left], &h.state);
        assert_eq!(back.state.hidden, vec![Slot::Right], "add takes the slot out of hidden");
        let only = plan(ShowCmd::Hide(EXTRA.to_vec()), &EXTRA, &[Slot::Left], &old4);
        assert_eq!((only.state.limit, only.state.pinned.len()), (1, 0));
        assert_eq!(ShowState::parse(&only.state.render()), only.state, "hidden round-trips");
        assert!(plan(ShowCmd::Show(3), &EXTRA, &[], &only.state).state.hidden.is_empty(), "show N clears hidden");
        assert_eq!((p.state.limit, p.state.pinned.clone(), p.setting), (2, vec![Slot::Up], 4));
        assert_eq!(parse_show("add", Some("main")).unwrap_err(), "The main screen is always shown");
    }

    #[test]
    fn show_command_parsing() {
        assert_eq!(parse_show("show", Some("4")), Ok(ShowCmd::Show(4)));
        assert_eq!(parse_show("more", Some("2")), Ok(ShowCmd::More(2)));
        assert_eq!(parse_show("add", Some("up-left")), Ok(ShowCmd::Add(Slot::UpLeft)));
        assert!(parse_show("show", None).is_err() && parse_show("show", Some("x")).is_err());
        assert!(parse_show("add", Some("sideways")).unwrap_err().starts_with("No screen called sideways"));
    }

    #[test]
    fn the_recorded_screens_setting_is_raised_never_lowered() {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(argv_with_screens(&v(&["--6dof", "--screens", "2"]), 4), v(&["--6dof", "--screens", "4"]));
        assert_eq!(argv_with_screens(&v(&["--screens", "6"]), 3), v(&["--screens", "6"]));
        assert_eq!(argv_with_screens(&v(&["--screens=3", "--voice"]), 5), v(&["--screens=5", "--voice"]));
        assert_eq!(argv_with_screens(&v(&["--voice"]), 6), v(&["--voice", "--screens", "6"]));
        assert_eq!(argv_with_screens(&v(&["--voice"]), 3), v(&["--voice"]), "absent = the default 4");
        assert_eq!(argv_with_screens(&v(&["--voice"]), 9), v(&["--voice", "--screens", "6"]));
    }

    #[test]
    fn run_show_writes_the_state_tells_wayvr_and_records_the_setting() {
        struct H(Vec<String>);
        impl Hypr for H {
            fn run(&mut self, args: &[&str]) -> Result<String, String> {
                self.0.push(args.join(" "));
                match args {
                    ["-j", "monitors", "all"] => {
                        let mut m = desktop();
                        m.extend([mon("PIMAX-LEFT", 1920, -1920, 0, 11), mon("PIMAX-RIGHT", 1920, 1920, 0, 12), mon("PIMAX-UP", 1920, 0, -1080, 13)]);
                        Ok(json(&m))
                    }
                    ["-j", "clients"] => Ok("[]".into()),
                    _ => Err(format!("must not run hyprctl {args:?}")),
                }
            }
            fn sleep(&mut self, _d: Duration) {}
        }
        let d = crate::vr::test_dir("screens-run-show");
        let ctl = d.join("wayvrctl");
        let log = d.join("ctl.log");
        std::fs::write(&ctl, format!("#!/bin/sh\necho \"$*\" >> '{}'\n", log.display())).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&ctl, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let state = d.join("omarchyvr-screens.state");
        crate::vr::supervise::record_argv(&d, "desk", &["--6dof".to_string(), "--screens".into(), "4".into()]).unwrap();
        let env = ShowEnv { state_file: &state, session_dir: &d, wayvrctl: &ctl, running: true, dry_run: false };
        let mut h = H(Vec::new());
        let line = run_show(&ShowCmd::Show(6), &mut h, &env).unwrap();
        assert_eq!(line, "6 screens need Screens = 6 in the Pimax app and a Work restart. Showing 4 screens");
        assert!(h.0.iter().all(|c| c.starts_with("-j ")), "only reads (no output is made or removed live): {:?}", h.0);
        let st = read_state(&state);
        assert_eq!((st.limit, st.pinned), (6, vec![Slot::Left, Slot::Right, Slot::Up]));
        let calls = std::fs::read_to_string(&log).unwrap();
        assert!(calls.starts_with("desk-screens main=SUNSHINE left=PIMAX-LEFT right=PIMAX-RIGHT up=PIMAX-UP"), "{calls}");
        assert!(!calls.contains("hide"), "{calls}");
        assert_eq!(crate::vr::supervise::read_argv(&d).unwrap().1, ["--6dof", "--screens", "6"], "a restart keeps it");
        // show 2 shrinks what shows, the setting stays at 6
        let line = run_show(&ShowCmd::Show(2), &mut H(Vec::new()), &env).unwrap();
        assert_eq!(line, "Showing 2 screens");
        assert_eq!(crate::vr::supervise::read_argv(&d).unwrap().1, ["--6dof", "--screens", "6"]);
        assert!(std::fs::read_to_string(&log).unwrap().contains("right=PIMAX-RIGHT:hide"));
        // no session: a plain error, nothing written
        let _ = std::fs::remove_file(&state);
        let off = ShowEnv { running: false, ..ShowEnv { state_file: &state, session_dir: &d, wayvrctl: &ctl, running: true, dry_run: false } };
        assert_eq!(run_show(&ShowCmd::Show(3), &mut H(Vec::new()), &off).unwrap_err(), "No Work session is running");
        assert!(!state.exists());
    }
}
