//! Implementation of the `pimax` subcommands (library part so the pure pieces can be unit-tested).
pub mod args;
pub mod buzz;
pub mod calib_cmd;
pub mod controllers;
pub mod imu;
pub mod info;
pub mod keepalived;
pub mod logger;
pub mod logtail;
pub mod mount_shared;
pub mod orient;
pub mod probe;
pub mod video_on;
pub mod vr;

/// Error type of the subcommands: a message for stderr and an exit code.
#[derive(Debug)]
pub struct CliError {
    /// Message printed to stderr.
    pub msg: String,
    /// Process exit code.
    pub code: i32,
}

impl CliError {
    /// Exit code 1.
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            code: 1,
        }
    }
    /// Exit code 2 (usage).
    pub fn usage(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            code: 2,
        }
    }
}

impl<E: std::fmt::Display> From<E> for CliError {
    fn from(e: E) -> Self {
        CliError::new(e.to_string())
    }
}

/// Result of a subcommand.
pub type CliResult = Result<(), CliError>;

/// `$XDG_RUNTIME_DIR/pimax-rate` (falls back to `/tmp`): the file `video-on` writes and `keepalived` reads.
pub fn rate_file() -> std::path::PathBuf {
    std::path::Path::new(&std::env::var_os("XDG_RUNTIME_DIR").unwrap_or_else(|| "/tmp".into()))
        .join("pimax-rate")
}

/// Hex dump with spaces (`aa bb cc`).
pub fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 3);
    for (i, x) in b.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        s.push_str(&format!("{x:02x}"));
    }
    s
}
