//! The Setup page: every omarchyvr-doctor check, mapped into a checklist item, plus the checks docs/LESSONS.md adds
//! (setcap per build, PIMAX_CAM, the Basalt IMU gate, pacman hooks, modinfo, udev rule file, builds, voice models,
//! launcher and app installation, Steam runtime file). Each item carries its fix:
//!
//! * [`Fix::User`]: commands the app runs directly (as you), with a progress log; some only while nothing runs.
//! * [`Fix::Root`]: the documented `sudo` lines, shown with Copy and "Run with pkexec" (the user confirms; LESSONS
//!   row 21). Lines that are not a documented fix shape (package installs, reboots, lines that need editing) are
//!   copy-only: [`RootFix::pkexec`] says why.
//! * [`Fix::Manual`]: instructions and lines to copy.
use crate::caps;
use crate::cmd::{pkexec_cmd, quote, Cmd};
use crate::facts::{gib, Facts, Live, RELEASE_BINS};
use crate::kernel::Verdict;
use crate::launcher::InstallState;
use crate::paths::{BuildDir, Paths};
use omarchyvr_doctor::Level;
use std::path::PathBuf;

/// Sections of the page, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    /// USB, udev, keepalive.
    Headset,
    /// GPU, NVIDIA patches, connector, kernel log.
    Display,
    /// Monado builds.
    Builds,
    /// WayVR, Basalt, Rust binaries.
    Desk,
    /// Launcher and this app.
    App,
    /// Voice, Steam runtime, IPD, calibration, disk.
    Extras,
}

impl Group {
    /// Section title.
    pub fn title(self) -> &'static str {
        match self {
            Group::Headset => "Headset and access",
            Group::Display => "GPU and display (NVIDIA)",
            Group::Builds => "Monado builds",
            Group::Desk => "Desk mode parts",
            Group::App => "Launcher and app",
            Group::Extras => "Extras",
        }
    }

    /// All groups.
    pub const ALL: [Group; 6] = [Group::Headset, Group::Display, Group::Builds, Group::Desk, Group::App, Group::Extras];
}

/// What happens after a user fix succeeded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FollowUp {
    /// A relink dropped `cap_sys_nice`: show the setcap root fix for this binary right away (LESSONS row 7).
    Setcap(PathBuf),
}

/// A fix the app runs as the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserFix {
    /// Why.
    pub why: String,
    /// Commands in order.
    pub steps: Vec<Cmd>,
    /// Afterwards.
    pub follow_up: Option<FollowUp>,
    /// Only while no part of the stack runs (installing the launcher, relinking Monado: LESSONS row 8).
    pub needs_idle: bool,
}

/// A fix that needs root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootFix {
    /// Why.
    pub why: String,
    /// The documented lines (with `sudo`), shown and copied as they are.
    pub lines: Vec<String>,
    /// How to undo it.
    pub undo: Vec<String>,
    /// Reboot afterwards.
    pub reboot: bool,
    /// Can it be run with pkexec from the app? `Err(reason)` = copy only.
    pub pkexec: Result<(), String>,
}

impl RootFix {
    /// A root fix; whether it may run through pkexec is decided by the safety rules.
    pub fn new(why: impl Into<String>, lines: Vec<String>, undo: Vec<String>, reboot: bool) -> RootFix {
        let pkexec = crate::safety::check(&pkexec_cmd(&lines));
        RootFix {
            why: why.into(),
            lines,
            undo,
            reboot,
            pkexec,
        }
    }

    /// Copy only, with a reason.
    pub fn copy_only(mut self, reason: &str) -> RootFix {
        self.pkexec = Err(reason.to_string());
        self
    }

    /// The pkexec command (when allowed).
    pub fn command(&self) -> Option<Cmd> {
        self.pkexec.is_ok().then(|| pkexec_cmd(&self.lines))
    }
}

/// How to fix an item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fix {
    /// Nothing to do.
    None,
    /// Run as the user.
    User(UserFix),
    /// Root.
    Root(RootFix),
    /// Text and lines to copy.
    Manual {
        /// What to do.
        why: String,
        /// Lines to copy.
        lines: Vec<String>,
    },
}

/// One row of the Setup page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// Stable id (`setcap-desk`, `keepalive`, ...).
    pub id: String,
    /// Section.
    pub group: Group,
    /// Title.
    pub title: String,
    /// Result.
    pub level: Level,
    /// What was found.
    pub detail: String,
    /// LESSONS row behind the check.
    pub row: Option<u8>,
    /// Where it comes from: the doctor or this app.
    pub from_doctor: bool,
    /// The fix (only meaningful when the level is Warn or Fail).
    pub fix: Fix,
}

impl Item {
    pub(crate) fn new(id: &str, group: Group, title: &str, level: Level, detail: impl Into<String>, row: Option<u8>, fix: Fix) -> Item {
        Item {
            id: id.to_string(),
            group,
            title: title.to_string(),
            level,
            detail: detail.into(),
            row,
            from_doctor: false,
            fix,
        }
    }

    /// Needs attention.
    pub fn bad(&self) -> bool {
        self.level >= Level::Warn
    }
}

fn user(why: &str, steps: Vec<Cmd>) -> Fix {
    Fix::User(UserFix {
        why: why.into(),
        steps,
        follow_up: None,
        needs_idle: false,
    })
}

fn manual(why: &str, lines: &[&str]) -> Fix {
    Fix::Manual {
        why: why.into(),
        lines: lines.iter().map(|s| s.to_string()).collect(),
    }
}

/// `install` a file atomically next to the target, then rename (works while the old binary runs).
pub(crate) fn install_cmd(src: &std::path::Path, dst: &std::path::Path, mode: &str) -> Cmd {
    let tmp = format!("{}.new", dst.display());
    Cmd::shell(&format!(
        "install -Dm{mode} {} {} && mv -f {} {}",
        quote(&src.display().to_string()),
        quote(&tmp),
        quote(&tmp),
        quote(&dst.display().to_string())
    ))
}

/// `cmake -DPIMAX_CAM=ON . && ninja` in a build directory, then the setcap prompt (the relink drops the capability).
pub fn rebuild_fix(b: &BuildDir, reconfigure: bool) -> UserFix {
    let mut steps = Vec::new();
    if reconfigure {
        steps.push(Cmd::new("cmake", &["-DPIMAX_CAM=ON", "."]).cwd(&b.dir));
    }
    steps.push(Cmd::new("ninja", &[]).cwd(&b.dir));
    UserFix {
        why: format!(
            "Rebuild {} ({}){}; the relink drops cap_sys_nice, so the setcap prompt follows.",
            b.label,
            b.dir.display(),
            if reconfigure { " with -DPIMAX_CAM=ON" } else { "" }
        ),
        steps,
        follow_up: Some(FollowUp::Setcap(b.service())),
        needs_idle: true,
    }
}

/// The setcap root fix for one service binary (LESSONS row 7).
pub fn setcap_fix(service: &std::path::Path) -> RootFix {
    RootFix::new(
        "GPU priority for the compositor thread; every relink of monado-service drops it.",
        vec![caps::setcap_command(service)],
        vec![format!("sudo setcap -r {}", quote(&service.display().to_string()))],
        false,
    )
}

/// True when this install carries the Rust sources, so a user fix may build from them. A clean install (tools in
/// `~/.local/bin`, no `rust/` workspace) must never be told to build (LESSONS row 60).
pub fn repo_build_possible(facts: &Facts, p: &Paths) -> bool {
    facts.has(&p.rust.join("Cargo.toml"))
}

/// Build + install the launcher (only while nothing runs: LESSONS row 8).
pub fn launcher_fix(p: &Paths) -> UserFix {
    UserFix {
        why: "Build the pimax launcher and install it to ~/.local/bin (only while no desk/VR session runs).".into(),
        steps: vec![
            Cmd::new("cargo", &["build", "--release", "-p", "omarchyvr-tools"]).cwd(&p.rust),
            install_cmd(&p.launcher_built, &p.launcher_installed, "755"),
        ],
        follow_up: None,
        needs_idle: true,
    }
}

/// Install this app: binary to ~/.local/bin, icon, desktop entry (no root).
pub fn app_fix(p: &Paths) -> UserFix {
    let tmpl = p.repo.join("linux/omarchyvr.desktop");
    let svg = p.repo.join("linux/omarchyvr.svg");
    let apps = p.desktop_entry.parent().map(|d| d.display().to_string()).unwrap_or_default();
    UserFix {
        why: "Install the app (binary, icon, desktop entry 'omarchyvr' in ~/.local/share/applications).".into(),
        steps: vec![
            Cmd::new("cargo", &["build", "--release", "-p", "omarchyvr-app"]).cwd(&p.rust),
            install_cmd(&p.app_built, &p.app_installed, "755"),
            Cmd::new("install", &["-Dm644", &svg.display().to_string(), &p.icon.display().to_string()]),
            Cmd::shell(&format!(
                "mkdir -p {apps} && sed 's|@OMARCHYVR_APP@|{}|g' {} > {}.new && mv -f {}.new {}",
                p.app_installed.display(),
                quote(&tmpl.display().to_string()),
                quote(&p.desktop_entry.display().to_string()),
                quote(&p.desktop_entry.display().to_string()),
                quote(&p.desktop_entry.display().to_string()),
                apps = quote(&apps),
            )),
        ],
        follow_up: None,
        needs_idle: false,
    }
}

/// The documented pacman hook install (INVENTORY 3.4), with absolute paths (pkexec runs as root: `$HOME` is /root).
pub fn hooks_fix(p: &Paths) -> RootFix {
    let l = quote(&p.repo.join("linux").display().to_string());
    RootFix::new(
        "The hooks re-apply the 3 NVIDIA patches before every nvidia-open-dkms build and verify merged_head_sync after it.",
        vec![format!(
            "sudo env L={l} bash -c 'install -Dm755 \"$L/nvidia-patch-reapply.sh\" /usr/local/lib/omarchyvr/nvidia-patch-reapply.sh && \
for p in nvidia-2h1or-viewport nvidia-2h1or-flip-fix nvidia-evo-makeroom-bounded-wait; do install -Dm644 \"$L/$p.patch\" \"/usr/local/lib/omarchyvr/$p.patch\"; done && \
install -Dm644 \"$L/nvidia-pimax-patch.hook\" /etc/pacman.d/hooks/65-nvidia-pimax-patch.hook && \
install -Dm644 \"$L/nvidia-pimax-verify.hook\" /etc/pacman.d/hooks/75-nvidia-pimax-verify.hook'"
        )],
        vec![
            "sudo rm /etc/pacman.d/hooks/65-nvidia-pimax-patch.hook /etc/pacman.d/hooks/75-nvidia-pimax-verify.hook".into(),
            "sudo rm -r /usr/local/lib/omarchyvr".into(),
        ],
        false,
    )
}

/// The NVIDIA patch script a root fix runs: the root-owned copy the pacman hooks installed, when it is there (the
/// repository copy is writable by the user), else the repository's.
pub fn reapply_script(facts: &Facts, p: &Paths) -> String {
    if facts.has(std::path::Path::new(crate::facts::ROOT_REAPPLY)) {
        crate::facts::ROOT_REAPPLY.to_string()
    } else if facts.has(std::path::Path::new(crate::facts::ROOT_REAPPLY_LEGACY)) {
        crate::facts::ROOT_REAPPLY_LEGACY.to_string()
    } else {
        quote(&p.repo.join("linux/nvidia-patch-reapply.sh").display().to_string())
    }
}

/// Doctor fix lines with the reapply script replaced by [`reapply_script`].
fn with_root_reapply(lines: &[String], facts: &Facts, p: &Paths) -> Vec<String> {
    let script = reapply_script(facts, p);
    lines
        .iter()
        .map(|l| {
            l.split(' ')
                .map(|w| if w.ends_with("/nvidia-patch-reapply.sh") { script.clone() } else { w.to_string() })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

/// Map the doctor's report into items (the doctor's `setcap` check is replaced by the per-build items).
pub fn from_doctor(facts: &Facts, live: &Live, p: &Paths) -> Vec<Item> {
    let Some(r) = &facts.report else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for c in &r.checks {
        let group = match c.id {
            "headset" | "tobii" | "udev" | "keepalive" => Group::Headset,
            "gpu" | "patches" | "merged-head-sync" | "connector" | "lease" | "kernel-log" => Group::Display,
            "stale-service" => Group::Desk,
            "setcap" => continue,
            _ => Group::Extras,
        };
        let mut level = c.level;
        let mut detail = c.detail.clone();
        let mut row = None;
        let mut fix = match &c.fix {
            Some(f) if f.root => {
                let rf = RootFix::new(f.why.clone(), with_root_reapply(&f.commands, facts, p), f.undo.clone(), f.reboot);
                Fix::Root(match c.id {
                    "connector" => rf.copy_only("the EDID override lines name this PC's PCI address and connector: check them, then run them in a terminal"),
                    _ => rf,
                })
            }
            Some(f) => Fix::Manual {
                why: f.why.clone(),
                lines: f.commands.clone(),
            },
            None => Fix::None,
        };
        // the EDID override in one click: the root job's EDID mode finds the headset's connector itself (it waits for
        // the display up to 30 s), so the lines no longer have to be checked and run by hand
        if c.id == "connector" && c.level == Level::Fail && !c.detail.contains("not connected") {
            let i = crate::install::Install::of_home(&p.home);
            if facts.has(&i.root_script()) && facts.has(&i.plan_env()) {
                let undo = c.fix.as_ref().map(|f| f.undo.clone()).unwrap_or_default();
                fix = Fix::Root(RootFix::new(
                    "Set up the headset's display mode: the job finds the headset's display (waits up to 30 s for it), writes the EDID override and rebuilds the boot image. One password; restart afterwards.",
                    vec![crate::autosetup::edid_fix_line(&i)],
                    undo,
                    true,
                ));
            }
        }
        match c.id {
            "keepalive" => {
                fix = user(
                    "The keepalive keeps the headset display powered over USB.",
                    vec![Cmd::new("systemctl", &["--user", "enable", "--now", "omarchyvr-keepalived"])],
                )
            }
            "tobii" => {
                row = Some(3);
                fix = manual("Power the eye module on: Tools > Eye tracker > Power on (pimax-eye power-on, an approved command).", &[]);
            }
            "merged-head-sync" => row = Some(10),
            "kernel-log" => {
                row = Some(9);
                match facts.kernel_verdict() {
                    Verdict::WayvrOnly(ev) => {
                        level = Level::Warn;
                        detail = format!(
                            "{} WayVR Xid 31 MMU fault(s) this boot: recoverable per-app faults; the Desk page lets you start anyway (--force). A reboot clears the list.",
                            ev.len()
                        );
                        fix = manual("Nothing to fix; reboot when convenient to clear the list.", &[]);
                    }
                    Verdict::Reboot(ev) => {
                        detail = format!(
                            "{}; reboot before the next headset session",
                            ev.iter().map(|e| e.summary()).collect::<Vec<_>>().join("; ")
                        );
                        fix = manual("The display engine may be wedged: reboot (the app never reboots for you).", &["systemctl reboot"]);
                    }
                    Verdict::Clean => {}
                }
            }
            "stale-service" => {
                row = Some(1);
                if let Some(s) = &live.service {
                    level = Level::Info;
                    detail = format!("desk/VR session running (pid {})", s.pid);
                    fix = Fix::None;
                } else if level >= Level::Warn {
                    fix = user(
                        "A running service holds the display lease: stop it politely (SIGINT, SIGKILL only after 30 s).",
                        vec![Cmd::with_args(p.launcher(&|x| facts.has(x)).display().to_string(), crate::launcher::vr_stop_args(true))],
                    );
                }
            }
            _ => {}
        }
        let mut it = Item::new(c.id, group, c.title, level, detail, row, fix);
        it.from_doctor = true;
        out.push(it);
    }
    out
}

/// The headset's parts behind its control chip (34a4:0044): the cameras (34a4:0014, the USB3 side) and the eye tracker
/// (2104:0561). Judged only while the control chip is on USB (asleep, nothing can be told). Real setup 2026-10-17: the
/// setup said "done" while the cameras were gone (USB3 side off) and the eye module was off after a headset restart;
/// Work then failed. Each gets its one step: the cameras a single headset restart (`reset-headset --once`, the
/// captured restart command; if they stay off the headset needs a power cycle: the "Get the headset ready" guide),
/// the eye tracker the captured eye-module enable, once (`pimax-eye power-on`; the VR session sends the same by itself).
pub fn headset_part_items(facts: &Facts, p: &Paths) -> Vec<Item> {
    let has = |v: u16, pid: u16| facts.usb.contains(&(v, pid));
    if facts.at == 0 || !has(0x34a4, 0x0044) {
        return vec![];
    }
    let mut v = Vec::new();
    v.push(if has(0x34a4, 0x0014) {
        Item::new("headset-camera", Group::Headset, "Headset cameras", Level::Ok, "34a4:0014 on USB", Some(26), Fix::None)
    } else {
        let launcher = p.launcher(&|x| facts.has(x)).display().to_string();
        Item::new("headset-camera", Group::Headset, "Headset cameras", Level::Fail,
            "34a4:0014 not on USB while the headset's control chip is (its USB3 side is off)", Some(26),
            user("Restart the headset once with its own restart command (captured from Pimax Play). If the cameras stay off, the headset needs a power cycle: Get the headset ready (the power button first).",
                vec![Cmd::with_args(launcher, vec!["vr".into(), "reset-headset".into(), "--once".into()])]))
    });
    v.push(if has(0x2104, 0x0561) {
        Item::new("eye-chip", Group::Headset, "Eye tracker", Level::Ok, "2104:0561 on USB", Some(11), Fix::None)
    } else {
        Item::new("eye-chip", Group::Headset, "Eye tracker", Level::Warn,
            "2104:0561 not on USB: the eye module is off (normal after the headset restarted; a VR start switches it on)", Some(11),
            user("Switch the eye module on: the captured enable command, sent once (approved), then wait for 2104:0561.",
                vec![Cmd::with_args("flock", vec!["-o".into(), "-w".into(), "10".into(), p.headset_lock.display().to_string(),
                    p.bin("pimax-eye").display().to_string(), "power-on".into()])]))
    });
    v
}

/// The whole checklist, grouped and in a stable order.
pub fn build(facts: &Facts, live: &Live, p: &Paths) -> Vec<Item> {
    let mut v = from_doctor(facts, live, p);

    // Headset and access
    let rules = quote(&p.repo.join("linux/70-pimax.rules").display().to_string());
    let udev_lines = vec![
        format!("sudo install -m644 {rules} /etc/udev/rules.d/70-pimax.rules"),
        "sudo udevadm control --reload".to_string(),
        "sudo udevadm trigger --subsystem-match=hidraw --subsystem-match=usb --action=change".to_string(),
    ];
    let udev_undo = vec!["sudo rm /etc/udev/rules.d/70-pimax.rules".to_string(), "sudo udevadm control --reload".to_string()];
    let why = "uaccess for 34a4:* and the eye chip 2104:0561, so the tools need no root.";
    v.push(match facts.udev_rule {
        Some(true) => Item::new("udev-rule", Group::Headset, "udev rule file", Level::Ok, "/etc/udev/rules.d/70-pimax.rules matches the repository", None, Fix::None),
        Some(false) => Item::new("udev-rule", Group::Headset, "udev rule file", Level::Warn, "/etc/udev/rules.d/70-pimax.rules differs from linux/70-pimax.rules", None,
            Fix::Root(RootFix::new(why, udev_lines, udev_undo, false))),
        None => Item::new("udev-rule", Group::Headset, "udev rule file", Level::Fail, "/etc/udev/rules.d/70-pimax.rules is not installed", None,
            Fix::Root(RootFix::new(why, udev_lines, udev_undo, false))),
    });
    v.extend(headset_part_items(facts, p));
    v.push(if facts.lock_holders.is_empty() {
        Item::new("headset-lock", Group::Headset, "Headset lock", Level::Ok, format!("{} is free", p.headset_lock.display()), None, Fix::None)
    } else {
        let who: Vec<String> = facts.lock_holders.iter().map(|h| h.describe()).collect();
        Item::new("headset-lock", Group::Headset, "Headset lock", Level::Info, format!("held: {}", who.join("; ")), None,
            manual("Device tools started from the app wait for this lock; whoever holds it is using the headset.", &[]))
    });

    // GPU and display
    let nvidia = facts.doctor_level("gpu").is_some_and(|l| l != Level::Info);
    if nvidia {
        v.push(if facts.hooks[0] && facts.hooks[1] {
            Item::new("pacman-hooks", Group::Display, "pacman hooks", Level::Ok, "65-nvidia-pimax-patch.hook and 75-nvidia-pimax-verify.hook installed", Some(10), Fix::None)
        } else {
            Item::new("pacman-hooks", Group::Display, "pacman hooks", Level::Warn, "missing: the next nvidia-open-dkms upgrade would drop the 3 patches", Some(10), Fix::Root(hooks_fix(p)))
        });
        let reapply = reapply_script(facts, p);
        v.push(match facts.modinfo_mhs {
            Some(true) => Item::new("modinfo", Group::Display, "Installed nvidia-modeset module", Level::Ok, "modinfo lists merged_head_sync: the next boot loads the patched module", Some(10), Fix::None),
            Some(false) => Item::new("modinfo", Group::Display, "Installed nvidia-modeset module", Level::Warn, "modinfo does not list merged_head_sync: the installed module is not patched", Some(10),
                Fix::Root(RootFix::new("Re-apply the 3 patches, rebuild the module with DKMS and the boot image; then reboot.", vec![format!("sudo {reapply} --rebuild")], vec![], true))),
            None => Item::new("modinfo", Group::Display, "Installed nvidia-modeset module", Level::Info, "modinfo not available", Some(10), Fix::None),
        });
    }

    // Monado builds
    for b in &facts.builds {
        let k = b.build.key;
        let main = k == "desk";
        if b.built() {
            v.push(Item::new(&format!("build-{k}"), Group::Builds, b.build.label, Level::Ok,
                format!("{} (branch {})", b.build.dir.display(), b.branch.as_deref().unwrap_or("?")), Some(16), Fix::None));
            let bad = if main { Level::Fail } else { Level::Warn };
            v.push(match b.sys_nice {
                Some(true) => Item::new(&format!("setcap-{k}"), Group::Builds, &format!("cap_sys_nice ({k})"), Level::Ok, "getcap: cap_sys_nice=ep", Some(7), Fix::None),
                Some(false) => Item::new(&format!("setcap-{k}"), Group::Builds, &format!("cap_sys_nice ({k})"), bad,
                    format!("{} has no cap_sys_nice (dropped by the last relink)", b.service.display()), Some(7), Fix::Root(setcap_fix(&b.service))),
                None => Item::new(&format!("setcap-{k}"), Group::Builds, &format!("cap_sys_nice ({k})"), Level::Info, "getcap not available", Some(7), Fix::None),
            });
            // the one build carries every feature: one line per feature marker (ok / missing)
            if let (true, Some(m)) = (main, b.markers) {
                for (id, name, marker, have) in [
                    ("slam", "6DoF head tracking", "PIMAX_SLAM", m.slam),
                    ("imu-gate", "6DoF IMU gate (safe auto exposure)", "held from the tracker", m.imu_gate),
                    ("eye", "Eye tracking", "PIMAX_EYE", m.eye),
                    ("ctrl-track", "Optical controller tracking", "PIMAX_CTRL_TRACK", m.ctrl_track),
                    ("anchor", "Room anchor", "PIMAX_ANCHOR", m.anchor),
                    ("ctrl-inhand", "Controllers when picked up", "PIMAX_CTRL_INHAND", m.ctrl_inhand),
                    ("ctrl-angle", "Controller angle", "PIMAX_CTRL_ANGLE", m.ctrl_angle),
                    ("anchor-glide", "Smooth room corrections", "PIMAX_ANCHOR_GLIDE", m.anchor_glide),
                    ("hands", "Camera hands", "PIMAX_HT_HZ", m.hands),
                ] {
                    let level = if have { Level::Ok } else if id == "hands" { Level::Warn } else { Level::Fail };
                    let detail = if have {
                        format!("{marker} found in monado-service")
                    } else {
                        format!("{marker} missing: this build cannot do it; rebuild the one full build (Setup > Install)")
                    };
                    v.push(Item::new(&format!("feature-{id}"), Group::Builds, &format!("Feature: {name}"), level, detail, Some(16), Fix::None));
                }
            }
        } else if !main && b.cmake.is_none() {
            // an optional developer build (~/src/monado-slam, ~/src/monado) that is not there: nothing to show on a
            // clean install (it is not part of the install, so it must not look missing)
            continue;
        } else {
            let fix = if b.cmake.is_some() {
                Fix::User(rebuild_fix(&b.build, false))
            } else {
                manual("Configure and build Monado in this directory (see docs/INVENTORY.md 3.1).", &[])
            };
            v.push(Item::new(&format!("build-{k}"), Group::Builds, b.build.label, if main { Level::Fail } else { Level::Info },
                format!("{} is not built", b.service.display()), Some(16), fix));
        }
        if k != "pimax" && b.cmake.is_some() {
            let level = match b.pimax_cam() {
                Some(true) => Level::Ok,
                _ if main => Level::Fail,
                _ => Level::Warn,
            };
            let detail = match b.pimax_cam() {
                Some(true) => "PIMAX_CAM:BOOL=ON in CMakeCache.txt".to_string(),
                Some(false) => "PIMAX_CAM:BOOL=OFF: 6DoF would silently run 3DoF".to_string(),
                None => "PIMAX_CAM is not in CMakeCache.txt".to_string(),
            };
            let fix = if level == Level::Ok { Fix::None } else { Fix::User(rebuild_fix(&b.build, true)) };
            v.push(Item::new(&format!("pimax-cam-{k}"), Group::Builds, &format!("Camera / 6DoF code ({k})"), level, detail, Some(6), fix));
        }
        if main {
            if let Some(m) = b.markers {
                v.push(if m.imu_gate {
                    Item::new("ae-safe", Group::Builds, "Auto exposure safe (IMU gate)", Level::Ok, "the desk build has the Basalt IMU gate (slam d3138d4)", Some(12), Fix::None)
                } else {
                    Item::new("ae-safe", Group::Builds, "Auto exposure safe (IMU gate)", Level::Warn,
                        "no IMU gate in this binary: an exposure push can freeze 6DoF", Some(12),
                        manual("Rebuild from branch desk (it contains Monado slam d3138d4).", &[]))
                });
            }
        }
    }

    // Desk mode parts
    v.push(if facts.has(&p.wayvr_bin) {
        Item::new("wayvr", Group::Desk, "WayVR (eye-gaze branch)", Level::Ok, p.wayvr_bin.display().to_string(), None, Fix::None)
    } else {
        Item::new("wayvr", Group::Desk, "WayVR (eye-gaze branch)", Level::Fail, format!("{} is missing from this install", p.wayvr_bin.display()), None,
            manual("Run the installer (./install.sh builds the WayVR fork into ~/.local/lib/omarchyvr/wayvr), or set wayvr_bin in paths.toml.", &[]))
    });
    v.push(if facts.has(&p.basalt_lib) {
        Item::new("basalt", Group::Desk, "Basalt (6DoF back end)", Level::Ok, p.basalt_lib.display().to_string(), None, Fix::None)
    } else {
        Item::new("basalt", Group::Desk, "Basalt (6DoF back end)", Level::Warn, format!("{} missing: 6DoF cannot start", p.basalt_lib.display()), None,
            manual("Run the installer (./install.sh builds Basalt into ~/.local/lib/omarchyvr/basalt), or set basalt_lib in paths.toml.", &[]))
    });
    let missing: Vec<&(&str, &str)> = RELEASE_BINS.iter().filter(|(b, _)| !facts.has(&p.bin(b)) && !facts.has(&p.installed_bin(b))).collect();
    v.push(if missing.is_empty() {
        Item::new("rust-bins", Group::Desk, "Rust tools (release build)", Level::Ok, format!("{} tools in {}", RELEASE_BINS.len(), p.release.display()), None, Fix::None)
    } else {
        let mut args: Vec<String> = vec!["build".into(), "--release".into()];
        let mut pkgs: Vec<&str> = missing.iter().map(|(_, pkg)| *pkg).collect();
        pkgs.dedup();
        for pk in &pkgs {
            args.push("-p".into());
            args.push(pk.to_string());
        }
        Item::new("rust-bins", Group::Desk, "Rust tools (release build)", Level::Warn,
            format!("missing: {}", missing.iter().map(|(b, _)| *b).collect::<Vec<_>>().join(", ")), None,
            if repo_build_possible(facts, p) { user("Build the missing tools in the workspace.", vec![Cmd::with_args("cargo", args).cwd(&p.rust)]) } else { manual("Some tools are missing from this install: press Fix on the Home page (it repairs the install).", &[]) })
    });

    // Launcher and app
    // a build from the repo is only possible when this install carries the sources (a clean install may not)
    let can_build = repo_build_possible(facts, p);
    let lf = if can_build { Fix::User(launcher_fix(p)) } else { manual("The launcher is missing from this install: press Fix on the Home page (it repairs the install).", &[]) };
    v.push(match facts.launcher {
        Some(InstallState::UpToDate) => Item::new("launcher", Group::App, "omarchyvr launcher", Level::Ok, "~/.local/bin/omarchyvr matches the build", Some(8), Fix::None),
        Some(InstallState::NotInstalled) => Item::new("launcher", Group::App, "omarchyvr launcher", Level::Warn, "not installed in ~/.local/bin (the app uses the workspace build)", Some(8), lf),
        Some(InstallState::Differs) => Item::new("launcher", Group::App, "omarchyvr launcher", Level::Warn, "~/.local/bin/omarchyvr differs from the workspace build", Some(8), lf),
        Some(InstallState::BuildStale) => Item::new("launcher", Group::App, "omarchyvr launcher", Level::Warn, "the sources changed after the last build", Some(8), lf),
        Some(InstallState::InstalledOnly) => Item::new("launcher", Group::App, "omarchyvr launcher", Level::Ok, "installed in ~/.local/bin", Some(8), Fix::None),
        Some(InstallState::Missing) | None => Item::new("launcher", Group::App, "omarchyvr launcher", Level::Fail, "not built and not installed", Some(8), lf),
    });
    let app_ok = matches!(facts.app, Some(InstallState::UpToDate) | Some(InstallState::InstalledOnly)) && facts.desktop_entry;
    v.push(Item::new("app", Group::App, "This app (desktop entry 'omarchyvr')", if app_ok { Level::Ok } else { Level::Info },
        if app_ok { "installed in ~/.local/bin with its desktop entry".to_string() } else { format!("desktop entry {}, binary {:?}", if facts.desktop_entry { "present" } else { "missing" }, facts.app.unwrap_or(InstallState::Missing)) },
        None, if app_ok { Fix::None } else if can_build { Fix::User(app_fix(p)) } else { manual("The app's desktop entry or binary is missing from this install: press Fix on the Home page.", &[]) }));

    // Extras
    v.push(if facts.has(&p.voice_model) && facts.has(&p.sherpa_lib) {
        Item::new("voice", Group::Extras, "Voice models", Level::Ok,
            format!("Parakeet v3 + sherpa-onnx{}", if facts.has(&p.vad_model) { " + Silero VAD" } else { "; no Silero VAD (energy VAD is used)" }), Some(19), Fix::None)
    } else {
        Item::new("voice", Group::Extras, "Voice models", Level::Warn,
            format!("missing: {}{}", if facts.has(&p.voice_model) { "" } else { "Parakeet v3 model " }, if facts.has(&p.sherpa_lib) { "" } else { "sherpa-onnx C library" }), Some(19),
            manual("Put parakeet-tdt-0.6b-v3-int8 into ~/models and install sherpa-onnx into ~/.local/lib/omarchyvr/sherpa-onnx (or set sherpa_lib in paths.toml).", &[]))
    });
    v.push(if !facts.openvr.exists {
        Item::new("steam-runtime", Group::Extras, "Steam runtime (OpenVR)", Level::Info, "no openvrpaths.vrpath (Tools > Steam runtime)", Some(20), Fix::None)
    } else if !facts.openvr.xrizer {
        Item::new("steam-runtime", Group::Extras, "Steam runtime (OpenVR)", Level::Info, "OpenVR runtime is SteamVR (Tools > Steam runtime switches to xrizer)", Some(20), Fix::None)
    } else if facts.openvr.read_only {
        Item::new("steam-runtime", Group::Extras, "Steam runtime (OpenVR)", Level::Ok, "xrizer, file read-only (SteamVR cannot rewrite it)", Some(20), Fix::None)
    } else {
        Item::new("steam-runtime", Group::Extras, "Steam runtime (OpenVR)", Level::Warn, "xrizer, but writable: a SteamVR install rewrites it", Some(20),
            user("Make openvrpaths.vrpath read-only (chmod 444).", vec![Cmd::new("chmod", &["444", &p.openvr_paths.display().to_string()])]))
    });
    v.push(match facts.ipd_mm {
        Some(mm) => Item::new("ipd", Group::Extras, "IPD", Level::Ok, format!("{mm:.1} mm in ipd.env, passed to Monado as OXR_DEBUG_IPD_MM"), Some(18), Fix::None),
        None => Item::new("ipd", Group::Extras, "IPD", Level::Info, "no ipd.env yet (Tools > IPD)", Some(18), Fix::None),
    });
    v.push(if facts.eye_calibration {
        Item::new("eye-calibration", Group::Extras, "Eye calibration", Level::Ok, p.eye_calibration.display().to_string(), Some(18), Fix::None)
    } else {
        Item::new("eye-calibration", Group::Extras, "Eye calibration", Level::Info, "none saved yet (Tools > Eye calibration)", Some(18), Fix::None)
    });
    if let Some(mounted) = facts.shared_drive {
        use omarchyvr_tools::mount_shared::{mount_command, SHARED_TARGET};
        let uuid = facts.shared_drive_uuid.clone().unwrap_or_default();
        v.push(if mounted {
            Item::new("shared-drive", Group::Extras, "Shared NTFS drive", Level::Ok, format!("UUID {uuid} mounted on {SHARED_TARGET}"), Some(21), Fix::None)
        } else {
            Item::new("shared-drive", Group::Extras, "Shared NTFS drive", Level::Warn, format!("UUID {uuid} is not mounted (always by UUID: NVMe names swap between boots)"), Some(21),
                Fix::Root(RootFix::new("Mount the shared drive by UUID with the in-kernel ntfs3 driver, files owned by you.", vec![mount_command(&uuid, facts.ids.0, facts.ids.1)], vec![format!("sudo umount {SHARED_TARGET}")], false)))
        });
    }
    if let Some(free) = facts.free_home {
        let low = free < (10u64 << 30);
        v.push(Item::new("disk", Group::Extras, "Free disk space", if low { Level::Warn } else { Level::Ok },
            format!("{} free in {} (a 45 s camera recording is about 2.2 GB)", gib(free), p.home.display()), Some(15), Fix::None));
    }
    v.extend(crate::manual::items(facts, p));
    v.extend(crate::previous::items(facts, p));
    v.sort_by_key(|i| i.group);
    v
}

/// Counts for the header (`ok`, `warn`, `fail`).
pub fn counts(items: &[Item]) -> (usize, usize, usize) {
    let ok = items.iter().filter(|i| i.level <= Level::Ok).count();
    let warn = items.iter().filter(|i| i.level == Level::Warn).count();
    let fail = items.iter().filter(|i| i.level == Level::Fail).count();
    (ok, warn, fail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builds::MarkerCache;
    use crate::demo;
    use crate::facts::{collect_facts, collect_live};
    use crate::monlog::LogFollower;

    fn items_of(m: &crate::machine::FakeMachine, p: &Paths) -> Vec<Item> {
        let facts = collect_facts(m, p, &demo::opts(p), &MarkerCache::default(), None, true, true);
        let mut f = LogFollower::new(&p.session_dir.join("monado.log"));
        let live = collect_live(m, p, &mut f);
        build(&facts, &live, p)
    }

    fn item<'a>(v: &'a [Item], id: &str) -> &'a Item {
        v.iter().find(|i| i.id == id).unwrap_or_else(|| panic!("no item {id}: {:?}", v.iter().map(|i| &i.id).collect::<Vec<_>>()))
    }

    /// LESSONS row 60: a clean install has no repo build, no rust/ workspace and no developer Monado builds.
    #[test]
    fn a_clean_install_never_offers_a_build_or_shows_developer_builds_as_missing() {
        let (mut m, p) = demo::idle_machine();
        m.host.files.retain(|k, _| !k.to_string_lossy().starts_with("/home/u/src/monado"));
        m.metas.retain(|k, _| !k.to_string_lossy().starts_with("/home/u/src/monado")); // no developer builds on a clean install
        let v = items_of(&m, &p);
        let facts = collect_facts(&m, &p, &demo::opts(&p), &MarkerCache::default(), None, true, true);
        assert!(!repo_build_possible(&facts, &p), "the demo repo has no rust/Cargo.toml");
        for i in &v {
            if let Fix::User(u) = &i.fix {
                assert!(!u.steps.iter().any(|c| c.display().contains("cargo")), "{} offers a cargo build on a clean install", i.id);
            }
        }
        assert!(v.iter().all(|i| i.id != "build-slam" && i.id != "build-pimax"), "optional developer builds are not listed when absent");
        assert!(v.iter().all(|i| !i.detail.contains("no source in")));
    }

    #[test]
    fn the_hooks_fix_uses_the_repo_dir_not_the_old_lab() {
        let (_, p) = demo::idle_machine();
        let r = hooks_fix(&p);
        assert!(r.lines[0].contains(&p.repo.join("linux").display().to_string()));
        assert!(!r.lines[0].contains("pimax-re"));
        assert_eq!(crate::safety::check(&crate::cmd::pkexec_cmd(&r.lines)), Ok(()));
    }

    /// 2026-10-17: the setup said "done" with the cameras gone and the eye module off. Both are items with one step.
    #[test]
    fn the_headset_parts_behind_the_control_chip() {
        let (_, p) = demo::idle_machine();
        let mut f = Facts { at: 1, ..Facts::default() };
        // asleep (no control chip): nothing can be told about the parts
        f.usb = vec![(0x34a4, 0x0043), (0x34a4, 0x0014)];
        assert!(headset_part_items(&f, &p).is_empty());
        // awake, everything there
        f.usb = vec![(0x34a4, 0x0043), (0x34a4, 0x0044), (0x34a4, 0x0014), (0x2104, 0x0561)];
        let v = headset_part_items(&f, &p);
        assert_eq!((item(&v, "headset-camera").level, item(&v, "eye-chip").level), (Level::Ok, Level::Ok));
        // awake, the cameras and the eye module off (real state after the 13:07 restart)
        f.usb = vec![(0x34a4, 0x0043), (0x34a4, 0x0044)];
        let v = headset_part_items(&f, &p);
        let cam = item(&v, "headset-camera");
        assert_eq!(cam.level, Level::Fail);
        match &cam.fix {
            Fix::User(u) => {
                assert_eq!(u.steps.len(), 1);
                assert_eq!(u.steps[0].args, ["vr", "reset-headset", "--once"], "ONE restart, never the retry chain");
                assert!(u.why.contains("Get the headset ready"), "{}", u.why);
            }
            x => panic!("{x:?}"),
        }
        let eye = item(&v, "eye-chip");
        assert_eq!(eye.level, Level::Warn);
        match &eye.fix {
            Fix::User(u) => assert_eq!(u.steps[0].args.last().map(String::as_str), Some("power-on")),
            x => panic!("{x:?}"),
        }
        // not collected yet: nothing
        assert!(headset_part_items(&Facts::default(), &p).is_empty());
    }

    #[test]
    fn doctor_checks_are_mapped_with_groups_and_rows() {
        let (m, p) = demo::idle_machine();
        let v = items_of(&m, &p);
        for id in ["headset", "tobii", "udev", "keepalive", "gpu", "patches", "merged-head-sync", "connector", "lease", "kernel-log", "stale-service"] {
            assert!(item(&v, id).from_doctor, "{id}");
        }
        assert!(v.iter().all(|i| i.id != "setcap"), "replaced by setcap-<build>");
        assert_eq!(item(&v, "headset").group, Group::Headset);
        assert_eq!(item(&v, "merged-head-sync").row, Some(10));
        // LESSONS row 9: the doctor fails on any Xid; WayVR Xid 31 only is a warning here
        let k = item(&v, "kernel-log");
        assert_eq!(k.level, Level::Warn);
        assert!(k.detail.contains("2 WayVR Xid 31"));
        assert_eq!(k.row, Some(9));
        // sorted by group
        let groups: Vec<Group> = v.iter().map(|i| i.group).collect();
        let mut sorted = groups.clone();
        sorted.sort();
        assert_eq!(groups, sorted);
    }

    #[test]
    fn lessons_checks_on_the_demo_pc() {
        let (m, p) = demo::idle_machine();
        let v = items_of(&m, &p);
        assert_eq!(item(&v, "setcap-desk").level, Level::Ok);
        let slam = item(&v, "setcap-slam");
        assert_eq!(slam.level, Level::Warn);
        assert_eq!(slam.row, Some(7));
        match &slam.fix {
            Fix::Root(r) => {
                assert_eq!(r.lines, vec!["sudo setcap cap_sys_nice+ep /home/u/src/monado-slam/build/src/xrt/targets/service/monado-service".to_string()]);
                assert!(r.pkexec.is_ok());
                assert!(r.command().unwrap().display().starts_with("/usr/bin/pkexec /bin/sh -c"));
            }
            f => panic!("{f:?}"),
        }
        assert_eq!(item(&v, "pimax-cam-desk").level, Level::Ok);
        assert!(v.iter().all(|i| i.id != "pimax-cam-pimax"), "the 3DoF build is meant to be OFF");
        assert_eq!(item(&v, "ae-safe").level, Level::Ok);
        assert_eq!(item(&v, "pacman-hooks").level, Level::Ok);
        assert_eq!(item(&v, "modinfo").level, Level::Ok);
        assert_eq!(item(&v, "udev-rule").level, Level::Ok);
        assert_eq!(item(&v, "launcher").level, Level::Ok);
        assert_eq!(item(&v, "wayvr").level, Level::Ok);
        assert_eq!(item(&v, "rust-bins").level, Level::Ok);
        assert_eq!(item(&v, "steam-runtime").level, Level::Ok);
        assert_eq!(item(&v, "ipd").level, Level::Ok);
        assert_eq!(item(&v, "eye-calibration").level, Level::Info);
        let (ok, warn, fail) = counts(&v);
        assert!(ok > 15 && fail == 0, "{ok} {warn} {fail}");
    }

    #[test]
    fn broken_pieces_get_the_right_fix_kind() {
        let (mut m, p) = demo::idle_machine();
        m.file("/home/u/omarchyvr/rust/Cargo.toml", "[workspace]\n"); // a repo checkout: the build fixes are offered
        m.file(
            "/home/u/.local/lib/omarchyvr/monado/CMakeCache.txt",
            include_str!("../tests/fixtures/CMakeCache-desk.txt").replace("PIMAX_CAM:BOOL=ON", "PIMAX_CAM:BOOL=OFF"),
        );
        m.host.files.remove(std::path::Path::new("/etc/pacman.d/hooks/65-nvidia-pimax-patch.hook"));
        m.host.files.remove(std::path::Path::new("/etc/udev/rules.d/70-pimax.rules"));
        m.host.files.remove(std::path::Path::new("/home/u/omarchyvr/rust/target/release/pimax-cam"));
        m.host.files.remove(std::path::Path::new("/home/u/.local/bin/pimax-cam"));
        m.file("/home/u/.local/bin/omarchyvr", "older");
        m.cmd("systemctl --user is-active omarchyvr-keepalived", 3, "inactive\n");
        m.set_meta("/home/u/.config/openvr/openvrpaths.vrpath", crate::machine::FileMeta { len: 52, mode: 0o100644, ..Default::default() });
        let v = items_of(&m, &p);
        // row 6: user fix = reconfigure + ninja, then the setcap prompt; only while nothing runs
        match &item(&v, "pimax-cam-desk").fix {
            Fix::User(u) => {
                assert_eq!(u.steps[0].display(), "cd /home/u/.local/lib/omarchyvr/monado && cmake -DPIMAX_CAM=ON .");
                assert_eq!(u.steps[1].display(), "cd /home/u/.local/lib/omarchyvr/monado && ninja");
                assert_eq!(u.follow_up, Some(FollowUp::Setcap(p.desk_build().service())));
                assert!(u.needs_idle);
            }
            f => panic!("{f:?}"),
        }
        assert_eq!(item(&v, "pimax-cam-desk").level, Level::Fail);
        // row 10: hooks = root, runnable with pkexec, absolute paths
        match &item(&v, "pacman-hooks").fix {
            Fix::Root(r) => {
                assert!(r.pkexec.is_ok(), "{:?}", r.pkexec);
                assert!(r.lines[0].contains("L=/home/u/omarchyvr/linux"));
            }
            f => panic!("{f:?}"),
        }
        assert!(matches!(item(&v, "udev-rule").fix, Fix::Root(_)));
        assert_eq!(item(&v, "udev-rule").level, Level::Fail);
        match &item(&v, "rust-bins").fix {
            Fix::User(u) => assert_eq!(u.steps[0].display(), "cd /home/u/omarchyvr/rust && cargo build --release -p pimax-cam"),
            f => panic!("{f:?}"),
        }
        // row 8: launcher install only while idle, atomic rename
        assert_eq!(item(&v, "launcher").level, Level::Warn);
        match &item(&v, "launcher").fix {
            Fix::User(u) => {
                assert!(u.needs_idle);
                assert!(u.steps[1].display().contains("mv -f"));
            }
            f => panic!("{f:?}"),
        }
        match &item(&v, "keepalive").fix {
            Fix::User(u) => assert_eq!(u.steps[0].display(), "systemctl --user enable --now omarchyvr-keepalived"),
            f => panic!("{f:?}"),
        }
        match &item(&v, "steam-runtime").fix {
            Fix::User(u) => assert_eq!(u.steps[0].display(), "chmod 444 /home/u/.config/openvr/openvrpaths.vrpath"),
            f => panic!("{f:?}"),
        }
    }

    #[test]
    fn copy_only_root_fixes_and_reboot() {
        let (mut m, p) = demo::idle_machine();
        m.cmd(
            "journalctl -k -b --no-pager -q",
            0,
            "Oct 13 23:16:30 mau kernel: NVRM: Xid (PCI:0000:01:00): 31, pid=128586, name=DoE.exe, channel 0x1a\n",
        );
        m.file("/sys/class/drm/card1-DP-1/modes", "3840x2160\n");
        let v = items_of(&m, &p);
        let k = item(&v, "kernel-log");
        assert_eq!(k.level, Level::Fail);
        match &k.fix {
            Fix::Manual { lines, .. } => assert_eq!(lines, &vec!["systemctl reboot".to_string()]),
            f => panic!("{f:?}"),
        }
        match &item(&v, "connector").fix {
            Fix::Root(r) => assert!(r.pkexec.is_err(), "EDID override lines must be reviewed: copy only"),
            f => panic!("{f:?}"),
        }
    }

    #[test]
    fn shared_drive_by_uuid_only_and_only_where_it_exists() {
        let (m, p) = demo::idle_machine();
        assert!(items_of(&m, &p).iter().all(|i| i.id != "shared-drive"), "no such disk on the demo PC");
        let (mut m, p) = demo::idle_machine();
        m.link("/dev/disk/by-uuid/0123456789ABCDEF", "../../nvme1n1p2");
        assert!(items_of(&m, &p).iter().all(|i| i.id != "shared-drive"), "no UUID set: no check (no default drive)");
        m.file(&p.home.join(omarchyvr_config::paths::PATHS_FILE).display().to_string(), "shared_drive_uuid = \"0123456789ABCDEF\"\n");
        m.file("/proc/mounts", "/dev/nvme0n1p2 / btrfs rw 0 0\n");
        let v = items_of(&m, &p);
        let s = item(&v, "shared-drive");
        assert_eq!(s.row, Some(21));
        match &s.fix {
            Fix::Root(r) => {
                assert!(r.lines[0].contains("mount -t ntfs3") && r.lines[0].contains("UUID=0123456789ABCDEF"));
                assert!(r.pkexec.is_ok(), "{:?}", r.pkexec);
            }
            f => panic!("{f:?}"),
        }
        m.file("/proc/mounts", "/dev/nvme1n1p2 /mnt/shared ntfs3 rw 0 0\n");
        assert_eq!(item(&items_of(&m, &p), "shared-drive").level, Level::Ok);
    }

    #[test]
    fn root_fixes_prefer_the_root_owned_reapply_script_and_quote_paths() {
        let (mut m, p) = demo::idle_machine();
        m.file("/sys/module/nvidia_modeset/parameters/merged_head_sync", "0\n");
        let v = items_of(&m, &p);
        match &item(&v, "merged-head-sync").fix {
            Fix::Root(r) => assert!(r.lines[0].contains("merged_head_sync=1"), "{:?}", r.lines),
            f => panic!("{f:?}"),
        }
        m.cmd("modinfo -F parm nvidia-modeset", 0, "config_file:path (charp)\n");
        m.file("/usr/local/lib/omarchyvr/nvidia-patch-reapply.sh", "#!/bin/bash\n");
        let v = items_of(&m, &p);
        match &item(&v, "modinfo").fix {
            Fix::Root(r) => assert_eq!(r.lines, vec!["sudo /usr/local/lib/omarchyvr/nvidia-patch-reapply.sh --rebuild".to_string()]),
            f => panic!("{f:?}"),
        }
        // a machine installed before the rename still has the old root directory
        m.host.files.remove(std::path::Path::new("/usr/local/lib/omarchyvr/nvidia-patch-reapply.sh"));
        m.file("/usr/local/lib/pimax/nvidia-patch-reapply.sh", "#!/bin/bash\n");  // names:keep
        let v = items_of(&m, &p);
        match &item(&v, "modinfo").fix {
            Fix::Root(r) => assert_eq!(r.lines, vec!["sudo /usr/local/lib/pimax/nvidia-patch-reapply.sh --rebuild".to_string()]),  // names:keep
            f => panic!("{f:?}"),
        }
        let spaced = Paths::new(std::path::Path::new("/home/a b"), std::path::Path::new("/home/a b/omarchyvr"), std::path::Path::new("/tmp/s"), std::path::Path::new("/run/user/1"));
        let b = &spaced.builds[0];
        let fix = setcap_fix(&b.service());
        assert_eq!(fix.lines[0], "sudo setcap cap_sys_nice+ep '/home/a b/.local/lib/omarchyvr/monado/src/xrt/targets/service/monado-service'");
        assert!(fix.pkexec.is_ok(), "{:?}", fix.pkexec);
        let h = hooks_fix(&spaced);
        assert!(h.lines[0].contains("L='/home/a b/omarchyvr/linux'"), "{}", h.lines[0]);
    }

    #[test]
    fn running_session_is_not_a_stale_service() {
        let (m, p) = demo::running_machine();
        let v = items_of(&m, &p);
        let s = item(&v, "stale-service");
        assert_eq!(s.level, Level::Info);
        assert!(s.detail.contains("372460"));
        let (mut m, p) = demo::idle_machine();
        m.process(555, "monado-service", "/x/monado-service", &[], 1, &[]);
        let v = items_of(&m, &p);
        match &item(&v, "stale-service").fix {
            Fix::User(u) => assert_eq!(u.steps[0].display(), "/home/u/.local/bin/omarchyvr vr stop --grace 30 --all"),
            f => panic!("{f:?}"),
        }
    }

    #[test]
    fn every_fix_command_passes_the_safety_rules() {
        let (m, p) = demo::idle_machine();
        let mut all = items_of(&m, &p);
        let (mut m2, _) = demo::idle_machine();
        m2.host.files.clear();
        all.extend(items_of(&m2, &p));
        for i in &all {
            match &i.fix {
                Fix::User(u) => {
                    for c in &u.steps {
                        assert_eq!(crate::safety::check(c), Ok(()), "{}: {}", i.id, c.display());
                    }
                }
                Fix::Root(r) => {
                    if let Some(c) = r.command() {
                        assert_eq!(crate::safety::check(&c), Ok(()), "{}", i.id);
                    }
                }
                _ => {}
            }
        }
        for b in &p.builds {
            for c in rebuild_fix(b, true).steps {
                assert_eq!(crate::safety::check(&c), Ok(()));
            }
        }
        for c in app_fix(&p).steps.iter().chain(launcher_fix(&p).steps.iter()) {
            assert_eq!(crate::safety::check(c), Ok(()), "{}", c.display());
        }
    }
}
