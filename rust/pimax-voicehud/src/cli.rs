//! Command line of `pimax-voice-hud`.

use crate::placement::Placement;
use crate::protocol::State;
use std::path::PathBuf;

pub const USAGE: &str = "\
pimax-voice-hud: live voice-to-text captions in the headset (OpenXR overlay client for Monado, needs pimax-voice)

usage:
  pimax-voice-hud [run] [options]      show the captions in the headset (default); waits for Monado and the socket
  pimax-voice-hud preview OUT.png [options]   render the texture exactly as it is uploaded and write it as PNG (no headset)
  pimax-voice-hud dump [options]       connect to the socket and print the events and the display lines (no headset)

run options:
  --socket PATH        pimax-voice socket (default: $PIMAX_VOICE_SOCKET, else $XDG_RUNTIME_DIR/pimax-voice.sock,
                       else /tmp/pimax-voice-<uid>.sock)
  --distance M         metres from the head to the quad centre (default 2.0)
  --width M            quad width in metres; the texture is 1600x400 so a pixel is width/1600 (default 1.6 = 1 mm)
  --down-deg DEG       the quad centre is this many degrees below the view centre (default 20)
  --lines N            most wrapped text lines (default 3)
  --keep N             finals kept next to the partial (default 2)
  --timeout SECS       hide the text after this long without speech/partial/final; 0 = never; errors stay (default 12)
  --font PATH          font file; default: the first existing of NotoSans-Medium, NotoSans-Regular, LiberationSans-Regular,
                       JetBrainsMonoNerdFont-Regular under /usr/share/fonts
  --font-size PX       font size (em) in texture pixels (default 64)
  --placement N        overlay z order, higher is drawn on top (default 10; WayVR uses 5)
  --idle-dot           keep the mic dot and level bar visible while connected, even without text
  --format srgb|unorm  swapchain format preference (default srgb)
  --demo               show sample text instead of reading the socket
  --once               exit with an error if the OpenXR runtime or the overlay extension is missing or the session
                       ends, instead of retrying every 2 s

preview options (OUT.png first):
  --text TEXT          a final line (repeat for several; default sample text when nothing else is given)
  --partial TEXT       the partial of the active segment
  --status STATE       listening|paused|no_device|error|stopped      --detail TEXT   its detail text
  --speech             mic dot green;  --level DB   level bar (-60..0), needs --speech
  --backdrop RRGGBB    write the image flattened over this colour instead of raw RGBA (easier to look at)
  --lines N --keep N --font PATH --font-size PX --idle-dot  as above

dump options:
  --socket PATH        as above
  --seconds S          stop after S seconds (default: until Ctrl-C)
  --raw                also print every raw line, including ignored ones
  --lines N --keep N --timeout SECS --font PATH --font-size PX   as above (they shape the printed display lines)

environment: PIMAX_VOICE_SOCKET (socket path), XR_RUNTIME_JSON (OpenXR runtime), XR_LOADER_DEBUG (the OpenXR loader's own
messages are off unless this is set, e.g. XR_LOADER_DEBUG=all), PIMAX_VOICE_HUD_NO_PROBE (do not skip the OpenXR call while
$XDG_RUNTIME_DIR/monado_comp_ipc is missing; for a Monado built with another socket name).
signals: SIGINT / SIGTERM end the OpenXR session cleanly and exit 0.
";

#[derive(Clone, Debug, PartialEq)]
pub struct RunOpts {
    pub socket: Option<PathBuf>,
    pub placement: Placement,
    /// `--placement`: the overlay session's z order.
    pub z_order: u32,
    pub lines: usize,
    pub keep: usize,
    /// Seconds; 0 = never.
    pub timeout: f32,
    pub font: Option<PathBuf>,
    pub font_px: f32,
    pub idle_dot: bool,
    pub unorm: bool,
    pub demo: bool,
    pub once: bool,
}

impl Default for RunOpts {
    fn default() -> RunOpts {
        RunOpts {
            socket: None,
            placement: Placement::default(),
            z_order: 10,
            lines: 3,
            keep: 2,
            timeout: 12.0,
            font: None,
            font_px: 64.0,
            idle_dot: false,
            unorm: false,
            demo: false,
            once: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PreviewOpts {
    pub out: PathBuf,
    pub texts: Vec<String>,
    pub partial: Option<String>,
    pub status: Option<(State, String)>,
    pub speech: bool,
    pub level_db: Option<f32>,
    pub lines: usize,
    pub keep: usize,
    pub font: Option<PathBuf>,
    pub font_px: f32,
    pub idle_dot: bool,
    pub backdrop: Option<[u8; 3]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DumpOpts {
    pub socket: Option<PathBuf>,
    pub seconds: Option<f32>,
    pub raw: bool,
    pub lines: usize,
    pub keep: usize,
    pub timeout: f32,
    pub font: Option<PathBuf>,
    pub font_px: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Run(RunOpts),
    Preview(PreviewOpts),
    Dump(DumpOpts),
    Help,
    Version,
}

/// `--flag value` / `--flag=value` / bare words, one at a time.
struct Args<'a> {
    rest: &'a [String],
    pos: usize,
}

enum Arg<'a> {
    /// A flag with its inline `=value`, if any.
    Flag(&'a str, Option<&'a str>),
    Word(&'a str),
}

impl<'a> Args<'a> {
    fn new(rest: &'a [String]) -> Args<'a> {
        Args { rest, pos: 0 }
    }

    fn next(&mut self) -> Option<Arg<'a>> {
        let a = self.rest.get(self.pos)?;
        self.pos += 1;
        if let Some(flag) = a.strip_prefix("--") {
            return Some(match flag.split_once('=') {
                Some((f, v)) => Arg::Flag(&a[..f.len() + 2], Some(v)),
                None => Arg::Flag(a.as_str(), None),
            });
        }
        if a.len() > 1 && a.starts_with('-') {
            return Some(Arg::Flag(a.as_str(), None));
        }
        Some(Arg::Word(a.as_str()))
    }

    /// The value of `flag`: the inline one or the next argument.
    fn value<'b>(&mut self, flag: &str, inline: Option<&'b str>) -> Result<&'b str, String>
    where
        'a: 'b,
    {
        if let Some(v) = inline {
            return Ok(v);
        }
        let v = self
            .rest
            .get(self.pos)
            .ok_or_else(|| format!("{flag} needs a value"))?;
        self.pos += 1;
        Ok(v.as_str())
    }
}

fn num<T: std::str::FromStr + PartialOrd + std::fmt::Display + Copy>(
    flag: &str,
    s: &str,
    lo: T,
    hi: T,
) -> Result<T, String> {
    let v: T = s
        .trim()
        .parse()
        .map_err(|_| format!("{flag}: \"{s}\" is not a number"))?;
    // NaN fails both comparisons below the same way as an out-of-range value
    if !(v >= lo && v <= hi) {
        return Err(format!("{flag}: {s} is outside {lo}..{hi}"));
    }
    Ok(v)
}

fn parse_hex_rgb(flag: &str, s: &str) -> Result<[u8; 3], String> {
    let h = s.trim_start_matches('#');
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("{flag}: \"{s}\" is not an RRGGBB colour"));
    }
    let b = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0);
    Ok([b(0), b(2), b(4)])
}

/// Flags shared by the three modes; returns false when `flag` is not one of them.
struct Common {
    lines: usize,
    keep: usize,
    font: Option<PathBuf>,
    font_px: f32,
}

impl Common {
    fn take(&mut self, a: &mut Args, flag: &str, inline: Option<&str>) -> Result<bool, String> {
        match flag {
            "--lines" => self.lines = num(flag, a.value(flag, inline)?, 1usize, 8)?,
            "--keep" => self.keep = num(flag, a.value(flag, inline)?, 0usize, 20)?,
            "--font" => self.font = Some(PathBuf::from(a.value(flag, inline)?)),
            "--font-size" => self.font_px = num(flag, a.value(flag, inline)?, 16.0f32, 160.0)?,
            _ => return Ok(false),
        }
        Ok(true)
    }
}

fn unknown(flag: &str) -> String {
    format!("unknown option {flag}")
}

pub fn parse(args: &[String]) -> Result<Command, String> {
    let (cmd, rest) = match args.first().map(String::as_str) {
        None => ("run", args),
        Some("run") => ("run", &args[1..]),
        Some("preview") => ("preview", &args[1..]),
        Some("dump") => ("dump", &args[1..]),
        Some("help") | Some("--help") | Some("-h") => return Ok(Command::Help),
        Some("--version") | Some("-V") => return Ok(Command::Version),
        Some(_) => ("run", args),
    };
    match cmd {
        "preview" => parse_preview(rest),
        "dump" => parse_dump(rest),
        _ => parse_run(rest),
    }
}

fn parse_run(rest: &[String]) -> Result<Command, String> {
    let mut o = RunOpts::default();
    let mut common = Common {
        lines: o.lines,
        keep: o.keep,
        font: None,
        font_px: o.font_px,
    };
    let mut a = Args::new(rest);
    while let Some(arg) = a.next() {
        let (flag, inline) = match arg {
            Arg::Flag(f, v) => (f, v),
            Arg::Word(w) => return Err(format!("unexpected argument \"{w}\"")),
        };
        if flag == "--help" || flag == "-h" {
            return Ok(Command::Help);
        }
        if common.take(&mut a, flag, inline)? {
            continue;
        }
        match flag {
            "--socket" => o.socket = Some(PathBuf::from(a.value(flag, inline)?)),
            "--distance" => o.placement.distance = num(flag, a.value(flag, inline)?, 0.3f32, 50.0)?,
            "--width" => o.placement.width = num(flag, a.value(flag, inline)?, 0.05f32, 30.0)?,
            "--down-deg" => {
                o.placement.down_deg = num(flag, a.value(flag, inline)?, -80.0f32, 80.0)?
            }
            "--timeout" => o.timeout = num(flag, a.value(flag, inline)?, 0.0f32, 86400.0)?,
            "--placement" => o.z_order = num(flag, a.value(flag, inline)?, 0u32, u32::MAX)?,
            "--format" => {
                o.unorm = match a.value(flag, inline)? {
                    "srgb" => false,
                    "unorm" => true,
                    other => return Err(format!("--format: \"{other}\" is not srgb or unorm")),
                }
            }
            "--idle-dot" => o.idle_dot = true,
            "--demo" => o.demo = true,
            "--once" => o.once = true,
            _ => return Err(unknown(flag)),
        }
    }
    o.lines = common.lines;
    o.keep = common.keep;
    o.font = common.font;
    o.font_px = common.font_px;
    o.placement.validate()?;
    Ok(Command::Run(o))
}

fn parse_preview(rest: &[String]) -> Result<Command, String> {
    let mut out: Option<PathBuf> = None;
    let mut o = PreviewOpts {
        out: PathBuf::new(),
        texts: Vec::new(),
        partial: None,
        status: None,
        speech: false,
        level_db: None,
        lines: 3,
        keep: 2,
        font: None,
        font_px: 64.0,
        idle_dot: false,
        backdrop: None,
    };
    let mut common = Common {
        lines: o.lines,
        keep: o.keep,
        font: None,
        font_px: o.font_px,
    };
    let mut status: Option<State> = None;
    let mut detail = String::new();
    let mut a = Args::new(rest);
    while let Some(arg) = a.next() {
        let (flag, inline) = match arg {
            Arg::Word(w) => {
                if out.replace(PathBuf::from(w)).is_some() {
                    return Err(format!("unexpected argument \"{w}\" (one output file)"));
                }
                continue;
            }
            Arg::Flag(f, v) => (f, v),
        };
        if flag == "--help" || flag == "-h" {
            return Ok(Command::Help);
        }
        if common.take(&mut a, flag, inline)? {
            continue;
        }
        match flag {
            "--text" => o.texts.push(a.value(flag, inline)?.to_string()),
            "--partial" => o.partial = Some(a.value(flag, inline)?.to_string()),
            "--status" => {
                let s = a.value(flag, inline)?;
                match State::parse(s) {
                    State::Other(_) => {
                        return Err(format!(
                        "--status: \"{s}\" is not listening, paused, no_device, error or stopped"
                    ))
                    }
                    st => status = Some(st),
                }
            }
            "--detail" => detail = a.value(flag, inline)?.to_string(),
            "--speech" => o.speech = true,
            "--level" => o.level_db = Some(num(flag, a.value(flag, inline)?, -200.0f32, 50.0)?),
            "--backdrop" => o.backdrop = Some(parse_hex_rgb(flag, a.value(flag, inline)?)?),
            "--idle-dot" => o.idle_dot = true,
            _ => return Err(unknown(flag)),
        }
    }
    o.out = out.ok_or("preview needs the output file: pimax-voice-hud preview OUT.png")?;
    o.lines = common.lines;
    o.keep = common.keep;
    o.font = common.font;
    o.font_px = common.font_px;
    o.status = status.map(|s| (s, detail));
    Ok(Command::Preview(o))
}

fn parse_dump(rest: &[String]) -> Result<Command, String> {
    let mut o = DumpOpts {
        socket: None,
        seconds: None,
        raw: false,
        lines: 3,
        keep: 2,
        timeout: 12.0,
        font: None,
        font_px: 64.0,
    };
    let mut common = Common {
        lines: o.lines,
        keep: o.keep,
        font: None,
        font_px: o.font_px,
    };
    let mut a = Args::new(rest);
    while let Some(arg) = a.next() {
        let (flag, inline) = match arg {
            Arg::Flag(f, v) => (f, v),
            Arg::Word(w) => return Err(format!("unexpected argument \"{w}\"")),
        };
        if flag == "--help" || flag == "-h" {
            return Ok(Command::Help);
        }
        if common.take(&mut a, flag, inline)? {
            continue;
        }
        match flag {
            "--socket" => o.socket = Some(PathBuf::from(a.value(flag, inline)?)),
            "--seconds" => o.seconds = Some(num(flag, a.value(flag, inline)?, 0.0f32, 1e7)?),
            "--timeout" => o.timeout = num(flag, a.value(flag, inline)?, 0.0f32, 86400.0)?,
            "--raw" => o.raw = true,
            _ => return Err(unknown(flag)),
        }
    }
    o.lines = common.lines;
    o.keep = common.keep;
    o.font = common.font;
    o.font_px = common.font_px;
    Ok(Command::Dump(o))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Command, String> {
        parse(&args.iter().map(|s| (*s).to_string()).collect::<Vec<_>>())
    }

    fn run(args: &[&str]) -> RunOpts {
        match p(args).unwrap() {
            Command::Run(o) => o,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn defaults() {
        let o = run(&[]);
        assert_eq!(o, RunOpts::default());
        assert_eq!(o.placement, Placement::default());
        assert_eq!(
            (o.z_order, o.lines, o.keep, o.timeout, o.font_px),
            (10, 3, 2, 12.0, 64.0)
        );
        assert!(
            !o.demo && !o.once && !o.idle_dot && !o.unorm && o.socket.is_none() && o.font.is_none()
        );
        // "run" is optional and explicit
        assert_eq!(run(&["run"]), o);
    }

    #[test]
    fn run_flags() {
        let o = run(&[
            "--socket",
            "/tmp/s.sock",
            "--distance",
            "1.5",
            "--width=2.4",
            "--down-deg",
            "15",
            "--lines",
            "2",
            "--keep=1",
            "--timeout",
            "0",
            "--font",
            "/x/f.ttf",
            "--font-size",
            "48",
            "--placement",
            "20",
            "--idle-dot",
            "--format",
            "unorm",
            "--demo",
            "--once",
        ]);
        assert_eq!(o.socket, Some(PathBuf::from("/tmp/s.sock")));
        assert_eq!(
            o.placement,
            Placement {
                distance: 1.5,
                width: 2.4,
                down_deg: 15.0
            }
        );
        assert_eq!((o.lines, o.keep, o.timeout, o.z_order), (2, 1, 0.0, 20));
        assert_eq!(o.font, Some(PathBuf::from("/x/f.ttf")));
        assert_eq!(o.font_px, 48.0);
        assert!(o.idle_dot && o.unorm && o.demo && o.once);
        // the first argument may also be a flag
        assert!(run(&["--demo"]).demo);
    }

    #[test]
    fn run_errors() {
        for (args, needle) in [
            (vec!["--bogus"], "unknown option --bogus"),
            (vec!["--lines"], "--lines needs a value"),
            (vec!["--lines", "0"], "outside"),
            (vec!["--lines", "x"], "not a number"),
            (vec!["--distance", "0.1"], "outside"),
            (vec!["--width", "nan"], "outside"),
            (vec!["--down-deg", "120"], "outside"),
            (vec!["--timeout", "-1"], "outside"),
            (vec!["--font-size", "4"], "outside"),
            (vec!["--format", "rgb"], "srgb or unorm"),
            (vec!["stray"], "unexpected argument"),
            (vec!["--placement", "-3"], "not a number"),
        ] {
            let e = p(&args).unwrap_err();
            assert!(e.contains(needle), "{args:?}: {e}");
        }
    }

    #[test]
    fn help_and_version() {
        assert_eq!(p(&["--help"]).unwrap(), Command::Help);
        assert_eq!(p(&["-h"]).unwrap(), Command::Help);
        assert_eq!(p(&["help"]).unwrap(), Command::Help);
        assert_eq!(p(&["run", "--help"]).unwrap(), Command::Help);
        assert_eq!(p(&["preview", "--help"]).unwrap(), Command::Help);
        assert_eq!(p(&["dump", "-h"]).unwrap(), Command::Help);
        assert_eq!(p(&["--version"]).unwrap(), Command::Version);
        assert!(
            USAGE.contains("--placement") && USAGE.contains("--once") && USAGE.contains("--demo")
        );
    }

    #[test]
    fn preview() {
        match p(&["preview", "/tmp/o.png"]).unwrap() {
            Command::Preview(o) => {
                assert_eq!(o.out, PathBuf::from("/tmp/o.png"));
                assert!(o.texts.is_empty() && o.partial.is_none() && o.status.is_none());
                assert_eq!((o.lines, o.keep, o.font_px), (3, 2, 64.0));
            }
            other => panic!("{other:?}"),
        }
        match p(&[
            "preview",
            "--text",
            "Open the browser.",
            "/tmp/o.png",
            "--text=second",
            "--partial",
            "and the",
            "--status",
            "error",
            "--detail",
            "no mic",
            "--speech",
            "--level",
            "-25.5",
            "--backdrop",
            "#304050",
            "--lines",
            "2",
        ])
        .unwrap()
        {
            Command::Preview(o) => {
                assert_eq!(o.texts, ["Open the browser.", "second"]);
                assert_eq!(o.partial.as_deref(), Some("and the"));
                assert_eq!(o.status, Some((State::Error, "no mic".to_string())));
                assert!(o.speech);
                assert_eq!(o.level_db, Some(-25.5));
                assert_eq!(o.backdrop, Some([0x30, 0x40, 0x50]));
                assert_eq!(o.lines, 2);
            }
            other => panic!("{other:?}"),
        }
        assert!(p(&["preview"]).unwrap_err().contains("output file"));
        assert!(p(&["preview", "a.png", "b.png"])
            .unwrap_err()
            .contains("one output"));
        assert!(p(&["preview", "a.png", "--status", "nope"])
            .unwrap_err()
            .contains("--status"));
        assert!(p(&["preview", "a.png", "--backdrop", "12345"])
            .unwrap_err()
            .contains("RRGGBB"));
        assert!(p(&["preview", "a.png", "--demo"])
            .unwrap_err()
            .contains("unknown option"));
    }

    #[test]
    fn dump() {
        match p(&[
            "dump",
            "--socket",
            "/s",
            "--seconds",
            "5",
            "--raw",
            "--timeout",
            "3",
        ])
        .unwrap()
        {
            Command::Dump(o) => {
                assert_eq!(o.socket, Some(PathBuf::from("/s")));
                assert_eq!(o.seconds, Some(5.0));
                assert!(o.raw);
                assert_eq!(o.timeout, 3.0);
            }
            other => panic!("{other:?}"),
        }
        assert!(p(&["dump", "x"]).unwrap_err().contains("unexpected"));
        assert!(p(&["dump", "--demo"])
            .unwrap_err()
            .contains("unknown option"));
    }
}
