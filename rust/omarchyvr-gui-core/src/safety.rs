//! The hard rules, checked on every command right before it is started (the job runner and the terminal launcher call
//! [`check`]; a refused command never runs).
//!
//! * Never kill by name (`pkill`, `killall`, `fuser -k`) and never SIGKILL by hand: Monado is stopped only through
//!   `omarchyvr vr stop --grace 30` / `omarchyvr desk stop`, other processes only by PID with SIGINT (LESSONS row 1).
//! * Never force a DRM hotplug or write sysfs/debugfs, never flash firmware (row 10, INVENTORY 6.2).
//! * Never send eye calibration ops or unapproved replays (row 11); the app never talks HID itself.
//! * Never launch Steam games (row 20); never edit Hyprland config; never run WayVR headless renders (row 9).
//! * User commands never elevate and never touch system config (`/etc`, `/usr/src`, `/boot`, `/sys`, DKMS, initramfs,
//!   system units). Root happens only through [`crate::cmd::pkexec_cmd`] (`/usr/bin/pkexec /bin/sh -c <lines>`) with
//!   the documented fix lines; those may only use an allowlist of programs and write only documented destinations
//!   (row 21). The one exception is the one-click cap fix [`crate::caps::pkexec_setcap`]
//!   (`/usr/bin/pkexec /usr/bin/setcap cap_sys_nice+ep <absolute .../monado-service>`, nothing else).
//! * Scripts the app runs contain no command substitution, backticks or `eval`, so every command in them is visible to
//!   these checks; nested `sh -c` / `bash -c` scripts are checked recursively, and wrappers (`env`, `timeout`, `flock`,
//!   `xargs`, `nohup`, ...) and variable assignments are looked through.
//!
//! This is a guard against mistakes in the app's own command builders (every builder is tested against it), not a
//! sandbox for arbitrary input.
use crate::cmd::Cmd;

/// The pkexec the app uses (absolute: a shim earlier in PATH must not see the password).
pub const PKEXEC: &str = "/usr/bin/pkexec";

/// Validate a command (user or root).
pub fn check(cmd: &Cmd) -> Result<(), String> {
    if cmd.root {
        if cmd.program != PKEXEC {
            return Err(format!("root commands only run through {PKEXEC}"));
        }
        let script = match cmd.args.as_slice() {
            [sh, c, script] if sh == "/bin/sh" && c == "-c" => script,
            // the one-click cap fix (crate::caps::pkexec_setcap): this exact shape, on a monado-service only
            [prog, cap, target] if prog == crate::caps::SETCAP && cap == "cap_sys_nice+ep" => {
                let ok = target.starts_with('/')
                    && target.ends_with("/monado-service")
                    && !target.split('/').any(|c| c == "..")
                    && !target.chars().any(char::is_control);
                return if ok { Ok(()) } else { Err("setcap only on an absolute monado-service path".into()) };
            }
            _ => return Err(format!("pkexec is only used as `{PKEXEC} /bin/sh -c <documented fix lines>`")),
        };
        check_root_script(script)
    } else {
        check_common(&cmd.text())?;
        for (k, v) in &cmd.env {
            check_assignment(k, v, false)?;
        }
        let mut w = vec![cmd.program.clone()];
        w.extend(cmd.args.iter().cloned());
        check_user_words(&w, 0)
    }
}

fn basename(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// Rules for every command text (substring rules: names, paths, flags that are never right).
pub fn check_common(text: &str) -> Result<(), String> {
    let lower = text.to_ascii_lowercase();
    let has = |s: &str| lower.contains(s);
    if has("pkill") || has("killall") {
        return Err("killing by name is forbidden: processes are stopped by PID only (LESSONS row 1)".into());
    }
    if has("/sys/class/drm") || has("edid_override") || has("/sys/kernel/debug") || has("/sys/") || has("/proc/sys") {
        return Err("DRM hotplug and sysfs/debugfs/procfs writes are forbidden (they desync Hyprland and wedge DP-1)".into());
    }
    if has("fwupd") || has("dfu-util") || has("firmware-update") || words(&lower).iter().any(|w| w == "flash") {
        return Err("the app never flashes firmware".into());
    }
    if has("steam://") || has("-applaunch") || has("rungameid") || has("steam-runtime-launch") {
        return Err("the app never launches Steam games (LESSONS row 20)".into());
    }
    if has("--with-cal") || has("--force-replay") || has("video-on") {
        return Err("eye calibration ops, forced replays and panel commands need the user's approval: not from the app (LESSONS row 11)".into());
    }
    if has(".config/hypr") || has("hyprctl keyword") || has("hyprctl reload") {
        return Err("the app never changes the Hyprland config".into());
    }
    if (has("wayvr") && has("--headless")) || has("run_hands_composite") {
        return Err("WayVR headless/offline renders are never run from the app (their Xid 31 faults, LESSONS row 9)".into());
    }
    if has("rm -rf") || has("rm -fr") || has("rm -r ") {
        return Err("recursive deletes are not run by the app".into());
    }
    Ok(())
}

/// Scripts must not hide commands from the checks: no `$( )`, backticks, process substitution or `eval`
/// (arithmetic `$(( ))` is fine).
pub fn check_visible(script: &str) -> Result<(), String> {
    let b = script.as_bytes();
    for i in 0..b.len() {
        if b[i] == b'`' {
            return Err("command substitution with backticks hides commands from the safety checks".into());
        }
        if b[i] == b'$' && b.get(i + 1) == Some(&b'(') && b.get(i + 2) != Some(&b'(') {
            return Err("command substitution $( ) hides commands from the safety checks".into());
        }
        if (b[i] == b'<' || b[i] == b'>') && b.get(i + 1) == Some(&b'(') {
            return Err("process substitution hides commands from the safety checks".into());
        }
    }
    if words(script).iter().any(|w| w == "eval") {
        return Err("eval hides commands from the safety checks".into());
    }
    Ok(())
}

fn is_assignment(w: &str) -> bool {
    match w.split_once('=') {
        Some((k, _)) => !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !k.starts_with(|c: char| c.is_ascii_digit()),
        None => false,
    }
}

/// Variables that change which program runs or what it loads are never set by the app.
fn check_assignment(k: &str, _v: &str, root: bool) -> Result<(), String> {
    let bad = k == "IFS" || k == "BASH_ENV" || k == "ENV" || k.starts_with("LD_") || (root && k == "PATH");
    if bad {
        return Err(format!("`{k}=` is not set by the app"));
    }
    Ok(())
}

const KEYWORDS: [&str; 13] = ["if", "then", "else", "elif", "do", "while", "until", "!", "{", "}", "(", ")", "time"];

/// The words of the command a segment really runs: assignments, shell keywords and wrappers (`env`, `exec`,
/// `command`, `nohup`, `setsid`, `nice`, `stdbuf`, `timeout`, `flock`, `xargs`) removed. A `flock ... -c CMD` or an
/// `env -S CMD` yields the inner command text as the second item.
pub fn command_words(w: &[String]) -> (Vec<String>, Option<String>) {
    let mut i = 0;
    let n = w.len();
    loop {
        let Some(x) = w.get(i) else { return (Vec::new(), None) };
        let name = basename(x);
        if KEYWORDS.contains(&x.as_str()) || is_assignment(x) {
            i += 1;
            continue;
        }
        match name {
            "exec" | "command" | "builtin" | "nohup" | "setsid" => i += 1,
            "env" => {
                i += 1;
                while let Some(o) = w.get(i) {
                    if o == "-S" || o == "--split-string" {
                        return (Vec::new(), w.get(i + 1).cloned());
                    }
                    if matches!(o.as_str(), "-u" | "--unset" | "-C" | "--chdir") {
                        i += 2;
                    } else if o.starts_with('-') || is_assignment(o) {
                        i += 1;
                    } else {
                        break;
                    }
                }
            }
            "nice" | "stdbuf" | "xargs" => {
                i += 1;
                while let Some(o) = w.get(i) {
                    if !o.starts_with('-') {
                        break;
                    }
                    let takes_value = matches!(o.as_str(), "-n" | "-I" | "-P" | "-d" | "-L" | "-s" | "-i" | "-o" | "-e" | "-a" | "-E");
                    i += if takes_value { 2 } else { 1 };
                }
            }
            "timeout" => {
                i += 1;
                while let Some(o) = w.get(i) {
                    if o == "-s" || o == "--signal" || o == "-k" || o == "--kill-after" {
                        i += 2;
                    } else if o.starts_with('-') {
                        i += 1;
                    } else {
                        break;
                    }
                }
                i += 1; // the duration
            }
            "flock" => {
                i += 1;
                while let Some(o) = w.get(i) {
                    if matches!(o.as_str(), "-w" | "--timeout" | "-E" | "--conflict-exit-code") {
                        i += 2;
                    } else if o.starts_with('-') {
                        i += 1;
                    } else {
                        break;
                    }
                }
                i += 1; // the lock file
                if w.get(i).is_some_and(|x| x == "-c" || x == "--command") {
                    return (Vec::new(), w.get(i + 1).cloned());
                }
            }
            _ => return (w[i.min(n)..].to_vec(), None),
        }
    }
}

/// Is this `kill` argument list a SIGKILL?
fn kill_is_sigkill(args: &[String]) -> bool {
    let is_kill = |s: &str| {
        let u = s.trim_start_matches('-').to_ascii_uppercase();
        u == "9" || u == "KILL" || u == "SIGKILL"
    };
    args.iter().enumerate().any(|(i, a)| {
        let au = a.to_ascii_uppercase();
        (a.starts_with('-') && is_kill(a) && a != "-")
            || ((a == "-s" || a == "-n" || a == "--signal") && args.get(i + 1).is_some_and(|v| is_kill(v)))
            || au.strip_prefix("--SIGNAL=").is_some_and(is_kill)
            || au.strip_prefix("-S").is_some_and(|r| !r.is_empty() && is_kill(r))
            || au.strip_prefix("-N").is_some_and(|r| !r.is_empty() && is_kill(r))
    })
}

fn check_program_rules(cw: &[String]) -> Result<(), String> {
    let Some(prog) = cw.first() else { return Ok(()) };
    let name = basename(prog);
    let args = &cw[1..];
    if name == "pkill" || name == "killall" {
        return Err("killing by name is forbidden (LESSONS row 1)".into());
    }
    if name == "kill" && kill_is_sigkill(args) {
        return Err("SIGKILL is never sent by the app (SIGINT by PID only; Monado via omarchyvr vr stop)".into());
    }
    if name == "fuser" && args.iter().any(|a| a.starts_with('-') && !a.starts_with("--") && a.contains('k') || a == "--kill") {
        return Err("fuser -k kills by file: forbidden".into());
    }
    Ok(())
}

fn check_user_words(w: &[String], depth: usize) -> Result<(), String> {
    if depth > 4 {
        return Err("too deeply nested shell scripts".into());
    }
    for a in w {
        if is_assignment(a) {
            let (k, v) = a.split_once('=').unwrap_or((a, ""));
            check_assignment(k, v, false)?;
        }
    }
    let (cw, inner) = command_words(w);
    if let Some(script) = inner {
        return check_user_script_depth(&script, depth + 1);
    }
    let Some(prog) = cw.first() else { return Ok(()) };
    let name = basename(prog);
    if matches!(name, "sudo" | "pkexec" | "su" | "doas" | "run0") {
        return Err(format!("`{name}` in a user command: root fixes are shown and run with pkexec by the user only"));
    }
    check_program_rules(&cw)?;
    if name == "systemctl" && !cw.iter().any(|a| a == "--user") {
        return Err("system units are not touched by the app (only systemctl --user)".into());
    }
    if name == "systemd-run" && !(cw.iter().any(|a| a == "--user") && cw.iter().any(|a| a == "--scope")) {
        return Err("systemd-run only as `systemd-run --user --scope` (the app's own transient scope)".into());
    }
    for a in w {
        let l = a.to_ascii_lowercase();
        for p in ["/etc/", "/usr/src/", "/boot", "/usr/local/lib/omarchyvr", "/usr/local/lib/pimax"] {  // names:keep
            if l.starts_with(p) || l.contains(&format!(" {p}")) || l.contains(&format!("={p}")) {
                return Err(format!("`{p}` in a user command: system configuration is only changed by root fixes (pkexec, shown first)"));
            }
        }
    }
    if matches!(name, "mkinitcpio" | "limine-mkinitcpio" | "dkms" | "modprobe" | "insmod" | "rmmod" | "udevadm" | "setcap") {
        return Err(format!("`{name}` is a root fix: shown and run with pkexec by the user only"));
    }
    // a nested script (sh -c / bash -c) is checked like a script
    if matches!(name, "sh" | "bash" | "dash" | "zsh") {
        if let Some(i) = cw.iter().position(|a| a == "-c") {
            if let Some(s) = cw.get(i + 1) {
                return check_user_script_depth(s, depth + 1);
            }
        }
    }
    // `sudo` & co. hidden behind a path somewhere in the arguments of a wrapper we do not know
    for a in &cw[1..] {
        if matches!(basename(a), "sudo" | "pkexec" | "doas" | "run0") && a.contains('/') {
            return Err(format!("`{a}` in a user command"));
        }
    }
    Ok(())
}

fn check_user_script_depth(script: &str, depth: usize) -> Result<(), String> {
    check_common(script)?;
    check_visible(script)?;
    for seg in segments(script) {
        check_user_words(&words(&seg), depth)?;
    }
    Ok(())
}

/// Validate a shell script the app runs as the user (terminal tools).
pub fn check_user_script(script: &str) -> Result<(), String> {
    check_user_script_depth(script, 0)
}

/// Where root fixes may write (the documented install locations).
const ROOT_DESTINATIONS: [&str; 10] = [
    "/etc/udev/rules.d/70-pimax.rules",
    "/etc/pacman.d/hooks/65-nvidia-pimax-patch.hook",
    "/etc/pacman.d/hooks/75-nvidia-pimax-verify.hook",
    "/usr/local/lib/omarchyvr/",
    "/usr/local/lib/pimax/", // machines installed before the rename keep this directory until they reinstall  // names:keep
    "/etc/modprobe.d/nvidia-pimax.conf",
    "/etc/pimax/",
    "/etc/mkinitcpio.conf.d/pimax.conf",
    "/mnt/shared",
    "/usr/src/nvidia-",
];

fn root_dest_ok(p: &str) -> bool {
    !p.contains("..") && ROOT_DESTINATIONS.iter().any(|d| p == d.trim_end_matches('/') || p.starts_with(d))
}

/// Non-option arguments (skipping the values of the given value options).
fn operands<'a>(args: &'a [String], value_opts: &[&str]) -> Vec<&'a String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if value_opts.contains(&a.as_str()) {
            i += 2;
            continue;
        }
        if !a.starts_with('-') {
            out.push(a);
        }
        i += 1;
    }
    out
}

/// Validate the script of a pkexec fix.
pub fn check_root_script(script: &str) -> Result<(), String> {
    check_root_depth(script, 0)
}

fn check_root_depth(script: &str, depth: usize) -> Result<(), String> {
    if depth > 3 {
        return Err("too deeply nested root scripts".into());
    }
    check_common(script)?;
    check_visible(script)?;
    for w in words(&script.to_ascii_lowercase()) {
        if matches!(w.as_str(), "reboot" | "poweroff" | "shutdown" | "halt") {
            return Err("the app never reboots or powers off the machine; do it yourself".into());
        }
    }
    for seg in segments(script) {
        let w = words(&seg);
        let mut i = 0;
        while i < w.len() && KEYWORDS.contains(&w[i].as_str()) {
            i += 1;
        }
        match w.get(i).map(String::as_str) {
            None | Some("done") | Some("fi") => continue,
            // `for p in a b c` is a word list, not a command
            Some("for") => continue,
            _ => {}
        }
        while let Some(x) = w.get(i) {
            if x == "sudo" || x == "env" {
                i += 1;
            } else if is_assignment(x) {
                let (k, v) = x.split_once('=').unwrap_or((x, ""));
                check_assignment(k, v, true)?;
                i += 1;
            } else {
                break;
            }
        }
        let rest = &w[i..];
        let Some(prog) = rest.first() else { continue };
        let name = basename(prog);
        let args = &rest[1..];
        check_program_rules(rest)?;
        match name {
            "set" if args.iter().all(|a| a.starts_with('-') && a.len() <= 4) => {}
            "true" | "echo" | "udevadm" | "dkms" | "mkinitcpio" | "limine-mkinitcpio" | "modprobe" => {}
            "bash" | "sh" => match args.iter().position(|a| a == "-c").and_then(|i| args.get(i + 1)) {
                Some(inner) => check_root_depth(inner, depth + 1)?,
                None => return Err("a root shell is only allowed with -c and a checked script".into()),
            },
            "sed" => {
                if !operands(args, &["-e", "-f"]).iter().all(|o| o.as_str() == "/etc/modprobe.d/nvidia-pimax.conf" || !o.starts_with('/')) || !seg.contains("/etc/modprobe.d/nvidia-pimax.conf") {
                    return Err("sed as root only edits /etc/modprobe.d/nvidia-pimax.conf".into());
                }
            }
            "mount" => {
                if !seg.contains("UUID=") || !args.windows(2).any(|p| p[0] == "-t" && p[1] == "ntfs3") {
                    return Err("mount only the shared drive: -t ntfs3 by UUID (NVMe device names swap between boots)".into());
                }
                if let Some(target) = operands(args, &["-t", "-o"]).last() {
                    if !target.starts_with("UUID=") && !root_dest_ok(target) {
                        return Err(format!("mount target {target} is not a documented one"));
                    }
                }
            }
            "tee" => {
                for o in operands(args, &[]) {
                    if !root_dest_ok(o) {
                        return Err(format!("root write to {o} is not a documented destination"));
                    }
                }
            }
            "install" | "cp" => {
                let ops = operands(args, &["-m", "-o", "-g", "-t", "-S"]);
                match ops.last() {
                    Some(d) if root_dest_ok(d) => {}
                    Some(d) => return Err(format!("root install/copy to {d} is not a documented destination")),
                    None => return Err("install/cp without a destination".into()),
                }
            }
            "mkdir" => {
                for o in operands(args, &["-m"]) {
                    if !root_dest_ok(o) {
                        return Err(format!("root mkdir {o} is not a documented destination"));
                    }
                }
            }
            "patch" => {
                let d = args.iter().position(|a| a == "-d").and_then(|i| args.get(i + 1));
                if !d.is_some_and(|d| d.starts_with("/usr/src/nvidia-") && !d.contains("..")) {
                    return Err("root patch only into /usr/src/nvidia-<version>".into());
                }
            }
            "setcap" => {
                if !args.last().is_some_and(|t| t.ends_with("/monado-service")) {
                    return Err("setcap only on a monado-service binary".into());
                }
            }
            "nvidia-patch-reapply.sh" => {
                if !(prog == "/usr/local/lib/omarchyvr/nvidia-patch-reapply.sh" || prog == "/usr/local/lib/pimax/nvidia-patch-reapply.sh" || prog.ends_with("/linux/nvidia-patch-reapply.sh")) {  // names:keep
                    return Err("the reapply script runs from /usr/local/lib/omarchyvr (or the old /usr/local/lib/pimax) or the repository's linux/ only".into());  // names:keep
                }
            }
            _ => return Err(format!("`{name}` is not one of the documented root fix programs; copy the command and run it yourself")),
        }
    }
    Ok(())
}

/// Split a shell text at unquoted `;`, `&&`, `||`, `|` and newlines; drops `#` comments.
pub fn segments(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut chars = text.chars().peekable();
    let mut at_word_start = true;
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                cur.push(c);
                if c == q {
                    quote = None;
                } else if c == '\\' && q == '"' {
                    if let Some(n) = chars.next() {
                        cur.push(n);
                    }
                }
            }
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    cur.push(c);
                }
                '\\' => {
                    cur.push(c);
                    if let Some(n) = chars.next() {
                        cur.push(n);
                    }
                }
                '#' if at_word_start => {
                    // comment to the end of the line
                    for n in chars.by_ref() {
                        if n == '\n' {
                            break;
                        }
                    }
                    out.push(std::mem::take(&mut cur));
                    at_word_start = true;
                    continue;
                }
                ';' | '\n' | '|' | '&' => {
                    if matches!(c, '|' | '&') && chars.peek() == Some(&c) {
                        chars.next();
                    }
                    out.push(std::mem::take(&mut cur));
                }
                _ => cur.push(c),
            },
        }
        at_word_start = quote.is_none() && (c.is_whitespace() || matches!(c, ';' | '|' | '&' | '\n'));
    }
    out.push(cur);
    out.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

/// Shell-like words (quotes removed, no expansion).
pub fn words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut have = false;
    let mut quote: Option<char> = None;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) if c == q => quote = None,
            Some('"') if c == '\\' => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            Some(_) => cur.push(c),
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    have = true;
                }
                '\\' => {
                    if let Some(n) = chars.next() {
                        cur.push(n);
                        have = true;
                    }
                }
                c if c.is_whitespace() => {
                    if have || !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                        have = false;
                    }
                }
                _ => cur.push(c),
            },
        }
    }
    if have || !cur.is_empty() {
        out.push(cur);
    }
    out
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::{pkexec_cmd, Cmd};

    fn user(s: &str) -> Result<(), String> {
        check(&Cmd::shell(s))
    }

    fn root(lines: &[&str]) -> Result<(), String> {
        check(&pkexec_cmd(&lines.iter().map(|s| s.to_string()).collect::<Vec<_>>()))
    }

    fn sv(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn words_segments_and_command_words() {
        assert_eq!(words("a 'b c' \"d\\\"e\" f\\ g"), vec!["a", "b c", "d\"e", "f g"]);
        assert_eq!(
            segments("echo 'x; y' | sudo tee /f && b; c # comment; d\ne"),
            vec!["echo 'x; y'", "sudo tee /f", "b", "c", "e"]
        );
        assert_eq!(command_words(&sv(&["A=1", "B=2", "kill", "-9", "1"])).0, sv(&["kill", "-9", "1"]));
        assert_eq!(command_words(&sv(&["env", "-i", "X=1", "timeout", "-s", "TERM", "5", "nohup", "pkill", "x"])).0, sv(&["pkill", "x"]));
        assert_eq!(command_words(&sv(&["flock", "-o", "-w", "10", "/tmp/l", "pimax-eye", "watch"])).0, sv(&["pimax-eye", "watch"]));
        assert_eq!(command_words(&sv(&["flock", "/tmp/l", "-c", "kill -9 1"])).1.as_deref(), Some("kill -9 1"));
        assert_eq!(command_words(&sv(&["xargs", "-n", "1", "kill", "-KILL"])).0, sv(&["kill", "-KILL"]));
        assert_eq!(command_words(&sv(&["do", "install", "a", "b"])).0, sv(&["install", "a", "b"]));
    }

    #[test]
    fn everyday_user_commands_pass() {
        for c in [
            Cmd::new("/home/u/.local/bin/omarchyvr", &["desk", "start", "--fps", "72", "--eye", "--6dof"]).env("OXR_DEBUG_IPD_MM", "65.9"),
            Cmd::new("/home/u/.local/bin/omarchyvr", &["desk", "stop", "--grace", "30"]),
            Cmd::new("/home/u/.local/bin/omarchyvr", &["vr", "stop", "--grace", "30", "--all"]),
            Cmd::new("systemctl", &["--user", "enable", "--now", "omarchyvr-keepalived"]),
            Cmd::new("cargo", &["build", "--release", "-p", "omarchyvr-tools"]),
            Cmd::new("install", &["-m755", "/r/target/release/omarchyvr", "/home/u/.local/bin/omarchyvr"]),
            Cmd::new("chmod", &["444", "/home/u/.config/openvr/openvrpaths.vrpath"]),
            Cmd::shell("cmake -DPIMAX_CAM=ON . && ninja"),
            Cmd::shell("pimax-cam broker & B=$!; sleep 1; kill -INT \"$B\"; wait \"$B\"; kill -0 \"$B\" 2>/dev/null"),
            Cmd::shell("i=0; while [ $i -lt 50 ]; do i=$((i + 1)); done"),
            Cmd::new("flock", &["-o", "-w", "10", "/tmp/omarchyvr-headset.lock", "/r/pimax-eye", "power-on"]),
            Cmd::new("systemd-run", &["--user", "--scope", "--quiet", "--collect", "--unit", "pimax-desk-1", "--", "/home/u/.local/bin/omarchyvr", "desk", "start"]),
        ] {
            assert_eq!(check(&c), Ok(()), "{}", c.display());
        }
    }

    #[test]
    fn forbidden_commands_are_refused() {
        for bad in [
            "pkill -f monado-service",
            "pkill -x -INT monado-service",
            "killall wayvr",
            "kill -9 1234",
            "kill -KILL 1234",
            "kill -kill 1234",
            "kill -s KILL 1234",
            "kill -s 9 1234",
            "kill -n 9 1234",
            "kill --signal=KILL 1234",
            "kill -SIGKILL 1234",
            "A=1 kill -9 1",
            "env X=1 kill -9 1",
            "timeout 5 kill -9 1",
            "nohup kill -9 1",
            "xargs -n 1 kill -9",
            "if true; then kill -9 1; fi",
            "{ kill -9 1; }",
            "bash -c 'sh -c \"kill -9 1\"'",
            "flock /tmp/l -c 'kill -9 1'",
            "fuser -k /dev/video0",
            "fuser -vk /dev/video0",
            "echo off > /sys/class/drm/card1-DP-1/status",
            "echo detect | tee /sys/class/drm/card1-DP-1/status",
            "echo 1 > /sys/bus/pci/devices/0000:01:00.0/remove",
            "echo x > /proc/sys/kernel/sysrq",
            "cat x > /sys/kernel/debug/dri/1/DP-1/edid_override",
            "fwupdmgr update",
            "pimax-tool flash fw.bin",
            "steam steam://rungameid/123",
            "steam -applaunch 123",
            "pimax-eye live --with-cal",
            "omarchyvr video-on 72 40",
            "sed -i s/x/y/ ~/.config/hypr/hyprland.conf",
            "hyprctl keyword monitor DP-1,disable",
            "sudo setcap cap_sys_nice+ep /x",
            "/usr/bin/sudo setcap cap_sys_nice+ep /x",
            "pkexec setcap cap_sys_nice+ep /x",
            "/usr/bin/pkexec true",
            "env /usr/bin/doas true",
            "install -m644 70-pimax.rules /etc/udev/rules.d/70-pimax.rules",
            "dkms build nvidia/610",
            "setcap cap_sys_nice+ep /x",
            "systemctl restart display-manager",
            "systemd-run --unit x omarchyvr desk start",
            "systemd-run --user --unit x omarchyvr desk start",
            "rm -rf /home/u",
            "wayvr --openxr --headless --show",
            "analysis/tools/run_hands_composite.sh",
            "echo $(pkill x)",
            "echo `id`",
            "eval \"$CMD\"",
            "cat <(true)",
            "LD_PRELOAD=/tmp/x.so omarchyvr desk start",
        ] {
            assert!(user(bad).is_err(), "should refuse: {bad}");
        }
        assert!(check(&Cmd::new("omarchyvr", &["desk", "start"]).env("LD_PRELOAD", "/tmp/x.so")).is_err());
    }

    #[test]
    fn documented_root_fixes_pass() {
        let ok: [&[&str]; 9] = [
            &["sudo setcap cap_sys_nice+ep '/home/u/src/monado-eye/build/src/xrt/targets/service/monado-service'"],
            &[
                "sudo install -m644 '/repo/linux/70-pimax.rules' /etc/udev/rules.d/70-pimax.rules",
                "sudo udevadm control --reload",
                "sudo udevadm trigger --subsystem-match=hidraw --subsystem-match=usb --action=change",
            ],
            &["echo 'options nvidia-modeset merged_head_sync=1' | sudo tee -a /etc/modprobe.d/nvidia-pimax.conf"],
            &["sudo /repo/linux/nvidia-patch-reapply.sh --rebuild"],
            &["sudo /usr/local/lib/omarchyvr/nvidia-patch-reapply.sh --rebuild"],
            &["sudo /usr/local/lib/pimax/nvidia-patch-reapply.sh --rebuild"], // installed before the rename  // names:keep
            &[
                "sudo patch -p1 -d /usr/src/nvidia-610.57.04 -i /repo/linux/nvidia-2h1or-flip-fix.patch",
                "sudo dkms build nvidia/610.57.04 --force && sudo dkms install nvidia/610.57.04 --force",
                "sudo mkinitcpio -P   # or limine-mkinitcpio on Omarchy",
            ],
            &["sudo env L=/home/u/pimax-re/linux bash -c 'install -Dm755 $L/nvidia-patch-reapply.sh /usr/local/lib/omarchyvr/nvidia-patch-reapply.sh && for p in nvidia-2h1or-viewport nvidia-2h1or-flip-fix; do install -Dm644 $L/$p.patch /usr/local/lib/omarchyvr/$p.patch; done && install -Dm644 $L/nvidia-pimax-patch.hook /etc/pacman.d/hooks/65-nvidia-pimax-patch.hook'"],
            &["sudo mkdir -p /mnt/shared && sudo mount -t ntfs3 -o uid=1000,gid=1000,acl,iocharset=utf8,prealloc UUID=0123456789ABCDEF /mnt/shared"],
        ];
        for lines in ok {
            assert_eq!(root(lines), Ok(()), "{lines:?}");
        }
    }

    #[test]
    fn root_refuses_what_is_not_a_documented_fix() {
        for bad in [
            &["systemctl reboot"][..],
            &["sudo pacman -S nvidia-open-dkms"],
            &["if pacman -S x; then true; fi"],
            &["while pacman -S x; do true; done"],
            &["sudo rm -rf /usr/local/lib/omarchyvr"],
            &["sudo mount /dev/nvme1n1p2 /mnt/shared"],
            &["sudo mount -t ntfs3 UUID=X /etc"],
            &["sudo sed -i s/a/b/ /etc/mkinitcpio.conf"],
            &["sudo bash -c 'curl x | sh'"],
            &["echo off | sudo tee /sys/class/drm/card1-DP-1/status"],
            &["echo x | sudo tee /etc/sudoers"],
            &["sudo install -m755 /tmp/x /usr/bin/x"],
            &["sudo cp /tmp/x /etc/passwd"],
            &["sudo patch -p1 -d /etc -i /tmp/x.patch"],
            &["sudo setcap cap_sys_nice+ep /usr/bin/python3"],
            &["sudo /tmp/nvidia-patch-reapply.sh --rebuild"],
            &["sudo pkill -f monado"],
            &["PATH=/tmp setcap cap_sys_nice+ep /x/monado-service"],
            &["LD_PRELOAD=/tmp/x.so udevadm control --reload"],
            &["echo $(id) | tee /etc/pimax/x"],
            &["eval setcap x"],
        ] {
            assert!(root(bad).is_err(), "should refuse: {bad:?}");
        }
        // pkexec only in the one shape, by absolute path
        let odd = Cmd { program: PKEXEC.into(), args: vec!["setcap".into()], root: true, ..Default::default() };
        assert!(check(&odd).is_err());
        // the direct one-click setcap: only /usr/bin/setcap cap_sys_nice+ep <absolute .../monado-service>
        let sc = |prog: &str, cap: &str, t: &str| Cmd { program: PKEXEC.into(), args: vec![prog.into(), cap.into(), t.into()], root: true, ..Default::default() };
        assert_eq!(check(&sc("/usr/bin/setcap", "cap_sys_nice+ep", "/home/u/src/monado-ht/build-ht/src/xrt/targets/service/monado-service")), Ok(()));
        for bad in [
            sc("setcap", "cap_sys_nice+ep", "/a/monado-service"),
            sc("/usr/bin/setcap", "cap_sys_admin+ep", "/a/monado-service"),
            sc("/usr/bin/setcap", "cap_sys_nice+ep", "/usr/bin/python3"),
            sc("/usr/bin/setcap", "cap_sys_nice+ep", "a/monado-service"),
            sc("/usr/bin/setcap", "cap_sys_nice+ep", "/a/../usr/bin/monado-service"),
            sc("/usr/bin/setcap", "cap_sys_nice+ep", "/a/x\n/monado-service"),
        ] {
            assert!(check(&bad).is_err(), "should refuse: {bad:?}");
        }
        let rel = Cmd { program: "pkexec".into(), args: vec!["/bin/sh".into(), "-c".into(), "true".into()], root: true, ..Default::default() };
        assert!(check(&rel).is_err(), "PATH-resolved pkexec");
        let sh = Cmd { program: PKEXEC.into(), args: vec!["sh".into(), "-c".into(), "true".into()], root: true, ..Default::default() };
        assert!(check(&sh).is_err(), "only /bin/sh");
        let not_pkexec = Cmd { program: "sudo".into(), args: vec!["-c".into()], root: true, ..Default::default() };
        assert!(check(&not_pkexec).is_err());
    }
}
