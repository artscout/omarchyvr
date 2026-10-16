//! Opening the TUI tools (`pimax-eye watch`, `pimax-eye ipd --watch`, `pimax-lenstest`, `run_slam_view.sh`, ...) in the
//! user's terminal: `$TERMINAL` if it is on PATH, else foot, alacritty, kitty. The tool runs in `bash -c` with a footer
//! that keeps the window open until Enter, so the result stays readable. The terminal is started detached (own
//! session); the app never signals it afterwards.
use crate::cmd::{quote, Cmd};
use std::path::{Path, PathBuf};

/// The terminals tried after `$TERMINAL`, in order.
pub const FALLBACKS: [&str; 3] = ["foot", "alacritty", "kitty"];

/// Find a program on PATH (absolute paths are taken as they are).
pub fn which(name: &str, path_var: &str, is_file: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    if name.contains('/') {
        return is_file(Path::new(name)).then(|| PathBuf::from(name));
    }
    path_var
        .split(':')
        .filter(|d| !d.is_empty())
        .map(|d| Path::new(d).join(name))
        .find(|p| is_file(p))
}

/// Pick the terminal: `$TERMINAL` (its first word; it may carry options), else the fallbacks.
pub fn pick_terminal(
    terminal_var: Option<&str>,
    path_var: &str,
    is_file: &dyn Fn(&Path) -> bool,
) -> Option<(PathBuf, Vec<String>)> {
    if let Some(t) = terminal_var.map(str::trim).filter(|t| !t.is_empty()) {
        let mut w = t.split_whitespace();
        if let Some(first) = w.next() {
            if let Some(p) = which(first, path_var, is_file) {
                return Some((p, w.map(str::to_string).collect()));
            }
        }
    }
    FALLBACKS
        .iter()
        .find_map(|f| which(f, path_var, is_file).map(|p| (p, Vec::new())))
}

/// The shell text run in the terminal: a title line, the tool, then "press Enter to close".
pub fn wrap_script(title: &str, script: &str) -> String {
    format!(
        "printf '\\033]0;%s\\007' {t}; echo {t}; echo; {script}\nrc=$?; echo; echo \"[{title_plain} finished, exit code $rc] press Enter to close\"; read -r _\n",
        t = quote(&format!("Pimax: {title}")),
        title_plain = title.replace(['"', '$', '`', '\\'], "")
    )
}

/// The full command that opens `script` in `terminal` (with its extra options).
pub fn terminal_cmd(terminal: &Path, extra: &[String], title: &str, script: &str) -> Cmd {
    let name = terminal.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let body = wrap_script(title, script);
    let mut args: Vec<String> = extra.to_vec();
    let full_title = format!("Pimax: {title}");
    match name.as_str() {
        // command and arguments follow directly
        "foot" => {
            args.push(format!("--title={full_title}"));
            args.extend(["bash".into(), "-c".into(), body]);
        }
        "kitty" => {
            args.extend(["--title".into(), full_title]);
            args.extend(["bash".into(), "-c".into(), body]);
        }
        "xdg-terminal-exec" => {
            args.extend(["bash".into(), "-c".into(), body]);
        }
        // everything else takes -e (alacritty, ghostty, xterm, konsole, gnome-terminal's -- is not supported)
        _ => {
            if name == "alacritty" {
                args.extend(["--title".into(), full_title]);
            }
            if !args.iter().any(|a| a == "-e") {
                args.push("-e".into());
            }
            args.extend(["bash".into(), "-c".into(), body]);
        }
    }
    Cmd::with_args(terminal.display().to_string(), args)
}

/// Start the terminal detached (own session, no stdin, output to `log`). The tool script is validated first.
pub fn launch(cmd: &Cmd, script_for_check: &str, log: &Path) -> Result<u32, String> {
    crate::safety::check(&Cmd::shell(script_for_check))?;
    // the terminal and its options ($TERMINAL may carry some) too
    crate::safety::check(cmd)?;
    spawn_detached(cmd, log)
}

/// Open a root fix (`pkexec /bin/sh -c ...`, checked with the root rules) in a terminal: without a polkit
/// authentication agent, pkexec asks for the password there.
pub fn launch_root(terminal: &Path, extra: &[String], title: &str, root: &Cmd, log: &Path) -> Result<u32, String> {
    if !root.root {
        return Err("not a root command".into());
    }
    crate::safety::check(root)?;
    crate::safety::check(&Cmd::with_args(terminal.display().to_string(), extra.to_vec()))?;
    let script = format!("echo 'Runs as root (polkit asks for your password):'; echo {q}; echo; {cmd}", q = quote(&root.display()), cmd = root.display());
    spawn_detached(&terminal_cmd(terminal, extra, title, &script), log)
}

fn spawn_detached(cmd: &Cmd, log: &Path) -> Result<u32, String> {
    let out = std::fs::File::create(log).map_err(|e| format!("{}: {e}", log.display()))?;
    let err = out.try_clone().map_err(|e| e.to_string())?;
    let mut c = std::process::Command::new(&cmd.program);
    c.args(&cmd.args)
        .envs(cmd.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(std::process::Stdio::null())
        .stdout(out)
        .stderr(err);
    for k in &cmd.env_remove {
        c.env_remove(k);
    }
    if let Some(d) = &cmd.cwd {
        c.current_dir(d);
    }
    // SAFETY: setsid(2) is async-signal-safe; the terminal gets its own session so it survives the app.
    unsafe {
        use std::os::unix::process::CommandExt;
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let child = c.spawn().map_err(|e| format!("cannot start {}: {e}", cmd.program))?;
    let pid = child.id();
    // reap it in the background so it does not stay a zombie of the app
    std::thread::spawn(move || {
        let mut child = child;
        let _ = child.wait();
    });
    Ok(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fs(files: &'static [&'static str]) -> impl Fn(&Path) -> bool {
        move |p: &Path| files.iter().any(|f| Path::new(f) == p)
    }

    #[test]
    fn terminal_choice_order() {
        let have = fs(&["/usr/bin/foot", "/usr/bin/xdg-terminal-exec", "/usr/bin/kitty"]);
        let path = "/usr/local/bin:/usr/bin";
        assert_eq!(
            pick_terminal(Some("xdg-terminal-exec"), path, &have),
            Some((PathBuf::from("/usr/bin/xdg-terminal-exec"), vec![]))
        );
        // $TERMINAL with options, or not installed -> fallbacks in order
        assert_eq!(
            pick_terminal(Some("kitty --single-instance"), path, &have),
            Some((PathBuf::from("/usr/bin/kitty"), vec!["--single-instance".to_string()]))
        );
        assert_eq!(pick_terminal(Some("wezterm"), path, &have).unwrap().0, PathBuf::from("/usr/bin/foot"));
        assert_eq!(pick_terminal(None, path, &have).unwrap().0, PathBuf::from("/usr/bin/foot"));
        let only_kitty = fs(&["/usr/bin/kitty"]);
        assert_eq!(pick_terminal(Some(""), path, &only_kitty).unwrap().0, PathBuf::from("/usr/bin/kitty"));
        assert_eq!(pick_terminal(None, path, &fs(&[])), None);
    }

    #[test]
    fn argument_conventions() {
        let s = "pimax-eye watch";
        let c = terminal_cmd(Path::new("/usr/bin/foot"), &[], "Eye view", s);
        assert_eq!(c.args[0], "--title=Pimax: Eye view");
        assert_eq!(c.args[1..3], ["bash".to_string(), "-c".to_string()]);
        assert!(c.args[3].contains("pimax-eye watch\nrc=$?"));
        assert!(c.args[3].contains("read -r _"));
        let c = terminal_cmd(Path::new("/usr/bin/xdg-terminal-exec"), &[], "IPD", s);
        assert_eq!(c.args[0], "bash");
        let c = terminal_cmd(Path::new("/usr/bin/alacritty"), &[], "IPD", s);
        assert_eq!(c.args[..4], ["--title".to_string(), "Pimax: IPD".to_string(), "-e".to_string(), "bash".to_string()]);
        let c = terminal_cmd(Path::new("/usr/bin/xterm"), &["-fa".into(), "Mono".into()], "IPD", s);
        assert_eq!(c.args[..4], ["-fa".to_string(), "Mono".to_string(), "-e".to_string(), "bash".to_string()]);
        let c = terminal_cmd(Path::new("/usr/bin/kitty"), &[], "IPD", s);
        assert_eq!(c.args[..3], ["--title".to_string(), "Pimax: IPD".to_string(), "bash".to_string()]);
    }

    #[test]
    fn root_terminal_needs_a_checked_root_command() {
        let log = std::env::temp_dir().join(format!("pimax-term-root-{}.log", std::process::id()));
        let user = Cmd::new("setcap", &["cap_sys_nice+ep", "/x"]);
        assert!(launch_root(Path::new("/nonexistent/foot"), &[], "x", &user, &log).is_err());
        let bad = crate::cmd::pkexec_cmd(&["sudo pacman -S x".to_string()]);
        assert!(launch_root(Path::new("/nonexistent/foot"), &[], "x", &bad, &log).unwrap_err().contains("pacman"));
        assert!(!log.exists());
    }

    #[test]
    fn unsafe_tool_scripts_are_not_launched() {
        let c = terminal_cmd(Path::new("/usr/bin/foot"), &[], "x", "pkill -f monado");
        let log = std::env::temp_dir().join(format!("pimax-term-test-{}.log", std::process::id()));
        assert!(launch(&c, "pkill -f monado", &log).is_err());
        assert!(!log.exists(), "refused before anything was created");
    }
}
