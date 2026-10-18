//! Desk layouts (presets): `omarchyvr desk start [--preset NAME]` (work-4 is the default) and the live switch
//! `omarchyvr desk preset NAME [--dry-run]` (voice "Max, focus mode", the app's Layout picker).
//!
//! **Locked, fixed layouts (user decisions 2026-10-18, docs/SCREENS.md).** The VR screens never move by themselves
//! and cannot be moved by hand or by voice: WayVR's `desk_locked` (written `true` into every preset's conf.d file)
//! refuses grab/move/resize, `screen-nudge` / `screen-place`, and every auto-reset of a desk screen, and places the
//! screens around a stored **layout anchor** (yaw + position, `<wayvr home>/config/wayvr/desk-anchor.conf`), not
//! around the head. `omarchyvr desk recenter` (voice "Max, recenter", the app's Recenter screens) sets the anchor
//! from the head now: the whole layout comes in front of the user with its shape kept. The next Work start puts the
//! layout where it was (first ever start: in front of the head).
//!
//! The geometry of every layout is ONE table, [`Preset::layout`] (sent to WayVR as `desk-screens --place --width W
//! --distance D --curvature C`; WayVR's `viewlayout::default_layout` puts the screens edge to edge around main,
//! 1.5 deg apart as seen from the anchor, each turned to face the user). Screen numbers: 1 main, 2 left, 3 right,
//! 4 up, 5 up-left, 6 up-right.
//!
//! | layout | screens shown | width x distance | curvature | passthrough |
//! |---|---|---|---|---|
//! | work-4 (default; `work`) | 1 2 3 4 | 1.6 m at 2.0 m | flat | normal |
//! | focus-3 (`focus`; old `work-3`) | 1 2 3 | 1.2 m at 1.3 m | flat | normal |
//! | work-5 | 1 2 3 5 6 | 1.6 m at 2.0 m | flat | normal |
//! | work-6 (old `assistant`) | 1-6 | 1.6 m at 2.0 m | flat | normal |
//! | cinema (old `work-1`) | 1 | 3.2 m at 3.5 m | 0.35 | dimmed (gain 0.35) |
//! | cinema-huge (`imax`, `huge-cinema`, `theater`) | 1 | 11 m at 8 m (68 deg) | 0.2 | off (black), backdrop off |
//!
//! Old names parse as aliases of the closest layout: `work-3` -> focus-3 (three screens), `work-1` -> cinema (one
//! screen), `assistant` -> work-6 (every screen; `--preset assistant` still turns the voice captions on at start).
//!
//! At start the preset is written to `<wayvr home>/config/wayvr/conf.d/60-preset.yaml` (rewritten on every start,
//! removed with `--preset none`, which only `desk start` takes) and adds launcher options; once WayVR runs the layout
//! is applied over IPC. The live switch rewrites the same file and applies everything without restarting anything:
//! * the backdrop through the passthrough control socket (`omarchyvr-passthrough ctl`: dim / normal / off);
//! * with VR screens (Hyprland outputs `PIMAX-*`, `desk start --screens N`, see `screens`): `wayvrctl desk-screens
//!   --place --hide-others` with the layout's numbers; the show state (`$XDG_RUNTIME_DIR/omarchyvr-screens.state`)
//!   pins exactly the layout's screens; the outputs a layout needs are made at start (`--screens` is raised to them);
//! * a session with one output only: `wayvrctl desk-layout N` (an arc) and, for 3+ screens, WayVR app panels
//!   (`wayvrctl process-launch`, from `~/.config/omarchyvr/work-panels.conf`, default a `foot` terminal on each side).
//!   Panels that already run are not started again; cinema hides them (they keep running).
//!
//! cinema-huge: a mid-theatre seat, 11 m wide at 8 m, 68 degrees ([`huge_size`]). It needs the WayVR fork with the
//! 20 m caps (branch `no-auto-reset`); with an older WayVR set [`WAYVR_MAX_WIDTH`] to 6.0. docs/CINEMA.md.
//!
//! `default_curvature` in the YAML applies to screens without a saved curvature; the live layout sets it on every
//! shown screen. `screen_max_height` (capture size) only changes at the next start.
use super::screens::Slot;
use crate::args::Args;
use crate::CliError;
use std::path::{Path, PathBuf};

/// File name in conf.d (after the launcher's 10-40 files and the Pimax app's 50-pimax-app.yaml: a preset wins).
pub const CONF_NAME: &str = "60-preset.yaml";

/// The layout used when `desk start` gets no `--preset` (user decision 2026-10-18: Work 4).
pub const DEFAULT: Preset = Preset::Work4;

/// A desk layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    /// 1 main in front, 2 left, 3 right (turned toward you), 4 up above the main; medium screens.
    Work4,
    /// Three medium screens close to you in a gentle arc (1 main, 2 left, 3 right).
    Focus3,
    /// Main, left, right, up-left, up-right.
    Work5,
    /// All six screens.
    Work6,
    /// One big curved screen, the room dimmed.
    Cinema,
    /// A huge cinema: the main screen as big as WayVR allows, ~68 degrees wide, the room black ([`huge_size`]).
    CinemaHuge,
}

/// cinema-huge, the wish: a mid-theatre seat, an 11 m screen 8 m away (68 degrees of view). One source of truth.
pub const HUGE_TARGET_WIDTH: f32 = 11.0;
pub const HUGE_TARGET_DISTANCE: f32 = 8.0;
/// cinema-huge: gentle curve so the edges stay readable.
pub const HUGE_CURVATURE: f32 = 0.2;
/// What our WayVR accepts: `desk-layout` / `desk-screens` clamp width and distance to 20 m (WayVR branch
/// `no-auto-reset`, `DESK_MAX_WIDTH` / `DESK_MAX_DISTANCE`; `grab_size_max` / `grab_dist_max` 20 m). An older WayVR
/// clamped to 6 m wide / 8 m away: then set this back to 6.0 and the preset keeps the angle at 6 m / 4.36 m.
pub const WAYVR_MAX_WIDTH: f32 = 12.0;

/// The huge cinema's (width, distance) in metres: the target, scaled down together (same angle on the eye) when WayVR's
/// width cap is lower (6 m at 4.36 m with a 6 m cap); 11 m at 8 m with today's 12 m.
pub fn huge_size() -> (f32, f32) {
    let k = (WAYVR_MAX_WIDTH / HUGE_TARGET_WIDTH).min(1.0);
    let r = |x: f32| (x * 100.0).round() / 100.0;
    (r(HUGE_TARGET_WIDTH * k), r(HUGE_TARGET_DISTANCE * k))
}

/// The screen layout of a preset: the one geometry table.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    /// The screens shown, by slot (screen numbers: 1 main, 2 left, 3 right, 4 up, 5 up-left, 6 up-right).
    pub slots: &'static [Slot],
    /// Screens shown (= `slots.len()`; `wayvrctl desk-layout N` in a one-output session).
    pub screens: u32,
    pub curvature: f32,
    /// Width of each screen, metres.
    pub width: f32,
    /// Distance from the head (the layout anchor), metres.
    pub distance: f32,
}

const MAIN: &[Slot] = &[Slot::Main];
const FOUR: &[Slot] = &[Slot::Main, Slot::Left, Slot::Right, Slot::Up];
const THREE: &[Slot] = &[Slot::Main, Slot::Left, Slot::Right];
const FIVE: &[Slot] = &[Slot::Main, Slot::Left, Slot::Right, Slot::UpLeft, Slot::UpRight];
const SIX: &[Slot] = &[Slot::Main, Slot::Left, Slot::Right, Slot::Up, Slot::UpLeft, Slot::UpRight];

impl Preset {
    /// The horizontal field of view in degrees a screen of `width` metres at `distance` metres fills.
    pub fn fov_deg(width: f32, distance: f32) -> f32 {
        2.0 * (width / 2.0 / distance).atan().to_degrees()
    }

    pub const ALL: [Preset; 6] = [Preset::Work4, Preset::Focus3, Preset::Work5, Preset::Work6, Preset::Cinema, Preset::CinemaHuge];

    /// `none` -> None. Aliases: `work` = work-4, `focus` = focus-3, and the old names (`work-3` = focus-3,
    /// `work-1` = cinema, `assistant` = work-6).
    pub fn parse(s: &str) -> Result<Option<Self>, CliError> {
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "none" => None,
            "work-4" | "work4" | "work_4" | "work" | "work-mode" | "four" => Some(Self::Work4),
            "focus-3" | "focus3" | "focus_3" | "focus" | "work-3" | "work3" => Some(Self::Focus3),
            "work-5" | "work5" | "work_5" | "five" => Some(Self::Work5),
            "work-6" | "work6" | "work_6" | "six" | "all" | "assistant" | "llm" => Some(Self::Work6),
            "cinema" | "movie" | "work-1" | "work1" => Some(Self::Cinema),
            "cinema-huge" | "cinema_huge" | "cinemahuge" | "huge-cinema" | "huge_cinema" | "huge" | "imax" | "theater" | "theatre" => {
                Some(Self::CinemaHuge)
            }
            _ => {
                return Err(CliError::usage(format!(
                    "--preset: work-4 (default), focus-3, work-5, work-6, cinema, cinema-huge (imax) or none, not {s}"
                )))
            }
        })
    }

    /// The old `assistant` name also turned the voice captions on at start; it still does.
    pub fn captions_alias(s: &str) -> bool {
        matches!(s.trim().to_ascii_lowercase().as_str(), "assistant" | "llm")
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Work4 => "work-4",
            Self::Focus3 => "focus-3",
            Self::Work5 => "work-5",
            Self::Work6 => "work-6",
            Self::Cinema => "cinema",
            Self::CinemaHuge => "cinema-huge",
        }
    }

    /// The name people see ("Work 4", "Huge cinema").
    pub fn label(self) -> &'static str {
        match self {
            Self::Work4 => "Work 4",
            Self::Focus3 => "Focus 3",
            Self::Work5 => "Work 5",
            Self::Work6 => "Work 6",
            Self::Cinema => "Cinema",
            Self::CinemaHuge => "Huge cinema",
        }
    }

    /// The conf.d text (the first line names the preset; `omarchyvr-voice` reads it back for "undo").
    pub fn conf(self) -> String {
        let body = match self {
            Self::Cinema => "default_curvature: 0.35\n",
            Self::CinemaHuge => "default_curvature: 0.2\n",
            Self::Work4 | Self::Focus3 | Self::Work5 | Self::Work6 => "default_curvature: 0.0\nscreen_max_height: 2160\n",
        };
        format!(
            "# omarchyvr desk --preset {} (rewritten every start and by `omarchyvr desk preset`)\n{body}# locked layouts: screens never move by themselves or by hand; `desk recenter` moves the whole layout\ndesk_locked: true\n",
            self.name()
        )
    }

    /// Extra `omarchyvr-passthrough run` arguments.
    pub fn passthrough_args(self) -> &'static [&'static str] {
        match self {
            Self::Cinema => &["--gain", "0.35"],
            // a dark room: the passthrough app starts with no camera layer (black)
            Self::CinemaHuge => &["--off"],
            _ => &[],
        }
    }

    /// The live passthrough command (`omarchyvr-passthrough ctl ...`).
    pub fn passthrough_ctl(self) -> &'static str {
        match self {
            Self::Cinema => "dim",
            Self::CinemaHuge => "off",
            _ => "normal",
        }
    }

    /// The preset sets the WayVR skybox/backdrop to off too (a dark room); the voice layer puts the old one back.
    pub fn skybox_off(self) -> bool {
        self == Self::CinemaHuge
    }

    /// The Hyprland outputs this layout needs (`desk start --screens` is raised to it): the highest screen number.
    pub fn outputs_needed(self) -> u8 {
        self.layout().slots.iter().map(|s| super::screens::number_of(*s) as u8).max().unwrap_or(1)
    }

    /// THE geometry table (docs/SCREENS.md "Locked layouts").
    pub fn layout(self) -> Layout {
        let l = |slots: &'static [Slot], curvature, width, distance| Layout { slots, screens: slots.len() as u32, curvature, width, distance };
        match self {
            Self::Work4 => l(FOUR, 0.0, 1.6, 2.0),
            Self::Focus3 => l(THREE, 0.0, 1.2, 1.3),
            Self::Work5 => l(FIVE, 0.0, 1.6, 2.0),
            Self::Work6 => l(SIX, 0.0, 1.6, 2.0),
            Self::Cinema => l(MAIN, 0.35, 3.2, 3.5),
            Self::CinemaHuge => l(MAIN, HUGE_CURVATURE, huge_size().0, huge_size().1),
        }
    }
}

/// Remove and parse `--preset NAME`; without it the default layout (work-4).
pub fn parse_opt(a: &mut Args) -> Result<Option<Preset>, CliError> {
    Ok(parse_opt_captions(a)?.0)
}

/// [`parse_opt`], and whether the name was the old `assistant` (voice captions on).
pub fn parse_opt_captions(a: &mut Args) -> Result<(Option<Preset>, bool), CliError> {
    match a.opt("--preset")? {
        Some(v) => Ok((Preset::parse(&v)?, Preset::captions_alias(&v))),
        None => Ok((Some(DEFAULT), false)),
    }
}

/// Write (or, without a preset, remove) `conf.d/60-preset.yaml` of a WayVR home.
pub fn write_conf(wayvr_home: &Path, p: Option<Preset>) -> std::io::Result<()> {
    let conf_d = wayvr_home.join("config/wayvr/conf.d");
    let f = conf_d.join(CONF_NAME);
    match p {
        Some(p) => {
            std::fs::create_dir_all(&conf_d)?;
            std::fs::write(f, p.conf())
        }
        None => match std::fs::remove_file(f) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        },
    }
}

/// The preset named in a WayVR home's `60-preset.yaml` (None: no file or no preset line).
pub fn read_conf(wayvr_home: &Path) -> Option<Preset> {
    let t = std::fs::read_to_string(wayvr_home.join("config/wayvr/conf.d").join(CONF_NAME)).ok()?;
    let first = t.lines().next()?;
    let name = first.strip_prefix("# omarchyvr desk --preset ")?.split_whitespace().next()?;
    Preset::parse(name).ok().flatten()
}

/// One app panel of work-3.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Panel {
    /// WayVR process name (`work-left`, `work-right`).
    pub name: String,
    pub exec: String,
    pub args: String,
}

/// `~/.config/omarchyvr/work-panels.conf` (`left = CMD ARGS`, `right = CMD ARGS`, `#` comments); missing keys: `foot`.
pub fn parse_panels(text: Option<&str>) -> Vec<Panel> {
    let mut v = vec![("left", "foot".to_string()), ("right", "foot".to_string())];
    for l in text.unwrap_or("").lines() {
        let l = l.trim();
        if l.starts_with('#') {
            continue;
        }
        if let Some((k, val)) = l.split_once('=') {
            let (k, val) = (k.trim(), val.trim());
            if let Some(slot) = v.iter_mut().find(|(n, _)| *n == k) {
                if !val.is_empty() {
                    slot.1 = val.to_string();
                }
            }
        }
    }
    v.into_iter()
        .map(|(side, cmd)| {
            let mut it = cmd.splitn(2, char::is_whitespace);
            Panel {
                name: format!("work-{side}"),
                exec: it.next().unwrap_or("foot").to_string(),
                args: it.next().unwrap_or("").trim().to_string(),
            }
        })
        .collect()
}

pub fn panels_path(home: &Path) -> PathBuf {
    home.join(".config/omarchyvr/work-panels.conf")
}

/// One step of a live preset switch.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// A line for the passthrough control socket.
    Passthrough(&'static str),
    /// Start an app panel inside WayVR unless one with this name runs.
    Panel(Panel),
    /// `wayvrctl ARGS`.
    Wayvrctl(Vec<String>),
}

/// What `omarchyvr desk preset P` does, in order. `screens`: the VR screens' visibility under this preset (Some when
/// the session has `PIMAX-*` outputs): then `desk-screens` replaces the arc and the app panels. The layout is always
/// placed with its own numbers: WayVR (locked) builds it around the layout anchor, so a switch never moves the
/// layout to where the head looks now; only `desk recenter` does that.
pub fn live_steps(p: Preset, panels: &[Panel], screens: Option<&[(super::screens::Slot, String, bool)]>) -> Vec<Step> {
    let mut v = vec![Step::Passthrough(p.passthrough_ctl())];
    if p.skybox_off() {
        v.push(Step::Wayvrctl(vec!["skybox".into(), "off".into()]));
    }
    let l = p.layout();
    if let Some(vis) = screens.filter(|s| s.len() > 1) {
        v.push(Step::Wayvrctl(super::screens::desk_screens_args(vis, Some((l.width, l.distance, l.curvature)), true)));
        return v;
    }
    if l.screens >= 3 {
        v.extend(panels.iter().take(l.screens as usize - 1).cloned().map(Step::Panel));
    }
    v.push(Step::Wayvrctl(vec![
        "desk-layout".into(),
        l.screens.to_string(),
        "--curvature".into(),
        format!("{}", l.curvature),
        "--width".into(),
        format!("{}", l.width),
        "--distance".into(),
        format!("{}", l.distance),
    ]));
    v
}

/// `wayvrctl process-launch` arguments of a panel (Static: placed by the layout that follows).
pub fn panel_launch_args(p: &Panel) -> Vec<String> {
    let mut v = vec![
        "process-launch".into(),
        "--name".into(),
        p.name.clone(),
        p.exec.clone(),
        "1600x1000".into(),
        "static".into(),
    ];
    if !p.args.is_empty() {
        // positional `icon` comes before `args`: an empty icon path
        v.push(String::new());
        v.push(p.args.clone());
    }
    v
}

/// Send one line to the passthrough control socket.
pub fn passthrough_send(sock: &Path, line: &str) -> Result<String, String> {
    use std::io::{BufRead, BufReader, Write};
    let mut s = std::os::unix::net::UnixStream::connect(sock)
        .map_err(|e| format!("{}: {e} (passthrough not running, or a build without the control socket)", sock.display()))?;
    let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(2)));
    writeln!(s, "{line}").map_err(|e| e.to_string())?;
    let mut r = String::new();
    BufReader::new(s).read_line(&mut r).map_err(|e| e.to_string())?;
    Ok(r.trim().to_string())
}

/// The passthrough control socket (same rule as `omarchyvr-passthrough`).
pub fn passthrough_socket(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    if let Some(p) = env("PIMAX_PASSTHROUGH_SOCKET").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    match env("XDG_RUNTIME_DIR").filter(|p| !p.is_empty()) {
        Some(d) => PathBuf::from(d).join("omarchyvr-passthrough.sock"),
        // SAFETY: getuid cannot fail.
        None => PathBuf::from(format!("/tmp/omarchyvr-passthrough-{}.sock", unsafe { libc::getuid() })),
    }
}

/// Run the live steps (or print them with `dry_run`). Returns notes; an error only when nothing could be applied.
pub fn apply_live(
    p: Preset,
    wayvrctl: &Path,
    pt_sock: &Path,
    panels: &[Panel],
    screens: Option<&[(super::screens::Slot, String, bool)]>,
    dry_run: bool,
    out: &mut dyn FnMut(&str),
) -> Result<(), String> {
    use std::process::{Command, Stdio};
    let mut failures = Vec::new();
    let steps = live_steps(p, panels, screens);
    let mut launched = false;
    for s in &steps {
        match s {
            Step::Passthrough(line) => {
                if dry_run {
                    out(&format!("would send `{line}` to {}", pt_sock.display()));
                    continue;
                }
                match passthrough_send(pt_sock, line) {
                    Ok(r) => out(&format!("passthrough: {r}")),
                    Err(e) => failures.push(format!("passthrough: {e}")),
                }
            }
            Step::Panel(panel) => {
                let args = panel_launch_args(panel);
                if dry_run {
                    out(&format!("would run (unless `{}` runs) {} {}", panel.name, wayvrctl.display(), shell_join(&args)));
                    continue;
                }
                let list = Command::new(wayvrctl).arg("process-list").stderr(Stdio::null()).output();
                let running = match &list {
                    Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).contains(&format!("\"name\":\"{}\"", panel.name)),
                    _ => false,
                };
                if running {
                    out(&format!("{} already runs", panel.name));
                    continue;
                }
                match Command::new(wayvrctl).args(&args).stdout(Stdio::null()).stderr(Stdio::null()).status() {
                    Ok(st) if st.success() => {
                        out(&format!("started panel {} ({})", panel.name, panel.exec));
                        launched = true;
                    }
                    Ok(st) => failures.push(format!("wayvrctl process-launch {}: {st}", panel.name)),
                    Err(e) => failures.push(format!("{}: {e}", wayvrctl.display())),
                }
            }
            Step::Wayvrctl(args) => {
                if dry_run {
                    out(&format!("would run {} {}", wayvrctl.display(), shell_join(args)));
                    continue;
                }
                if launched {
                    // let the new windows map before they are arranged
                    std::thread::sleep(std::time::Duration::from_millis(1500));
                }
                match Command::new(wayvrctl).args(args).stdout(Stdio::null()).stderr(Stdio::null()).status() {
                    Ok(st) if st.success() => out(&format!("layout: {}", shell_join(args))),
                    Ok(st) => failures.push(format!(
                        "wayvrctl {} failed ({st}): WayVR not running or older than aac483a; the layout applies at the next start",
                        args[0]
                    )),
                    Err(e) => failures.push(format!("{}: {e}", wayvrctl.display())),
                }
            }
        }
    }
    for f in &failures {
        out(&format!("WARNING: {f}"));
    }
    if !dry_run && failures.len() >= steps.len() {
        return Err("nothing applied live (desk mode not running?)".into());
    }
    Ok(())
}

fn shell_join(v: &[String]) -> String {
    v.iter()
        .map(|s| if s.is_empty() || s.contains(' ') { format!("'{s}'") } else { s.clone() })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_names_aliases_and_default() {
        let mut a = Args(vec!["--preset".into(), "cinema".into(), "--eye".into()]);
        assert_eq!(parse_opt(&mut a).unwrap(), Some(Preset::Cinema));
        assert_eq!(a.0, ["--eye"]);
        assert_eq!(parse_opt(&mut Args(vec![])).unwrap(), Some(Preset::Work4), "work-4 is the default (2026-10-18)");
        assert_eq!(DEFAULT, Preset::Work4);
        assert_eq!(Preset::parse("none").unwrap(), None);
        for (n, p) in [
            ("work", Preset::Work4),
            ("work4", Preset::Work4),
            ("focus", Preset::Focus3),
            ("Focus-3", Preset::Focus3),
            ("work5", Preset::Work5),
            ("work6", Preset::Work6),
            // the old names: the closest new layout
            ("work-3", Preset::Focus3),
            ("work3", Preset::Focus3),
            ("work-1", Preset::Cinema),
            ("assistant", Preset::Work6),
            ("llm", Preset::Work6),
        ] {
            assert_eq!(Preset::parse(n).unwrap(), Some(p), "{n}");
        }
        assert!(Preset::captions_alias("assistant") && !Preset::captions_alias("work-6"));
        let mut a = Args(vec!["--preset".into(), "assistant".into()]);
        assert_eq!(parse_opt_captions(&mut a).unwrap(), (Some(Preset::Work6), true));
        assert_eq!(parse_opt(&mut Args(vec!["--preset=disco".into()])).unwrap_err().code, 2);
        for p in Preset::ALL {
            assert_eq!(Preset::parse(p.name()).unwrap(), Some(p));
            assert!(!p.label().is_empty());
        }
        assert_eq!(Preset::Cinema.passthrough_args(), ["--gain", "0.35"]);
    }

    #[test]
    fn the_geometry_table() {
        use Slot::*;
        let row = |p: Preset| {
            let l = p.layout();
            (l.slots.to_vec(), l.screens, l.width, l.distance, l.curvature)
        };
        assert_eq!(row(Preset::Work4), (vec![Main, Left, Right, Up], 4, 1.6, 2.0, 0.0));
        assert_eq!(row(Preset::Focus3), (vec![Main, Left, Right], 3, 1.2, 1.3, 0.0));
        assert_eq!(row(Preset::Work5), (vec![Main, Left, Right, UpLeft, UpRight], 5, 1.6, 2.0, 0.0));
        assert_eq!(row(Preset::Work6), (vec![Main, Left, Right, Up, UpLeft, UpRight], 6, 1.6, 2.0, 0.0));
        assert_eq!(row(Preset::Cinema), (vec![Main], 1, 3.2, 3.5, 0.35));
        assert_eq!(row(Preset::CinemaHuge), (vec![Main], 1, 11.0, 8.0, 0.2));
        // the outputs a layout needs: the highest screen number in it
        let need: Vec<u8> = Preset::ALL.iter().map(|p| p.outputs_needed()).collect();
        assert_eq!(need, [4, 3, 6, 6, 1, 1]);
        // focus-3 is closer than work-4 but about as wide on the eye
        let (f, w) = (Preset::Focus3.layout(), Preset::Work4.layout());
        assert!(f.distance < w.distance && (Preset::fov_deg(f.width, f.distance) - Preset::fov_deg(w.width, w.distance)).abs() < 10.0);
        // every work layout is flat and passthrough normal; every conf locks the screens
        for p in Preset::ALL {
            assert!(p.conf().contains("\ndesk_locked: true\n"), "{p:?}");
            if p.layout().screens > 1 {
                assert_eq!((p.layout().curvature, p.passthrough_ctl()), (0.0, "normal"), "{p:?}");
            }
        }
    }

    #[test]
    fn cinema_huge_names_numbers_and_steps() {
        for n in ["cinema-huge", "huge-cinema", "imax", "theater", "theatre", "huge", "cinema_huge"] {
            assert_eq!(Preset::parse(n).unwrap(), Some(Preset::CinemaHuge), "{n}");
        }
        assert_eq!(Preset::CinemaHuge.name(), "cinema-huge");
        let l = Preset::CinemaHuge.layout();
        assert_eq!((l.screens, l.width, l.distance, l.curvature), (1, 11.0, 8.0, 0.2));
        assert!(l.width <= WAYVR_MAX_WIDTH && l.distance >= 0.4);
        // the same angle as the wish (11 m at 8 m = 68 degrees), 65..75 degrees
        let fov = Preset::fov_deg(l.width, l.distance);
        let wish = Preset::fov_deg(HUGE_TARGET_WIDTH, HUGE_TARGET_DISTANCE);
        assert!((65.0..=75.0).contains(&fov) && (fov - wish).abs() < 0.5, "{fov} {wish}");
        let normal = Preset::Cinema.layout();
        assert!(fov > Preset::fov_deg(normal.width, normal.distance) + 10.0);
        assert_eq!(Preset::CinemaHuge.passthrough_args(), ["--off"]);
        assert_eq!(Preset::CinemaHuge.passthrough_ctl(), "off");
        assert!(Preset::CinemaHuge.conf().contains("default_curvature: 0.2"));
        // the live steps: room black, backdrop off, one screen
        let s = live_steps(Preset::CinemaHuge, &[], None);
        assert_eq!(s.len(), 3);
        assert_eq!(s[0], Step::Passthrough("off"));
        assert_eq!(s[1], Step::Wayvrctl(vec!["skybox".into(), "off".into()]));
        assert_eq!(
            s[2],
            Step::Wayvrctl(["desk-layout", "1", "--curvature", "0.2", "--width", "11", "--distance", "8"].map(String::from).to_vec())
        );
        // with VR screens: placed (--place) with the huge numbers, the others hidden
        let vis = vec![(Slot::Main, "SUNSHINE".to_string(), true), (Slot::Left, "PIMAX-LEFT".to_string(), false)];
        let s = live_steps(Preset::CinemaHuge, &[], Some(&vis));
        let Step::Wayvrctl(a) = &s[2] else { panic!("{s:?}") };
        let line = a.join(" ");
        assert!(line.starts_with("desk-screens --place --width 11 --distance 8 --curvature 0.2 --hide-others"), "{line}");
        // only the huge cinema turns the backdrop off
        assert!(!live_steps(Preset::Cinema, &[], Some(&vis)).iter().any(|s| matches!(s, Step::Wayvrctl(a) if a[0] == "skybox")));
        // the conf round-trips
        let d = crate::vr::test_dir("preset-huge");
        write_conf(&d, Some(Preset::CinemaHuge)).unwrap();
        assert_eq!(read_conf(&d), Some(Preset::CinemaHuge));
    }

    #[test]
    fn conf_files_are_written_read_back_and_removed() {
        let d = crate::vr::test_dir("preset");
        let f = d.join("config/wayvr/conf.d").join(CONF_NAME);
        write_conf(&d, Some(Preset::Focus3)).unwrap();
        let t = std::fs::read_to_string(&f).unwrap();
        assert!(t.contains("default_curvature: 0.0") && t.contains("screen_max_height: 2160") && t.contains("desk_locked: true"));
        assert_eq!(read_conf(&d), Some(Preset::Focus3));
        write_conf(&d, Some(Preset::Cinema)).unwrap();
        assert!(std::fs::read_to_string(&f).unwrap().contains("default_curvature: 0.35"));
        assert_eq!(read_conf(&d), Some(Preset::Cinema));
        // an old file (work-3) reads back as its new layout
        std::fs::write(&f, "# omarchyvr desk --preset work-3 (old)\ndefault_curvature: 0.0\n").unwrap();
        assert_eq!(read_conf(&d), Some(Preset::Focus3));
        write_conf(&d, None).unwrap();
        assert!(!f.exists());
        assert_eq!(read_conf(&d), None);
        write_conf(&d, None).unwrap();
        // every preset is valid flat YAML: `key: value` lines and comments only
        for p in Preset::ALL {
            for l in p.conf().lines() {
                assert!(l.starts_with('#') || l.split_once(": ").is_some_and(|(k, v)| !k.contains(' ') && !v.is_empty()), "{l}");
            }
        }
    }

    #[test]
    fn panels_config() {
        let d = parse_panels(None);
        assert_eq!(d.len(), 2);
        assert_eq!((d[0].name.as_str(), d[0].exec.as_str(), d[0].args.as_str()), ("work-left", "foot", ""));
        let c = parse_panels(Some("# mine\nright = nautilus --new-window\nleft=\nbogus = x\n"));
        assert_eq!(c[0].exec, "foot");
        assert_eq!((c[1].exec.as_str(), c[1].args.as_str()), ("nautilus", "--new-window"));
        assert_eq!(panel_launch_args(&c[0]), ["process-launch", "--name", "work-left", "foot", "1600x1000", "static"]);
        assert_eq!(panel_launch_args(&c[1])[6..], [String::new(), "--new-window".to_string()]);
    }

    #[test]
    fn live_steps_per_preset_with_one_output() {
        let panels = parse_panels(None);
        let s = live_steps(Preset::Focus3, &panels, None);
        assert_eq!(s[0], Step::Passthrough("normal"));
        assert!(matches!(&s[1], Step::Panel(p) if p.name == "work-left"));
        assert!(matches!(&s[2], Step::Panel(p) if p.name == "work-right"));
        assert_eq!(
            s[3],
            Step::Wayvrctl(["desk-layout", "3", "--curvature", "0", "--width", "1.2", "--distance", "1.3"].map(String::from).to_vec())
        );
        let s = live_steps(Preset::Cinema, &panels, None);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0], Step::Passthrough("dim"));
        assert!(matches!(&s[1], Step::Wayvrctl(a) if a[1] == "1" && a[3] == "0.35"));
        // two panels at most (left and right), whatever the layout's count
        assert_eq!(live_steps(Preset::Work6, &panels, None).iter().filter(|s| matches!(s, Step::Panel(_))).count(), 2);
    }

    #[test]
    fn with_vr_screens_the_layout_is_placed_with_its_numbers_and_starts_no_panels() {
        let vis = vec![
            (Slot::Main, "SUNSHINE".to_string(), true),
            (Slot::Left, "PIMAX-LEFT".to_string(), true),
            (Slot::Right, "PIMAX-RIGHT".to_string(), true),
            (Slot::Up, "PIMAX-UP".to_string(), true),
            (Slot::UpLeft, "PIMAX-UPLEFT".to_string(), false),
        ];
        let s = live_steps(Preset::Work4, &parse_panels(None), Some(&vis));
        assert_eq!(s.len(), 2);
        assert!(!s.iter().any(|x| matches!(x, Step::Panel(_))));
        let Step::Wayvrctl(a) = &s[1] else { panic!() };
        assert_eq!(a.join(" "), "desk-screens --place --width 1.6 --distance 2 --curvature 0 --hide-others main=SUNSHINE left=PIMAX-LEFT right=PIMAX-RIGHT up=PIMAX-UP up-left=PIMAX-UPLEFT:hide");
        // every switch places (WayVR builds it around the layout anchor; never around the head)
        for p in Preset::ALL {
            assert!(live_steps(p, &[], Some(&vis)).iter().any(|s| matches!(s, Step::Wayvrctl(a) if a[0] == "desk-screens" && a[1] == "--place")), "{p:?}");
        }
        // only the main output: the old arc
        let one = vec![(Slot::Main, "SUNSHINE".to_string(), true)];
        assert!(matches!(live_steps(Preset::Cinema, &[], Some(&one)).last(), Some(Step::Wayvrctl(a)) if a[0] == "desk-layout"));
    }

    #[test]
    fn dry_run_prints_and_touches_nothing() {
        let mut lines = Vec::new();
        apply_live(
            Preset::Focus3,
            Path::new("/nonexistent/wayvrctl"),
            Path::new("/nonexistent/pt.sock"),
            &parse_panels(None),
            None,
            true,
            &mut |l| lines.push(l.to_string()),
        )
        .unwrap();
        assert_eq!(lines.len(), 4);
        assert!(lines[0].contains("`normal`") && lines[3].contains("desk-layout 3"));
    }

    #[test]
    fn desk_start_uses_the_preset() {
        use crate::vr::desk::{parse_desk, plan};
        let env = |k: &str| (k == "HOME").then(|| "/home/u".to_string());
        let parse = |v: &[&str]| parse_desk(&mut Args(v.iter().map(|s| s.to_string()).collect()), &env).unwrap();
        let o = parse(&["--preset", "cinema"]);
        assert_eq!(o.preset, Some(Preset::Cinema));
        let pt = plan(&o).into_iter().find(|s| s.name == "passthrough").unwrap();
        assert_eq!(pt.args, ["run", "--gain", "0.35"]);
        // the old assistant: every screen, and the captions
        let o = parse(&["--preset", "assistant"]);
        assert_eq!(o.preset, Some(Preset::Work6));
        assert!(o.voice);
        assert!(plan(&o).iter().any(|s| s.name == "voicecap"));
        // no --preset: work-4, passthrough normal
        let o = parse(&[]);
        assert_eq!(o.preset, Some(Preset::Work4));
        assert!(!o.voice);
        assert_eq!(plan(&o).into_iter().find(|s| s.name == "passthrough").unwrap().args, ["run"]);
        let o = parse(&["--preset", "none"]);
        assert!(o.preset.is_none());
        // the outputs follow the layout: work-5 needs up-right (6) even with --screens 4
        assert_eq!(parse(&["--preset", "work-5", "--screens", "4"]).screens, 6);
        assert_eq!(parse(&["--preset", "focus-3", "--screens", "2"]).screens, 3);
        assert_eq!(parse(&["--preset", "cinema", "--screens", "2"]).screens, 2, "never lowered");
    }
}
