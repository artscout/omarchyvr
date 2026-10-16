//! Commands the app may run, how they are shown, and the root path (`pkexec`).
//!
//! Root is only ever run by the user (LESSONS row 21): a root fix is shown as the documented `sudo ...` lines, with
//! Copy, and a "Run with pkexec" button that the user has to confirm. [`pkexec_cmd`] turns the shown lines into exactly
//! one `pkexec /bin/sh -c '...'` call; nothing is elevated silently and the app never calls `sudo` itself.
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

static DRY_RUN: AtomicBool = AtomicBool::new(false);

/// Demo mode and the UI tests: no process is started, signalled or opened anywhere in the app, whatever the code
/// path (jobs, terminals, SIGINT, "open folder"). Read-only checks still run.
pub fn set_dry_run(on: bool) {
    DRY_RUN.store(on, Ordering::SeqCst);
}

/// See [`set_dry_run`].
pub fn dry_run() -> bool {
    DRY_RUN.load(Ordering::SeqCst)
}

/// Refuse while in dry-run mode.
pub fn allow_start(what: &str) -> Result<(), String> {
    if dry_run() {
        Err(format!("dry run: not starting {what}"))
    } else {
        Ok(())
    }
}

/// A process to start.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Cmd {
    /// Program (absolute path or a name on PATH).
    pub program: String,
    /// Arguments.
    pub args: Vec<String>,
    /// Extra environment.
    pub env: Vec<(String, String)>,
    /// Variables removed from the inherited environment.
    pub env_remove: Vec<String>,
    /// Working directory.
    pub cwd: Option<PathBuf>,
    /// Runs as root through pkexec (validated against the root rules).
    pub root: bool,
}

impl Cmd {
    /// `program args...`.
    pub fn new(program: impl Into<String>, args: &[&str]) -> Cmd {
        Cmd {
            program: program.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    /// From owned arguments.
    pub fn with_args(program: impl Into<String>, args: Vec<String>) -> Cmd {
        Cmd {
            program: program.into(),
            args,
            ..Default::default()
        }
    }

    /// `/bin/sh -c script` (user commands only; the script is validated like any command).
    pub fn shell(script: &str) -> Cmd {
        Cmd::new("/bin/sh", &["-c", script])
    }

    /// Add an environment variable.
    pub fn env(mut self, k: &str, v: impl Into<String>) -> Cmd {
        self.env.retain(|(n, _)| n != k);
        self.env.push((k.to_string(), v.into()));
        self
    }

    /// Set the working directory.
    pub fn cwd(mut self, d: impl Into<PathBuf>) -> Cmd {
        self.cwd = Some(d.into());
        self
    }

    /// The command as one shell line (what the user would type).
    pub fn display(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(d) = &self.cwd {
            parts.push(format!("cd {} &&", quote(&d.display().to_string())));
        }
        for (k, v) in &self.env {
            parts.push(format!("{k}={}", quote(v)));
        }
        parts.push(quote(&self.program));
        parts.extend(self.args.iter().map(|a| quote(a)));
        parts.join(" ")
    }

    /// Program, arguments and environment as one text (for the safety checks).
    pub fn text(&self) -> String {
        let mut s = String::new();
        for (k, v) in &self.env {
            s += &format!("{k}={v} ");
        }
        s += &self.program;
        for a in &self.args {
            s.push(' ');
            s += a;
        }
        s
    }
}

/// Quote for a POSIX shell when needed (`'it'\''s'`).
pub fn quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@%^".contains(c))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Remove `sudo ` from documented root lines (`sudo setcap ...`, `echo x | sudo tee f`) for running them as root.
pub fn strip_sudo(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    loop {
        match rest.find("sudo ") {
            Some(i) => {
                let at_word = i == 0 || {
                    let prev = rest[..i].chars().last().unwrap_or(' ');
                    prev.is_whitespace() || "|&;(".contains(prev)
                };
                out += &rest[..i];
                if !at_word {
                    out += "sudo ";
                }
                rest = &rest[i + 5..];
            }
            None => {
                out += rest;
                return out;
            }
        }
    }
}

/// The documented root lines as one script (`set -e`, one line each, `sudo` removed).
pub fn root_script(lines: &[String]) -> String {
    let mut s = String::from("set -e\n");
    for l in lines {
        let l = strip_sudo(l.trim());
        if !l.is_empty() {
            s += &l;
            s.push('\n');
        }
    }
    s
}

/// `/usr/bin/pkexec /bin/sh -c '<script>'`: one polkit prompt for the whole fix.
pub fn pkexec_cmd(lines: &[String]) -> Cmd {
    Cmd {
        program: crate::safety::PKEXEC.into(),
        args: vec!["/bin/sh".into(), "-c".into(), root_script(lines)],
        root: true,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_and_display() {
        assert_eq!(quote("plain/path-1.2"), "plain/path-1.2");
        assert_eq!(quote("two words"), "'two words'");
        assert_eq!(quote("it's"), "'it'\\''s'");
        assert_eq!(quote(""), "''");
        let c = Cmd::new("omarchyvr", &["desk", "start", "--fps", "72"]).env("OXR_DEBUG_IPD_MM", "65.9");
        assert_eq!(c.display(), "OXR_DEBUG_IPD_MM=65.9 omarchyvr desk start --fps 72");
        let c = Cmd::new("ninja", &[]).cwd("/home/u/src/monado-eye/build");
        assert_eq!(c.display(), "cd /home/u/src/monado-eye/build && ninja");
    }

    #[test]
    fn sudo_is_removed_only_as_a_word() {
        assert_eq!(strip_sudo("sudo setcap cap_sys_nice+ep /x"), "setcap cap_sys_nice+ep /x");
        assert_eq!(
            strip_sudo("echo 'options x' | sudo tee /etc/modprobe.d/nvidia-pimax.conf"),
            "echo 'options x' | tee /etc/modprobe.d/nvidia-pimax.conf"
        );
        assert_eq!(strip_sudo("pseudo x"), "pseudo x");
        assert_eq!(
            strip_sudo("sudo dkms build nvidia/610 --force && sudo dkms install nvidia/610 --force"),
            "dkms build nvidia/610 --force && dkms install nvidia/610 --force"
        );
    }

    #[test]
    fn pkexec_runs_exactly_the_shown_lines() {
        let c = pkexec_cmd(&["sudo setcap cap_sys_nice+ep /a/monado-service".to_string()]);
        assert!(c.root);
        assert_eq!(c.program, "/usr/bin/pkexec");
        assert_eq!(c.args[..2], ["/bin/sh".to_string(), "-c".to_string()]);
        assert_eq!(c.args[2], "set -e\nsetcap cap_sys_nice+ep /a/monado-service\n");
        assert!(c.display().starts_with("/usr/bin/pkexec /bin/sh -c 'set -e"));
    }
}
