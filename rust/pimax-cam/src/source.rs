//! Frame sources: live V4L2 ([`crate::v4l2::V4l2Capture`]) or a recording ([`crate::record::Replay`]).

use std::io;
use std::path::Path;

/// One raw frame as delivered by the device: `FRAME_H` rows of `stride` bytes (grey interpretation).
#[derive(Clone, Copy, Debug)]
pub struct RawFrame<'a> {
    /// Frame bytes; valid until the source advances.
    pub data: &'a [u8],
    /// Row stride in bytes (2560 for this device).
    pub stride: usize,
    /// Host `CLOCK_MONOTONIC` time (V4L2 buffer timestamp, or rebased replay time), ns.
    pub host_ts_ns: u64,
    /// Driver sequence number.
    pub sequence: u32,
}

/// Something that yields raw frames.
pub trait Source: Send {
    /// Move to the next frame. `Ok(false)` on timeout (live) or end of file (replay without loop).
    fn advance(&mut self, timeout_ms: i32) -> io::Result<bool>;
    /// The current frame (after a successful [`Source::advance`]).
    fn current(&self) -> Option<RawFrame<'_>>;
    /// One-line description for logs.
    fn describe(&self) -> String;
    /// True for a replay that has reached its end.
    fn finished(&self) -> bool {
        false
    }
    /// Buffers the driver flagged as corrupt (skipped).
    fn corrupt_frames(&self) -> u64 {
        0
    }
    /// True for a recording.
    fn is_replay(&self) -> bool {
        false
    }
    /// Changes whenever the frames start coming from a reopened device or a new broker (the device clock and the
    /// frame counter may have restarted). 0 for sources that never reopen.
    fn generation(&self) -> u64 {
        0
    }
    /// False while the source is reopening its camera (or the broker reports its camera down).
    fn up(&self) -> bool {
        true
    }
    /// Advance and return the frame.
    fn next(&mut self, timeout_ms: i32) -> io::Result<Option<RawFrame<'_>>> {
        if self.advance(timeout_ms)? {
            Ok(self.current())
        } else {
            Ok(None)
        }
    }
}

/// Replay options.
#[derive(Clone, Copy, Debug, Default)]
pub struct ReplayOptions {
    /// Restart at the end.
    pub looping: bool,
    /// Pace frames with their recorded spacing (otherwise as fast as asked).
    pub realtime: bool,
}

/// Open a source from a string: empty or `auto` = autodetect the camera, `/dev/...` = that device, `broker` or
/// `broker:<socket>` = the frames of a running `pimax-cam broker` ([`crate::broker`]), anything else = a recording
/// file. `fps` = 0 keeps the driver default.
///
/// A live source (the camera, or a broker serving the camera) comes back wrapped in
/// [`crate::resilient::Resilient`]: after an unplug, a stall or a broker restart it reopens by itself instead of
/// failing (`PIMAX_CAM_RECONNECT=0` turns that off). The first open must succeed: it still fails fast.
pub fn open(spec: &str, fps: u32, replay: ReplayOptions) -> io::Result<Box<dyn Source>> {
    let first = open_once(spec, fps, replay)?;
    if first.is_replay() || !crate::resilient::enabled() {
        return Ok(first);
    }
    let broker = is_broker_spec(spec);
    let (policy, label) = if broker {
        (crate::resilient::Policy::broker(), "camera broker")
    } else {
        (crate::resilient::Policy::camera(), "camera")
    };
    let spec = spec.to_string();
    Ok(Box::new(crate::resilient::Resilient::new(
        first,
        Box::new(move || open_once(&spec, fps, replay)),
        policy,
        label,
    )))
}

/// True for `broker` / `broker:<socket>`.
pub fn is_broker_spec(spec: &str) -> bool {
    spec.trim()
        .strip_prefix("broker")
        .is_some_and(|r| r.is_empty() || r.starts_with(':'))
}

/// [`open`] without the reopen wrapper.
pub fn open_once(spec: &str, fps: u32, replay: ReplayOptions) -> io::Result<Box<dyn Source>> {
    let spec = spec.trim();
    if let Some(rest) = spec.strip_prefix("broker") {
        if rest.is_empty() || rest.starts_with(':') {
            let path = match rest.trim_start_matches(':') {
                "" => crate::broker::default_socket(),
                p => std::path::PathBuf::from(p),
            };
            return Ok(Box::new(crate::broker::BrokerSource::connect(&path)?));
        }
    }
    if spec.is_empty() || spec == "auto" {
        let dev = crate::v4l2::find_device().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "camera 34a4:0014 not found under /sys/class/video4linux",
            )
        })?;
        return crate::v4l2::V4l2Capture::open(&dev, fps, 6)
            .map(|c| Box::new(c) as Box<dyn Source>)
            .map_err(|e| {
                let hint = if e.raw_os_error() == Some(libc::EBUSY) {
                    " (busy: another process is streaming it, e.g. a running omarchyvr-passthrough)"
                } else {
                    ""
                };
                io::Error::new(e.kind(), format!("{}: {e}{hint}", dev.display()))
            });
    }
    if spec.starts_with("/dev/") {
        return Ok(Box::new(crate::v4l2::V4l2Capture::open(
            Path::new(spec),
            fps,
            6,
        )?));
    }
    Ok(Box::new(crate::record::Replay::open(
        Path::new(spec),
        replay,
    )?))
}
