//! Records what the Pimax headset shows (docs/RECORD.md).
//!
//! * [`shm`]: the shared-memory ring Monado's recorder tap (`comp_recorder.c`) fills; it drops frames rather than
//!   ever waiting, so recording can never stall the compositor.
//! * [`encode`]: ffmpeg (NVENC, else x264/x265 on the CPU) writing a fragmented mp4.
//! * [`run`]: the recorder process; [`cli`]: `omarchyvr record start|stop|status`.
//! * [`settings`]: `~/.config/omarchyvr/record.toml`; [`status`]: `<session>/record.status` and events.
pub mod cli;
pub mod encode;
pub mod run;
pub mod settings;
pub mod shm;
pub mod status;
pub mod tracking;
