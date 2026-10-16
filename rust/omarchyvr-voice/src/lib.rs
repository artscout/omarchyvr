//! Local voice-to-text for the Pimax Dream Air on Linux (`omarchyvr-voice`).
//!
//! mic (PipeWire, the headset's USB microphone) -> 32 ms frames -> VAD ([`vad`]: Silero via sherpa-onnx, or energy)
//! -> segments ([`segment`]: pre-roll, hangover, max length, partial schedule) -> decoding ([`engine`]: Parakeet TDT
//! 0.6B v3 int8 on the CPU, English + Spanish) -> events ([`protocol`]: JSON lines) -> subscribers ([`server`]: Unix
//! socket, stdout) and commands ([`intent`]). Voice commands ("Pimax, cinema mode"): [`catalogue`] (the TOML
//! intent list), [`command`] (offline matcher), [`llm`] (optional fallback), [`act`] (actions, confirmation, undo). [`pipeline`] wires it up; [`capture`] (feature `pipewire`) and
//! [`source`] provide the audio. Nothing leaves the machine: the models are local files and the socket is local.
#![deny(unsafe_op_in_unsafe_fn)]

pub mod act;
pub mod agent;
pub mod audit;
pub mod bench;
pub mod captions;
pub mod claude;
pub mod claudelive;
pub mod catalogue;
pub mod command;
#[cfg(feature = "pipewire")]
pub mod capture;
pub mod desk;
pub mod engine;
pub mod fuzzy;
pub mod gaze;
pub mod intent;
pub mod lang;
pub mod leak;
pub mod llm;
pub mod nlu;
pub mod pipeline;
pub mod planner;
pub mod protocol;
pub mod remote;
pub mod router;
pub mod rules;
pub mod segment;
pub mod statetest;
pub mod server;
pub mod source;
pub mod vad;
pub mod vkbd;
pub mod variant;
pub mod windows;
pub mod world;
