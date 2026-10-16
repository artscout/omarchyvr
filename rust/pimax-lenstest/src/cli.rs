//! Command line of `pimax-lenstest`.

use crate::patterns::{square, Dir, Pattern};
use crate::state::EyeSelect;
use std::path::PathBuf;

pub const USAGE: &str = "\
pimax-lenstest: lens test patterns for the Pimax Dream Air (OpenXR client, run it with Monado running)

usage:
  pimax-lenstest [run] [options]       show the patterns in the headset (default)
  pimax-lenstest list                  patterns, steps, and what to look for in each
  pimax-lenstest config [--lens-conf F]  print the lens.conf and what the model predicts (no headset needed)
  pimax-lenstest preview DIR [--width W] [--height H] [--fov L,R,U,D]   write every pattern as PNG (no headset)
  pimax-lenstest bench-swim [--width W] [--height H] [--frames N]   time the CPU renderer of the swim pattern (no headset)
  pimax-lenstest simulate DIR [--lens-conf F] [--div N]   what the panel gets for every pattern through the lens model (CPU model of the compositor)
  pimax-lenstest compare CAPTURE_DIR SIM_DIR   real compositor pictures against the model's (rms difference per picture)
  pimax-lenstest dump2png RAW OUT.png [--down N] [--rot-left D] [--rot-right D]   compositor dump -> small PNG
  pimax-lenstest dumpdiff A.raw B.raw [--png OUT.png] [--down N]   compare two compositor dumps pixel by pixel

run options:
  --pattern NAME        start with this pattern (grid rings channels fov-edge square100 square25 corners align swim flat gaze)
  --patterns A,B,..     only these patterns, in this order
  --eye both|left|right which eye shows the pattern (the other is black); stick click cycles it
  --auto SECS           auto mode: next step every SECS seconds (trigger toggles it)
  --seconds S           quit after S seconds
  --format srgb|unorm   swapchain format preference (default srgb: the bytes reach the panel unchanged)
  --eye-axis            centre grid/rings/channels/align on each eye's optical axis (default with a non-zero cant_deg:
                        on the head-forward direction, so the two crosses fuse into one)
  --no-hud              no on-screen text (the pattern labels stay)
  --no-hud-quad         bake the on-screen text into each eye's image (shifted so the two copies fuse) instead of
                        submitting it as a head-locked quad layer (default: quad; capture mode has no text at all)
  --square-size DEG     size of the moving square (default 5)
  --square-dir D        right|left|up|down (default right); stick up/down cycles it
  --lens-conf FILE      lens.conf to report (default: PIMAX_LENS_CONF or ~/.config/omarchyvr/lens.conf)
  --timing              print the per-frame timing (wait, locate, CPU render, upload/GPU draw, end) every 2 s (or PIMAX_LENSTEST_TIMING=1)
  --swim-cpu            render the swim pattern on the CPU (default: GPU shader; the CPU path is the reference and the fallback)
  --gaze-points N       gaze pattern: accuracy grid of 13 or 17 dots (default 17)
  --gaze-out PATH       gaze pattern: results JSON file (.json) or directory (default ~/.local/share/omarchyvr/eye-accuracy)
  --res-div N           render at 1/N of the recommended resolution (faster, for tests)
  --capture DIR         capture mode: show every pattern and step, save the compositor output as PNG into DIR
                        (monado-service needs XRT_COMPOSITOR_DUMP_DIR and XRT_COMPOSITOR_DUMP_TRIGGER set)
  --settle N            frames to wait before a capture (default 40)
  --down N              capture PNG downscale factor (default 4)
  --keep-raw            keep the raw compositor dumps next to the PNGs (109 MB each; for dumpdiff)
";

#[derive(Clone, Debug, PartialEq)]
pub struct RunOpts {
    pub patterns: Vec<Pattern>,
    pub start: Option<Pattern>,
    pub eye: EyeSelect,
    pub auto_secs: Option<f32>,
    pub seconds: Option<f32>,
    pub unorm: bool,
    pub hud: bool,
    /// Text as a compositor quad layer in view space (fuses with canted displays); false: baked into each eye.
    pub hud_quad: bool,
    /// Old behaviour: per-eye optical-axis patterns even when the displays are canted.
    pub eye_axis: bool,
    pub square_deg: f32,
    pub dir: Dir,
    pub lens_conf: Option<PathBuf>,
    pub res_div: u32,
    pub capture: Option<PathBuf>,
    pub settle_frames: u32,
    pub down: u32,
    pub keep_raw: bool,
    pub timing: bool,
    pub swim_cpu: bool,
    /// Gaze pattern: grid size and where the results JSON goes.
    pub gaze_points: usize,
    pub gaze_out: Option<PathBuf>,
}

impl Default for RunOpts {
    fn default() -> RunOpts {
        RunOpts {
            patterns: Pattern::DEFAULT_CYCLE.to_vec(),
            start: None,
            eye: EyeSelect::Both,
            auto_secs: None,
            seconds: None,
            unorm: false,
            hud: true,
            hud_quad: true,
            eye_axis: false,
            square_deg: square::DEFAULT_SIZE_DEG,
            dir: Dir::Right,
            lens_conf: None,
            res_div: 1,
            capture: None,
            settle_frames: 40,
            down: 4,
            keep_raw: false,
            timing: false,
            swim_cpu: false,
            gaze_points: 17,
            gaze_out: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Run(RunOpts),
    List,
    Config {
        lens_conf: Option<PathBuf>,
    },
    Preview {
        dir: PathBuf,
        w: u32,
        h: u32,
        fov: [f32; 4],
    },
    Dump2Png {
        raw: PathBuf,
        out: PathBuf,
        down: u32,
        rot_left: u32,
        rot_right: u32,
    },
    DumpDiff {
        a: PathBuf,
        b: PathBuf,
        png: Option<PathBuf>,
        down: u32,
    },
    BenchSwim {
        w: u32,
        h: u32,
        frames: u32,
    },
    Simulate {
        dir: PathBuf,
        lens_conf: Option<PathBuf>,
        div: u32,
    },
    Compare {
        capture: PathBuf,
        sim: PathBuf,
    },
    Help,
}

/// Split `--opt=value` into two arguments.
fn normalize(args: &[String]) -> Vec<String> {
    let mut v = Vec::new();
    for a in args {
        match a.split_once('=') {
            Some((k, val)) if k.starts_with("--") => {
                v.push(k.to_string());
                v.push(val.to_string());
            }
            _ => v.push(a.clone()),
        }
    }
    v
}

fn value<'a>(it: &mut std::slice::Iter<'a, String>, opt: &str) -> Result<&'a String, String> {
    it.next().ok_or_else(|| format!("{opt} needs a value"))
}

fn num<T: std::str::FromStr>(s: &str, opt: &str) -> Result<T, String> {
    s.parse()
        .map_err(|_| format!("{opt}: '{s}' is not a number"))
}

fn pattern(s: &str) -> Result<Pattern, String> {
    Pattern::from_name(s).ok_or_else(|| {
        format!(
            "unknown pattern '{s}' (one of: {})",
            Pattern::ALL.map(|p| p.name()).join(" ")
        )
    })
}

pub fn parse(args: &[String]) -> Result<Command, String> {
    let args = normalize(args);
    let (cmd, rest) = match args.first().map(String::as_str) {
        Some("run") => ("run", &args[1..]),
        Some("list") => ("list", &args[1..]),
        Some("config") => ("config", &args[1..]),
        Some("preview") => ("preview", &args[1..]),
        Some("dump2png") => ("dump2png", &args[1..]),
        Some("dumpdiff") => ("dumpdiff", &args[1..]),
        Some("bench-swim") => ("bench-swim", &args[1..]),
        Some("simulate") => ("simulate", &args[1..]),
        Some("compare") => ("compare", &args[1..]),
        Some("help") | Some("--help") | Some("-h") => return Ok(Command::Help),
        _ => ("run", &args[..]),
    };
    let mut it = rest.iter();
    match cmd {
        "list" => {
            if it.next().is_some() {
                return Err("list takes no arguments".into());
            }
            Ok(Command::List)
        }
        "config" => {
            let mut conf = None;
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--lens-conf" => conf = Some(PathBuf::from(value(&mut it, a)?)),
                    other => return Err(format!("config: unknown option {other}")),
                }
            }
            Ok(Command::Config { lens_conf: conf })
        }
        "preview" => {
            let mut dir = None;
            let (mut w, mut h) = (960u32, 888u32);
            let mut fov = [50.0f32, 50.0, 44.5, 44.5];
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--width" => w = num(value(&mut it, a)?, a)?,
                    "--height" => h = num(value(&mut it, a)?, a)?,
                    "--fov" => {
                        let v = value(&mut it, a)?;
                        let p: Vec<f32> =
                            v.split(',').map(|x| num(x, a)).collect::<Result<_, _>>()?;
                        if p.len() != 4 {
                            return Err(
                                "--fov needs four numbers: left,right,up,down (degrees)".into()
                            );
                        }
                        fov = [p[0], p[1], p[2], p[3]];
                    }
                    s if !s.starts_with("--") && dir.is_none() => dir = Some(PathBuf::from(s)),
                    other => return Err(format!("preview: unexpected {other}")),
                }
            }
            if w < 16 || h < 16 {
                return Err("preview: width and height must be at least 16".into());
            }
            Ok(Command::Preview {
                dir: dir.ok_or("preview needs an output directory")?,
                w,
                h,
                fov,
            })
        }
        "bench-swim" => {
            let (mut w, mut h, mut frames) = (3552u32, 3840u32, 5u32);
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--width" => w = num(value(&mut it, a)?, a)?,
                    "--height" => h = num(value(&mut it, a)?, a)?,
                    "--frames" => frames = num(value(&mut it, a)?, a)?,
                    other => return Err(format!("bench-swim: unexpected {other}")),
                }
            }
            if w < 16 || h < 16 || frames == 0 {
                return Err(
                    "bench-swim: width and height must be at least 16, frames at least 1".into(),
                );
            }
            Ok(Command::BenchSwim { w, h, frames })
        }
        "dump2png" => {
            let (mut raw, mut out) = (None, None);
            let (mut down, mut rl, mut rr) = (4u32, 90u32, 270u32);
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--down" => down = num(value(&mut it, a)?, a)?,
                    "--rot-left" => rl = num(value(&mut it, a)?, a)?,
                    "--rot-right" => rr = num(value(&mut it, a)?, a)?,
                    s if !s.starts_with("--") && raw.is_none() => raw = Some(PathBuf::from(s)),
                    s if !s.starts_with("--") && out.is_none() => out = Some(PathBuf::from(s)),
                    other => return Err(format!("dump2png: unexpected {other}")),
                }
            }
            if down == 0 {
                return Err("dump2png: --down must be at least 1".into());
            }
            Ok(Command::Dump2Png {
                raw: raw.ok_or("dump2png needs RAW and OUT.png")?,
                out: out.ok_or("dump2png needs RAW and OUT.png")?,
                down,
                rot_left: rl,
                rot_right: rr,
            })
        }
        "compare" => {
            let dirs: Vec<PathBuf> = it.map(PathBuf::from).collect();
            match dirs.as_slice() {
                [a, b] => Ok(Command::Compare {
                    capture: a.clone(),
                    sim: b.clone(),
                }),
                _ => Err("compare needs CAPTURE_DIR and SIM_DIR".into()),
            }
        }
        "simulate" => {
            let (mut dir, mut conf, mut div) = (None, None, 4u32);
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--lens-conf" => conf = Some(PathBuf::from(value(&mut it, a)?)),
                    "--div" => div = num(value(&mut it, a)?, a)?,
                    s if !s.starts_with("--") && dir.is_none() => dir = Some(PathBuf::from(s)),
                    other => return Err(format!("simulate: unexpected {other}")),
                }
            }
            if !(1..=16).contains(&div) {
                return Err("simulate: --div must be 1..16".into());
            }
            Ok(Command::Simulate {
                dir: dir.ok_or("simulate needs an output directory")?,
                lens_conf: conf,
                div,
            })
        }
        "dumpdiff" => {
            let (mut a, mut b, mut png) = (None, None, None);
            let mut down = 4u32;
            while let Some(arg) = it.next() {
                match arg.as_str() {
                    "--png" => png = Some(PathBuf::from(value(&mut it, arg)?)),
                    "--down" => down = num(value(&mut it, arg)?, arg)?,
                    s if !s.starts_with("--") && a.is_none() => a = Some(PathBuf::from(s)),
                    s if !s.starts_with("--") && b.is_none() => b = Some(PathBuf::from(s)),
                    other => return Err(format!("dumpdiff: unexpected {other}")),
                }
            }
            if down == 0 {
                return Err("dumpdiff: --down must be at least 1".into());
            }
            Ok(Command::DumpDiff {
                a: a.ok_or("dumpdiff needs A.raw and B.raw")?,
                b: b.ok_or("dumpdiff needs A.raw and B.raw")?,
                png,
                down,
            })
        }
        _ => {
            let mut o = RunOpts::default();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--pattern" => o.start = Some(pattern(value(&mut it, a)?)?),
                    "--patterns" => {
                        let list: Vec<Pattern> = value(&mut it, a)?
                            .split(',')
                            .filter(|s| !s.is_empty())
                            .map(pattern)
                            .collect::<Result<_, _>>()?;
                        if list.is_empty() {
                            return Err("--patterns needs at least one pattern".into());
                        }
                        o.patterns = list;
                    }
                    "--eye" => {
                        let v = value(&mut it, a)?;
                        o.eye = EyeSelect::from_name(v)
                            .ok_or_else(|| format!("--eye: '{v}' is not both, left or right"))?;
                    }
                    "--auto" => {
                        let s: f32 = num(value(&mut it, a)?, a)?;
                        if s <= 0.0 {
                            return Err("--auto needs a positive number of seconds".into());
                        }
                        o.auto_secs = Some(s);
                    }
                    "--seconds" => o.seconds = Some(num(value(&mut it, a)?, a)?),
                    "--format" => {
                        let v = value(&mut it, a)?;
                        o.unorm = match v.to_ascii_lowercase().as_str() {
                            "srgb" => false,
                            "unorm" => true,
                            _ => return Err(format!("--format: '{v}' is not srgb or unorm")),
                        };
                    }
                    "--no-hud" => o.hud = false,
                    "--no-hud-quad" => o.hud_quad = false,
                    "--eye-axis" => o.eye_axis = true,
                    "--square-size" => {
                        o.square_deg = num(value(&mut it, a)?, a)?;
                        if !(0.5..=40.0).contains(&o.square_deg) {
                            return Err("--square-size must be between 0.5 and 40 degrees".into());
                        }
                    }
                    "--square-dir" => {
                        let v = value(&mut it, a)?;
                        o.dir = Dir::from_name(v).ok_or_else(|| {
                            format!("--square-dir: '{v}' is not right, left, up or down")
                        })?;
                    }
                    "--lens-conf" => o.lens_conf = Some(PathBuf::from(value(&mut it, a)?)),
                    "--res-div" => {
                        o.res_div = num(value(&mut it, a)?, a)?;
                        if !(1..=16).contains(&o.res_div) {
                            return Err("--res-div must be 1..16".into());
                        }
                    }
                    "--capture" => o.capture = Some(PathBuf::from(value(&mut it, a)?)),
                    "--settle" => o.settle_frames = num(value(&mut it, a)?, a)?,
                    "--down" => {
                        o.down = num(value(&mut it, a)?, a)?;
                        if o.down == 0 {
                            return Err("--down must be at least 1".into());
                        }
                    }
                    "--keep-raw" => o.keep_raw = true,
                    "--timing" => o.timing = true,
                    "--swim-cpu" => o.swim_cpu = true,
                    "--gaze-points" => {
                        o.gaze_points = num(value(&mut it, a)?, a)?;
                        if !matches!(o.gaze_points, 13 | 17) {
                            return Err("--gaze-points must be 13 or 17".into());
                        }
                    }
                    "--gaze-out" => o.gaze_out = Some(PathBuf::from(value(&mut it, a)?)),
                    other => return Err(format!("unknown option {other} (try --help)")),
                }
            }
            if let Some(s) = o.start {
                if !o.patterns.contains(&s) {
                    return Err(format!("--pattern {} is not in --patterns", s.name()));
                }
            }
            Ok(Command::Run(o))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Result<Command, String> {
        parse(&s.split_whitespace().map(str::to_string).collect::<Vec<_>>())
    }

    fn run(s: &str) -> RunOpts {
        match p(s).unwrap() {
            Command::Run(o) => o,
            c => panic!("{c:?}"),
        }
    }

    #[test]
    fn timing_flags_and_bench_parse() {
        match p("--timing --swim-cpu").unwrap() {
            Command::Run(o) => assert!(o.timing && o.swim_cpu),
            c => panic!("{c:?}"),
        }
        assert_eq!(
            p("bench-swim --width 800 --height 600 --frames 3").unwrap(),
            Command::BenchSwim {
                w: 800,
                h: 600,
                frames: 3
            }
        );
        assert!(p("bench-swim --frames 0").is_err());
    }

    #[test]
    fn defaults_and_the_implicit_run() {
        assert_eq!(p("").unwrap(), Command::Run(RunOpts::default()));
        assert_eq!(p("run").unwrap(), Command::Run(RunOpts::default()));
        let d = RunOpts::default();
        assert_eq!(d.patterns.len(), 10);
        assert!(d.hud && !d.unorm && d.capture.is_none() && d.auto_secs.is_none());
    }

    #[test]
    fn run_options() {
        let o = run("--pattern rings --patterns grid,rings,corners --eye left --auto 8 --seconds 120 --format unorm --no-hud --square-size 7 --square-dir up --lens-conf /x/l.conf --res-div 2");
        assert_eq!(o.start, Some(Pattern::Rings));
        assert_eq!(
            o.patterns,
            vec![Pattern::Grid, Pattern::Rings, Pattern::Corners]
        );
        assert_eq!(o.eye, EyeSelect::Left);
        assert_eq!(o.auto_secs, Some(8.0));
        assert_eq!(o.seconds, Some(120.0));
        assert!(o.unorm && !o.hud);
        assert_eq!(o.square_deg, 7.0);
        assert_eq!(o.dir, Dir::Up);
        assert_eq!(o.lens_conf, Some(PathBuf::from("/x/l.conf")));
        assert_eq!(o.res_div, 2);
        // --opt=value form
        let o = run("--eye=right --auto=2.5 --capture=/tmp/c --settle=10 --down=8");
        assert_eq!(
            (o.eye, o.auto_secs, o.capture, o.settle_frames, o.down),
            (
                EyeSelect::Right,
                Some(2.5),
                Some(PathBuf::from("/tmp/c")),
                10,
                8
            )
        );
    }

    #[test]
    fn run_errors() {
        for bad in [
            "--pattern nope",
            "--patterns grid,zzz",
            "--eye middle",
            "--auto 0",
            "--auto x",
            "--auto",
            "--format bgr",
            "--square-size 100",
            "--square-dir sideways",
            "--res-div 0",
            "--down 0",
            "--bogus",
            "--pattern corners --patterns grid",
            "--patterns ,",
        ] {
            assert!(p(bad).is_err(), "{bad} should fail");
        }
        assert!(p("--pattern nope")
            .unwrap_err()
            .contains("unknown pattern 'nope'"));
    }

    #[test]
    fn other_commands() {
        assert_eq!(p("list").unwrap(), Command::List);
        assert!(p("list x").is_err());
        assert_eq!(p("config").unwrap(), Command::Config { lens_conf: None });
        assert_eq!(
            p("config --lens-conf a.conf").unwrap(),
            Command::Config {
                lens_conf: Some("a.conf".into())
            }
        );
        assert!(p("config --x").is_err());
        assert_eq!(p("--help").unwrap(), Command::Help);
        assert_eq!(p("help").unwrap(), Command::Help);
        assert_eq!(
            p("preview /tmp/pv --width 400 --height 360 --fov 40,45,30,35").unwrap(),
            Command::Preview {
                dir: "/tmp/pv".into(),
                w: 400,
                h: 360,
                fov: [40.0, 45.0, 30.0, 35.0]
            }
        );
        assert!(p("preview").is_err());
        assert!(p("preview d --fov 1,2,3").is_err());
        assert!(p("preview d --width 3").is_err());
        assert_eq!(
            p("dump2png a.raw b.png --down 8 --rot-left 0 --rot-right 180").unwrap(),
            Command::Dump2Png {
                raw: "a.raw".into(),
                out: "b.png".into(),
                down: 8,
                rot_left: 0,
                rot_right: 180
            }
        );
        match p("dump2png a.raw b.png").unwrap() {
            Command::Dump2Png {
                down,
                rot_left,
                rot_right,
                ..
            } => assert_eq!((down, rot_left, rot_right), (4, 90, 270)),
            c => panic!("{c:?}"),
        }
        assert!(p("dump2png a.raw").is_err());
        assert!(p("dump2png a b --down 0").is_err());
        assert_eq!(
            p("dumpdiff a.raw b.raw").unwrap(),
            Command::DumpDiff {
                a: "a.raw".into(),
                b: "b.raw".into(),
                png: None,
                down: 4
            }
        );
        assert_eq!(
            p("dumpdiff a.raw b.raw --png d.png --down 2").unwrap(),
            Command::DumpDiff {
                a: "a.raw".into(),
                b: "b.raw".into(),
                png: Some("d.png".into()),
                down: 2
            }
        );
        assert!(
            p("dumpdiff a.raw").is_err()
                && p("dumpdiff a b --down 0").is_err()
                && p("dumpdiff a b --zzz").is_err()
        );
        assert!(run("--keep-raw").keep_raw && !run("").keep_raw);
        assert!(run("--eye-axis").eye_axis && !run("").eye_axis);
        assert!(!run("--no-hud-quad").hud_quad && run("").hud_quad);
        assert_eq!(
            p("compare a b").unwrap(),
            Command::Compare {
                capture: "a".into(),
                sim: "b".into()
            }
        );
        assert!(p("compare a").is_err() && p("compare a b c").is_err());
        assert_eq!(
            p("simulate /tmp/s").unwrap(),
            Command::Simulate {
                dir: "/tmp/s".into(),
                lens_conf: None,
                div: 4
            }
        );
        assert_eq!(
            p("simulate /tmp/s --lens-conf a.conf --div 8").unwrap(),
            Command::Simulate {
                dir: "/tmp/s".into(),
                lens_conf: Some("a.conf".into()),
                div: 8
            }
        );
        assert!(
            p("simulate").is_err()
                && p("simulate d --div 0").is_err()
                && p("simulate d --div 99").is_err()
                && p("simulate d --x").is_err()
        );
    }

    #[test]
    fn gaze_options_and_the_default_cycle() {
        let o = run("--patterns gaze --gaze-points 13 --gaze-out /tmp/r.json");
        assert_eq!(o.patterns, vec![Pattern::Gaze]);
        assert_eq!(
            (o.gaze_points, o.gaze_out),
            (13, Some("/tmp/r.json".into()))
        );
        let d = run("");
        assert_eq!((d.gaze_points, d.gaze_out), (17, None));
        assert!(!d.patterns.contains(&Pattern::Gaze), "gaze only on request");
        assert_eq!(d.patterns.len() + 1, Pattern::ALL.len());
        assert!(p("--gaze-points 15").is_err() && p("--gaze-points x").is_err());
        assert!(
            p("--pattern gaze").is_err(),
            "--pattern must be in --patterns"
        );
        assert!(run("--pattern gaze --patterns grid,gaze").start == Some(Pattern::Gaze));
    }

    #[test]
    fn usage_mentions_every_option_that_exists() {
        for opt in [
            "--gaze-points",
            "--gaze-out",
            "--pattern",
            "--patterns",
            "--eye",
            "--auto",
            "--seconds",
            "--format",
            "--no-hud",
            "--no-hud-quad",
            "--eye-axis",
            "--square-size",
            "--square-dir",
            "--lens-conf",
            "--res-div",
            "--capture",
            "--settle",
            "--down",
            "--rot-left",
            "--rot-right",
            "--fov",
            "--keep-raw",
            "--png",
            "dumpdiff",
            "simulate",
            "--div",
        ] {
            assert!(USAGE.contains(opt), "{opt} missing from the usage text");
        }
        for pat in Pattern::ALL {
            assert!(USAGE.contains(pat.name()), "{}", pat.name());
        }
    }
}
