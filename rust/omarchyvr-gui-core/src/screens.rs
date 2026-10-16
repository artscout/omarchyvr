//! The VR screens switch of the desk settings ("Screens", 1 to 6, `desk start --screens N`) and its readiness line:
//! what the last desk (Work) start made of the Hyprland outputs (`<session dir>/screens.txt`, written by the
//! launcher's `vr/screens.rs`). See docs/SCREENS.md.
use omarchyvr_tools::vr::screens::{Report, DEFAULT, MAX, REPORT_FILE};
use std::path::Path;

pub use omarchyvr_tools::vr::screens::{DEFAULT as DEFAULT_SCREENS, MAX as MAX_SCREENS};

/// A valid count (1..=6), else the default.
pub fn clamp(n: u8) -> u8 {
    if (1..=MAX).contains(&n) {
        n
    } else {
        DEFAULT
    }
}

/// How the readiness line looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    Info,
}

/// The readiness line for `wanted` screens. `running`: a desk session runs now.
pub fn readiness(session_dir: &Path, wanted: u8, running: bool) -> (Level, String) {
    readiness_from(std::fs::read_to_string(session_dir.join(REPORT_FILE)).ok(), wanted, running)
}

/// [`readiness`] with the report text already read (the app reads it in the facts collector, not on the UI thread).
pub fn readiness_from(text: Option<String>, wanted: u8, running: bool) -> (Level, String) {
    let wanted = clamp(wanted);
    match (text, running) {
        (None, _) | (_, false) if wanted == 1 => (Level::Info, "VR screens: 1 (the desktop only)".into()),
        (Some(t), true) => {
            let r = Report::parse(&t);
            let names: Vec<String> = r.outputs.iter().map(|(s, n)| format!("{} {n}", s.key())).collect();
            let main = r.main.clone().unwrap_or_else(|| "?".into());
            let line = if names.is_empty() {
                format!("VR screens: 1 ({main})")
            } else {
                format!("VR screens: {} ({main} in the centre; virtual outputs {})", r.screens(), names.join(", "))
            };
            match r.error {
                Some(e) => (Level::Warn, format!("{line}. Fewer screens than asked: {e}")),
                None => (Level::Ok, line),
            }
        }
        _ => (
            Level::Info,
            format!(
                "VR screens: {wanted} at the next Work start (the desktop + {} virtual outputs, created before VR starts, removed after)",
                wanted - 1
            ),
        ),
    }
}

/// The voice phrase behind each app button (the same intent as saying it: `omarchyvr-voice do --no-wake`).
pub fn arrange_phrase(together: bool) -> &'static str {
    if together {
        "put everything back together"
    } else {
        "put everything in its own window"
    }
}

impl crate::app::AppCore {
    /// The "Organize" / "Back together" buttons: run the voice intent (`win.organize` / `win.together`) through
    /// `omarchyvr-voice do`, so the app and the voice do exactly the same.
    pub fn screens_arrange(&mut self, together: bool) {
        let phrase = arrange_phrase(together);
        if self.demo {
            self.message = format!("demo: would run omarchyvr-voice do --no-wake \"{phrase}\"");
            return;
        }
        let installed = self.paths.installed_bin("omarchyvr-voice");
        let bin = if installed.exists() { installed } else { self.paths.bin("omarchyvr-voice") };
        self.task(Some("screens-arrange"), move || {
            let out = std::process::Command::new(&bin).args(["do", "--no-wake", phrase]).output();
            let msg = match out {
                Ok(o) => {
                    let text = String::from_utf8_lossy(&o.stdout).to_string() + &String::from_utf8_lossy(&o.stderr);
                    let last = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
                    if o.status.success() {
                        format!("{}: {last}", if together { "Back together" } else { "Organize" })
                    } else {
                        format!("{}: failed: {last}", bin.display())
                    }
                }
                Err(e) => format!("{}: {e}", bin.display()),
            };
            Box::new(move |c: &mut crate::app::AppCore| c.message = msg)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_lines() {
        let d = std::env::temp_dir().join(format!("pimax-gui-screens-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(readiness(&d, 1, false).0, Level::Info);
        assert!(readiness(&d, 4, false).1.contains("4 at the next Work start"));
        std::fs::write(d.join(REPORT_FILE), "screens: 4 (SUNSHINE, left = PIMAX-LEFT, right = PIMAX-RIGHT, up = PIMAX-UP)\ncreated: PIMAX-LEFT\n").unwrap();
        let (l, t) = readiness(&d, 4, true);
        assert_eq!(l, Level::Ok);
        assert!(t.contains("VR screens: 4 (SUNSHINE in the centre; virtual outputs left PIMAX-LEFT"), "{t}");
        std::fs::write(d.join(REPORT_FILE), "screens: 1 (SUNSHINE)\nerror: 1 of 4 screens (cannot create PIMAX-LEFT: no headless backend)\n").unwrap();
        let (l, t) = readiness(&d, 4, true);
        assert_eq!(l, Level::Warn);
        assert!(t.contains("Fewer screens than asked: 1 of 4"), "{t}");
        assert_eq!(clamp(0), 4);
        assert_eq!((clamp(6), clamp(7)), (6, 4));
        assert_eq!(arrange_phrase(false), "put everything in its own window");
        assert_eq!(clamp(2), 2);
        let _ = std::fs::remove_dir_all(d);
    }
}
