//! The launcher as a pimaxd client (P6) against a fake pimaxd on a temp socket. Temp HOME, sysfs, proc and session
//! dir; nothing real is started or stopped.
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

fn snapshot(overall: &str, observe: bool) -> String {
    format!(
        r#"{{"v":1,"t":1.0,"overall":"{overall}","safe_mode":false,"safe_reason":null,"observe_only":{observe},"gpu_gate":"ok","gpu_evidence":[],"intent":{{"session":"on","by":"cli","mode":"desk"}},"components":[{{"id":"monado","wanted":true,"health":"{}","why":null}}],"next_action":null}}"#,
        if overall == "ok" { "ok" } else { "starting" }
    )
}

/// A fake pimaxd: answers `state` (down until a `set session=on`, then ok) and `set`; records every request.
fn fake_pimaxd(sock: &Path, observe: bool) -> Arc<Mutex<Vec<String>>> {
    let _ = fs::remove_file(sock);
    let l = UnixListener::bind(sock).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s2 = seen.clone();
    std::thread::spawn(move || {
        let mut on = false;
        for c in l.incoming() {
            let Ok(mut c) = c else { break };
            let mut line = String::new();
            if BufReader::new(&c).read_line(&mut line).is_err() {
                continue;
            }
            let line = line.trim().to_string();
            s2.lock().unwrap().push(line.clone());
            let reply = if line.contains(r#""cmd":"set""#) {
                on = line.contains(r#""session":"on""#);
                r#"{"ok":true,"intent":"on","toggled":[]}"#.to_string()
            } else {
                snapshot(if on { "ok" } else { "down" }, observe)
            };
            let _ = writeln!(c, "{reply}");
        }
    });
    seen
}

/// The fake socket: short, because a unix socket path has at most 107 bytes.
fn sock(root: &Path) -> PathBuf {
    let tag = root.file_name().unwrap().to_string_lossy().replace("daemon-cli-", "");
    std::env::temp_dir().join(format!("pxd{}{tag}.sock", std::process::id()))
}

fn world(tag: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("daemon-cli-{tag}"));
    let _ = fs::remove_dir_all(&root);
    for d in ["session", "run", "proc", "sys", "bin", "home"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    for (n, body) in [("journalctl", "echo 'Oct 13 23:00:00 host kernel: ok'"), ("systemctl", "echo active"), ("notify-send", "exit 0")] {
        let p = root.join("bin").join(n);
        fs::write(&p, format!("#!/bin/bash\n{body}\n")).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    }
    root
}

fn pimax(root: &Path, args: &[&str], extra: &[(&str, &str)]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_pimax"));
    c.args(args)
        .env_clear()
        .env("HOME", root.join("home"))
        .env("PATH", format!("{}:/usr/bin:/bin", root.join("bin").display()))
        .env("PIMAX_SYSFS_ROOT", root.join("sys"))
        .env("PIMAX_PROC_ROOT", root.join("proc"))
        .env("PIMAX_NO_WATCHDOG", "1")
        .env("PIMAX_NO_NOTIFY", "1")
        .env("PIMAX_VR_SESSION_DIR", root.join("session"))
        .env("PIMAX_MONADO_BUILD", root.join("build"))
        .env("XDG_RUNTIME_DIR", root.join("run"))
        .env("PIMAX_AUTOD_SOCKET", sock(root));
    for (k, v) in extra {
        c.env(k, v);
    }
    c.output().unwrap()
}

fn text(o: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

#[test]
fn an_executing_pimaxd_starts_the_session_and_the_launcher_follows_it() {
    let root = world("exec");
    let seen = fake_pimaxd(&sock(&root), false);
    let o = pimax(&root, &["desk", "start", "--6dof", "--no-daemon-not-a-flag-test"], &[]);
    assert!(!o.status.success(), "unknown flags are still refused: {}", text(&o));
    let o = pimax(&root, &["desk", "start", "--voice"], &[]);
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert!(t.contains("pimaxd starts the session") && t.contains("monado: ok"), "{t}");
    let seen = seen.lock().unwrap().clone();
    assert!(seen.iter().any(|l| l == r#"{"cmd":"set","session":"on","mode":"desk","by":"cli"}"#), "{seen:?}");
    // the arguments are recorded for pimaxd to replay; the launcher itself started nothing
    assert_eq!(fs::read_to_string(root.join("session/start-argv.txt")).unwrap(), "desk\n--voice\n");
    assert!(!root.join("session/monado.pid").exists());
    assert!(!root.join("session/watchdog.pid").exists());
}

#[test]
fn no_daemon_dry_run_and_pimaxd_children_never_ask_pimaxd() {
    let root = world("bypass");
    let seen = fake_pimaxd(&sock(&root), false);
    for (args, env) in [
        (vec!["desk", "start", "--dry-run"], vec![]),
        (vec!["vr", "start", "--dry-run"], vec![]),
        (vec!["desk", "start", "--no-daemon", "--passthrough-bin", "/nonexistent/p"], vec![]),
        (vec!["desk", "start", "--passthrough-bin", "/nonexistent/p"], vec![("PIMAX_WATCHDOG", "1")]),
        (vec!["desk", "start", "--passthrough-bin", "/nonexistent/p"], vec![("PIMAX_NO_DAEMON", "1")]),
    ] {
        let _ = pimax(&root, &args, &env);
    }
    assert!(seen.lock().unwrap().is_empty(), "{:?}", seen.lock().unwrap());
    // --no-daemon is not recorded (pimaxd replays the arguments)
    let argv = fs::read_to_string(root.join("session/start-argv.txt")).unwrap();
    assert!(!argv.contains("--no-daemon"), "{argv}");
}

#[test]
fn stop_tells_pimaxd_then_stops_directly_and_status_shows_it() {
    let root = world("stop");
    let seen = fake_pimaxd(&sock(&root), true);
    let o = pimax(&root, &["desk", "stop"], &[]);
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert!(t.contains("told pimaxd: session off") && t.contains("monado-service is not running"), "{t}");
    assert!(seen.lock().unwrap().iter().any(|l| l.contains(r#""session":"off""#)));
    assert!(root.join("session/user-stop").exists());
    let o = pimax(&root, &["vr", "stop"], &[]);
    assert!(text(&o).contains("told pimaxd: session off"), "{}", text(&o));
    let o = pimax(&root, &["vr", "status"], &[]);
    assert!(text(&o).contains("pimaxd: overall down, GPU gate ok, intent on (by cli), observe only"), "{}", text(&o));
}

#[test]
fn without_pimaxd_nothing_changes() {
    let root = world("absent");
    let o = pimax(&root, &["desk", "stop"], &[]);
    let t = text(&o);
    assert!(o.status.success() && !t.contains("pimaxd"), "{t}");
}
