//! The preflight of a desk start: docs/LESSONS.md rows 1-10 (plus 12, 13, 18, 19 where they apply) as a pure
//! function of what was collected ([`Live`], [`Facts`]) and what the user asked for ([`DeskToggles`]).
//!
//! The result is a list of findings for the banner and a [`Gate`]: `Ready`, `NeedsConsent` (only WayVR Xid 31 faults:
//! the user may start anyway, which adds `--force`) or `Blocked`. `--force` makes the launcher skip *all* its checks,
//! so consent is only offered when nothing else is wrong; the launcher-side checks (headset, keepalive, connector,
//! service) are mirrored here for that reason.
use crate::builds::BuildInfo;
use crate::facts::{gib, Facts, Live};
use crate::kernel::Verdict;
use crate::launcher::{DeskPlan, DeskToggles, InstallState};
use crate::paths::Paths;
use pimax_doctor::Level;

/// How bad a finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Sev {
    /// Fine.
    Ok,
    /// Good to know.
    Info,
    /// Yellow, does not stop the start.
    Warn,
    /// Yellow; the start needs the user's explicit "start anyway" (`--force`).
    Consent,
    /// Red; no start.
    Block,
}

/// A button next to a finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// `pimax desk stop --grace 30` (the session's leftovers, by pid file).
    StopSession,
    /// `pimax vr stop --grace 30 --all` (monado-service processes the launcher did not start).
    StopForeignMonado,
    /// SIGINT one process by pid (never Monado; asks first).
    InterruptPid {
        /// Pid.
        pid: u32,
        /// Its name when it was seen (checked again before the signal).
        comm: String,
    },
    /// Open this item on the Setup page.
    Setup(String),
    /// Start again without eye tracking.
    WithoutEye,
}

/// One line of the banner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// LESSONS row, if the rule comes from there.
    pub row: Option<u8>,
    /// Stable id.
    pub id: &'static str,
    /// Severity.
    pub sev: Sev,
    /// One line.
    pub title: String,
    /// More text.
    pub detail: String,
    /// Buttons.
    pub actions: Vec<Action>,
}

/// May the start go ahead?
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    /// Yes.
    Ready,
    /// Only with the user's consent (`--force`); the reasons.
    NeedsConsent(Vec<String>),
    /// No; the reasons.
    Blocked(Vec<String>),
}

/// The whole preflight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preflight {
    /// Findings, worst first.
    pub findings: Vec<Finding>,
    /// The verdict.
    pub gate: Gate,
}

impl Preflight {
    /// Worst severity.
    pub fn worst(&self) -> Sev {
        self.findings.iter().map(|f| f.sev).max().unwrap_or(Sev::Ok)
    }

    /// A finding by id.
    pub fn get(&self, id: &str) -> Option<&Finding> {
        self.findings.iter().find(|f| f.id == id)
    }
}

struct Out(Vec<Finding>);

impl Out {
    fn add(&mut self, row: Option<u8>, id: &'static str, sev: Sev, title: impl Into<String>, detail: impl Into<String>, actions: Vec<Action>) {
        self.0.push(Finding {
            row,
            id,
            sev,
            title: title.into(),
            detail: detail.into(),
            actions,
        });
    }
}

/// The preflight while something it needs is still being read in the background: no start yet.
pub fn checking(detail: &str) -> Preflight {
    Preflight {
        findings: vec![Finding {
            row: None,
            id: "checking",
            sev: Sev::Block,
            title: "Checking this PC...".into(),
            detail: detail.into(),
            actions: vec![],
        }],
        gate: Gate::Blocked(vec!["still checking this PC".into()]),
    }
}

/// Evaluate the preflight for starting desk mode with `t`. `plan` is the launcher's plan for `t`; `chosen` the
/// inspection of its Monado build.
pub fn evaluate(
    live: &Live,
    facts: &Facts,
    t: &DeskToggles,
    plan: &Result<DeskPlan, String>,
    chosen: Option<&BuildInfo>,
    p: &Paths,
) -> Preflight {
    let mut o = Out(Vec::new());

    // Nothing is known before the first background check: no start on missing facts.
    if facts.at == 0 {
        return checking("The first check of the headset, the Monado builds and the kernel log runs in the background (a few seconds).");
    }

    // Row 1: one Monado at a time; a start waits for a previous one to exit.
    if let Some(s) = &live.service {
        o.add(Some(1), "previous-stop", Sev::Block, format!("Desk/VR session is running (monado-service pid {})", s.pid),
            "Stop it first; the next start waits until it has exited.", vec![Action::StopSession]);
    } else if !live.foreign_monado.is_empty() {
        let list: Vec<String> = live.foreign_monado.iter().map(u32::to_string).collect();
        o.add(Some(1), "previous-stop", Sev::Block,
            format!("monado-service pid {} is running outside the launcher session", list.join(", ")),
            "A second Monado fails with 'monado-service is already running'. Stop it with `pimax vr stop --grace 30 --all` (SIGINT, then SIGKILL only after 30 s; never pkill).",
            vec![Action::StopForeignMonado]);
    } else if live.desk_clients_alive() {
        let names: Vec<&str> = live.session.iter().filter(|x| x.alive && x.name != "monado").map(|x| x.name).collect();
        o.add(Some(1), "previous-stop", Sev::Block, format!("Leftovers of the last session still run: {}", names.join(", ")),
            "`pimax desk stop` stops them by pid (the camera broker last).", vec![Action::StopSession]);
    } else {
        o.add(Some(1), "previous-stop", Sev::Ok, "No monado-service running",
            "A start first waits (up to 45 s) until no monado-service is left.", vec![]);
    }

    // Row 5 + 8: the launcher is the only way Monado starts; it must exist, support the flags, and be current.
    let launcher_missing = !facts.has(&p.launcher_installed) && !facts.has(&p.launcher_built);
    let caps = &facts.caps;
    let mut unsupported = Vec::new();
    if t.eye && !caps.eye {
        unsupported.push("--eye");
    }
    if t.sixdof && !caps.sixdof {
        unsupported.push("--6dof");
    }
    if t.ctrl_track && !caps.ctrl_track {
        unsupported.push("--ctrl-track");
    }
    if t.voice && !caps.voice {
        unsupported.push("--voice");
    }
    if !t.passthrough && !caps.no_passthrough {
        unsupported.push("--no-passthrough");
    }
    if !t.perf && !caps.no_perf {
        unsupported.push("--no-perf");
    }
    if t.anchor && !caps.anchor {
        unsupported.push("--anchor");
    }
    if t.preset != crate::launcher::Preset::None && !caps.preset {
        unsupported.push("--preset");
    }
    if launcher_missing {
        o.add(Some(5), "launcher", Sev::Block, "The pimax launcher is not installed or built",
            "Monado is only ever started through `pimax desk start` (its env builder). Build and install it on the Setup page.",
            vec![Action::Setup("launcher".into())]);
    } else if !unsupported.is_empty() {
        o.add(Some(5), "launcher", Sev::Block, format!("The installed launcher has no {}", unsupported.join(" ")),
            "Install the current build (Setup > pimax launcher), or switch the option off.", vec![Action::Setup("launcher".into())]);
    } else {
        match facts.launcher {
            Some(InstallState::Differs) | Some(InstallState::BuildStale) => o.add(Some(8), "launcher", Sev::Warn,
                "The installed launcher differs from the workspace build",
                "Install it on the Setup page while nothing runs (replacing ~/.local/bin/pimax during a session is what row 8 forbids).",
                vec![Action::Setup("launcher".into())]),
            _ => o.add(Some(5), "launcher", Sev::Ok, "Monado starts through `pimax desk start` only",
                "The launcher's env builder sets the compositor (DP-1, mode 2 = 72 Hz / 3 = 90 Hz, Wayland direct, FORCE_NVIDIA=0); the app never hand-rolls it.",
                vec![]),
        }
    }
    if !facts.stale_env.is_empty() {
        let l: Vec<String> = facts.stale_env.iter().map(|(k, v)| format!("{k}={v}")).collect();
        o.add(Some(5), "stale-env", Sev::Info, "Experiment variables in the app's environment are not passed on",
            format!("Removed from the launcher's environment: {}", l.join(" ")), vec![]);
    }
    let plan = match plan {
        Ok(p) => Some(p),
        Err(e) => {
            o.add(Some(5), "plan", Sev::Block, "The launcher refuses these options", e.clone(), vec![]);
            None
        }
    };

    // Launcher-side preflight (mirrored so that --force is never offered over a real problem).
    for (id, title) in [("headset", "Headset not on USB"), ("keepalive", "pimax-keepalived is not active"), ("connector", "Headset display connector not ready")] {
        if let Some(c) = facts.doctor(id).filter(|c| c.level == Level::Fail) {
            o.add(None, match id { "headset" => "headset", "keepalive" => "keepalive", _ => "connector" }, Sev::Block, title, c.detail.clone(),
                vec![Action::Setup(id.to_string())]);
        }
    }

    // Row 2: cameras are single-consumer; a busy /dev/video* means a stalled earlier run.
    if t.needs_cameras() {
        if facts.camera_nodes.is_empty() {
            o.add(Some(2), "camera-busy", Sev::Warn, "No Pimax camera found under /sys/class/video4linux",
                "Passthrough and 6DoF need the camera (34a4:0014). Is the headset on?", vec![]);
        } else if !facts.camera_holders.is_empty() {
            let mut actions = Vec::new();
            let session_pids: Vec<u32> = live.session.iter().filter_map(|s| s.pid).collect();
            for h in &facts.camera_holders {
                if session_pids.contains(&h.pid) {
                    if !actions.contains(&Action::StopSession) {
                        actions.push(Action::StopSession);
                    }
                } else if h.comm != "monado-service" {
                    actions.push(Action::InterruptPid { pid: h.pid, comm: h.comm.clone() });
                }
            }
            let who: Vec<String> = facts.camera_holders.iter().map(|h| h.describe()).collect();
            o.add(Some(2), "camera-busy", Sev::Block, "Camera busy: only one process can stream it",
                format!("{}. Stop that process first (by pid).", who.join("; ")), actions);
        } else {
            let nodes: Vec<String> = facts.camera_nodes.iter().map(|n| n.display().to_string()).collect();
            let how = if t.sixdof {
                "6DoF, controllers and passthrough share it through `pimax-cam broker` (started first, stopped last by the launcher)"
            } else {
                "passthrough opens it directly (one consumer)"
            };
            o.add(Some(2), "camera-busy", Sev::Ok, format!("Camera free ({})", nodes.join(", ")), how, vec![]);
        }
    }

    // Row 3: the eye chip has one owner.
    if t.eye {
        if !facts.tobii {
            o.add(Some(3), "eye-busy", Sev::Warn, "Eye tracker (2104:0561) not enumerated",
                "Tools > Power on eye module (`pimax-eye power-on`), or start without eye tracking.", vec![Action::WithoutEye]);
        } else if !facts.eye_holders.is_empty() {
            let mut actions: Vec<Action> = facts
                .eye_holders
                .iter()
                .filter(|h| h.comm != "monado-service")
                .map(|h| Action::InterruptPid { pid: h.pid, comm: h.comm.clone() })
                .collect();
            actions.push(Action::WithoutEye);
            let who: Vec<String> = facts.eye_holders.iter().map(|h| h.describe()).collect();
            o.add(Some(3), "eye-busy", Sev::Block, "Eye tracker busy: one process only",
                format!("{}. Quit it (pimax-eye watch / ipd / pimax-eyecal) or start without eye tracking.", who.join("; ")), actions);
        } else {
            o.add(Some(3), "eye-busy", Sev::Ok, "Eye tracker free",
                "While desk mode runs with eye tracking, the eye tools (view, IPD, calibration) are greyed out.", vec![]);
        }
    }

    // Row 4: one main OpenXR app; stray clients would attach to the new Monado.
    if !facts.stray_clients.is_empty() {
        let who: Vec<String> = facts.stray_clients.iter().map(|(p, c)| format!("{c} (pid {p})")).collect();
        let actions = facts.stray_clients.iter().map(|(p, c)| Action::InterruptPid { pid: *p, comm: c.clone() }).collect();
        o.add(Some(4), "xr-clients", Sev::Warn, "OpenXR clients from an earlier run are still alive",
            format!("{}. Monado shows only one main app: in desk mode that is the passthrough, WayVR is an overlay. Quit them before starting.", who.join(", ")),
            actions);
    }

    // Rows 6, 7, 12: the build the launcher will use.
    if let Some(b) = chosen {
        let svc = b.service.display().to_string();
        if !b.built() {
            o.add(Some(6), "build", Sev::Block, format!("No Monado build at {}", b.build.dir.display()),
                "Build it on the Setup page.", vec![Action::Setup(format!("build-{}", b.build.key))]);
        }
        if t.monado_uses_cameras() && b.built() {
            match b.pimax_cam() {
                Some(true) => o.add(Some(6), "pimax-cam", Sev::Ok, "Build has PIMAX_CAM=ON (CMakeCache.txt)",
                    "After the start the log is watched for 'built without -DPIMAX_CAM=ON'.", vec![]),
                Some(false) => o.add(Some(6), "pimax-cam", Sev::Block,
                    format!("{} was built with PIMAX_CAM=OFF", b.build.dir.display()),
                    "6DoF would silently run 3DoF. Reconfigure + rebuild: cmake -DPIMAX_CAM=ON . && ninja (Setup), then setcap.",
                    vec![Action::Setup(format!("pimax-cam-{}", b.build.key))]),
                None => o.add(Some(6), "pimax-cam", Sev::Block,
                    format!("Cannot read PIMAX_CAM from {}/CMakeCache.txt", b.build.dir.display()),
                    "6DoF needs a build configured with -DPIMAX_CAM=ON.", vec![Action::Setup(format!("pimax-cam-{}", b.build.key))]),
            }
            if t.anchor && b.markers.is_some_and(|k| !k.anchor) {
                o.add(None, "anchor-hook", Sev::Block, "This Monado build has no room anchor hook (PIMAX_ANCHOR)",
                    "Rebuild the desk build (branch desk) or switch the room anchor hook off.", vec![Action::Setup(format!("build-{}", b.build.key))]);
            }
            if let Some(k) = b.markers {
                if !k.slam {
                    o.add(Some(6), "slam-code", Sev::Block, "This Monado build has no 6DoF code (no PIMAX_SLAM)",
                        "Use the desk build (~/src/monado-eye/build, branch desk).", vec![]);
                } else if t.sixdof && !k.imu_gate {
                    o.add(Some(12), "ae-safe", Sev::Block, "Build lacks the Basalt IMU gate (Monado slam d3138d4)",
                        "With auto exposure on, the first 0x51 push froze cameras and IMU in builds without it. Rebuild from branch desk.", vec![]);
                }
            }
        }
        match b.sys_nice {
            Some(true) => o.add(Some(7), "setcap", Sev::Ok, "monado-service has cap_sys_nice", svc, vec![]),
            Some(false) => o.add(Some(7), "setcap", Sev::Warn, "monado-service lacks cap_sys_nice (dropped by the last relink)",
                format!("GPU priority requests fail and frames stutter. Press Fix on Home (or just Play): it restores it with one password prompt. ({svc})"),
                vec![Action::Setup(format!("setcap-{}", b.build.key))]),
            None => {}
        }
        // one build carries everything (.claude/CLAUDE.md section 3): a build that lost a feature is refused
        if let (true, Some(k)) = (b.built(), b.markers) {
            let miss = k.missing(t.hands);
            if miss.is_empty() {
                o.add(Some(16), "markers", Sev::Ok, "Monado build has every feature", format!("{} (6DoF, eye, controllers, anchor, IMU gate{})", b.build.dir.display(), if t.hands { ", hands" } else { "" }), vec![]);
            } else {
                o.add(Some(16), "markers", Sev::Block, format!("The Monado build is incomplete: it lacks {}", miss.join(", ")),
                    format!("{} is missing {}. The app only starts the one full build; rebuild it (Setup) or switch back to the full build.", b.build.dir.display(), miss.join(", ")),
                    vec![Action::Setup(format!("build-{}", b.build.key))]);
            }
        }
    }
    if let (None, Some(pl)) = (chosen, plan) {
        o.add(Some(6), "build", Sev::Block, format!("{} was not checked yet", pl.monado_build.display()),
            "The next background check reads it (PIMAX_MONADO_BUILD points to a build the app did not know); press Re-check.", vec![]);
    }
    if let Some(pl) = plan {
        let mut missing: Vec<String> = pl.clients.iter().filter(|c| !facts.has(&c.program) && !exists_hint(&c.program, p, facts)).map(|c| c.program.display().to_string()).collect();
        missing.extend(pl.sixdof_bins.iter().filter(|b| !facts.has(b)).map(|b| b.display().to_string()));
        if let Some(bl) = &pl.basalt {
            if !facts.has(bl) {
                missing.push(format!("{} (Basalt)", bl.display()));
            }
        }
        if !missing.is_empty() {
            o.add(None, "binaries", Sev::Block, "Missing programs for this mode", missing.join(", "),
                vec![Action::Setup("rust-bins".into())]);
        }
    }

    // Row 9: Xid classification.
    match &facts.kernel_error {
        Some(e) => o.add(Some(9), "xid", Sev::Warn, "Kernel log not readable", e.clone(), vec![]),
        None => match facts.kernel_verdict() {
            Verdict::Clean => o.add(Some(9), "xid", Sev::Ok, "No Xid / display engine errors this boot", "", vec![]),
            Verdict::WayvrOnly(ev) => {
                let l: Vec<String> = ev.iter().map(|e| e.summary()).collect();
                o.add(Some(9), "xid", Sev::Consent, format!("{} WayVR MMU fault(s) (Xid 31) this boot", ev.len()),
                    format!("{}. Per-app faults the GPU recovers from; the launcher refuses them, so the start needs your OK (--force).", l.join("; ")), vec![]);
            }
            Verdict::Reboot(ev) => {
                let l: Vec<String> = ev.iter().map(|e| e.summary()).collect();
                o.add(Some(9), "xid", Sev::Block, "GPU/display errors this boot: reboot recommended",
                    format!("{}. A session on top of a wedged display engine can hang the machine.", l.join("; ")), vec![]);
            }
        },
    }

    // Row 10: the flip-hang fix must be loaded.
    let nvidia = facts.doctor_level("gpu").is_some_and(|l| l != Level::Info);
    if nvidia || facts.merged_head_sync.is_some() {
        match facts.merged_head_sync.as_deref() {
            Some("1") | Some("Y") => o.add(Some(10), "merged-head-sync", Sev::Ok, "merged_head_sync = 1 (flip-hang fix loaded)", "", vec![]),
            Some(v) => o.add(Some(10), "merged-head-sync", Sev::Block, format!("merged_head_sync = {v}"),
                "The display can freeze (flip never completes). Set it to 1 and reboot (Setup).", vec![Action::Setup("merged-head-sync".into())]),
            None => o.add(Some(10), "merged-head-sync", Sev::Block, "The loaded nvidia_modeset has no merged_head_sync",
                "The running module is not the patched build. Re-apply the 3 patches, rebuild, reboot (Setup).", vec![Action::Setup("merged-head-sync".into())]),
        }
        if !(facts.hooks[0] && facts.hooks[1]) {
            o.add(Some(10), "pacman-hooks", Sev::Warn, "pacman hooks missing: the next driver update drops the patches",
                "Install them on the Setup page (root, shown first).", vec![Action::Setup("pacman-hooks".into())]);
        }
    }

    // Rows 13, 18, 19 and open issues: information for the chosen mode.
    if t.sixdof {
        o.add(Some(13), "lit-room", Sev::Info, "6DoF: start in a lit room with the cameras free",
            "Not face down on a cushion: the cameras must see the room, or the IMU dead-reckons for metres.", vec![]);
    }
    if let Some(mm) = facts.ipd_mm {
        o.add(Some(18), "ipd", Sev::Info, format!("IPD {mm:.1} mm from ~/.config/pimax/ipd.env"),
            "Passed to Monado and its desk clients as OXR_DEBUG_IPD_MM.", vec![]);
    }
    if t.voice {
        if !facts.has(&p.voice_model) || !facts.has(&p.sherpa_lib) {
            o.add(Some(19), "voice", Sev::Block, "Voice model or sherpa-onnx library missing",
                format!("{} / {}", p.voice_model.display(), p.sherpa_lib.display()), vec![Action::Setup("voice".into())]);
        } else {
            o.add(Some(19), "voice", Sev::Info, "Voice: the headset microphone sends zeros while the display is off",
                "Speak once the picture is up.", vec![]);
        }
    }
    if t.hz90 {
        o.add(None, "hz", Sev::Info, "90 Hz is new: same start commands as Pimax Play on Windows (open issue 9)", "Check the picture in the headset; 72 Hz is the long-tested rate.", vec![]);
    }
    if let Some(free) = facts.free_home.filter(|f| *f < (5u64 << 30)) {
        o.add(Some(15), "disk", Sev::Warn, format!("Only {} free in your home", gib(free)), "Recordings and logs can fill it.", vec![]);
    }

    let mut findings = o.0;
    findings.sort_by_key(|f| std::cmp::Reverse(f.sev));
    let gate = gate_of(&findings);
    Preflight { findings, gate }
}

/// The verdict of a list of findings: any block blocks, else any consent needs the user's OK.
pub fn gate_of(findings: &[Finding]) -> Gate {
    let blocks: Vec<String> = findings.iter().filter(|f| f.sev == Sev::Block).map(|f| f.title.clone()).collect();
    let consents: Vec<String> = findings.iter().filter(|f| f.sev == Sev::Consent).map(|f| f.title.clone()).collect();
    if !blocks.is_empty() {
        Gate::Blocked(blocks)
    } else if !consents.is_empty() {
        Gate::NeedsConsent(consents)
    } else {
        Gate::Ready
    }
}

/// The preflight of `pimax vr start` (a game session: Monado alone, no desk clients) from the desk preflight of
/// the same switches: the same rules, without the desk programs (passthrough, WayVR, voice) in the missing-binaries
/// check.
pub fn for_vr(desk: Preflight, plan: Option<&DeskPlan>, facts: &Facts) -> Preflight {
    let mut findings: Vec<Finding> = desk.findings.into_iter().filter(|f| f.id != "binaries").collect();
    if let Some(pl) = plan {
        let mut missing: Vec<String> = pl.sixdof_bins.iter().filter(|b| !facts.has(b)).map(|b| b.display().to_string()).collect();
        if let Some(bl) = pl.basalt.as_ref().filter(|b| !pl.sixdof_bins.is_empty() && !facts.has(b)) {
            missing.push(format!("{} (Basalt)", bl.display()));
        }
        if !missing.is_empty() {
            findings.push(Finding {
                row: None,
                id: "binaries",
                sev: Sev::Block,
                title: "Missing programs for 6DoF".into(),
                detail: missing.join(", "),
                actions: vec![Action::Setup("rust-bins".into())],
            });
        }
    }
    findings.sort_by_key(|f| std::cmp::Reverse(f.sev));
    let gate = gate_of(&findings);
    Preflight { findings, gate }
}

/// What blocks any start through the launcher besides the session itself: the launcher-side checks that `--force`
/// would skip (headset on USB, keepalive, display connector), merged_head_sync (row 10) and a kernel log that needs a
/// reboot (row 9). (id of the Setup item, text). Used by the tool sessions; the desk preflight reports the same.
pub fn launcher_side(facts: &Facts) -> Vec<(&'static str, String)> {
    let mut v = Vec::new();
    for (id, title) in [("headset", "Headset not on USB"), ("keepalive", "pimax-keepalived is not active"), ("connector", "Headset display connector not ready")] {
        if let Some(c) = facts.doctor(id).filter(|c| c.level == Level::Fail) {
            v.push((id, format!("{title}: {}", c.detail)));
        }
    }
    let nvidia = facts.doctor_level("gpu").is_some_and(|l| l != Level::Info);
    if (nvidia || facts.merged_head_sync.is_some()) && !matches!(facts.merged_head_sync.as_deref(), Some("1") | Some("Y")) {
        v.push(("merged-head-sync", "merged_head_sync is not 1: the display can freeze (LESSONS row 10)".to_string()));
    }
    if let Verdict::Reboot(ev) = facts.kernel_verdict() {
        v.push((
            "kernel-log",
            format!(
                "GPU/display errors this boot ({}): reboot before a headset session (LESSONS row 9)",
                ev.first().map(|e| e.summary()).unwrap_or_default()
            ),
        ));
    }
    v
}

/// Programs outside the checked list (custom `--*-bin` paths) are not judged here; the launcher checks them.
fn exists_hint(prog: &std::path::Path, p: &Paths, _f: &Facts) -> bool {
    let known = crate::facts::RELEASE_BINS.iter().any(|(b, _)| p.bin(b) == prog) || prog == p.wayvr_bin;
    !known
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builds::MarkerCache;
    use crate::demo;
    use crate::facts::{collect_facts, collect_live};
    use crate::launcher::desk_plan;
    use crate::machine::FakeMachine;
    use crate::monlog::LogFollower;

    fn eval_on(m: &FakeMachine, p: &Paths, t: DeskToggles) -> Preflight {
        let facts = collect_facts(m, p, &demo::opts(p), &MarkerCache::default(), None, true, true);
        let mut fl = LogFollower::new(&p.session_dir.join("monado.log"));
        let live = collect_live(m, p, &mut fl);
        let env = |k: &str| match k {
            "HOME" => Some("/home/u".to_string()),
            "XDG_RUNTIME_DIR" => Some("/run/user/1000".to_string()),
            _ => None,
        };
        let plan = desk_plan(&t, &env);
        let chosen = plan.as_ref().ok().and_then(|pl| facts.build(&pl.monado_build).cloned());
        evaluate(&live, &facts, &t, &plan, chosen.as_ref(), p)
    }

    fn sev(pf: &Preflight, id: &str) -> Sev {
        pf.get(id).unwrap_or_else(|| panic!("no finding {id}: {:#?}", pf.findings)).sev
    }

    #[test]
    fn demo_pc_needs_consent_for_the_wayvr_xid31_only() {
        let (m, p) = demo::idle_machine();
        let pf = eval_on(&m, &p, DeskToggles::with_eye());
        assert_eq!(sev(&pf, "xid"), Sev::Consent);
        assert!(pf.get("xid").unwrap().detail.contains("Xid 31 from wayvr (pid 2499196)"));
        assert_eq!(sev(&pf, "previous-stop"), Sev::Ok);
        assert_eq!(sev(&pf, "camera-busy"), Sev::Ok);
        assert_eq!(sev(&pf, "eye-busy"), Sev::Ok);
        assert_eq!(sev(&pf, "pimax-cam"), Sev::Ok);
        assert_eq!(sev(&pf, "setcap"), Sev::Ok);
        assert_eq!(sev(&pf, "merged-head-sync"), Sev::Ok);
        assert_eq!(sev(&pf, "launcher"), Sev::Ok);
        assert!(matches!(pf.gate, Gate::NeedsConsent(_)), "{:?}", pf.gate);
        assert_eq!(sev(&pf, "ipd"), Sev::Info);
        assert_eq!(sev(&pf, "lit-room"), Sev::Info);
    }

    #[test]
    fn clean_kernel_log_is_ready() {
        let (mut m, p) = demo::idle_machine();
        m.cmd("journalctl -k -b --no-pager -q", 0, "kernel: quiet\n");
        let pf = eval_on(&m, &p, DeskToggles::with_eye());
        assert_eq!(pf.gate, Gate::Ready, "{:#?}", pf.findings);
    }

    #[test]
    fn other_xids_and_display_engine_errors_block() {
        let (mut m, p) = demo::idle_machine();
        m.cmd(
            "journalctl -k -b --no-pager -q",
            0,
            &format!("{}Oct 14 16:00:00 mau kernel: nvidia-modeset: ERROR: GPU:0: Idling display engine timed out: 0x0000c67e:0:0:1174\n", demo::KERNEL_LOG),
        );
        let pf = eval_on(&m, &p, DeskToggles::with_eye());
        assert_eq!(sev(&pf, "xid"), Sev::Block);
        assert!(matches!(pf.gate, Gate::Blocked(_)));
        assert_eq!(pf.findings[0].sev, Sev::Block, "worst first");
    }

    #[test]
    fn running_session_and_foreign_monado_block_with_the_right_stop() {
        let (m, p) = demo::running_machine();
        let pf = eval_on(&m, &p, DeskToggles::with_eye());
        assert_eq!(pf.get("previous-stop").unwrap().actions, vec![Action::StopSession]);
        let (mut m, p) = demo::idle_machine();
        m.process(555, "monado-service", "/home/u/src/monado-slam/build/src/xrt/targets/service/monado-service", &[], 10, &[]);
        let pf = eval_on(&m, &p, DeskToggles::with_eye());
        let f = pf.get("previous-stop").unwrap();
        assert_eq!(f.sev, Sev::Block);
        assert_eq!(f.actions, vec![Action::StopForeignMonado]);
        assert!(f.title.contains("555"));
    }

    #[test]
    fn busy_camera_and_eye_chip_block_with_pid_actions() {
        let (mut m, p) = demo::idle_machine();
        m.process(4242, "pimax-cam", "/home/u/pimax-re/rust/target/release/pimax-cam broker", &[], 10, &["/dev/video0"]);
        m.process(4243, "pimax-eye", "pimax-eye watch", &[], 10, &["/dev/bus/usb/001/009"]);
        let pf = eval_on(&m, &p, DeskToggles::with_eye());
        let cam = pf.get("camera-busy").unwrap();
        assert_eq!(cam.sev, Sev::Block);
        assert_eq!(cam.actions, vec![Action::InterruptPid { pid: 4242, comm: "pimax-cam".into() }]);
        let eye = pf.get("eye-busy").unwrap();
        assert_eq!(eye.sev, Sev::Block);
        assert!(eye.actions.contains(&Action::WithoutEye));
        // without eye tracking the eye chip does not matter; without cameras the camera does not either
        let t = DeskToggles { eye: false, sixdof: false, passthrough: false, ..DeskToggles::with_eye() };
        let pf = eval_on(&m, &p, t);
        assert!(pf.get("eye-busy").is_none() && pf.get("camera-busy").is_none());
    }

    #[test]
    fn camera_held_by_a_session_leftover_offers_desk_stop() {
        let (mut m, p) = demo::idle_machine();
        m.process(372448, "pimax-cam", "/home/u/pimax-re/rust/target/release/pimax-cam broker", &[], 10, &["/dev/video0"]);
        m.file("/tmp/pimax-vr-session/broker.pid", "372448\n");
        let pf = eval_on(&m, &p, DeskToggles::with_eye());
        assert_eq!(pf.get("camera-busy").unwrap().actions, vec![Action::StopSession]);
        assert_eq!(sev(&pf, "previous-stop"), Sev::Block, "leftover broker of the last session");
    }

    #[test]
    fn build_without_camera_code_or_capability() {
        let (mut m, p) = demo::idle_machine();
        m.file(
            "/home/u/src/monado-eye/build/CMakeCache.txt",
            include_str!("../tests/fixtures/CMakeCache-desk.txt").replace("PIMAX_CAM:BOOL=ON", "PIMAX_CAM:BOOL=OFF"),
        );
        let svc = "/home/u/src/monado-eye/build/src/xrt/targets/service/monado-service";
        m.cmd(&format!("getcap {svc}"), 0, "");
        let pf = eval_on(&m, &p, DeskToggles::with_eye());
        assert_eq!(sev(&pf, "pimax-cam"), Sev::Block);
        assert_eq!(pf.get("pimax-cam").unwrap().actions, vec![Action::Setup("pimax-cam-desk".into())]);
        assert_eq!(sev(&pf, "setcap"), Sev::Warn);
        // 3DoF desk mode does not need the camera code
        let t = DeskToggles { sixdof: false, ..DeskToggles::with_eye() };
        assert!(eval_on(&m, &p, t).get("pimax-cam").is_none());
    }

    #[test]
    fn launcher_without_the_requested_flag_blocks() {
        let (mut m, p) = demo::idle_machine();
        m.cmd("/home/u/.local/bin/pimax desk help", 0, "usage: pimax desk <command>\n start [--no-passthrough] [--eye] [--voice]\n");
        let pf = eval_on(&m, &p, DeskToggles::with_eye());
        let f = pf.get("launcher").unwrap();
        assert_eq!(f.sev, Sev::Block);
        assert!(f.title.contains("--6dof"));
    }

    #[test]
    fn merged_head_sync_and_hooks() {
        let (mut m, p) = demo::idle_machine();
        m.file("/sys/module/nvidia_modeset/parameters/merged_head_sync", "0\n");
        m.host.files.remove(std::path::Path::new("/etc/pacman.d/hooks/75-nvidia-pimax-verify.hook"));
        let pf = eval_on(&m, &p, DeskToggles::with_eye());
        assert_eq!(sev(&pf, "merged-head-sync"), Sev::Block);
        assert_eq!(sev(&pf, "pacman-hooks"), Sev::Warn);
        m.host.files.remove(std::path::Path::new("/sys/module/nvidia_modeset/parameters/merged_head_sync"));
        let pf = eval_on(&m, &p, DeskToggles::with_eye());
        assert!(pf.get("merged-head-sync").unwrap().title.contains("no merged_head_sync"));
    }

    #[test]
    fn launcher_side_blocks_match_what_force_would_skip() {
        let (m, p) = demo::idle_machine();
        let f = crate::facts::collect_facts(&m, &p, &demo::opts(&p), &MarkerCache::default(), None, true, true);
        assert!(launcher_side(&f).is_empty(), "WayVR Xid 31 alone is not a block");
        let (mut m, p) = demo::idle_machine();
        m.cmd("systemctl --user is-active pimax-keepalived", 3, "inactive\n");
        m.file("/sys/module/nvidia_modeset/parameters/merged_head_sync", "0\n");
        let f = crate::facts::collect_facts(&m, &p, &demo::opts(&p), &MarkerCache::default(), None, true, true);
        let ids: Vec<&str> = launcher_side(&f).iter().map(|(i, _)| *i).collect();
        assert_eq!(ids, vec!["keepalive", "merged-head-sync"]);
    }

    #[test]
    fn unsupported_flags_block_instead_of_being_dropped() {
        let (mut m, p) = demo::idle_machine();
        m.cmd("/home/u/.local/bin/pimax desk help", 0, "usage: pimax desk <command>\n start [--eye] [--6dof] [--voice]\n");
        let t = DeskToggles { passthrough: false, ..DeskToggles::with_eye() };
        let f = eval_on(&m, &p, t).get("launcher").cloned().unwrap();
        assert_eq!(f.sev, Sev::Block);
        assert!(f.title.contains("--no-passthrough"), "{}", f.title);
    }

    #[test]
    fn stray_clients_and_missing_binaries() {
        let (mut m, p) = demo::idle_machine();
        m.process(900, "pimax-lenstest", "pimax-lenstest", &[], 10, &[]);
        m.host.files.remove(std::path::Path::new("/home/u/.local/lib/libbasalt.so"));
        let pf = eval_on(&m, &p, DeskToggles::with_eye());
        assert_eq!(sev(&pf, "xr-clients"), Sev::Warn);
        let b = pf.get("binaries").unwrap();
        assert_eq!(b.sev, Sev::Block);
        assert!(b.detail.contains("libbasalt.so"));
    }
}
