//! Controllers only when picked up (PIMAX_CTRL_INHAND, docs/CONTROLLERS-INHAND.md): the switch's values, the files
//! the Monado Pimax driver reads and writes, shared by the app, the CLI (`omarchyvr controllers use|status`,
//! `--controllers` of `desk start` / `vr start`) and voice.
//!
//! * `$XDG_RUNTIME_DIR/pimax-ctrl-inhand`: status written by the driver (`pid`, `mode`, `left` / `right` =
//!   held | down | shown | off, `left_why` / `right_why`).
//! * `$XDG_RUNTIME_DIR/pimax-ctrl-inhand-mode`: a run-time change (`auto` | `always` | `off`), taken by the driver
//!   within 100 ms (it deletes the file).
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The switch: "When picked up" / "Always" / "Off".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CtrlMode {
    /// Shown only while in the hand (Work's default).
    #[default]
    WhenHeld,
    /// Shown whenever the controller reports (Play's default: games expect controllers).
    Always,
    /// Never shown, inputs inactive.
    Off,
}

impl CtrlMode {
    pub const ALL: [CtrlMode; 3] = [CtrlMode::WhenHeld, CtrlMode::Always, CtrlMode::Off];

    /// The CLI / saved name: `when-held`, `always`, `off`.
    pub fn name(self) -> &'static str {
        match self {
            CtrlMode::WhenHeld => "when-held",
            CtrlMode::Always => "always",
            CtrlMode::Off => "off",
        }
    }

    /// The driver's value (PIMAX_CTRL_INHAND and the request file): `auto`, `always`, `off`.
    pub fn env_value(self) -> &'static str {
        match self {
            CtrlMode::WhenHeld => "auto",
            CtrlMode::Always => "always",
            CtrlMode::Off => "off",
        }
    }

    /// What the app shows.
    pub fn label(self) -> &'static str {
        match self {
            CtrlMode::WhenHeld => "When picked up",
            CtrlMode::Always => "Always",
            CtrlMode::Off => "Off",
        }
    }

    /// Any of the names above (also `auto`, `on`, `held`).
    pub fn parse(s: &str) -> Option<CtrlMode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "when-held" | "when_held" | "auto" | "held" | "picked-up" | "when-picked-up" => {
                Some(CtrlMode::WhenHeld)
            }
            "always" | "on" => Some(CtrlMode::Always),
            "off" | "never" => Some(CtrlMode::Off),
            _ => None,
        }
    }
}

fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// `$XDG_RUNTIME_DIR/pimax-ctrl-inhand`.
pub fn status_path() -> PathBuf {
    runtime_dir().join("pimax-ctrl-inhand")
}

/// `$XDG_RUNTIME_DIR/pimax-ctrl-inhand-mode`.
pub fn request_path() -> PathBuf {
    runtime_dir().join("pimax-ctrl-inhand-mode")
}

/// One controller in the status file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Side {
    /// `held`, `down`, `shown` (mode always, not held) or `off`.
    pub state: String,
    /// Why it last changed: `motion`, `tilt`, `input`, `still`, `no reports`, `start`, `switched off`.
    pub why: String,
}

impl Side {
    /// Shown in the headset now.
    pub fn shown(&self) -> bool {
        matches!(self.state.as_str(), "held" | "shown")
    }
    /// Short words for the app: "in hand", "put down", "shown", "off".
    pub fn words(&self) -> &'static str {
        match self.state.as_str() {
            "held" => "in hand",
            "down" if self.why == "no reports" || self.why == "start" => "not in use",
            "down" => "put down",
            "shown" => "shown",
            _ => "hidden",
        }
    }
}

/// The driver's status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub pid: u32,
    pub mode: CtrlMode,
    pub left: Side,
    pub right: Side,
}

/// Parse the status file's text.
pub fn parse_status(text: &str) -> Option<Status> {
    let get = |k: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(k).and_then(|r| r.strip_prefix('=')))
            .map(|v| v.trim().to_string())
    };
    let side = |k: &str| {
        Some(Side {
            state: get(k)?,
            why: get(&format!("{k}_why")).unwrap_or_default(),
        })
    };
    Some(Status {
        pid: get("pid")?.parse().ok()?,
        mode: CtrlMode::parse(&get("mode")?)?,
        left: side("left")?,
        right: side("right")?,
    })
}

/// The status of a running driver (`None`: no file, or its writer is gone). `alive(pid)` tells whether a pid runs.
pub fn read_status(path: &Path, alive: impl Fn(u32) -> bool) -> Option<Status> {
    let s = parse_status(&std::fs::read_to_string(path).ok()?)?;
    alive(s.pid).then_some(s)
}

/// `/proc/<pid>` exists.
pub fn pid_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

/// Ask the running driver for `mode` and wait until it took the request (deleted the file), at most `wait`.
/// Err (request removed again) when nobody took it: no VR session, or a Monado build without PIMAX_CTRL_INHAND.
pub fn request(path: &Path, mode: CtrlMode, wait: Duration) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, format!("{}\n", mode.env_value()))
        .map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    let t0 = Instant::now();
    while t0.elapsed() < wait {
        if !path.exists() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = std::fs::remove_file(path);
    Err("the running VR session did not take the change: no VR runs, or its Monado build has no PIMAX_CTRL_INHAND".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for m in CtrlMode::ALL {
            assert_eq!(CtrlMode::parse(m.name()), Some(m));
            assert_eq!(CtrlMode::parse(m.env_value()), Some(m));
        }
        assert_eq!(CtrlMode::parse("x"), None);
        assert_eq!(CtrlMode::default(), CtrlMode::WhenHeld);
    }

    #[test]
    fn status_parses() {
        let t = "pid=7\nmode=auto\nleft=held\nleft_why=motion\nright=down\nright_why=no reports\n";
        let s = parse_status(t).unwrap();
        assert_eq!((s.pid, s.mode), (7, CtrlMode::WhenHeld));
        assert!(s.left.shown() && !s.right.shown());
        assert_eq!((s.left.words(), s.right.words()), ("in hand", "not in use"));
        let d = Side {
            state: "down".into(),
            why: "still".into(),
        };
        assert_eq!(d.words(), "put down");
        assert!(parse_status("pid=x\n").is_none());
    }

    #[test]
    fn request_without_a_taker_is_removed() {
        let dir = std::env::temp_dir().join(format!("inhand-req-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("pimax-ctrl-inhand-mode");
        assert!(request(&p, CtrlMode::Off, Duration::from_millis(60)).is_err());
        assert!(!p.exists());
        // a "driver" that takes it
        let p2 = p.clone();
        let h = std::thread::spawn(move || {
            for _ in 0..100 {
                if let Ok(s) = std::fs::read_to_string(&p2) {
                    assert_eq!(s.trim(), "always");
                    std::fs::remove_file(&p2).unwrap();
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        assert!(request(&p, CtrlMode::Always, Duration::from_millis(800)).is_ok());
        h.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
