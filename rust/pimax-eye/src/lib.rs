//! Tobii EyeChip (USB 2104:0561) of the Pimax Dream Air.
//!
//! The chip sits behind the headset's internal hub, speaks Tobii's TTP over one bulk pair (OUT EP 0x05, IN EP 0x83) and,
//! once a session is up, pushes 1242-byte gaze notifications at about 120.5 Hz. Layers, from the wire up:
//!
//! * [`tlv`]: `tag | len | value` items and the fixed-point types,
//! * [`ttp`]: bulk frame wrapper, 24-byte message header, incremental stream decoder,
//! * [`gaze`]: the stream-0x0505 notification decoder (per-eye origin, direction, pupil, validity, timestamp),
//! * [`pcap`] / [`replay`]: USBPcap files and the extraction of the Windows session-start transfers,
//! * [`transport`] / [`usbfs`] / [`session`]: USB access (Linux usbfs, no libusb) and the replay/stream runner,
//! * [`correction`]: the host-side accuracy correction of the gaze direction (fitted by `pimax-lenstest --patterns gaze`),
//! * [`clock`] / [`track`] / [`live`]: host-clock mapping, OpenXR head-frame gaze rays, the long-running reconnecting
//!   session used by the Monado driver (via `pimax-capi`),
//! * [`fake`]: an in-process chip for tests,
//! * [`analysis`] / [`cli`]: the `pimax-eye` tool.
//!
//! Nothing in this crate invents protocol bytes: everything sent to the device is copied from a capture of the Windows
//! software. See `analysis/overnight/eye.md` for the decoded format and what is still missing.
#![deny(unsafe_code)]

pub mod analysis;
pub mod calfit;
pub mod calib;
pub mod clock;
pub mod cli;
pub mod correction;
pub mod fake;
pub mod filter;
pub mod fixation;
pub mod drift;
pub mod frames;
pub mod power;
pub mod gaze;
pub mod ipd;
pub mod ipd_watch;
pub mod live;
pub mod pcap;
pub mod replay;
pub mod replaycmd;
pub mod rng;
pub mod session;
pub mod sigint;
pub mod tlv;
pub mod track;
pub mod transport;
pub mod ttp;
pub mod usbfs;
pub mod watch;
