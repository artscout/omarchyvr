//! Linux hidraw I/O for the Pimax Dream Air (34a4:0044).
//!
//! * [`discovery`]: sysfs lookup of the four USB interfaces and the bridge log node.
//! * [`Session`]: one thread that wakes the headset, sends the 213 ms keepalive, keeps IF1-IF3 open and
//!   drained (the firmware detaches when nobody reads them), reconnects automatically, matches command
//!   replies and delivers IMU / controller / status / log events through a [`Handler`] or a channel.
//! * [`fake`]: an in-process fake device (seqpacket socketpairs) used by tests and benchmarks.
//!
//! No async runtime: `ppoll(2)` on at most five fds. Protocol bytes live in [`pimax_proto`].
#![deny(unsafe_op_in_unsafe_fn)]

pub mod discovery;
pub mod fake;
pub mod haptics;
pub mod hid;
mod session;
pub mod sys;

pub use pimax_proto as proto;
pub use session::{
    Backend, Config, Event, Handler, HidrawBackend, Link, NoopHandler, RateSource, Session, State,
    Stats,
};

/// Errors of the I/O layer.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No device session at the moment (also returned when the device vanishes during a request).
    #[error("headset not connected")]
    NotConnected,
    /// No reply within the timeout, or `wait_connected` timed out.
    #[error("timed out")]
    Timeout,
    /// Payload too long for one command.
    #[error("payload longer than {} bytes", proto::MAX_DATA)]
    TooLong,
    /// The reply could not be used (for example an invalid calibration document).
    #[error("bad reply: {0}")]
    BadReply(String),
    /// Operating system error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

impl Error {
    /// Negative errno, the convention of the C API.
    pub fn to_errno(&self) -> i32 {
        -match self {
            Error::NotConnected => libc::ENOTCONN,
            Error::Timeout => libc::ETIMEDOUT,
            Error::TooLong => libc::EMSGSIZE,
            Error::BadReply(_) => libc::EPROTO,
            Error::Io(e) => e.raw_os_error().unwrap_or(libc::EIO),
        }
    }
}

/// Result alias.
pub type Result<T> = std::result::Result<T, Error>;
