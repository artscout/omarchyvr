//! Subtitles (live captions of what you say, under the main screen in desk mode): the live switch of the running
//! session and the default of the next desk start.
//!
//! Live: `pimax desk captions on|off` (the launcher flips the flag file pimax-voice reads, the same switch as the voice
//! commands "captions on/off", and starts `pimax-voice listen` and `pimax-voice captions` into the session when they
//! are missing). An older launcher without that command: the app flips the flag file itself (it works while the
//! captions run). Default: `--voice` at the app's desk start (the "voice" switch of the app's state). VR mode has no
//! WayVR, so no subtitles there. Demo mode only simulates.
use crate::app::AppCore;
use crate::cmd::Cmd;
use crate::job::{JobKind, JobSpec, Step};

/// The flag file in `$XDG_RUNTIME_DIR` (pimax-voice's `act::captions_off_path`).
pub const OFF_FLAG: &str = "pimax-voice-captions.off";

impl AppCore {
    /// The live subtitles: Some(on) while a desk session runs, None otherwise.
    pub fn subtitles_live(&self) -> Option<bool> {
        if !self.live.desk_clients_alive() {
            return None;
        }
        if self.demo {
            return Some(self.demo_subtitles);
        }
        Some(self.live.alive("voicecap") && !self.live.captions_off)
    }

    /// Switch the subtitles of the running desk session.
    pub fn set_subtitles(&mut self, on: bool) {
        let word = if on { "on" } else { "off" };
        if self.demo {
            self.demo_subtitles = on;
            self.message = format!("demo mode: would run pimax desk captions {word}");
            return;
        }
        if !self.live.desk_clients_alive() {
            self.message = "No desk session runs: \"Subtitles on when desk starts\" decides the next start.".into();
            return;
        }
        if self.facts.caps.captions {
            let c = Cmd::with_args(self.launcher_bin().display().to_string(), vec!["desk".into(), "captions".into(), word.into()]);
            self.spawn(JobSpec { kind: JobKind::Other("captions".into()), label: format!("Subtitles {word}"), steps: vec![Step::Run(c)] });
            return;
        }
        // an older launcher: the flag file only (works while pimax-voice captions runs)
        let flag = self.paths.runtime_dir.join(OFF_FLAG);
        let running = self.live.alive("voicecap");
        self.task(None, move || {
            let r = if on {
                match std::fs::remove_file(&flag) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
                    _ => Ok(()),
                }
            } else {
                std::fs::write(&flag, "off\n").map_err(|e| e.to_string())
            };
            Box::new(move |c: &mut AppCore| {
                c.message = match (r, on, running) {
                    (Err(e), _, _) => format!("Subtitles: {e}"),
                    (Ok(()), true, false) => "Subtitles on, but they do not run in this session: install the current launcher (Setup > Install) to start them from here, or restart desk mode with subtitles.".into(),
                    (Ok(()), _, _) => format!("Subtitles {}", if on { "on" } else { "off" }),
                };
                c.refresh(false);
            })
        });
    }

    /// The default of the next desk start (`--voice`), saved with the app's state.
    pub fn set_subtitles_default(&mut self, on: bool) {
        self.toggles.voice = on;
        self.save_state();
    }
}

#[cfg(test)]
mod tests {
    use crate::app::AppCore;
    use pimax_config::Store;

    #[test]
    fn the_live_switch_needs_a_desk_session_and_demo_mode_only_simulates() {
        let d = std::env::temp_dir().join(format!("pimax-subtitles-{}", std::process::id()));
        let mut idle = AppCore::demo(false, Store::new(d.join("a"), d.join("b")));
        assert_eq!(idle.subtitles_live(), None, "no desk session");
        let mut a = AppCore::demo(true, Store::new(d.join("c"), d.join("e")));
        assert_eq!(a.subtitles_live(), Some(false));
        a.set_subtitles(true);
        assert_eq!(a.subtitles_live(), Some(true));
        assert!(a.message.contains("would run pimax desk captions on") && a.job.is_none());
        idle.set_subtitles_default(true);
        assert!(idle.toggles.voice);
        assert!(crate::launcher::desk_flags(&idle.toggles).contains(&"--voice".to_string()));
        let _ = std::fs::remove_dir_all(d);
    }
}
