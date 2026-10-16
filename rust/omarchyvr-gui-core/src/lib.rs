//! Core of the Pimax desktop app: everything except drawing. The UI crate (`omarchyvr-app`) only renders what this
//! crate computes and forwards clicks to it.
//!
//! * [`facts`] collects the live status and the slower facts from the machine ([`machine::Machine`]: real, or a fake
//!   for tests and screenshots).
//! * [`preflight`] implements docs/LESSONS.md rows 1-10 for a desk start; [`checklist`] the Setup page's install list
//!   (omarchyvr-doctor plus the LESSONS checks) with fixes; [`findings`] the pimax-device checks with their catalogue fixes
//!   and [`fixlog`] the applied ones; [`tools`] the Tools page; [`onboarding`] the headset module's setup steps;
//!   [`autofix`] the Autofix page (a local model or the user's Claude key; nothing is sent before Send).
//! * Commands only run through [`job`] (one at a time, ordered steps, safety-checked) or [`term`] (TUI tools in a
//!   terminal); [`safety`] refuses anything that breaks the hard rules (no kill by name, no SIGKILL, no hotplug, no
//!   firmware, no Steam games, root only through a confirmed pkexec of the documented fix lines).
//! * The UI thread never touches the machine: [`monitor`] polls in the background and [`worker`] runs every
//!   on-demand read (log tails, docs, terminals, signals, profile files) on its own thread.
//! * When something goes wrong: [`applog`] (the app's log file), [`crash`] (crash, hang and renderer reports, the
//!   panic hook and the hang watchdog), [`bundle`] ("Report a problem", redacted by [`redact`]), [`orphans`] (what
//!   still runs after a crash), [`instance`] (one app per session).
/// The headset modules, checks and fix catalogue the core is built on.
pub use pimax_device as device;

pub mod ai;
pub mod app;
pub mod applog;
pub mod auto;
pub mod autofix;
pub mod builds;
pub mod bundle;
pub mod caps;
pub mod checklist;
pub mod cmake;
pub mod cmd;
pub mod crash;
pub mod demo;
pub mod devices;
pub mod docs;
pub mod facts;
pub mod findings;
pub mod fixlog;
pub mod gamecheck;
pub mod guards;
pub mod guistate;
pub mod handtest;
pub mod headsets;
pub mod install;
pub mod instance;
pub mod job;
pub mod kernel;
pub mod launcher;
pub mod llm;
pub mod localai;
pub mod machine;
pub mod manual;
pub mod monitor;
pub mod monlog;
pub mod onboarding;
pub mod ipdmotor;
pub mod orphans;
pub mod paths;
pub mod play;
pub mod preflight;
pub mod probe;
pub mod procs;
pub mod record;
pub mod profiles;
pub mod quality;
pub mod redact;
pub mod reset;
pub mod room;
pub mod roomsetup;
pub mod safety;
pub mod screens;
pub mod subtitles;
pub mod system;
pub mod term;
pub mod tools;
pub mod upload;
pub mod wayvr;
pub mod worker;
