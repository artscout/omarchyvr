//! Live control of a running `pimax-passthrough run` (voice commands, the launcher's presets): a Unix stream socket,
//! `$PIMAX_PASSTHROUGH_SOCKET` or `$XDG_RUNTIME_DIR/pimax-passthrough.sock`, one text command per connection, one
//! reply line.
//!
//! Commands: `on`, `off`, `toggle`, `gain K` (0.05..4; 1 = normal, 0.35 = dimmed), `dim` (gain 0.35), `normal`
//! (gain 1), `brighter` / `darker` (one gain step), `status`. The reply is `ok on gain=0.35` (the state after the
//! command; the frame loop applies it on its next frame) or `error ...`. `pimax-passthrough ctl CMD...` is the client.
//! Off means the passthrough app submits no layer: the view behind the WayVR screens is black.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The gain of "dim" (the cinema preset).
pub const DIM_GAIN: f64 = 0.35;
/// Gain steps of `brighter` / `darker`.
pub const GAIN_STEPS: &[f64] = &[0.1, 0.2, 0.35, 0.5, 0.7, 1.0, 1.4, 2.0];

/// What the frame loop shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    /// Passthrough layer submitted.
    pub on: bool,
    /// Tone gain.
    pub gain: f64,
}

/// A parsed command.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cmd {
    /// Show the camera view.
    On,
    /// Black (no layer).
    Off,
    /// Flip on/off.
    Toggle,
    /// Set the gain.
    Gain(f64),
    /// One step brighter.
    Brighter,
    /// One step darker.
    Darker,
    /// Only report.
    Status,
}

/// Parse one command line.
pub fn parse(line: &str) -> Result<Cmd, String> {
    let w: Vec<&str> = line.split_whitespace().collect();
    Ok(match w.as_slice() {
        ["on"] => Cmd::On,
        ["off"] | ["black"] => Cmd::Off,
        ["toggle"] => Cmd::Toggle,
        ["dim"] => Cmd::Gain(DIM_GAIN),
        ["normal"] | ["bright"] => Cmd::Gain(1.0),
        ["brighter"] => Cmd::Brighter,
        ["darker"] | ["dimmer"] => Cmd::Darker,
        ["status"] => Cmd::Status,
        ["gain", v] => {
            let g: f64 = v.parse().map_err(|_| format!("bad gain {v}"))?;
            if !(0.05..=4.0).contains(&g) {
                return Err(format!("gain {g} out of 0.05..4"));
            }
            Cmd::Gain(g)
        }
        _ => return Err(format!("unknown command {line:?} (on, off, toggle, gain K, dim, normal, brighter, darker, status)")),
    })
}

/// The state after `cmd`.
pub fn apply(v: View, cmd: Cmd) -> View {
    match cmd {
        Cmd::On => View { on: true, ..v },
        Cmd::Off => View { on: false, ..v },
        Cmd::Toggle => View { on: !v.on, ..v },
        // asking for a gain also means "show it"
        Cmd::Gain(g) => View { on: true, gain: g },
        Cmd::Brighter => View {
            on: true,
            gain: GAIN_STEPS.iter().copied().find(|&s| s > v.gain + 1e-6).unwrap_or(v.gain),
        },
        Cmd::Darker => View {
            on: true,
            gain: GAIN_STEPS.iter().rev().copied().find(|&s| s < v.gain - 1e-6).unwrap_or(v.gain),
        },
        Cmd::Status => v,
    }
}

/// The reply line for a state.
pub fn describe(v: View) -> String {
    format!("ok {} gain={:.2}", if v.on { "on" } else { "off" }, v.gain)
}

/// `$PIMAX_PASSTHROUGH_SOCKET`, else `$XDG_RUNTIME_DIR/pimax-passthrough.sock`, else `/tmp/pimax-passthrough-<uid>.sock`.
pub fn socket_path() -> PathBuf {
    if let Some(p) = std::env::var_os("PIMAX_PASSTHROUGH_SOCKET").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|p| !p.is_empty()) {
        Some(d) => PathBuf::from(d).join("pimax-passthrough.sock"),
        // SAFETY: getuid cannot fail.
        None => PathBuf::from(format!("/tmp/pimax-passthrough-{}.sock", unsafe { libc::getuid() })),
    }
}

/// The server half: a listener thread; the frame loop calls [`Server::poll`] once per frame.
pub struct Server {
    rx: mpsc::Receiver<Cmd>,
    state: Arc<Mutex<View>>,
    path: PathBuf,
}

impl Server {
    /// Bind the socket (a stale file from a crashed run is replaced; a live server elsewhere is an error).
    pub fn start(path: &Path, initial: View) -> Result<Server, String> {
        if UnixStream::connect(path).is_ok() {
            return Err(format!("{} is in use (another pimax-passthrough?)", path.display()));
        }
        let _ = std::fs::remove_file(path);
        let l = UnixListener::bind(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let state = Arc::new(Mutex::new(initial));
        let (tx, rx) = mpsc::channel();
        let st = state.clone();
        std::thread::Builder::new()
            .name("pt-control".into())
            .spawn(move || {
                for c in l.incoming().flatten() {
                    let _ = c.set_read_timeout(Some(Duration::from_secs(2)));
                    let mut line = String::new();
                    let mut w = match c.try_clone() {
                        Ok(w) => w,
                        Err(_) => continue,
                    };
                    if BufReader::new(c).read_line(&mut line).is_err() {
                        continue;
                    }
                    let reply = match parse(line.trim()) {
                        Ok(cmd) => {
                            let mut s = st.lock().unwrap_or_else(|e| e.into_inner());
                            *s = apply(*s, cmd);
                            let r = describe(*s);
                            drop(s);
                            if tx.send(cmd).is_err() {
                                break;
                            }
                            r
                        }
                        Err(e) => format!("error {e}"),
                    };
                    let _ = writeln!(w, "{reply}");
                }
            })
            .map_err(|e| format!("control thread: {e}"))?;
        Ok(Server { rx, state, path: path.to_path_buf() })
    }

    /// Apply the pending commands to `v` (the loop's own state); true if anything changed.
    pub fn poll(&self, v: &mut View) -> bool {
        let before = *v;
        while let Ok(c) = self.rx.try_recv() {
            *v = apply(*v, c);
        }
        if *v != before {
            *self.state.lock().unwrap_or_else(|e| e.into_inner()) = *v;
            true
        } else {
            false
        }
    }

    /// Tell the server about a change made elsewhere (keyboard).
    pub fn set(&self, v: View) {
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = v;
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Client: send one command, return the reply line.
pub fn send(path: &Path, cmd: &str) -> Result<String, String> {
    parse(cmd)?;
    let mut s = UnixStream::connect(path)
        .map_err(|e| format!("{}: {e} (is pimax-passthrough run running?)", path.display()))?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    writeln!(s, "{cmd}").map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).map_err(|e| e.to_string())?;
    let line = line.trim().to_string();
    if line.starts_with("ok") {
        Ok(line)
    } else {
        Err(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_apply() {
        let v = View { on: true, gain: 1.0 };
        assert_eq!(apply(v, parse("off").unwrap()), View { on: false, gain: 1.0 });
        assert_eq!(apply(v, parse("dim").unwrap()).gain, DIM_GAIN);
        let off = View { on: false, gain: 1.0 };
        assert!(apply(off, parse("gain 0.5").unwrap()).on, "a gain shows the view");
        assert_eq!(apply(v, Cmd::Brighter).gain, 1.4);
        assert_eq!(apply(View { on: true, gain: 0.35 }, Cmd::Darker).gain, 0.2);
        assert_eq!(apply(View { on: true, gain: 2.0 }, Cmd::Brighter).gain, 2.0);
        assert_eq!(apply(View { on: true, gain: 0.1 }, Cmd::Darker).gain, 0.1);
        assert!(parse("gain 9").is_err() && parse("gain x").is_err() && parse("explode").is_err());
        assert_eq!(describe(View { on: false, gain: 0.35 }), "ok off gain=0.35");
    }

    #[test]
    fn socket_round_trip() {
        let dir = std::env::temp_dir().join(format!("pt-ctl-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("c.sock");
        let srv = Server::start(&p, View { on: true, gain: 1.0 }).unwrap();
        assert!(Server::start(&p, View { on: true, gain: 1.0 }).is_err(), "second server refused");
        assert_eq!(send(&p, "dim").unwrap(), "ok on gain=0.35");
        assert_eq!(send(&p, "off").unwrap(), "ok off gain=0.35");
        assert!(send(&p, "bogus").is_err());
        let mut v = View { on: true, gain: 1.0 };
        assert!(srv.poll(&mut v));
        assert_eq!(v, View { on: false, gain: 0.35 });
        drop(srv);
        assert!(!p.exists());
    }
}
