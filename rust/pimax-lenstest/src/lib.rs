//! `pimax-lenstest`: lens test patterns for the Pimax Dream Air under Monado, as an OpenXR (Vulkan) client.
//!
//! The patterns are pure functions of the eye projection ([`patterns`]); everything else is plumbing: a state
//! machine for pattern/step/eye selection and the timed auto mode ([`state`]), controller and keyboard input
//! ([`input`]), the command line ([`cli`]), the OpenXR/Vulkan session ([`xr`]) and the capture of the compositor's
//! final output as PNGs ([`dump`], [`capture`]).

// The 3-byte and 4-byte pixel loops read best with `chunks_exact`; `as_chunks` would only add noise.
#![allow(clippy::chunks_exact_to_as_chunks)]

pub mod capture;
pub mod cli;
pub mod color;
pub mod compare;
pub mod dump;
pub mod gaze;
pub mod gazedraw;
pub mod gazexr;
pub mod hudquad;
pub mod geometry;
pub mod image;
pub mod input;
pub mod patterns;
pub mod preview;
pub mod report;
pub mod sim;
pub mod state;
pub mod swimbench;
pub mod swimgpu;
pub mod timing;
pub mod tune;
pub mod xr;
