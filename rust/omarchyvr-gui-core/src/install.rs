//! The installer's one root job and the uninstaller, as the app shows them (docs/INSTALL.md, docs/UNINSTALL.md).
//!
//! `./install.sh` builds and installs the set as the user and prepares the root job: `<share>/root-plan.env` (the
//! inputs) and `<share>/root-plan.txt` (exactly what it changes, from `omarchyvr-root.sh plan`). The setup wizard's
//! "System setup" step shows that text and runs `omarchyvr-root.sh apply <plan>` through ONE pkexec prompt. The
//! uninstall button runs the root part the same way (from the root-owned copy in `/usr/local/lib/omarchyvr`), then
//! the user part (`uninstall.sh --no-root --yes`), which also quits the app.
use crate::checklist::RootFix;
use std::path::{Path, PathBuf};

/// Where the root job keeps its manifest (root-owned, world-readable).
pub const ROOT_MANIFEST: &str = "/var/lib/omarchyvr/manifest.tsv";
/// The root-owned copy of the root job (installed by `apply`; used by the uninstaller).
pub const ROOT_SCRIPT_INSTALLED: &str = "/usr/local/lib/omarchyvr/omarchyvr-root.sh";

/// The files of one install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Install {
    /// `~/.local/share/omarchyvr`.
    pub share: PathBuf,
}

impl Install {
    /// The default install of this user.
    pub fn of_home(home: &Path) -> Install {
        Install { share: home.join(".local/share/omarchyvr") }
    }
    /// The install manifest (`./install.sh` wrote it).
    pub fn manifest(&self) -> PathBuf {
        self.share.join("manifest.tsv")
    }
    /// The root job's inputs.
    pub fn plan_env(&self) -> PathBuf {
        self.share.join("root-plan.env")
    }
    /// What the root job changes, as text.
    pub fn plan_txt(&self) -> PathBuf {
        self.share.join("root-plan.txt")
    }
    /// The installed data (docs, linux/, patches/, installer/).
    pub fn data(&self) -> PathBuf {
        self.share.join("data")
    }
    /// The root job script in the installed data.
    pub fn root_script(&self) -> PathBuf {
        self.data().join("installer/omarchyvr-root.sh")
    }
    /// The user part of the uninstaller.
    pub fn uninstall_script(&self) -> PathBuf {
        self.data().join("installer/uninstall.sh")
    }
    /// The uninstall plan the app writes before the root part.
    pub fn uninstall_env(&self) -> PathBuf {
        self.share.join("uninstall-plan.env")
    }
    /// Left by `./install.sh` for the app: check this PC once more at the next start (a changed Monado lost its
    /// realtime permission, ...). The app removes it when the checks are green.
    pub fn post_install_check(&self) -> PathBuf {
        self.share.join("post-install-check")
    }
}

/// The wizard's "System setup" root job, when `./install.sh` prepared one. `plan_text` is the content of
/// `root-plan.txt`; the shown line is the only thing pkexec runs.
pub fn setup_fix(i: &Install, plan_text: &str) -> RootFix {
    let reboot = plan_text.lines().any(|l| l.contains("dkms build") || l.contains("mkinitcpio") || l.contains("EDID override"));
    RootFix {
        why: format!(
            "One password for everything below. Nothing else on this PC changes; ./uninstall.sh (or Ready > Uninstall omarchyvr) \
             reverses it.\n\n{}",
            plan_text.trim_end()
        ),
        lines: vec![format!("{} apply {}", i.root_script().display(), i.plan_env().display())],
        undo: vec![format!("{} uninstall {}", ROOT_SCRIPT_INSTALLED, i.uninstall_env().display())],
        reboot,
        pkexec: Ok(()),
    }
}

/// The uninstaller's root part (from the root-owned copy when the root job ran, else from the installed data).
pub fn uninstall_root_fix(i: &Install, root_installed: bool) -> RootFix {
    let script = if root_installed { PathBuf::from(ROOT_SCRIPT_INSTALLED) } else { i.root_script() };
    RootFix {
        why: "Removes the system part of omarchyvr: the udev rule, the NVIDIA patches and pacman hooks (the stock \
              driver source is restored and rebuilt), the EDID override and initramfs entries, and the realtime \
              permission. A snapper snapshot is taken first. Then the app removes your user files (downloads and \
              recordings are kept) and closes."
            .into(),
        lines: vec![format!("{} uninstall {}", script.display(), i.uninstall_env().display())],
        undo: vec![],
        reboot: true,
        pkexec: Ok(()),
    }
}

/// The uninstall plan's text (the user and home the root part may touch).
pub fn uninstall_env_text(user: &str, home: &Path) -> String {
    format!("OVR_USER={user}\nOVR_HOME={}\nOVR_SNAPSHOT=yes\nOVR_KEEP_LEGACY_CAPS=no\n", home.display())
}

/// Did the root job run? (its manifest exists)
pub fn root_done(exists: &dyn Fn(&Path) -> bool) -> bool {
    exists(Path::new(ROOT_MANIFEST))
}

/// What the app knows about the install (read in the background).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallInfo {
    /// Read at least once.
    pub loaded: bool,
    /// `./install.sh` left a root job: its text (`root-plan.txt`).
    pub plan_text: Option<String>,
    /// The root job ran (its manifest exists).
    pub root_done: bool,
    /// An install manifest exists (an install by `./install.sh`).
    pub manifest: bool,
    /// The NVIDIA driver and the patch on it (see [`crate::nvidia_patch`]).
    pub nvidia: crate::nvidia_patch::NvidiaInfo,
    /// `./install.sh` asked for one check after the install ([`Install::post_install_check`]).
    pub post_install: bool,
}

/// Read the install state (files only; cheap).
pub fn read_info(i: &Install) -> InstallInfo {
    InstallInfo {
        loaded: true,
        plan_text: std::fs::read_to_string(i.plan_txt()).ok().filter(|t| !t.trim().is_empty()),
        root_done: root_done(&|p| p.exists()),
        manifest: i.manifest().is_file(),
        nvidia: crate::nvidia_patch::read_info(i),
        post_install: i.post_install_check().is_file(),
    }
}

/// The demo machine's install (screenshots): the root job of a typical first install, already applied.
pub fn demo_info() -> InstallInfo {
    InstallInfo {
        loaded: true,
        plan_text: Some(
            "omarchyvr system setup (plan only, nothing changes):\n 1. Restore point: a snapper snapshot of /\n 2. Headset access: udev rule /etc/udev/rules.d/70-pimax.rules (only Pimax devices 34a4:* and the eye tracker 2104:0561; keeps the hubs above the headset awake)\n 3. Uninstall helper: /usr/local/lib/omarchyvr/omarchyvr-root.sh\n 4. NVIDIA 610.57.04 (tested): display patches for the headset's two-head mode, re-applied after driver updates\n      would: patch /usr/src/nvidia-610.57.04 (originals kept as *.pimax-orig), dkms build + install; stock module if the build fails\n 5. Headset display mode at boot: EDID override for [0000:01:00.0].DP-0 (non-desktop, native modes)\n      would: limine-mkinitcpio\n 6. Realtime priority for the VR compositor: cap_sys_nice on ~/.local/lib/omarchyvr/monado/.../monado-service (once; Play never asks again)\nREBOOT=yes\n"
                .into(),
        ),
        root_done: true,
        manifest: true,
        nvidia: crate::nvidia_patch::demo_info(),
        post_install: false,
    }
}

/// Write the NVIDIA choice into `root-plan.env` (no plan file yet: nothing to do).
pub fn write_plan_allow(i: &Install, allow: bool) -> std::io::Result<()> {
    let p = i.plan_env();
    let Ok(text) = std::fs::read_to_string(&p) else { return Ok(()) };
    let new = crate::nvidia_patch::set_plan_allow(&text, allow);
    if new == text {
        return Ok(());
    }
    let tmp = p.with_extension("env.tmp");
    std::fs::write(&tmp, new)?;
    std::fs::rename(&tmp, &p)
}

/// Write the choice into the plan and make `root-plan.txt` again with the root job's own `plan` mode (any user,
/// changes nothing).
pub fn refresh_plan(i: &Install, allow: bool) -> Result<(), String> {
    write_plan_allow(i, allow).map_err(|e| e.to_string())?;
    if !i.plan_env().is_file() || !i.root_script().is_file() {
        return Ok(());
    }
    let out = std::process::Command::new(i.root_script()).arg("plan").arg(i.plan_env()).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("the plan failed").to_string());
    }
    std::fs::write(i.plan_txt(), &out.stdout).map_err(|e| e.to_string())
}

/// Before the automatic setup runs the root job: the headset the person chose (never the install-time guess), the
/// NVIDIA choice and the EDID step go into the plan, and the plan text is made again with the job's own `plan` mode.
/// Returns the new plan text (None: no plan on this PC).
pub fn prepare_plan(i: &Install, allow: bool, pick: crate::headset_choice::HeadsetPick, usb: crate::headset_choice::HeadsetUsb) -> Result<Option<String>, String> {
    if !i.plan_env().is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(i.plan_env()).map_err(|e| e.to_string())?;
    let new = crate::headset_choice::plan_for_pick(&text, pick, usb);
    if new != text {
        let tmp = i.plan_env().with_extension("env.tmp");
        std::fs::write(&tmp, new).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, i.plan_env()).map_err(|e| e.to_string())?;
    }
    refresh_plan(i, allow)?;
    Ok(std::fs::read_to_string(i.plan_txt()).ok().filter(|t| !t.trim().is_empty()))
}

/// The label of the uninstaller's root job (the job runner's label of a confirmed root fix).
pub const UNINSTALL_TITLE: &str = "Uninstall omarchyvr";

/// The user part of the uninstaller, started after the root part: detached in its own session with its output in a
/// log file in the home directory (it also quits this app).
pub fn uninstall_user_cmd(i: &Install, home: &Path, stamp: &str) -> crate::cmd::Cmd {
    let log = home.join(format!("omarchyvr-uninstall-{stamp}.log"));
    crate::cmd::Cmd::shell(&format!(
        "setsid -f {} --no-root --yes > {} 2>&1 < /dev/null",
        crate::cmd::quote(&i.uninstall_script().display().to_string()),
        crate::cmd::quote(&log.display().to_string())
    ))
}

impl crate::app::AppCore {
    /// The install of this user.
    pub fn install(&self) -> Install {
        Install::of_home(&self.paths.home)
    }

    /// Read the install state in the background.
    pub fn refresh_install(&mut self) {
        if self.task_busy("install-info") {
            return;
        }
        let i = self.install();
        self.task(Some("install-info"), move || {
            let info = read_info(&i);
            Box::new(move |c: &mut crate::app::AppCore| c.onboarding.install = info)
        });
    }

    /// The wizard's System setup: the root job of `./install.sh` (one pkexec prompt), shown first.
    pub fn ask_system_setup(&mut self) -> bool {
        let Some(text) = self.onboarding.install.plan_text.clone() else { return false };
        // the switch "Allow the patch on untested NVIDIA drivers" goes into the plan the root job reads
        if !self.demo {
            if let Err(e) = write_plan_allow(&self.install(), self.state.nvidia_untested) {
                self.message = format!("cannot save the NVIDIA choice in {}: {e}", self.install().plan_env().display());
                return false;
            }
        }
        let fix = setup_fix(&self.install(), &text);
        self.pending = Some(crate::app::Pending::Root { title: "Set up this PC".into(), fix, catalogue: None });
        true
    }

    /// The switch "Allow the patch on untested NVIDIA drivers": saved in the app state (shared with the CLI through
    /// `gui-state.toml`), written into the setup plan, and the plan text the wizard shows is made again. The system
    /// copy (`/etc/omarchyvr/nvidia.conf`) changes only when the setup runs (Apply, one password).
    pub fn set_nvidia_untested(&mut self, on: bool) {
        self.state.nvidia_untested = on;
        self.write_state();
        if self.demo || self.task_busy("nvidia-plan") {
            return;
        }
        let i = self.install();
        self.task(Some("nvidia-plan"), move || {
            let err = refresh_plan(&i, on).err();
            let info = read_info(&i);
            Box::new(move |c: &mut crate::app::AppCore| {
                c.onboarding.install = info;
                if let Some(e) = err {
                    c.message = format!("The NVIDIA choice is saved, but the setup plan could not be updated: {e}");
                }
            })
        });
    }

    /// Ready > Uninstall omarchyvr: first the root part (one pkexec prompt, shown first), then the user part.
    pub fn ask_uninstall(&mut self) {
        let i = self.install();
        let user = self.machine.env("USER").unwrap_or_default();
        if user.is_empty() || !i.uninstall_script().is_file() {
            self.message = format!(
                "Uninstall needs the installed data ({}): run ./uninstall.sh in your omarchyvr clone instead.",
                i.uninstall_script().display()
            );
            return;
        }
        if let Err(e) = std::fs::write(i.uninstall_env(), uninstall_env_text(&user, &self.paths.home)) {
            self.message = format!("cannot write {}: {e}", i.uninstall_env().display());
            return;
        }
        let fix = uninstall_root_fix(&i, Path::new(ROOT_SCRIPT_INSTALLED).is_file());
        self.pending = Some(crate::app::Pending::Root { title: UNINSTALL_TITLE.into(), fix, catalogue: None });
    }

    /// The uninstaller's confirmation as the demo shows it (screenshots; nothing is written).
    pub fn ask_uninstall_demo(&mut self) {
        let fix = uninstall_root_fix(&self.install(), true);
        self.pending = Some(crate::app::Pending::Root { title: UNINSTALL_TITLE.into(), fix, catalogue: None });
    }

    /// Show what the uninstaller would remove (dry run, no password).
    pub fn uninstall_dry_run(&mut self) {
        let i = self.install();
        let c = crate::cmd::Cmd::new(i.uninstall_script().display().to_string(), &["--dry-run"]);
        self.spawn(crate::job::JobSpec {
            kind: crate::job::JobKind::Other("uninstall-dry-run".into()),
            label: "Uninstall: what would be removed (dry run)".into(),
            steps: vec![crate::job::Step::Run(c)],
        });
    }

    /// A job ended: after the System setup the state is read again; after the uninstaller's root part the user part
    /// starts (and quits the app).
    pub(crate) fn install_job_done(&mut self, label: &str, ok: bool) {
        if !label.starts_with("Root fix: ") {
            return;
        }
        self.refresh_install();
        if ok && label.contains(UNINSTALL_TITLE) {
            let stamp = omarchyvr_config::store::stamp(self.machine.now_unix());
            let c = uninstall_user_cmd(&self.install(), &self.paths.home, &stamp);
            self.message = "System part removed. Removing your user files now; the app closes.".into();
            self.spawn(crate::job::JobSpec {
                kind: crate::job::JobKind::Other("uninstall".into()),
                label: "Uninstall: user files".into(),
                steps: vec![crate::job::Step::Run(c)],
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setup_fix_runs_exactly_the_root_job_and_passes_the_safety_checks() {
        let i = Install::of_home(Path::new("/home/u"));
        let f = setup_fix(&i, " 1. Headset access: udev rule\n 4. NVIDIA 610.57.04 (tested)\n      would: patch ..., dkms build + install\n");
        assert_eq!(
            f.lines,
            vec!["/home/u/.local/share/omarchyvr/data/installer/omarchyvr-root.sh apply /home/u/.local/share/omarchyvr/root-plan.env".to_string()]
        );
        assert!(f.reboot);
        assert!(f.why.contains("Headset access"));
        let c = f.command().expect("pkexec");
        assert_eq!(c.program, "/usr/bin/pkexec");
        crate::safety::check(&c).expect("allowed by the safety rules");
        let u = uninstall_root_fix(&i, true);
        let c = u.command().expect("pkexec");
        crate::safety::check(&c).expect("allowed by the safety rules");
        assert!(u.lines[0].starts_with("/usr/local/lib/omarchyvr/omarchyvr-root.sh uninstall /home/u/.local/share/omarchyvr/uninstall-plan.env"));
        let c = uninstall_user_cmd(&i, Path::new("/home/u"), "20261016T200000Z");
        crate::safety::check(&c).expect("the user part passes the user rules");
        assert!(c.text().contains("uninstall.sh --no-root --yes > /home/u/omarchyvr-uninstall-20261016T200000Z.log"), "{}", c.text());
    }

    #[test]
    fn prepare_plan_rewrites_the_plan_file_from_the_choice() {
        use crate::headset_choice::{HeadsetPick, HeadsetUsb};
        let d = std::env::temp_dir().join(format!("omarchyvr-plan-pick-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let i = Install { share: d.clone() };
        assert_eq!(prepare_plan(&Install { share: d.join("none") }, false, HeadsetPick::Other, HeadsetUsb::Absent).unwrap(), None);
        std::fs::write(i.plan_env(), "OVR_USER=u\nOVR_NVIDIA=skip\nOVR_NVIDIA_VERSION=610.57.04\nOVR_NVIDIA_WANT=patch\nOVR_EDID=\n").unwrap();
        // no root script in this share: only the env is written
        prepare_plan(&i, false, HeadsetPick::Model(0), HeadsetUsb::Asleep).unwrap();
        let t = std::fs::read_to_string(i.plan_env()).unwrap();
        assert!(t.contains("OVR_NVIDIA=patch\n") && t.contains("OVR_EDID=auto\n") && t.contains("OVR_HEADSET=asleep\n") && t.contains("OVR_NVIDIA_ALLOW_UNTESTED=no\n"), "{t}");
        prepare_plan(&i, false, HeadsetPick::Other, HeadsetUsb::Asleep).unwrap();
        let t = std::fs::read_to_string(i.plan_env()).unwrap();
        assert!(t.contains("OVR_NVIDIA=skip\n") && t.contains("OVR_EDID=\n"), "{t}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_nvidia_choice_goes_into_the_plan_the_root_job_reads() {
        let d = std::env::temp_dir().join(format!("omarchyvr-plan-allow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let i = Install { share: d.clone() };
        write_plan_allow(&i, true).expect("no plan yet: nothing to do");
        assert!(!i.plan_env().exists(), "a missing plan is not created");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(i.plan_env(), "OVR_USER=u\nOVR_NVIDIA=patch\nOVR_NVIDIA_VERSION=999.1\n").unwrap();
        write_plan_allow(&i, true).unwrap();
        let t = std::fs::read_to_string(i.plan_env()).unwrap();
        assert!(t.contains("OVR_NVIDIA_ALLOW_UNTESTED=yes\n") && t.contains("OVR_USER=u\n"), "{t}");
        write_plan_allow(&i, false).unwrap();
        let t = std::fs::read_to_string(i.plan_env()).unwrap();
        assert!(t.contains("OVR_NVIDIA_ALLOW_UNTESTED=no\n") && !t.contains("=yes"), "{t}");
        // the values the root job accepts: only safe characters
        assert!(t.lines().all(|l| l.chars().all(|c| c.is_ascii_alphanumeric() || "_=./:@+ -".contains(c))), "{t}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_safety_rules_refuse_other_root_scripts_and_modes() {
        for bad in [
            "/home/u/evil/omarchyvr-root.sh apply /home/u/p.env",
            "/home/u/.local/share/omarchyvr/data/installer/omarchyvr-root.sh rm /home/u/p.env",
            "/home/u/.local/share/omarchyvr/data/installer/omarchyvr-root.sh apply /home/u/../etc/p.env",
            "/home/u/.local/share/omarchyvr/data/installer/omarchyvr-root.sh apply relative.env",
            "omarchyvr-root.sh apply /home/u/p.env",
        ] {
            let f = RootFix { why: String::new(), lines: vec![bad.into()], undo: vec![], reboot: false, pkexec: Ok(()) };
            let c = f.command().expect("cmd");
            assert!(crate::safety::check(&c).is_err(), "{bad} must be refused");
        }
    }
}
