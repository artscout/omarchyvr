//! The component table (plan section 2.2): one static [`Spec`] per component. The graph, the policy, the probes and
//! the reconciler all read this table; [`crate::components`] wraps each row in a [`crate::Component`].
use crate::model::{Class, Dep, Desired, Eye, Grace, Id, Kind, Level, Mode, Rung, Toggle};

/// Where a process records its pid (relative to the launcher's session dir unless `abs` names another root).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PidSpec {
    /// File name (`monado.pid`).
    pub file: &'static str,
    /// `/proc/<pid>/comm` (cut at 15 characters by the kernel).
    pub comm: &'static str,
    /// Root of `file`.
    pub root: PidRoot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PidRoot {
    /// The launcher's session dir (`/tmp/pimax-vr-session`).
    Session,
    /// `~/.local/state/pimax/ai`.
    AiState,
}

/// When a component is part of "everything on".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Always watched (devices, host settings, the keepalive, the gate).
    Always,
    /// Only while a session runs.
    Session,
    /// On its own toggle, with or without a session (none since ai.server became Work-only, 2026-10-16).
    Own,
}

/// One row of the table.
#[derive(Debug, Clone, Copy)]
pub struct Spec {
    pub id: Id,
    pub label: &'static str,
    pub kind: Kind,
    pub deps: &'static [Dep],
    pub toggle: Toggle,
    pub class: Class,
    pub ladder: &'static [Rung],
    pub grace: Grace,
    /// A GPU client: no automatic start while the gate is not ok.
    pub gpu: bool,
    pub pid: Option<PidSpec>,
    pub scope: Scope,
    /// Part of "everything on" for this desired state (the toggle; true for always-on rows).
    pub rule: fn(&Desired) -> bool,
}

const fn r(level: Level, what: &'static str) -> Rung {
    Rung { level, what }
}

const fn g(start_s: f64, settle_s: f64, present_s: f64, absent_s: f64) -> Grace {
    Grace { start_s, settle_s, present_s, absent_s }
}

const fn sess(file: &'static str, comm: &'static str) -> Option<PidSpec> {
    Some(PidSpec { file, comm, root: PidRoot::Session })
}

fn yes(_: &Desired) -> bool {
    true
}
fn desk(d: &Desired) -> bool {
    d.mode == Mode::Desk
}
fn sixdof(d: &Desired) -> bool {
    d.sixdof
}
fn broker_wanted(d: &Desired) -> bool {
    d.sixdof || (desk(d) && d.passthrough)
}
fn cam_wanted(d: &Desired) -> bool {
    broker_wanted(d)
}
fn eye_on(d: &Desired) -> bool {
    d.eye != Eye::Off
}
fn passthrough(d: &Desired) -> bool {
    desk(d) && d.passthrough
}
fn passthrough_6dof(d: &Desired) -> bool {
    d.sixdof
}
fn ctrltrack(d: &Desired) -> bool {
    d.sixdof && d.ctrltrack
}
fn perf(d: &Desired) -> bool {
    desk(d) && d.perf
}
fn screens(d: &Desired) -> bool {
    desk(d) && d.screens > 1
}
fn voice(d: &Desired) -> bool {
    d.voice
}
fn voice_desk(d: &Desired) -> bool {
    d.voice && desk(d)
}
fn voicehud(d: &Desired) -> bool {
    d.voice && d.voicehud
}
/// The local AI models run in Work (desk) sessions only, never in Play / SteamVR (user decision 2026-10-16): a
/// switch from Work to Play stops them.
fn ai(d: &Desired) -> bool {
    d.ai_server && desk(d)
}
fn laser(d: &Desired) -> bool {
    desk(d) && eye_on(d) && d.eye_laser
}
fn anchor(d: &Desired) -> bool {
    d.sixdof && d.anchor
}
fn hands(d: &Desired) -> bool {
    d.sixdof && d.hands
}
fn floor(d: &Desired) -> bool {
    d.sixdof && d.floor_follow
}
fn preset(d: &Desired) -> bool {
    desk(d) && d.preset != "none"
}
/// Started on demand only (`pimax record start`, the app, voice); never part of "everything on".
fn on_demand(_: &Desired) -> bool {
    false
}

const WAIT: Rung = r(Level::L0, "wait");
const TELL: Rung = r(Level::L5, "tell the user");
const L2: Rung = r(Level::L2, "restart this process");

/// The table. Order is irrelevant (the graph sorts it); ids are unique (tested).
pub static SPECS: &[Spec] = &[
    Spec {
        id: "usb.hid",
        label: "Headset (USB 34a4:0044)",
        kind: Kind::Device,
        deps: &[],
        toggle: Toggle::None,
        class: Class::None,
        ladder: &[WAIT, TELL],
        grace: g(0.0, 0.0, 4.0, 6.0),
        gpu: false,
        pid: None,
        scope: Scope::Always,
        rule: yes,
    },
    Spec {
        id: "usb.cam",
        label: "Tracking cameras (34a4:0014)",
        kind: Kind::Device,
        deps: &[Dep::hard("usb.hid")],
        toggle: Toggle::None,
        class: Class::Device,
        ladder: &[r(Level::L0, "wait 20 s for it to return"), r(Level::L4, "reset-headset (approved exit bytes)"), TELL],
        grace: g(150.0, 0.0, 0.0, 20.0),
        gpu: false,
        pid: None,
        scope: Scope::Session,
        rule: cam_wanted,
    },
    Spec {
        id: "usb.eye",
        label: "Eye tracker (34a4:0043)",
        kind: Kind::Device,
        deps: &[Dep::hard("usb.hid")],
        toggle: Toggle::None,
        class: Class::None,
        ladder: &[TELL],
        grace: g(0.0, 0.0, 4.0, 6.0),
        gpu: false,
        pid: None,
        scope: Scope::Session,
        rule: eye_on,
    },
    Spec {
        id: "keepalive",
        label: "Keepalive (pimax-keepalived.service)",
        kind: Kind::Process,
        deps: &[Dep::soft("usb.hid")],
        toggle: Toggle::None,
        class: Class::Leaf,
        ladder: &[r(Level::L0, "systemd Restart=always"), r(Level::L2, "systemctl --user restart once"), TELL],
        grace: g(10.0, 3.0, 0.0, 0.0),
        gpu: false,
        pid: None,
        scope: Scope::Always,
        rule: yes,
    },
    Spec {
        id: "display.dp1",
        label: "Headset display (DP)",
        kind: Kind::Device,
        deps: &[Dep::hard("usb.hid"), Dep::hard("keepalive")],
        toggle: Toggle::None,
        class: Class::None,
        ladder: &[r(Level::L0, "wait for the drm uevent (never a hotplug)"), TELL],
        grace: g(0.0, 0.0, 2.0, 6.0),
        gpu: false,
        pid: None,
        scope: Scope::Session,
        rule: yes,
    },
    Spec {
        id: "gpu.gate",
        label: "GPU safety gate",
        kind: Kind::Gate,
        deps: &[],
        toggle: Toggle::None,
        class: Class::None,
        ladder: &[r(Level::L5, "reboot banner (clears on reboot)")],
        grace: g(0.0, 0.0, 0.0, 0.0),
        gpu: false,
        pid: None,
        scope: Scope::Always,
        rule: yes,
    },
    Spec {
        id: "host.caps",
        label: "monado-service cap_sys_nice",
        kind: Kind::Setting,
        deps: &[],
        toggle: Toggle::None,
        class: Class::None,
        ladder: &[r(Level::L5, "the pkexec setcap line")],
        grace: g(0.0, 0.0, 0.0, 0.0),
        gpu: false,
        pid: None,
        scope: Scope::Always,
        rule: yes,
    },
    Spec {
        id: "host.mhs",
        label: "NVIDIA merged_head_sync",
        kind: Kind::Setting,
        deps: &[],
        toggle: Toggle::None,
        class: Class::None,
        ladder: &[r(Level::L5, "catalogue fix")],
        grace: g(0.0, 0.0, 0.0, 0.0),
        gpu: false,
        pid: None,
        scope: Scope::Always,
        rule: yes,
    },
    Spec {
        id: "host.runtime-link",
        label: "OpenXR active runtime",
        kind: Kind::Setting,
        deps: &[],
        toggle: Toggle::None,
        class: Class::None,
        ladder: &[r(Level::L1, "atomic relink (the launcher does it at start)"), TELL],
        grace: g(0.0, 0.0, 0.0, 0.0),
        gpu: false,
        pid: None,
        scope: Scope::Always,
        rule: yes,
    },
    Spec {
        id: "broker",
        label: "Camera broker (pimax-cam)",
        kind: Kind::Process,
        deps: &[Dep::hard("usb.cam")],
        toggle: Toggle::Restart("monado"),
        class: Class::Core,
        ladder: &[r(Level::L3, "broker + Monado subtree (SLAM cannot reconnect yet)"), TELL],
        grace: g(10.0, 3.0, 0.0, 0.0),
        gpu: false,
        pid: sess("broker.pid", "pimax-cam"),
        scope: Scope::Session,
        rule: broker_wanted,
    },
    Spec {
        id: "monado",
        label: "VR runtime (monado-service)",
        kind: Kind::Process,
        deps: &[
            Dep::hard("usb.hid"),
            Dep::hard("keepalive"),
            Dep::hard("display.dp1"),
            Dep::hard("gpu.gate"),
            Dep::hard_if("broker", sixdof),
            Dep::soft("host.caps"),
            Dep::soft("host.mhs"),
            Dep::soft("host.runtime-link"),
        ],
        toggle: Toggle::Restart("monado"),
        class: Class::Core,
        ladder: &[r(Level::L3, "ordered subtree restart (SIGINT only, never SIGKILL)"), TELL],
        grace: g(45.0, 5.0, 0.0, 0.0),
        gpu: true,
        pid: sess("monado.pid", "monado-service"),
        scope: Scope::Session,
        rule: yes,
    },
    Spec {
        id: "eye",
        label: "Eye tracking",
        kind: Kind::Feature,
        deps: &[Dep::hard("monado"), Dep::hard("usb.eye")],
        toggle: Toggle::Restart("monado"),
        class: Class::None,
        ladder: &[TELL],
        grace: g(0.0, 0.0, 0.0, 0.0),
        gpu: false,
        pid: None,
        scope: Scope::Session,
        rule: eye_on,
    },
    Spec {
        id: "eye.laser",
        label: "Eye laser pointer",
        kind: Kind::Setting,
        deps: &[Dep::hard("eye"), Dep::hard("wayvr")],
        toggle: Toggle::Live,
        class: Class::None,
        ladder: &[r(Level::L1, "re-apply after a WayVR restart")],
        grace: g(0.0, 0.0, 0.0, 0.0),
        gpu: false,
        pid: None,
        scope: Scope::Session,
        rule: laser,
    },
    Spec {
        id: "sixdof",
        label: "6DoF head tracking (SLAM)",
        kind: Kind::Feature,
        deps: &[Dep::hard("monado"), Dep::hard("broker")],
        toggle: Toggle::Restart("monado"),
        class: Class::None,
        ladder: &[r(Level::L0, "SLAM relocalises by itself"), TELL],
        grace: g(0.0, 0.0, 0.0, 0.0),
        gpu: false,
        pid: None,
        scope: Scope::Session,
        rule: sixdof,
    },
    Spec {
        id: "wayvr",
        label: "WayVR desktop",
        kind: Kind::Process,
        deps: &[Dep::hard("monado")],
        toggle: Toggle::Live,
        class: Class::XrClient,
        ladder: &[L2, r(Level::L1, "re-apply the preset and the laser"), TELL],
        grace: g(20.0, 3.0, 0.0, 0.0),
        gpu: true,
        pid: sess("wayvr.pid", "wayvr"),
        scope: Scope::Session,
        rule: desk,
    },
    Spec {
        id: "passthrough",
        label: "Passthrough",
        kind: Kind::Process,
        deps: &[Dep::hard("monado"), Dep::hard_if("broker", passthrough_6dof), Dep::hard("usb.cam")],
        toggle: Toggle::Live,
        class: Class::XrClient,
        ladder: &[r(Level::L1, "re-send gain and on state"), L2, TELL],
        grace: g(15.0, 3.0, 0.0, 0.0),
        gpu: true,
        pid: sess("passthrough.pid", "pimax-passthrou"),
        scope: Scope::Session,
        rule: passthrough,
    },
    Spec {
        id: "ctrltrack",
        label: "Controller tracking",
        kind: Kind::Process,
        deps: &[Dep::hard("broker"), Dep::hard("monado")],
        toggle: Toggle::Restart("monado"),
        class: Class::Leaf,
        ladder: &[L2, TELL],
        grace: g(10.0, 3.0, 0.0, 0.0),
        gpu: false,
        pid: sess("ctrltrack.pid", "pimax-ctrltrack"),
        scope: Scope::Session,
        rule: ctrltrack,
    },
    Spec {
        // camera hands (Mercury) inside Monado: a leaf feature, nothing depends on it; a failure only disables the
        // hands (Monado goes on), so pimaxd never restarts anything for it, it only tells
        id: "hands",
        label: "Camera hand tracking",
        kind: Kind::Feature,
        deps: &[Dep::hard("monado"), Dep::hard("broker")],
        toggle: Toggle::Restart("monado"),
        class: Class::None,
        ladder: &[TELL],
        grace: g(0.0, 0.0, 0.0, 0.0),
        gpu: false,
        pid: None,
        scope: Scope::Session,
        rule: hands,
    },
    Spec {
        id: "perf",
        label: "Performance panel",
        kind: Kind::Process,
        deps: &[Dep::hard("monado"), Dep::hard("wayvr")],
        toggle: Toggle::Live,
        class: Class::Leaf,
        ladder: &[L2, TELL],
        grace: g(10.0, 3.0, 0.0, 0.0),
        gpu: false,
        pid: sess("perf.pid", "pimax-perf"),
        scope: Scope::Session,
        rule: perf,
    },
    Spec {
        // the VR screens on demand (`pimax desk screens watch`: Hyprland events -> wayvrctl desk-screens); a leaf:
        // if it fails the screens just stay as they are
        id: "screens",
        label: "VR screens on demand",
        kind: Kind::Process,
        deps: &[Dep::hard("monado"), Dep::hard("wayvr")],
        toggle: Toggle::Live,
        class: Class::Leaf,
        ladder: &[L2, TELL],
        grace: g(10.0, 3.0, 0.0, 0.0),
        gpu: false,
        pid: sess("screens.pid", "pimax"),
        scope: Scope::Session,
        rule: screens,
    },
    Spec {
        id: "voice.listen",
        label: "Voice listener",
        kind: Kind::Process,
        deps: &[],
        toggle: Toggle::Live,
        class: Class::Leaf,
        ladder: &[L2, TELL],
        grace: g(10.0, 3.0, 0.0, 0.0),
        gpu: false,
        pid: sess("voice.pid", "pimax-voice"),
        scope: Scope::Session,
        rule: voice,
    },
    Spec {
        id: "voice.captions",
        label: "Live captions",
        kind: Kind::Process,
        deps: &[Dep::hard("voice.listen"), Dep::hard("wayvr")],
        toggle: Toggle::Live,
        class: Class::Leaf,
        ladder: &[L2, TELL],
        grace: g(10.0, 3.0, 0.0, 0.0),
        gpu: false,
        pid: sess("voicecap.pid", "pimax-voice"),
        scope: Scope::Session,
        rule: voice_desk,
    },
    Spec {
        id: "voice.act",
        label: "Voice commands",
        kind: Kind::Process,
        deps: &[Dep::hard("voice.listen"), Dep::hard("wayvr"), Dep::soft("ai.server")],
        toggle: Toggle::Live,
        class: Class::Leaf,
        ladder: &[L2, TELL],
        grace: g(10.0, 3.0, 0.0, 0.0),
        gpu: false,
        pid: sess("voiceact.pid", "pimax-voice"),
        scope: Scope::Session,
        rule: voice_desk,
    },
    Spec {
        id: "voicehud",
        label: "Voice HUD overlay",
        kind: Kind::Process,
        deps: &[Dep::hard("monado"), Dep::hard("voice.listen")],
        toggle: Toggle::Live,
        class: Class::XrClient,
        ladder: &[L2, TELL],
        grace: g(10.0, 3.0, 0.0, 0.0),
        gpu: true,
        pid: sess("voicehud.pid", "pimax-voice-hud"),
        scope: Scope::Session,
        rule: voicehud,
    },
    Spec {
        id: "ai.server",
        label: "Local AI models (Work only)",
        kind: Kind::Process,
        deps: &[],
        toggle: Toggle::Live,
        class: Class::Leaf,
        ladder: &[r(Level::L2, "pimax-ai up (the saved shape), at most 3 per 30 min"), TELL],
        grace: g(60.0, 3.0, 0.0, 0.0),
        gpu: false,
        pid: Some(PidSpec { file: "llama-server.pid", comm: "llama-server", root: PidRoot::AiState }),
        scope: Scope::Session,
        rule: ai,
    },
    Spec {
        id: "anchor.relocalize",
        label: "Relocalise (room map)",
        kind: Kind::Job,
        deps: &[Dep::hard("monado"), Dep::hard("broker")],
        toggle: Toggle::Live,
        class: Class::None,
        ladder: &[r(Level::L2, "one retry after 10 s"), TELL],
        grace: g(0.0, 0.0, 0.0, 0.0),
        gpu: false,
        pid: None,
        scope: Scope::Session,
        rule: anchor,
    },
    Spec {
        id: "floor.follow",
        label: "Floor follow (experimental)",
        kind: Kind::Process,
        deps: &[Dep::hard("broker"), Dep::hard("monado")],
        toggle: Toggle::Live,
        class: Class::Leaf,
        ladder: &[L2, TELL],
        grace: g(10.0, 3.0, 0.0, 0.0),
        gpu: false,
        pid: sess("floor.pid", "pimax-roommap"),
        scope: Scope::Session,
        rule: floor,
    },
    Spec {
        id: "record",
        label: "Recorder (pimax-record)",
        kind: Kind::Process,
        // reads Monado's recorder tap; a crash or stop never touches Monado, WayVR or passthrough
        deps: &[Dep::hard("monado")],
        toggle: Toggle::Live,
        class: Class::Leaf,
        ladder: &[r(Level::L2, "restart the recorder (a new file)"), TELL],
        // stop: up to 25 s to finish the mp4 (normally under 1 s)
        grace: g(10.0, 2.0, 0.0, 0.0),
        // NVENC makes it a GPU client: no automatic start while the gate is not ok
        gpu: true,
        pid: sess("record.pid", "pimax-record"),
        scope: Scope::Session,
        rule: on_demand,
    },
    Spec {
        id: "preset",
        label: "Desk preset",
        kind: Kind::Setting,
        deps: &[Dep::hard("wayvr")],
        toggle: Toggle::Live,
        class: Class::None,
        ladder: &[r(Level::L1, "re-apply after a WayVR or passthrough restart")],
        grace: g(0.0, 0.0, 0.0, 0.0),
        gpu: false,
        pid: None,
        scope: Scope::Session,
        rule: preset,
    },
];

/// The row of `id`.
pub fn spec(id: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.id == id)
}

/// The session processes in the launcher's session dir, for adoption and the status table.
pub fn session_processes() -> impl Iterator<Item = &'static Spec> {
    SPECS.iter().filter(|s| matches!(s.pid, Some(PidSpec { root: PidRoot::Session, .. })))
}

/// Toggle keys of `[components]` in pimaxd.toml and of `set`/`toggle` requests -> component ids.
pub fn toggle_ids(key: &str) -> &'static [Id] {
    match key {
        "passthrough" => &["passthrough"],
        "sixdof" | "6dof" => &["sixdof"],
        "ctrltrack" | "ctrl_track" => &["ctrltrack"],
        "eye" => &["eye"],
        "eye_laser" | "eye.laser" => &["eye.laser"],
        "perf" => &["perf"],
        "screens" => &["screens"],
        "voice" => &["voice.listen", "voice.captions", "voice.act"],
        "voice.listen" => &["voice.listen"],
        "captions" | "voice.captions" => &["voice.captions"],
        "voice.act" | "act" => &["voice.act"],
        "voicehud" => &["voicehud"],
        "ai_server" | "ai" | "ai.server" => &["ai.server"],
        "anchor" | "anchor.relocalize" => &["anchor.relocalize"],
        "floor_follow" | "floor.follow" => &["floor.follow"],
        "hands" => &["hands"],
        "wayvr" => &["wayvr"],
        "preset" => &["preset"],
        "record" | "recording" => &["record"],
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn local_ai_runs_in_work_sessions_only() {
        let s = super::spec("ai.server").unwrap();
        assert_eq!(s.scope, Scope::Session, "never without a session");
        let work = Desired { ai_server: true, mode: crate::model::Mode::Desk, ..Desired::default() };
        let play = Desired { mode: crate::model::Mode::Vr, ..work.clone() };
        assert!((s.rule)(&work) && !(s.rule)(&play), "Play / SteamVR never load the models");
        assert!(!(s.rule)(&Desired { ai_server: false, ..work }), "the Local models switch off");
    }

    use super::*;

    #[test]
    fn ids_are_unique_and_deps_exist() {
        for (i, s) in SPECS.iter().enumerate() {
            assert!(SPECS[..i].iter().all(|o| o.id != s.id), "duplicate {}", s.id);
            for d in s.deps {
                assert!(spec(d.id).is_some(), "{} depends on unknown {}", s.id, d.id);
            }
            if let Toggle::Restart(p) = s.toggle {
                assert!(spec(p).is_some());
            }
            assert!(!s.ladder.is_empty(), "{} has no ladder", s.id);
            assert_eq!(s.pid.is_some(), s.kind == Kind::Process && s.id != "keepalive", "{}", s.id);
        }
    }

    #[test]
    fn default_desired_wants_the_desk_set() {
        let d = Desired::default();
        let on: Vec<_> = SPECS.iter().filter(|s| (s.rule)(&d)).map(|s| s.id).collect();
        for id in ["monado", "wayvr", "passthrough", "broker", "ctrltrack", "perf", "eye", "sixdof"] {
            assert!(on.contains(&id), "{id}");
        }
        for id in ["voice.listen", "voicehud", "ai.server", "floor.follow", "hands"] {
            assert!(!on.contains(&id), "{id}");
        }
        let vr = Desired { mode: Mode::Vr, sixdof: false, ..Desired::default() };
        assert!(!(spec("wayvr").unwrap().rule)(&vr));
        assert!(!(spec("broker").unwrap().rule)(&vr));
    }

    #[test]
    fn hands_are_an_opt_in_leaf_that_needs_6dof() {
        let s = spec("hands").unwrap();
        assert_eq!((s.kind, s.class, s.pid), (Kind::Feature, Class::None, None));
        assert!(SPECS.iter().all(|o| o.deps.iter().all(|d| d.id != "hands")), "nothing depends on the hands");
        assert!(!(s.rule)(&Desired::default()), "off by default");
        assert!((s.rule)(&Desired { hands: true, ..Desired::default() }));
        assert!(!(s.rule)(&Desired { hands: true, sixdof: false, ..Desired::default() }));
        assert_eq!(toggle_ids("hands"), &["hands"]);
    }
}
