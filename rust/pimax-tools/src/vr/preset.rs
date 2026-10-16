//! Desk presets: `pimax desk start [--preset cinema|work-3|work-1|assistant|none]` (cinema is the default) and the
//! live switch `pimax desk preset NAME [--dry-run]` (used by voice commands).
//!
//! At start a preset is written to `<wayvr home>/config/wayvr/conf.d/60-preset.yaml` (rewritten on every start,
//! removed with `--preset none`) and adds launcher options; once WayVR runs, the screen layout is applied over IPC.
//! The live switch rewrites the same file and applies everything without restarting anything:
//! * the backdrop through the passthrough control socket (`pimax-passthrough ctl`: dim / normal);
//! * the screens through `wayvrctl desk-layout N` (WayVR `aac483a`+): the first N screens in an arc, the rest hidden;
//! * work-3: Hyprland has a single output here, so the two side screens are WayVR app panels (`wayvrctl
//!   process-launch`, Wayland apps running inside WayVR's own compositor), from `~/.config/pimax/work-panels.conf`
//!   (`left = foot`, `right = nautilus --new-window`; default: a `foot` terminal on each side). Panels that already
//!   run are not started again; work-1 and cinema hide them (they keep running).
//!
//! * with VR screens (Hyprland outputs `PIMAX-*`, `desk start --screens N`, see `screens`) there are no app panels:
//!   `wayvrctl desk-screens --place --hide-others` puts the main screen in front of the head and the others left,
//!   right and above it, and the preset's show state (`$XDG_RUNTIME_DIR/pimax-screens.state`) says which are shown:
//!   cinema and work-1 the main screen only, work-3 main + left + right, assistant every screen with an app.
//!
//! | preset | screens | curvature | size | passthrough |
//! |---|---|---|---|---|
//! | cinema (default) | 1, 3.2 m wide at 3.5 m | 0.35 | big | dimmed (gain 0.35) |
//! | work-3 (`work`) | 3: desktop + 2 app panels, 1.6 m at 2 m | flat | | normal |
//! | work-1 | 1, 2 m at 2.2 m | flat | | normal |
//! | assistant | all, 2 m at 2.5 m | 0.15 | | normal, + voice captions at start |
//!
//! `default_curvature` in the YAML applies to screens without a saved curvature; the live layout sets it on every
//! shown screen. `screen_max_height` (capture size) only changes at the next start.
use crate::args::Args;
use crate::CliError;
use std::path::{Path, PathBuf};

/// File name in conf.d (after the launcher's 10-40 files and the Pimax app's 50-pimax-app.yaml: a preset wins).
pub const CONF_NAME: &str = "60-preset.yaml";

/// The preset used when `desk start` gets no `--preset`.
pub const DEFAULT: Preset = Preset::Cinema;

/// A desk preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Cinema,
    /// Three screens (the old `work` name).
    Work3,
    Work1,
    Assistant,
}

/// The screen layout of a preset (`wayvrctl desk-layout`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    /// Screens shown (0 = all).
    pub screens: u32,
    pub curvature: f32,
    /// Width of each screen, metres.
    pub width: f32,
    /// Distance from the head, metres.
    pub distance: f32,
}

impl Preset {
    pub const ALL: [Preset; 4] = [Preset::Cinema, Preset::Work3, Preset::Work1, Preset::Assistant];

    /// `none` -> None. `work` is an alias of `work-3`.
    pub fn parse(s: &str) -> Result<Option<Self>, CliError> {
        Ok(match s {
            "none" => None,
            "cinema" => Some(Self::Cinema),
            "work-3" | "work3" | "work" => Some(Self::Work3),
            "work-1" | "work1" => Some(Self::Work1),
            "assistant" | "llm" => Some(Self::Assistant),
            _ => {
                return Err(CliError::usage(format!(
                    "--preset: none, cinema, work-3 (work), work-1 or assistant, not {s}"
                )))
            }
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Cinema => "cinema",
            Self::Work3 => "work-3",
            Self::Work1 => "work-1",
            Self::Assistant => "assistant",
        }
    }

    /// The conf.d text (the first line names the preset; `pimax-voice` reads it back for "undo").
    pub fn conf(self) -> String {
        let body = match self {
            Self::Cinema => "default_curvature: 0.35\n",
            Self::Work3 | Self::Work1 => "default_curvature: 0.0\nscreen_max_height: 2160\n",
            Self::Assistant => "default_curvature: 0.15\n",
        };
        format!("# pimax desk --preset {} (rewritten every start and by `pimax desk preset`)\n{body}", self.name())
    }

    /// Extra `pimax-passthrough run` arguments.
    pub fn passthrough_args(self) -> &'static [&'static str] {
        match self {
            Self::Cinema => &["--gain", "0.35"],
            _ => &[],
        }
    }

    /// The live passthrough command (`pimax-passthrough ctl ...`).
    pub fn passthrough_ctl(self) -> &'static str {
        match self {
            Self::Cinema => "dim",
            _ => "normal",
        }
    }

    /// The preset turns the voice captions on.
    pub fn voice(self) -> bool {
        self == Self::Assistant
    }

    pub fn layout(self) -> Layout {
        let l = |screens, curvature, width, distance| Layout { screens, curvature, width, distance };
        match self {
            Self::Cinema => l(1, 0.35, 3.2, 3.5),
            Self::Work3 => l(3, 0.0, 1.6, 2.0),
            Self::Work1 => l(1, 0.0, 2.0, 2.2),
            Self::Assistant => l(0, 0.15, 2.0, 2.5),
        }
    }
}

/// Remove and parse `--preset NAME`; without it the default preset (cinema).
pub fn parse_opt(a: &mut Args) -> Result<Option<Preset>, CliError> {
    match a.opt("--preset")? {
        Some(v) => Preset::parse(&v),
        None => Ok(Some(DEFAULT)),
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
    let name = first.strip_prefix("# pimax desk --preset ")?.split_whitespace().next()?;
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

/// `~/.config/pimax/work-panels.conf` (`left = CMD ARGS`, `right = CMD ARGS`, `#` comments); missing keys: `foot`.
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
    home.join(".config/pimax/work-panels.conf")
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

/// What `pimax desk preset P` does, in order. `screens`: the VR screens' visibility under this preset (Some when
/// the session has `PIMAX-*` outputs): then `desk-screens` replaces the arc and the app panels.
pub fn live_steps(p: Preset, panels: &[Panel], screens: Option<&[(super::screens::Slot, String, bool)]>) -> Vec<Step> {
    let mut v = vec![Step::Passthrough(p.passthrough_ctl())];
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

/// The passthrough control socket (same rule as `pimax-passthrough`).
pub fn passthrough_socket(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    if let Some(p) = env("PIMAX_PASSTHROUGH_SOCKET").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    match env("XDG_RUNTIME_DIR").filter(|p| !p.is_empty()) {
        Some(d) => PathBuf::from(d).join("pimax-passthrough.sock"),
        // SAFETY: getuid cannot fail.
        None => PathBuf::from(format!("/tmp/pimax-passthrough-{}.sock", unsafe { libc::getuid() })),
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
        assert_eq!(parse_opt(&mut Args(vec![])).unwrap(), Some(Preset::Cinema), "cinema is the default");
        assert_eq!(Preset::parse("none").unwrap(), None);
        assert_eq!(Preset::parse("work").unwrap(), Some(Preset::Work3));
        assert_eq!(Preset::parse("work-1").unwrap(), Some(Preset::Work1));
        assert_eq!(Preset::parse("llm").unwrap(), Some(Preset::Assistant));
        assert_eq!(parse_opt(&mut Args(vec!["--preset=disco".into()])).unwrap_err().code, 2);
        for p in Preset::ALL {
            assert_eq!(Preset::parse(p.name()).unwrap(), Some(p));
        }
        assert!(Preset::Assistant.voice() && !Preset::Work3.voice());
        assert_eq!(Preset::Cinema.passthrough_args(), ["--gain", "0.35"]);
    }

    #[test]
    fn conf_files_are_written_read_back_and_removed() {
        let d = crate::vr::test_dir("preset");
        let f = d.join("config/wayvr/conf.d").join(CONF_NAME);
        write_conf(&d, Some(Preset::Work1)).unwrap();
        let t = std::fs::read_to_string(&f).unwrap();
        assert!(t.contains("default_curvature: 0.0") && t.contains("screen_max_height: 2160"));
        assert_eq!(read_conf(&d), Some(Preset::Work1));
        write_conf(&d, Some(Preset::Cinema)).unwrap();
        assert!(std::fs::read_to_string(&f).unwrap().contains("default_curvature: 0.35"));
        assert_eq!(read_conf(&d), Some(Preset::Cinema));
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
    fn live_steps_per_preset() {
        let panels = parse_panels(None);
        let s = live_steps(Preset::Work3, &panels, None);
        assert_eq!(s[0], Step::Passthrough("normal"));
        assert!(matches!(&s[1], Step::Panel(p) if p.name == "work-left"));
        assert!(matches!(&s[2], Step::Panel(p) if p.name == "work-right"));
        assert_eq!(
            s[3],
            Step::Wayvrctl(["desk-layout", "3", "--curvature", "0", "--width", "1.6", "--distance", "2"].map(String::from).to_vec())
        );
        let s = live_steps(Preset::Cinema, &panels, None);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0], Step::Passthrough("dim"));
        assert!(matches!(&s[1], Step::Wayvrctl(a) if a[1] == "1" && a[3] == "0.35"));
        assert!(!live_steps(Preset::Work1, &panels, None).iter().any(|s| matches!(s, Step::Panel(_))));
        assert!(matches!(live_steps(Preset::Assistant, &panels, None).last(), Some(Step::Wayvrctl(a)) if a[1] == "0"));
    }

    #[test]
    fn with_vr_screens_the_preset_places_them_and_starts_no_panels() {
        use crate::vr::screens::Slot;
        let vis = vec![
            (Slot::Main, "SUNSHINE".to_string(), true),
            (Slot::Left, "PIMAX-LEFT".to_string(), true),
            (Slot::Right, "PIMAX-RIGHT".to_string(), true),
            (Slot::Up, "PIMAX-UP".to_string(), false),
        ];
        let s = live_steps(Preset::Work3, &parse_panels(None), Some(&vis));
        assert_eq!(s.len(), 2);
        assert!(!s.iter().any(|x| matches!(x, Step::Panel(_))));
        let Step::Wayvrctl(a) = &s[1] else { panic!() };
        assert_eq!(a[0], "desk-screens");
        assert!(a.contains(&"--place".to_string()) && a.contains(&"--hide-others".to_string()) && a.contains(&"up=PIMAX-UP:hide".to_string()));
        // only the main output: the old arc
        let one = vec![(Slot::Main, "SUNSHINE".to_string(), true)];
        assert!(matches!(live_steps(Preset::Work1, &[], Some(&one)).last(), Some(Step::Wayvrctl(a)) if a[0] == "desk-layout"));
    }

    #[test]
    fn dry_run_prints_and_touches_nothing() {
        let mut lines = Vec::new();
        apply_live(
            Preset::Work3,
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
        let o = parse(&["--preset", "assistant"]);
        assert!(o.voice);
        assert!(plan(&o).iter().any(|s| s.name == "voicecap"));
        // no --preset: cinema
        let o = parse(&[]);
        assert_eq!(o.preset, Some(Preset::Cinema));
        assert!(!o.voice);
        let o = parse(&["--preset", "none"]);
        assert!(o.preset.is_none());
        assert_eq!(plan(&o).into_iter().find(|s| s.name == "passthrough").unwrap().args, ["run"]);
    }
}
