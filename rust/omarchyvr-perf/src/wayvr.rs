//! `wayvrctl batch` link (same pattern as `omarchyvr-voice captions`): started when needed, restarted when WayVR went away.

use crate::view::View;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Label ids in `gui/perf.xml`.
pub const LABELS: [&str; 4] = ["perf_l1", "perf_l2", "perf_l3", "perf_l4"];

/// Quote one argument for `wayvrctl batch` (shell-words syntax).
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The batch lines that make the panel show `v`: text and colour of each line.
pub fn lines_for(overlay: &str, v: &View) -> Vec<String> {
    let mut out = Vec::new();
    for (id, (text, level)) in LABELS.iter().zip(v.lines.iter()) {
        // an empty label is a single space (the command needs at least one word)
        let t = if text.is_empty() { " " } else { text.as_str() };
        out.push(format!("panel-modify {overlay} {id} set-text -- {}", quote(t)));
        out.push(format!("panel-modify {overlay} {id} set-color {}", level.hex()));
    }
    out
}

pub struct Wayvr {
    bin: PathBuf,
    overlay: String,
    child: Option<Child>,
    next_try: Instant,
}

impl Wayvr {
    pub fn new(bin: PathBuf, overlay: &str) -> Wayvr {
        Wayvr {
            bin,
            overlay: overlay.to_string(),
            child: None,
            next_try: Instant::now(),
        }
    }

    /// Send the view. `fresh` is set when a new link was just made (the caller should then resend even an unchanged
    /// view next time; here it is sent right away). Returns false without a link (retried at most once a second).
    pub fn show(&mut self, v: &View, fresh: &mut bool) -> bool {
        *fresh = false;
        if let Some(c) = self.child.as_mut() {
            if !matches!(c.try_wait(), Ok(None)) {
                self.child = None;
            }
        }
        if self.child.is_none() {
            let now = Instant::now();
            if now < self.next_try {
                return false;
            }
            self.next_try = now + Duration::from_secs(1);
            match Command::new(&self.bin)
                .arg("batch")
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                Ok(c) => {
                    self.child = Some(c);
                    *fresh = true;
                }
                Err(e) => {
                    eprintln!("omarchyvr-perf: cannot start {}: {e}", self.bin.display());
                    return false;
                }
            }
        }
        let mut data = lines_for(&self.overlay, v).join("\n");
        data.push('\n');
        let ok = self
            .child
            .as_mut()
            .and_then(|c| c.stdin.as_mut())
            .map(|s| s.write_all(data.as_bytes()).and_then(|_| s.flush()).is_ok())
            .unwrap_or(false);
        if !ok {
            self.stop();
        }
        ok
    }

    pub fn stop(&mut self) {
        if let Some(mut c) = self.child.take() {
            drop(c.stdin.take());
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Drop for Wayvr {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::Level;

    #[test]
    fn batch_lines_carry_text_and_colour() {
        let mut v = View::none();
        v.lines[0] = ("HMD 72/72".into(), Level::Green);
        v.lines[1] = ("it's".into(), Level::Red);
        let l = lines_for("perf", &v);
        assert_eq!(l.len(), 8);
        assert_eq!(l[0], "panel-modify perf perf_l1 set-text -- 'HMD 72/72'");
        assert_eq!(l[1], "panel-modify perf perf_l1 set-color #4cdc6c");
        assert_eq!(l[2], "panel-modify perf perf_l2 set-text -- 'it'\\''s'");
        assert_eq!(l[3], "panel-modify perf perf_l2 set-color #ff4d4d");
        assert_eq!(l[4], "panel-modify perf perf_l3 set-text -- ' '");
    }
}
