//! `omarchyvr vr hand-models status|download --yes`: fetch the Mercury hand-tracking models from Monado's public server
//! into `~/.local/share/monado/hand-tracking-models` (docs/HANDS.md "Models and licence").
//!
//! The models have NO licence, so omarchyvr never ships them: the user downloads them to their own PC for personal use.
//! The app shows the consent text first and passes `--yes`; `installer/install.sh --hand-models` calls the same command.
//! Runs as the user (no root). Needs `git` and `git-lfs`; a missing one is reported in plain words and nothing is installed.
//! The clone goes to a `.part` folder and is moved into place only when the models are complete, so a failed or
//! cancelled download leaves nothing half-installed; running it again starts over cleanly.
//! The git steps sit behind [`Git`] so the tests never touch the network.
use super::hands::MODELS;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Monado's public models repository.
pub const URL: &str = "https://gitlab.freedesktop.org/monado/utilities/hand-tracking-models.git";
/// The pinned commit (same as `HANDMODELS_COMMIT` in installer/components.lock; a test compares them).
pub const COMMIT: &str = "37a8a81bc8f433ac6cbdf2471909d2bac74beca1";
/// Where Monado looks, relative to `$HOME`.
pub const DIR: &str = ".local/share/monado/hand-tracking-models";

/// The consent text (shown by the app before the download, printed by the CLI).
pub const CONSENT: &str = "These hand-tracking models come from Monado's public server. They have no licence. \
They are downloaded by you, to this PC, for your personal use, and are never shipped by omarchyvr.";

/// What is on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// Both models are there and are real files.
    Installed,
    /// The folder exists but a model is missing or is still a git-lfs pointer (an unfinished download).
    Incomplete(String),
    /// Nothing there.
    Missing,
}

fn is_lfs_pointer(p: &Path) -> bool {
    match std::fs::read(p) {
        Ok(d) => d.len() < 1024 && d.starts_with(b"version https://git-lfs"),
        Err(_) => false,
    }
}

/// The state of the models folder `dir`.
pub fn state(dir: &Path) -> State {
    if !dir.exists() {
        return State::Missing;
    }
    for m in MODELS {
        let p = dir.join(m);
        if !p.is_file() {
            return State::Incomplete(format!("{m} is missing"));
        }
        if is_lfs_pointer(&p) {
            return State::Incomplete(format!("{m} was not downloaded completely"));
        }
    }
    State::Installed
}

/// The install folder for `home`.
pub fn dir_of(home: &Path) -> PathBuf {
    home.join(DIR)
}

/// The git steps (real: the `git` program; tests: a mock).
pub trait Git {
    /// `git lfs version` works.
    fn lfs_available(&mut self) -> bool;
    /// `git --version` works.
    fn git_available(&mut self) -> bool;
    /// Clone without downloading the large files yet.
    fn clone_repo(&mut self, url: &str, dir: &Path) -> Result<(), String>;
    /// Check out the pinned commit.
    fn checkout(&mut self, dir: &Path, commit: &str) -> Result<(), String>;
    /// Download the large files (the models).
    fn lfs_pull(&mut self, dir: &Path) -> Result<(), String>;
}

/// The real git. Output goes to stderr (the app shows it in its log); no password prompt can block it.
pub struct RealGit;

impl RealGit {
    fn run(args: &[&str], cwd: Option<&Path>, skip_smudge: bool) -> Result<(), String> {
        let mut c = Command::new("git");
        c.args(args).env("GIT_TERMINAL_PROMPT", "0").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::inherit());
        if skip_smudge {
            c.env("GIT_LFS_SKIP_SMUDGE", "1");
        }
        if let Some(d) = cwd {
            c.current_dir(d);
        }
        match c.status() {
            Ok(s) if s.success() => Ok(()),
            Ok(s) => Err(format!("git failed ({s})")),
            Err(e) => Err(format!("cannot run git: {e}")),
        }
    }
}

impl Git for RealGit {
    fn lfs_available(&mut self) -> bool {
        Command::new("git").args(["lfs", "version"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
    }
    fn git_available(&mut self) -> bool {
        Command::new("git").arg("--version").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
    }
    fn clone_repo(&mut self, url: &str, dir: &Path) -> Result<(), String> {
        Self::run(&["clone", "--progress", url, &dir.display().to_string()], None, true)
    }
    fn checkout(&mut self, dir: &Path, commit: &str) -> Result<(), String> {
        Self::run(&["-c", "advice.detachedHead=false", "checkout", "--quiet", commit], Some(dir), true)
    }
    fn lfs_pull(&mut self, dir: &Path) -> Result<(), String> {
        Self::run(&["lfs", "pull"], Some(dir), false)
    }
}

fn incomplete(s: State) -> String {
    match s {
        State::Incomplete(w) => format!("The download finished but is not complete ({w})."),
        _ => "The download produced no files.".into(),
    }
}

/// Download the models into `<home>/.local/share/monado/hand-tracking-models`. `say` gets one plain-words line per step.
/// Returns Ok when the models are installed afterwards (also when they already were).
pub fn download(home: &Path, git: &mut dyn Git, url: &str, commit: &str, say: &mut dyn FnMut(&str)) -> Result<(), String> {
    let dir = dir_of(home);
    if state(&dir) == State::Installed {
        say("The hand models are already installed.");
        return Ok(());
    }
    if !git.git_available() {
        return Err("git is not installed on this PC, so the hand models cannot be downloaded. Nothing was changed.".into());
    }
    if !git.lfs_available() {
        return Err("git-lfs is not installed on this PC, and the hand models need it. Install the package \"git-lfs\", then press Download again. Nothing was changed.".into());
    }
    let net = |e: String| format!("The model files could not be downloaded: {e}. Check the internet connection and press Download again.");
    if dir.join(".git").is_dir() {
        // an earlier download (the installer's) that stopped before the large files: finish it in place
        say("Step 1 of 3: found an unfinished download, continuing it.");
        git.checkout(&dir, commit).map_err(|e| format!("Could not select the model version: {e}"))?;
        say("Step 3 of 3: downloading the model files (this can take a few minutes).");
        git.lfs_pull(&dir).map_err(net)?;
    } else {
        if dir.exists() && std::fs::read_dir(&dir).map(|mut d| d.next().is_some()).unwrap_or(true) {
            return Err(format!("{} already exists and is not a download of the hand models; it was left alone.", dir.display()));
        }
        let part = dir.with_file_name("hand-tracking-models.part");
        if let Some(parent) = dir.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("Cannot create {}: {e}", parent.display()))?;
        }
        if part.exists() {
            let _ = std::fs::remove_dir_all(&part);
        }
        say("Step 1 of 3: connecting to Monado's server.");
        let r = git
            .clone_repo(url, &part)
            .map_err(|e| format!("Could not reach Monado's server: {e}. Check the internet connection and press Download again."))
            .and_then(|_| {
                say("Step 2 of 3: selecting the model version.");
                git.checkout(&part, commit).map_err(|e| format!("Could not select the model version: {e}"))
            })
            .and_then(|_| {
                say("Step 3 of 3: downloading the model files (this can take a few minutes).");
                git.lfs_pull(&part).map_err(net)
            })
            .and_then(|_| match state(&part) {
                State::Installed => Ok(()),
                s => Err(incomplete(s)),
            });
        if let Err(e) = r {
            let _ = std::fs::remove_dir_all(&part);
            return Err(e);
        }
        if dir.exists() {
            let _ = std::fs::remove_dir(&dir);
        }
        std::fs::rename(&part, &dir).map_err(|e| format!("Cannot move the models into {}: {e}", dir.display()))?;
    }
    match state(&dir) {
        State::Installed => {
            say("Done: the hand models are installed. The Camera hands switch is available now.");
            Ok(())
        }
        s => Err(incomplete(s)),
    }
}

/// `omarchyvr vr hand-models status|download --yes`.
pub fn run(what: &str, yes: bool, home: &Path) -> Result<(), String> {
    match what {
        "status" => {
            match state(&dir_of(home)) {
                State::Installed => vout!("hand models: installed ({})", dir_of(home).display()),
                State::Incomplete(w) => vout!("hand models: incomplete ({w})"),
                State::Missing => vout!("hand models: not installed"),
            }
            Ok(())
        }
        "download" => {
            vout!("{CONSENT}");
            if !yes {
                return Err("Add --yes to confirm the notice above (the app asks you first).".into());
            }
            download(home, &mut RealGit, URL, COMMIT, &mut |l| vout!("{l}"))
        }
        o => Err(format!("unknown hand-models command {o} (status | download --yes)")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Mock {
        git: bool,
        lfs: bool,
        fail_at: Option<&'static str>,
        calls: Vec<String>,
        pointers: bool,
    }
    impl Mock {
        fn ok() -> Mock {
            Mock { git: true, lfs: true, ..Default::default() }
        }
    }
    impl Git for Mock {
        fn git_available(&mut self) -> bool {
            self.git
        }
        fn lfs_available(&mut self) -> bool {
            self.lfs
        }
        fn clone_repo(&mut self, url: &str, dir: &Path) -> Result<(), String> {
            self.calls.push(format!("clone {url}"));
            if self.fail_at == Some("clone") {
                return Err("offline".into());
            }
            std::fs::create_dir_all(dir.join(".git")).unwrap();
            Ok(())
        }
        fn checkout(&mut self, _: &Path, c: &str) -> Result<(), String> {
            self.calls.push(format!("checkout {c}"));
            Ok(())
        }
        fn lfs_pull(&mut self, dir: &Path) -> Result<(), String> {
            self.calls.push("pull".into());
            if self.fail_at == Some("pull") {
                return Err("timeout".into());
            }
            for m in MODELS {
                let body: &[u8] = if self.pointers { b"version https://git-lfs.github.com/spec/v1\noid sha256:00\nsize 9\n" } else { &[7u8; 4096] };
                std::fs::write(dir.join(m), body).unwrap();
            }
            Ok(())
        }
    }

    fn home(n: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("omarchyvr-hm-{n}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn pinned_commit_matches_the_lock_file() {
        let lock = include_str!("../../../../installer/components.lock");
        assert!(lock.contains(&format!("HANDMODELS_URL={URL}")));
        assert!(lock.contains(&format!("HANDMODELS_COMMIT={COMMIT}")));
        assert!(lock.contains(&format!("HANDMODELS_DIR={DIR}")));
    }

    #[test]
    fn state_tells_missing_incomplete_and_installed() {
        let h = home("state");
        let d = dir_of(&h);
        assert_eq!(state(&d), State::Missing);
        std::fs::create_dir_all(&d).unwrap();
        assert!(matches!(state(&d), State::Incomplete(w) if w.contains("missing")));
        for m in MODELS {
            std::fs::write(d.join(m), "version https://git-lfs.github.com/spec/v1\noid sha256:aa\nsize 5\n").unwrap();
        }
        assert!(matches!(state(&d), State::Incomplete(w) if w.contains("not downloaded completely")));
        for m in MODELS {
            std::fs::write(d.join(m), vec![1u8; 5000]).unwrap();
        }
        assert_eq!(state(&d), State::Installed);
    }

    #[test]
    fn download_installs_and_reports_steps() {
        let h = home("ok");
        let mut g = Mock::ok();
        let mut said = Vec::new();
        download(&h, &mut g, "https://example.invalid/m.git", "abc", &mut |l| said.push(l.to_string())).unwrap();
        assert_eq!(state(&dir_of(&h)), State::Installed);
        assert_eq!(g.calls, ["clone https://example.invalid/m.git", "checkout abc", "pull"]);
        assert!(said[0].starts_with("Step 1 of 3") && said.last().unwrap().contains("Camera hands switch is available"));
        assert!(!h.join(".local/share/monado/hand-tracking-models.part").exists());
        // second run: nothing to do
        let mut g2 = Mock::ok();
        download(&h, &mut g2, "u", "c", &mut |_| {}).unwrap();
        assert!(g2.calls.is_empty());
    }

    #[test]
    fn missing_lfs_or_git_is_said_plainly_and_changes_nothing() {
        let h = home("nolfs");
        let mut g = Mock { git: true, lfs: false, ..Default::default() };
        let e = download(&h, &mut g, "u", "c", &mut |_| {}).unwrap_err();
        assert!(e.contains("git-lfs is not installed") && e.contains("Nothing was changed"), "{e}");
        assert!(g.calls.is_empty());
        assert!(!h.join(".local").exists());
        let mut g = Mock::default();
        assert!(download(&h, &mut g, "u", "c", &mut |_| {}).unwrap_err().contains("git is not installed"));
    }

    #[test]
    fn a_failed_download_leaves_nothing_half_installed() {
        for at in ["clone", "pull"] {
            let h = home(at);
            let mut g = Mock { git: true, lfs: true, fail_at: Some(at), ..Default::default() };
            let e = download(&h, &mut g, "u", "c", &mut |_| {}).unwrap_err();
            assert!(e.contains("internet connection"), "{e}");
            assert_eq!(state(&dir_of(&h)), State::Missing, "{at}");
            assert!(!h.join(".local/share/monado/hand-tracking-models.part").exists(), "{at}");
        }
        // pointer files only: refused, cleaned up
        let h = home("ptr");
        let mut g = Mock { pointers: true, ..Mock::ok() };
        assert!(download(&h, &mut g, "u", "c", &mut |_| {}).unwrap_err().contains("not complete"));
        assert_eq!(state(&dir_of(&h)), State::Missing);
    }

    #[test]
    fn a_foreign_folder_is_left_alone_and_an_unfinished_clone_is_continued() {
        let h = home("foreign");
        let d = dir_of(&h);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("mine.txt"), "x").unwrap();
        let e = download(&h, &mut Mock::ok(), "u", "c", &mut |_| {}).unwrap_err();
        assert!(e.contains("left alone"), "{e}");
        assert!(d.join("mine.txt").exists());

        let h = home("resume");
        let d = dir_of(&h);
        std::fs::create_dir_all(d.join(".git")).unwrap();
        let mut g = Mock::ok();
        download(&h, &mut g, "u", "c", &mut |_| {}).unwrap();
        assert_eq!(g.calls, ["checkout c", "pull"]);
        assert_eq!(state(&d), State::Installed);
    }

    #[test]
    fn the_cli_wants_the_consent_flag() {
        let h = home("cli");
        assert!(run("download", false, &h).unwrap_err().contains("--yes"));
        assert!(!h.join(".local").exists());
        assert!(run("status", false, &h).is_ok());
        assert!(run("bogus", true, &h).is_err());
    }
}
