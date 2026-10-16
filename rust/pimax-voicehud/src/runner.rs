//! The long-running `run` mode: build the HUD, then keep a session with the OpenXR runtime alive. If the runtime is
//! not there (Monado not running, or no overlay extension) it retries every 2 s with a one-line log; when a session
//! ends (runtime stopped or lost) it tears everything down and goes back to waiting. SIGINT / SIGTERM end the
//! session cleanly and exit 0.

use crate::cli::RunOpts;
use crate::client::Client;
use crate::demo::Demo;
use crate::display::Config;
use crate::font::{Font, DEFAULT_FONTS};
use crate::hud::{clamp_lines, Hud, Source};
use crate::log;
use crate::probe;
use crate::protocol;
use crate::sig;
use crate::xr::{self, Outcome, Runtime, XrConfig};
use std::process::ExitCode;
use std::time::Duration;

/// Wait between attempts to reach the OpenXR runtime.
pub const XR_RETRY: Duration = Duration::from_secs(2);
/// Repeat an unchanged "cannot connect" message only every this many attempts.
const REPEAT_EVERY: u32 = 30;
/// While the probe says Monado is down, call into the runtime anyway after this many skipped attempts (a Monado
/// built with another socket name must not be shut out for good).
const FORCE_EVERY: u32 = 30;

/// The display configuration the options describe.
pub fn display_config(o: &RunOpts) -> Config {
    Config {
        keep: o.keep,
        max_lines: o.lines,
        timeout: (o.timeout > 0.0).then(|| Duration::from_secs_f32(o.timeout)),
        idle_dot: o.idle_dot,
    }
}

pub fn xr_config(o: &RunOpts) -> XrConfig {
    XrConfig {
        placement: o.placement,
        z_order: o.z_order,
        prefer_unorm: o.unorm,
    }
}

/// Decides when a repeated failure message is printed again.
#[derive(Default)]
pub struct RepeatFilter {
    last: Option<String>,
    repeats: u32,
}

impl RepeatFilter {
    /// True when `msg` should be logged: it is new, or it was repeated `REPEAT_EVERY` times.
    pub fn should_log(&mut self, msg: &str) -> bool {
        if self.last.as_deref() == Some(msg) {
            self.repeats += 1;
            if self.repeats >= REPEAT_EVERY {
                self.repeats = 0;
                return true;
            }
            return false;
        }
        self.last = Some(msg.to_string());
        self.repeats = 0;
        true
    }

    pub fn reset(&mut self) {
        self.last = None;
        self.repeats = 0;
    }
}

/// Load the OpenXR loader (once) and create the overlay session.
fn connect(xrc: &XrConfig, entry: &mut Option<openxr::Entry>) -> Result<Runtime, String> {
    if entry.is_none() {
        *entry = Some(xr::load_entry()?);
    }
    match entry {
        Some(e) => Runtime::connect(xrc, e),
        None => Err("the OpenXR loader is not loaded".to_string()),
    }
}

pub fn run(opts: RunOpts) -> ExitCode {
    let mut notes = Vec::new();
    let font = match Font::load_first(opts.font.as_deref(), &DEFAULT_FONTS, &mut notes) {
        Ok(f) => f,
        Err(e) => {
            log::error(e);
            return ExitCode::from(2);
        }
    };
    for n in &notes {
        log::warn(n);
    }
    if let (_, Some(w)) = clamp_lines(opts.lines, opts.font_px) {
        log::warn(w);
    }
    let source = if opts.demo {
        Source::Demo(Demo::new(opts.timeout))
    } else {
        let path = opts.socket.clone().unwrap_or_else(protocol::socket_path);
        Source::Socket(Client::new(path, Duration::from_secs(1)))
    };
    log::info(format!(
        "font {}, {} px; quad {:.2} m wide at {:.2} m, {:.0} deg below the view centre{}",
        font.path().display(),
        opts.font_px,
        opts.placement.width,
        opts.placement.distance,
        opts.placement.down_deg,
        if opts.demo { "; demo text" } else { "" }
    ));
    let mut hud = Hud::new(source, font, opts.font_px, display_config(&opts));
    let xrc = xr_config(&opts);
    sig::install();
    // The OpenXR loader prints several lines per failed attempt; with a retry every 2 s our own one-line log is
    // enough. XR_LOADER_DEBUG=all (or any value) in the environment brings the loader's messages back.
    if std::env::var_os("XR_LOADER_DEBUG").is_none() {
        std::env::set_var("XR_LOADER_DEBUG", "none");
    }

    let mut filter = RepeatFilter::default();
    let mut entry: Option<openxr::Entry> = None;
    let mut skipped = 0u32;
    while !sig::stopped() {
        // Monado obviously down: do not call into the runtime (its client prints a long banner per attempt)
        let connected = match probe::runtime_down() {
            Some(why) if opts.once || skipped < FORCE_EVERY => {
                skipped += 1;
                Err(why)
            }
            _ => {
                skipped = 0;
                connect(&xrc, &mut entry)
            }
        };
        match connected {
            Ok(mut rt) => {
                filter.reset();
                log::info(format!("overlay session created: {}", rt.describe()));
                let outcome = rt.run(&mut hud, &sig::STOP);
                // tear down the session, swapchain and Vulkan objects before anything else happens
                drop(rt);
                match outcome {
                    Outcome::Quit => break,
                    Outcome::Lost(why) => {
                        if opts.once {
                            log::error(format!("{why}; exiting (--once)"));
                            return ExitCode::FAILURE;
                        }
                        log::warn(format!(
                            "{why}; waiting for the OpenXR runtime (retrying every {} s)",
                            XR_RETRY.as_secs()
                        ));
                    }
                }
            }
            Err(e) => {
                if opts.once {
                    log::error(format!("{e}; exiting (--once)"));
                    return ExitCode::FAILURE;
                }
                if filter.should_log(&e) {
                    log::warn(format!("{e}; retrying every {} s", XR_RETRY.as_secs()));
                }
            }
        }
        // keep reading the socket while there is no session
        sig::wait(XR_RETRY, |now| {
            hud.pump(now);
        });
    }
    log::info("stopped");
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeat_filter_logs_new_messages_and_a_reminder() {
        let mut f = RepeatFilter::default();
        assert!(f.should_log("a"));
        for _ in 0..REPEAT_EVERY - 1 {
            assert!(!f.should_log("a"));
        }
        assert!(f.should_log("a"), "reminder after {REPEAT_EVERY} attempts");
        assert!(!f.should_log("a"));
        assert!(f.should_log("b"), "a different message is logged at once");
        f.reset();
        assert!(f.should_log("b"));
    }

    #[test]
    fn options_map_to_the_display_config() {
        let o = RunOpts {
            lines: 2,
            keep: 5,
            timeout: 7.5,
            idle_dot: true,
            ..RunOpts::default()
        };
        let c = display_config(&o);
        assert_eq!((c.max_lines, c.keep, c.idle_dot), (2, 5, true));
        assert_eq!(c.timeout, Some(Duration::from_secs_f32(7.5)));
        // 0 = never hide
        let c = display_config(&RunOpts {
            timeout: 0.0,
            ..RunOpts::default()
        });
        assert_eq!(c.timeout, None);
        let x = xr_config(&RunOpts {
            z_order: 42,
            unorm: true,
            ..RunOpts::default()
        });
        assert_eq!((x.z_order, x.prefer_unorm), (42, true));
        assert_eq!(x.placement, o.placement);
    }
}
