//! Log lines on stderr with a short prefix: `voice-hud: ...`.

use std::fmt::Display;

pub fn info(msg: impl Display) {
    eprintln!("voice-hud: {msg}");
}

pub fn warn(msg: impl Display) {
    eprintln!("voice-hud: warning: {msg}");
}

pub fn error(msg: impl Display) {
    eprintln!("voice-hud: error: {msg}");
}
