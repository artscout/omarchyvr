//! The runtime intent file `$XDG_RUNTIME_DIR/pimaxd/intent` (tmpfs: a reboot means off). `key=value` lines:
//! `session=on|off|unset`, `by=user|auto|cli|adopted|none`, `mode=desk|vr`, `at=<unix s>`, `override.<id>=on|off`.
use crate::model::{By, Intent, Mode};
use std::path::Path;

pub fn to_text(i: &Intent) -> String {
    let mut s = format!("session={}\nby={}\nmode={}\nat={:.0}\n", i.session_name(), i.by.name(), i.mode.name(), i.at);
    for (k, v) in &i.overrides {
        s.push_str(&format!("override.{k}={}\n", if *v { "on" } else { "off" }));
    }
    s
}

pub fn from_text(t: &str) -> Intent {
    let mut i = Intent::default();
    for l in t.lines() {
        let Some((k, v)) = l.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        match k {
            "session" => {
                i.session = match v {
                    "on" => Some(true),
                    "off" => Some(false),
                    _ => None,
                }
            }
            "by" => i.by = By::parse(v).unwrap_or_default(),
            "mode" => i.mode = Mode::parse(v).unwrap_or_default(),
            "at" => i.at = v.parse().unwrap_or(0.0),
            _ => {
                if let Some(id) = k.strip_prefix("override.") {
                    i.overrides.insert(id.to_string(), v == "on");
                }
            }
        }
    }
    i
}

pub fn read(p: &Path) -> Option<Intent> {
    std::fs::read_to_string(p).ok().map(|t| from_text(&t))
}

pub fn write(p: &Path, i: &Intent) -> std::io::Result<()> {
    crate::util::write_atomic(p, to_text(i).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let mut i = Intent { session: Some(true), by: By::User, mode: Mode::Vr, at: 12.0, ..Default::default() };
        i.overrides.insert("voice.listen".into(), false);
        i.overrides.insert("perf".into(), true);
        assert_eq!(from_text(&to_text(&i)), i);
        assert_eq!(from_text("garbage\nsession=maybe"), Intent::default());
    }
}
