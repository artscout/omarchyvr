//! Manual steps of docs/MANUAL-STEPS.md the app now checks or runs (its "GUI coverage gaps" table): packages, the
//! keepalive unit with its Rust drop-in, the auto mode unit, the installed EDID and Hyprland's DP-1 line, the eye
//! calibration refit and the controller gyro fit.
//!
//! Root steps stay Copy or pkexec (package installs are copy only); Hyprland's config and Steam's settings are only
//! read and shown, never written (safety.rs). Everything here is read in the background with the other facts.
use crate::app::{AppCore, Confirm, Pending};
use crate::checklist::{install_cmd, Fix, Group, Item, RootFix, UserFix};
use crate::cmd::{quote, Cmd};
use crate::facts::Facts;
use crate::job::{JobKind, JobSpec, Step};
use crate::machine::Machine;
use crate::paths::Paths;
use pimax_doctor::Level;
use std::path::{Path, PathBuf};

/// The EDID installed for the NVIDIA driver (linux/DISPLAY.md).
pub const EDID_INSTALLED: &str = "/etc/pimax/dp-edid.bin";
/// The non-desktop EDID (Hyprland leases the panel by itself).
pub const EDID_NON_DESKTOP: &str = "linux/pimax-dp-edid-msvsdb.bin";
/// The native-mode EDID (a desktop monitor until Hyprland's config keeps it off).
pub const EDID_NATIVE: &str = "linux/pimax-dp-edid-native.bin";

/// Hyprland config files looked at for a DP-1 monitor line, in `~/.config/hypr`.
const HYPR_FILES: [&str; 4] = ["monitors.lua", "monitors.conf", "hyprland.lua", "hyprland.conf"];

/// What the manual-step checks found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManualFacts {
    /// `cargo` is installed (pacman or rustup).
    pub cargo: bool,
    /// Steam is installed.
    pub steam: bool,
    /// Proton versions in the Steam library.
    pub proton: Vec<String>,
    /// The keepalive unit file (user or packaged).
    pub keepalive_unit: Option<PathBuf>,
    /// Its ExecStart is the Rust `pimax keepalived` (a drop-in or the unit itself).
    pub keepalive_rust: bool,
    /// The auto mode daemon is built where its unit runs it (`<release>/pimax-autod`) or installed in /usr/bin.
    pub autod_build: bool,
    /// The auto mode unit file (user or packaged).
    pub autod_unit: Option<PathBuf>,
    /// `~/.config/pimax/auto.toml`.
    pub auto_config: bool,
    /// The installed EDID: "non-desktop", "native", "other" (None: none installed).
    pub edid: Option<&'static str>,
    /// The Hyprland config file and whether it has a DP-1 monitor line (None: no config found).
    pub hypr_dp1: Option<(PathBuf, bool)>,
    /// Eye calibration runs, newest first (at most 5).
    pub eyecal_runs: Vec<PathBuf>,
    /// The controller IMU logs of the last session.
    pub ctrl_imu_logs: Vec<PathBuf>,
}

/// A DP-1 monitor line that is not a comment.
fn has_dp1(text: &str) -> bool {
    text.lines().map(str::trim).any(|l| !l.starts_with('#') && !l.starts_with("--") && l.contains("DP-1") && l.contains("monitor"))
}

/// Collect (background only).
pub fn collect(m: &dyn Machine, p: &Paths) -> ManualFacts {
    let home = &p.home;
    let has = |x: &Path| m.exists(x);
    let mut f = ManualFacts {
        cargo: has(Path::new("/usr/bin/cargo")) || has(&home.join(".cargo/bin/cargo")),
        steam: has(Path::new("/usr/bin/steam")) || has(&home.join(".local/share/Steam/steam.sh")),
        ..Default::default()
    };
    let steam_root = home.join(".local/share/Steam");
    f.proton = m.list(&steam_root.join("steamapps/common")).into_iter().filter(|n| n.starts_with("Proton")).collect();
    f.proton.extend(m.list(&steam_root.join("compatibilitytools.d")));
    let units = home.join(".config/systemd/user");
    f.keepalive_unit = [units.join("pimax-keepalived.service"), PathBuf::from("/usr/lib/systemd/user/pimax-keepalived.service")].into_iter().find(|x| has(x));
    let rust_exec = |t: Option<String>| t.is_some_and(|t| t.lines().any(|l| l.trim_start().starts_with("ExecStart=") && l.contains("pimax keepalived")));
    f.keepalive_rust = rust_exec(m.read_str(&units.join("pimax-keepalived.service.d/rust.conf"))) || f.keepalive_unit.as_deref().is_some_and(|u| rust_exec(m.read_str(u)));
    f.autod_build = has(&p.bin("pimax-autod")) || has(Path::new("/usr/bin/pimax-autod"));
    f.autod_unit = [units.join("pimax-autod.service"), PathBuf::from("/usr/lib/systemd/user/pimax-autod.service")].into_iter().find(|x| has(x));
    f.auto_config = has(&p.config.join("auto.toml"));
    f.edid = m.read(Path::new(EDID_INSTALLED)).map(|b| {
        if m.read(&p.repo.join(EDID_NON_DESKTOP)).is_some_and(|x| x == b) {
            "non-desktop"
        } else if m.read(&p.repo.join(EDID_NATIVE)).is_some_and(|x| x == b) {
            "native"
        } else {
            "other"
        }
    });
    let hypr = home.join(".config/hypr");
    let configs: Vec<(PathBuf, String)> = HYPR_FILES.iter().map(|n| hypr.join(n)).filter_map(|x| m.read_str(&x).map(|t| (x, t))).collect();
    f.hypr_dp1 = configs.iter().find(|(_, t)| has_dp1(t)).map(|(x, _)| (x.clone(), true)).or_else(|| configs.first().map(|(x, _)| (x.clone(), false)));
    let runs = home.join(".local/share/pimax/eye-calibration");
    let mut v: Vec<PathBuf> = m.list(&runs).into_iter().filter(|n| n.starts_with("eyecal-") && n.ends_with(".json") && !n.contains("-refit")).map(|n| runs.join(n)).collect();
    v.sort();
    v.reverse();
    v.truncate(5);
    f.eyecal_runs = v;
    f.ctrl_imu_logs = ["ctrl-imu-left.csv", "ctrl-imu-right.csv"].iter().map(|n| p.session_dir.join(n)).filter(|x| has(x)).collect();
    f
}

/// A package the user installs (copy only: the app does not run package installs).
fn package_fix(why: &str, pkg: &str) -> Fix {
    Fix::Root(
        RootFix::new(why, vec![format!("sudo pacman -S --needed {pkg}")], vec![format!("sudo pacman -Rs {pkg}")], false)
            .copy_only("package installs are not run by the app: run the line in a terminal"),
    )
}

/// The keepalive unit and its Rust drop-in (MANUAL-STEPS 1.4), as the user.
pub fn keepalive_fix(p: &Paths, link_unit: bool) -> UserFix {
    let units = p.home.join(".config/systemd/user");
    let dir = units.join("pimax-keepalived.service.d");
    let s = |x: &Path| x.display().to_string();
    let mut steps = vec![Cmd::new("mkdir", &["-p", &s(&dir)])];
    if link_unit {
        steps.push(Cmd::new("ln", &["-sf", &s(&p.repo.join("linux/pimax-keepalived.service")), &s(&units.join("pimax-keepalived.service"))]));
    }
    steps.push(Cmd::shell(&format!("printf '[Service]\\nExecStart=\\nExecStart=%%h/.local/bin/pimax keepalived\\n' > {}", quote(&s(&dir.join("rust.conf"))))));
    steps.push(Cmd::new("systemctl", &["--user", "daemon-reload"]));
    steps.push(Cmd::new("systemctl", &["--user", "enable", "--now", "pimax-keepalived"]));
    UserFix {
        why: "Install the user unit and the drop-in that runs the Rust `pimax keepalived` (needs the launcher in ~/.local/bin), then enable it. A keepalive that already runs switches at its next start (reboot or headset power cycle).".into(),
        steps,
        follow_up: None,
        needs_idle: true,
    }
}

/// The auto mode unit (MANUAL-STEPS 3.5): build the daemon where the unit runs it, install the unit and a dry-run
/// config. Nothing is started: Developer > Auto mode starts it on request.
pub fn autod_fix(p: &Paths, f: &ManualFacts) -> UserFix {
    let s = |x: &Path| x.display().to_string();
    let mut steps = Vec::new();
    if !f.autod_build {
        steps.push(Cmd::new("cargo", &["build", "--release", "-p", "pimax-autod"]).cwd(&p.rust));
    }
    if f.autod_unit.is_none() {
        steps.push(install_cmd(&p.repo.join("packaging/systemd/pimax-autod.service"), &p.home.join(".config/systemd/user/pimax-autod.service"), "644"));
    }
    if !f.auto_config {
        steps.push(Cmd::new("install", &["-Dm644", &s(&p.repo.join("packaging/auto.toml.example")), &s(&p.config.join("auto.toml"))]));
    }
    steps.push(Cmd::new("systemctl", &["--user", "daemon-reload"]));
    UserFix {
        why: "Build pimax-autod, install its user unit and ~/.config/pimax/auto.toml (dry run until you set dry_run = false). Nothing starts: Developer > Auto mode starts it when you ask.".into(),
        steps,
        follow_up: None,
        needs_idle: false,
    }
}

/// The checklist items.
pub fn items(facts: &Facts, p: &Paths) -> Vec<Item> {
    let f = &facts.manual;
    let mut v = Vec::new();
    // 1.8 packages
    v.push(if f.cargo {
        Item::new("pkg-rust", Group::App, "Rust toolchain", Level::Ok, "cargo is installed", None, Fix::None)
    } else {
        Item::new("pkg-rust", Group::App, "Rust toolchain", Level::Warn, "cargo not found: the tools and this app are built with it", None, package_fix("Rust builds the tools; the build fixes here run cargo.", "rust"))
    });
    v.push(if f.steam {
        Item::new("pkg-steam", Group::Extras, "Steam", Level::Ok, "installed", None, Fix::None)
    } else {
        Item::new("pkg-steam", Group::Extras, "Steam", Level::Info, "not installed (only needed for Steam games)", None,
            package_fix("Steam for SteamVR and OpenXR games; the [multilib] repository must be enabled in /etc/pacman.conf.", "steam"))
    });
    if f.steam {
        v.push(if f.proton.is_empty() {
            Item::new("pkg-proton", Group::Extras, "Proton", Level::Info, "none in the Steam library (Windows VR games need it)", None,
                Fix::Manual { why: "In Steam: Library, search Proton Experimental, Install.".into(), lines: Vec::new() })
        } else {
            Item::new("pkg-proton", Group::Extras, "Proton", Level::Ok, f.proton.join(", "), None, Fix::None)
        });
    }
    // 1.4 keepalive unit and drop-in
    v.push(match (&f.keepalive_unit, f.keepalive_rust) {
        (Some(u), true) => Item::new("keepalive-unit", Group::Headset, "Keepalive unit", Level::Ok, format!("{} runs the Rust pimax keepalived", u.display()), None, Fix::None),
        (Some(u), false) => Item::new("keepalive-unit", Group::Headset, "Keepalive unit", Level::Warn,
            format!("{} still runs the Python script: the drop-in that runs pimax keepalived is missing", u.display()), None, Fix::User(keepalive_fix(p, false))),
        (None, _) => Item::new("keepalive-unit", Group::Headset, "Keepalive unit", Level::Warn, "no pimax-keepalived unit in ~/.config/systemd/user", None, Fix::User(keepalive_fix(p, true))),
    });
    // 3.5 auto mode unit
    let missing: Vec<&str> = [(!f.autod_build, "the daemon build"), (f.autod_unit.is_none(), "the user unit"), (!f.auto_config, "~/.config/pimax/auto.toml")]
        .into_iter()
        .filter_map(|(m, n)| m.then_some(n))
        .collect();
    v.push(if missing.is_empty() {
        Item::new("auto-mode", Group::App, "Auto mode daemon", Level::Ok, "built, unit and config installed (Developer > Auto mode starts it)", None, Fix::None)
    } else {
        Item::new("auto-mode", Group::App, "Auto mode daemon", Level::Info, format!("optional; missing: {}", missing.join(", ")), None, Fix::User(autod_fix(p, f)))
    });
    // 1.1 EDID and 1.2 Hyprland's DP-1 line (read only: the app never edits Hyprland's config)
    v.push(match f.edid {
        Some("non-desktop") => Item::new("edid", Group::Display, "Headset EDID", Level::Ok, "non-desktop EDID in /etc/pimax/dp-edid.bin: Hyprland leases the panel to Monado", Some(10), Fix::None),
        Some("native") => Item::new("edid", Group::Display, "Headset EDID", Level::Ok, "native-mode EDID in /etc/pimax/dp-edid.bin (a desktop monitor; Hyprland's config keeps it off)", Some(10), Fix::None),
        Some(_) => Item::new("edid", Group::Display, "Headset EDID", Level::Info, "/etc/pimax/dp-edid.bin matches none of the repository's EDID files", Some(10), Fix::None),
        None => Item::new("edid", Group::Display, "Headset EDID", Level::Info, "no EDID override installed (/etc/pimax/dp-edid.bin); see Headset connector for the lines", Some(10), Fix::None),
    });
    if let Some((file, dp1)) = &f.hypr_dp1 {
        let name = file.display().to_string().replace(&p.home.display().to_string(), "~");
        let lua = name.ends_with(".lua");
        let line = if lua { "hl.monitor({ output = \"DP-1\", mode = \"7104x3840@72\", position = \"8000x8000\", scale = 2 })" } else { "monitor = DP-1, 7104x3840@72, 8000x8000, 2" };
        v.push(match (f.edid, dp1) {
            (Some("non-desktop"), true) => Item::new("hypr-dp1", Group::Display, "Hyprland: headset monitor line", Level::Warn,
                format!("{name} has a DP-1 monitor line; with the non-desktop EDID Hyprland leases the panel itself: remove that line"), None,
                Fix::Manual { why: format!("Remove the DP-1 line from {name} (the app does not edit Hyprland's config)."), lines: Vec::new() }),
            (Some("non-desktop"), false) => Item::new("hypr-dp1", Group::Display, "Hyprland: headset monitor line", Level::Ok, "no DP-1 line (right with the non-desktop EDID)", None, Fix::None),
            (_, true) => Item::new("hypr-dp1", Group::Display, "Hyprland: headset monitor line", Level::Ok, format!("{name} keeps DP-1 (the headset) off the desktop"), None, Fix::None),
            (_, false) => Item::new("hypr-dp1", Group::Display, "Hyprland: headset monitor line", Level::Warn,
                format!("no DP-1 line in {name}: the headset shows up as a desktop monitor"), None,
                Fix::Manual { why: format!("Add this line to {name} (the app does not edit Hyprland's config):"), lines: vec![line.to_string()] }),
        });
    }
    v
}

/// The eye calibration refit of a saved run (MANUAL-STEPS 6.2): prints the table and writes `RUN-refit.toml` next to
/// the run; with `write`, also saves it as the calibration (the old file is kept as `.bak-<utc>`).
pub fn eyecal_job(p: &Paths, run: &Path, write: bool) -> JobSpec {
    let mut args = vec!["--refit".to_string(), run.display().to_string()];
    if write {
        args.push("--write".into());
        args.push(p.eye_calibration.display().to_string());
    }
    JobSpec {
        kind: JobKind::Other(if write { "eyecal-write" } else { "eyecal-refit" }.into()),
        label: if write { "Save the refit eye calibration".into() } else { "Refit the last eye calibration run".into() },
        steps: vec![Step::Run(Cmd::with_args(p.bin("pimax-eyecal").display().to_string(), args))],
    }
}

/// The controller gyro fit from the last session's IMU logs (MANUAL-STEPS 6.3); with `save`, writes
/// `~/.config/pimax/ctrl-imu.json`.
pub fn imufit_job(p: &Paths, logs: &[PathBuf], save: bool) -> JobSpec {
    let mut args = vec!["imufit".to_string()];
    args.extend(logs.iter().map(|l| l.display().to_string()));
    if save {
        args.push("--save".into());
    }
    JobSpec {
        kind: JobKind::Other("imufit".into()),
        label: if save { "Save the controller gyro fit".into() } else { "Fit the controller gyros".into() },
        steps: vec![Step::Run(Cmd::with_args(p.bin("pimax-ctrltrack").display().to_string(), args))],
    }
}

/// `systemctl --user <action> pimax-autod` (start, stop, enable, disable).
pub fn autod_unit_job(action: &str) -> Option<JobSpec> {
    matches!(action, "start" | "stop" | "enable" | "disable").then(|| JobSpec {
        kind: JobKind::Other("autod-unit".into()),
        label: format!("Auto mode: systemctl --user {action} pimax-autod"),
        steps: vec![Step::Run(Cmd::new("systemctl", &["--user", action, "pimax-autod"]))],
    })
}

fn job_text(j: &JobSpec) -> String {
    j.steps.iter().filter_map(|s| if let Step::Run(c) = s { Some(c.display()) } else { None }).collect::<Vec<_>>().join("\n")
}

impl AppCore {
    /// Why a refit cannot be saved yet: the eye build's calibration parser must read version 2 files (it does after
    /// a rebuild with the current pimax-eye; then cap_sys_nice has to be restored).
    pub fn eyecal_v2_blocker(&self) -> Option<String> {
        let b = self.facts.builds.iter().find(|b| b.build.key == "desk" && b.built())?;
        match b.markers {
            Some(m) if !m.eye_v2 => Some("The eye Monado build cannot read version 2 calibration files: rebuild it first (Setup > Install > Rebuild Monado (desk)), then restore cap_sys_nice.".into()),
            _ => None,
        }
    }

    /// Refit the newest eye calibration run (prints the table, writes RUN-refit.toml next to the run); `write` asks
    /// first, then saves it as the calibration (the old file is kept as .bak-<utc>).
    pub fn eyecal_refit(&mut self, write: bool) {
        let Some(run) = self.facts.manual.eyecal_runs.first().cloned() else {
            self.message = "No saved eye calibration run (~/.local/share/pimax/eye-calibration).".into();
            return;
        };
        if !write {
            self.spawn(eyecal_job(&self.paths, &run, false));
            return;
        }
        if let Some(b) = self.eyecal_v2_blocker() {
            self.message = b;
            return;
        }
        let j = eyecal_job(&self.paths, &run, true);
        self.pending = Some(Pending::Confirm {
            title: "Save the refit as your eye calibration?".into(),
            detail: format!(
                "Fits {} again and writes the result to ~/.config/pimax/eye-calibration.toml; the current file is kept as .bak-<utc>. Monado reads it at its next start.",
                run.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default()
            ),
            command: job_text(&j),
            what: Confirm::EyecalWrite(run),
        });
    }

    /// Fit the controller gyros from the last session's IMU logs; `save` asks first, then writes ctrl-imu.json.
    pub fn imufit(&mut self, save: bool) {
        let logs = self.facts.manual.ctrl_imu_logs.clone();
        if logs.is_empty() {
            self.message = "No controller IMU logs from the last session (/tmp/pimax-vr-session/ctrl-imu-*.csv).".into();
            return;
        }
        if !save {
            self.spawn(imufit_job(&self.paths, &logs, false));
            return;
        }
        let j = imufit_job(&self.paths, &logs, true);
        self.pending = Some(Pending::Confirm {
            title: "Save the controller gyro fit?".into(),
            detail: "Writes ~/.config/pimax/ctrl-imu.json from the last session's logs (the tracker also saves its own fit every 30 s and at shutdown).".into(),
            command: job_text(&j),
            what: Confirm::ImufitSave(logs),
        });
    }

    /// Confirmed: run the pending step.
    pub fn confirm_step(&mut self) {
        let Some(Pending::Confirm { what, .. }) = self.pending.take() else { return };
        let j = match what {
            Confirm::EyecalWrite(run) => eyecal_job(&self.paths, &run, true),
            Confirm::ImufitSave(logs) => imufit_job(&self.paths, &logs, true),
            Confirm::Daemon(line) => {
                self.system_send(line);
                return;
            }
        };
        self.spawn(j);
    }

    /// Start, stop, enable or disable the auto mode user unit (a job; the status is asked again afterwards).
    pub fn autod_unit(&mut self, action: &str) {
        if let Some(j) = autod_unit_job(action) {
            self.spawn(j);
            self.auto.status = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::FakeMachine;

    #[test]
    fn manual_steps_are_found_and_their_fixes_pass_the_safety_rules() {
        let mut m = FakeMachine::new();
        let p = crate::demo::paths();
        let mut f = Facts { manual: collect(&m, &p), ..Default::default() };
        assert!(!f.manual.cargo && f.manual.keepalive_unit.is_none() && f.manual.edid.is_none() && f.manual.hypr_dp1.is_none());
        let v = items(&f, &p);
        let get = |v: &[Item], id: &str| v.iter().find(|i| i.id == id).cloned().unwrap_or_else(|| panic!("{id}"));
        assert!(matches!(&get(&v, "pkg-rust").fix, Fix::Root(r) if r.pkexec.is_err() && r.lines == ["sudo pacman -S --needed rust"]), "package installs are copy only");
        for id in ["keepalive-unit", "auto-mode"] {
            let Fix::User(u) = &get(&v, id).fix else { panic!("{id}") };
            for c in &u.steps {
                crate::safety::check(c).unwrap_or_else(|e| panic!("{id}: {}: {e}", c.display()));
            }
        }
        let Fix::User(k) = &get(&v, "keepalive-unit").fix else { panic!() };
        assert!(k.needs_idle && k.steps.iter().any(|c| c.text().contains("ExecStart=%%h/.local/bin/pimax keepalived")));
        // a native EDID and a Hyprland config without the DP-1 line: the line to add is shown, nothing is written
        m.file("/usr/bin/cargo", "elf");
        m.file("/etc/pimax/dp-edid.bin", "N");
        m.file("/home/u/pimax-re/linux/pimax-dp-edid-native.bin", "N");
        m.file("/home/u/.config/hypr/monitors.lua", "hl.monitor({ output = \"DP-2\", mode = \"preferred\" })\n-- DP-1 monitor (old)\n");
        m.file("/home/u/.config/systemd/user/pimax-keepalived.service", "[Service]\nExecStart=/usr/bin/python3 x\n");
        m.file("/home/u/.config/systemd/user/pimax-keepalived.service.d/rust.conf", "[Service]\nExecStart=\nExecStart=%h/.local/bin/pimax keepalived\n");
        m.file("/home/u/.local/share/pimax/eye-calibration/eyecal-20261014-103635.json", "{}");
        m.file("/home/u/.local/share/pimax/eye-calibration/eyecal-20261014-103635-refit.toml", "x");
        m.file("/home/u/.local/share/pimax/eye-calibration/eyecal-20261015-090000.json", "{}");
        f.manual = collect(&m, &p);
        assert_eq!(f.manual.edid, Some("native"));
        assert!(f.manual.keepalive_rust && f.manual.cargo);
        assert_eq!(f.manual.eyecal_runs.len(), 2);
        assert!(f.manual.eyecal_runs[0].ends_with("eyecal-20261015-090000.json"), "newest first");
        let v = items(&f, &p);
        assert_eq!(get(&v, "keepalive-unit").level, Level::Ok);
        let h = get(&v, "hypr-dp1");
        assert_eq!(h.level, Level::Warn);
        assert!(matches!(&h.fix, Fix::Manual { lines, .. } if lines[0].starts_with("hl.monitor({ output = \"DP-1\"")));
        // the non-desktop EDID wants the line gone
        m.file("/home/u/pimax-re/linux/pimax-dp-edid-msvsdb.bin", "N");
        m.file("/home/u/.config/hypr/monitors.lua", "hl.monitor({ output = \"DP-1\", mode = \"7104x3840@72\" })\n");
        f.manual = collect(&m, &p);
        assert_eq!(get(&items(&f, &p), "hypr-dp1").level, Level::Warn);
        assert!(has_dp1("monitor = DP-1, 7104x3840@72, 8000x8000, 2") && !has_dp1("# monitor = DP-1"));
    }

    #[test]
    fn writing_steps_are_confirmed_first() {
        let d = std::env::temp_dir().join(format!("pimax-manual-{}", std::process::id()));
        let mut a = AppCore::demo(false, pimax_config::Store::new(d.join("cfg"), d.join("cache")));
        assert_eq!(a.facts.manual.eyecal_runs.len(), 1);
        assert!(a.eyecal_v2_blocker().is_none(), "the demo eye build reads version 2 files");
        a.eyecal_refit(true);
        assert!(matches!(&a.pending, Some(Pending::Confirm { what: Confirm::EyecalWrite(_), command, .. }) if command.contains("--write")));
        assert!(a.job.is_none());
        a.cancel_pending();
        a.imufit(true);
        assert!(a.pending.is_none(), "no logs: nothing to confirm");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn the_jobs_are_the_documented_lines() {
        let p = crate::demo::paths();
        let j = eyecal_job(&p, Path::new("/r/eyecal-1.json"), true);
        let Step::Run(c) = &j.steps[0] else { panic!() };
        assert_eq!(c.args, ["--refit", "/r/eyecal-1.json", "--write", "/home/u/.config/pimax/eye-calibration.toml"]);
        let j = imufit_job(&p, &[PathBuf::from("/tmp/pimax-vr-session/ctrl-imu-left.csv")], true);
        let Step::Run(c) = &j.steps[0] else { panic!() };
        assert!(c.program.ends_with("/pimax-ctrltrack") && c.args == ["imufit", "/tmp/pimax-vr-session/ctrl-imu-left.csv", "--save"]);
        assert!(autod_unit_job("restart").is_none() && autod_unit_job("start").is_some());
        for j in [eyecal_job(&p, Path::new("/r/x.json"), false), imufit_job(&p, &[], false), autod_unit_job("enable").unwrap()] {
            for s in &j.steps {
                if let Step::Run(c) = s {
                    crate::safety::check(c).unwrap_or_else(|e| panic!("{}: {e}", c.display()));
                }
            }
        }
    }
}
