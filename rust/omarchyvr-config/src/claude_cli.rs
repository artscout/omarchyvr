//! The Claude CLI provider's plumbing shared by the app and voice control: find the user's `claude` binary, ask it
//! for its version (the app's status line), run one test request (the app's "Test" button), the flags every run
//! uses, and the plain-words error messages.
//!
//! Voice control runs `claude -p` once per command, in an empty directory (`~/.cache/omarchyvr/claude-cli`), with no
//! tools, no settings, no MCP servers, no skills, no saved session. The CLI logs in by itself (the user's own
//! subscription or key): nothing here reads, passes or stores a token, and `--bare` is never used (it would ignore the
//! subscription login).
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Where `claude` is looked for after PATH (relative to `$HOME`).
pub const FALLBACK_PATHS: [&str; 2] = [".local/bin/claude", ".local/share/mise/installs/claude/latest/claude"];

/// How long `claude --version` may take.
pub const VERSION_TIMEOUT: Duration = Duration::from_secs(2);
/// How long the app's test request may take.
pub const TEST_TIMEOUT: Duration = Duration::from_secs(30);

/// What the app shows about the CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaudeCliStatus {
    /// No `claude` binary on PATH or in the usual places.
    NotFound,
    /// The binary answers `--version`.
    Found { path: PathBuf, version: String },
    /// The binary exists but did not answer `--version` in time, or failed.
    Broken { path: PathBuf, error: String },
}

impl ClaudeCliStatus {
    /// The CLI can be run.
    pub fn ok(&self) -> bool {
        matches!(self, ClaudeCliStatus::Found { .. })
    }

    /// One line for people.
    pub fn plain(&self) -> String {
        match self {
            ClaudeCliStatus::NotFound => "Claude CLI not found (install Claude Code, then log in with `claude`)".into(),
            ClaudeCliStatus::Found { path, version } => format!("Claude CLI {version} ({})", path.display()),
            ClaudeCliStatus::Broken { path, error } => format!("Claude CLI at {} does not work: {error}", path.display()),
        }
    }
}

/// The empty directory every run starts in (created): `$XDG_CACHE_HOME/omarchyvr/claude-cli`, else
/// `$HOME/.cache/omarchyvr/claude-cli`. Nothing in it means Claude Code finds no CLAUDE.md, no project settings, no
/// files to be tempted by.
pub fn work_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").filter(|v| !v.is_empty()).map(|h| Path::new(&h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    let d = base.join("omarchyvr/claude-cli");
    let _ = std::fs::create_dir_all(&d);
    d
}

fn is_exec(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The binary a `command` names: a path (`~` expanded) as it is, else PATH, else (for the plain `claude`) the
/// [`FALLBACK_PATHS`]. None: not installed.
pub fn find_command(cmd: &str) -> Option<PathBuf> {
    let cmd = if cmd.trim().is_empty() { "claude" } else { cmd.trim() };
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if cmd.contains('/') {
        let p = match (cmd.strip_prefix("~/"), &home) {
            (Some(rest), Some(h)) => h.join(rest),
            _ => PathBuf::from(cmd),
        };
        return is_exec(&p).then_some(p);
    }
    if let Some(path) = std::env::var_os("PATH") {
        if let Some(p) = std::env::split_paths(&path).map(|d| d.join(cmd)).find(|p| is_exec(p)) {
            return Some(p);
        }
    }
    if cmd == "claude" {
        return home.and_then(|h| FALLBACK_PATHS.iter().map(|f| h.join(f)).find(|p| is_exec(p)));
    }
    None
}

/// Why a process did not give an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    /// The binary does not exist.
    NotFound,
    /// It did not finish in time; it was killed.
    Timeout,
    /// Anything else (spawn failure).
    Other(String),
}

/// What a finished process left.
#[derive(Debug, Clone)]
pub struct Captured {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Kill a process and its group (the CLI may have started helpers). By PID only.
pub fn kill_group(pid: u32) {
    // SAFETY: plain signal calls; the group id equals the pid because the child was started with process_group(0).
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
        libc::kill(pid as i32, libc::SIGKILL);
    }
}

/// Run `cmd args` in `cwd`, feed `stdin` (then close it), collect the output; kill it at `timeout`.
pub fn run_capture(cmd: &Path, args: &[String], stdin: Option<&str>, cwd: &Path, timeout: Duration) -> Result<Captured, RunError> {
    use std::os::unix::process::CommandExt;
    let mut c = Command::new(cmd);
    c.args(args).envs(CLAUDE_ENV).current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0);
    let mut child = c.spawn().map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { RunError::NotFound } else { RunError::Other(e.to_string()) })?;
    let pid = child.id();
    if let Some(mut si) = child.stdin.take() {
        let text = stdin.unwrap_or("").to_string();
        std::thread::spawn(move || {
            let _ = si.write_all(text.as_bytes());
        });
    }
    let read = |mut r: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut s = Vec::new();
            let _ = r.read_to_end(&mut s);
            String::from_utf8_lossy(&s).into_owned()
        })
    };
    let out = read(Box::new(child.stdout.take().expect("piped")));
    let err = read(Box::new(child.stderr.take().expect("piped")));
    let t0 = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if t0.elapsed() >= timeout => {
                kill_group(pid);
                let _ = child.wait();
                return Err(RunError::Timeout);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => return Err(RunError::Other(e.to_string())),
        }
    };
    Ok(Captured { code: status.code(), stdout: out.join().unwrap_or_default(), stderr: err.join().unwrap_or_default() })
}

/// `<cmd> --version` with a 2 s limit.
pub fn claude_cli_status(cmd: &str) -> ClaudeCliStatus {
    let Some(path) = find_command(cmd) else { return ClaudeCliStatus::NotFound };
    match run_capture(&path, &["--version".to_string()], None, &work_dir(), VERSION_TIMEOUT) {
        Ok(c) if c.code == Some(0) && !c.stdout.trim().is_empty() => {
            // "2.1.296 (Claude Code)": the number
            let version = c.stdout.split_whitespace().next().unwrap_or("").to_string();
            ClaudeCliStatus::Found { path, version }
        }
        Ok(c) => ClaudeCliStatus::Broken { path, error: plain_error(&format!("{}{}", c.stdout, c.stderr)) },
        Err(RunError::Timeout) => ClaudeCliStatus::Broken { path, error: "it did not answer within 2 seconds".into() },
        Err(RunError::NotFound) => ClaudeCliStatus::NotFound,
        Err(RunError::Other(e)) => ClaudeCliStatus::Broken { path, error: e },
    }
}

/// The flags every run has (the prompt is not among them): print mode, the model, no tools, no settings sources,
/// no MCP servers, no saved session, no skills. Never `--bare`.
/// Environment set on every `claude` we start: the user's own CLAUDE.md files are never injected into the prompt.
pub const CLAUDE_ENV: [(&str, &str); 1] = [("CLAUDE_CODE_DISABLE_CLAUDE_MDS", "1")];

pub fn base_args(model: &str) -> Vec<String> {
    let m = if model.trim().is_empty() { "haiku" } else { model.trim() };
    ["-p", "--model", m, "--tools", "", "--setting-sources", "", "--strict-mcp-config", "--no-session-persistence", "--disable-slash-commands"].map(String::from).to_vec()
}

/// Plain words for what the CLI (or its stderr) said when a request failed.
pub fn plain_error(raw: &str) -> String {
    let l = raw.to_lowercase();
    let has = |xs: &[&str]| xs.iter().any(|x| l.contains(x));
    if has(&["not logged in", "/login", "please run /login", "invalid api key", "authentication_error", "oauth token", "401", "credit balance"]) {
        "Claude CLI is not logged in".into()
    } else if has(&["usage limit", "rate limit", "rate_limit", "429", "overloaded"]) {
        "Claude is busy or the usage limit is reached".into()
    } else if has(&["enotfound", "econnrefused", "econnreset", "etimedout", "fetch failed", "unable to connect", "network", "getaddrinfo", "could not resolve", "offline", "connection error"]) {
        "no internet".into()
    } else {
        let first: String = raw.lines().map(str::trim).find(|x| !x.is_empty()).unwrap_or("no answer").chars().take(100).collect();
        format!("Claude CLI error: {first}")
    }
}

/// The answer of one test request.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeCliTest {
    pub model: String,
    pub answer: String,
    pub ms: u64,
    pub cost_usd: Option<f64>,
}

impl ClaudeCliTest {
    /// "OK: haiku answered "OK" in 2.1 s".
    pub fn plain(&self) -> String {
        let cost = self.cost_usd.map(|c| format!(", ${c:.4}")).unwrap_or_default();
        format!("OK: {} answered \"{}\" in {:.1} s{cost}", self.model, self.answer, self.ms as f64 / 1000.0)
    }
}

/// One real request, for the app's Test button: the CLI answers a fixed question. `Err` is plain words ("Claude CLI
/// not found", "Claude CLI is not logged in", "no internet", "Claude CLI did not answer in time").
pub fn claude_cli_test(cmd: &str, model: &str, timeout: Duration) -> Result<ClaudeCliTest, String> {
    let path = find_command(cmd).ok_or("Claude CLI not found")?;
    let mut args = base_args(model);
    args.extend(["--system-prompt", "Reply with the single word OK.", "--output-format", "json"].map(String::from));
    let t0 = Instant::now();
    let c = run_capture(&path, &args, Some("ping"), &work_dir(), timeout).map_err(|e| match e {
        RunError::NotFound => "Claude CLI not found".to_string(),
        RunError::Timeout => "Claude CLI did not answer in time".to_string(),
        RunError::Other(e) => format!("Claude CLI error: {e}"),
    })?;
    parse_test(&c, model, t0.elapsed().as_millis() as u64)
}

fn parse_test(c: &Captured, model: &str, ms: u64) -> Result<ClaudeCliTest, String> {
    let v: serde_json::Value = serde_json::from_str(c.stdout.trim()).unwrap_or_default();
    let result = v.get("result").and_then(|r| r.as_str()).unwrap_or("").to_string();
    let is_error = v.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false);
    if is_error || (c.code != Some(0) && result.is_empty()) || (v.is_null() && result.is_empty()) {
        let raw = if result.is_empty() { format!("{}{}", c.stdout, c.stderr) } else { result };
        return Err(plain_error(&raw));
    }
    Ok(ClaudeCliTest { model: model.to_string(), answer: result.trim().chars().take(40).collect(), ms, cost_usd: v.get("total_cost_usd").and_then(|x| x.as_f64()) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script(name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("omarchyvr-cli-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("claude");
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[test]
    fn status_finds_the_version_or_says_why_not() {
        let ok = script("ver", "echo '9.9.9 (Claude Code)'");
        assert_eq!(claude_cli_status(ok.to_str().unwrap()), ClaudeCliStatus::Found { path: ok.clone(), version: "9.9.9".into() });
        assert!(claude_cli_status("/nonexistent/claude").plain().contains("not found"));
        let broken = script("broken", "echo 'Not logged in' >&2; exit 1");
        assert!(matches!(claude_cli_status(broken.to_str().unwrap()), ClaudeCliStatus::Broken { .. }));
        let slow = script("slow", "sleep 5");
        let t0 = Instant::now();
        assert!(matches!(claude_cli_status(slow.to_str().unwrap()), ClaudeCliStatus::Broken { .. }));
        assert!(t0.elapsed() < Duration::from_secs(4), "killed at the 2 s limit");
    }

    #[test]
    fn test_request_parses_the_result_and_maps_errors() {
        let ok = script("t-ok", r#"cat >/dev/null; echo '{"type":"result","is_error":false,"result":"OK","total_cost_usd":0.0002}'"#);
        let t = claude_cli_test(ok.to_str().unwrap(), "haiku", Duration::from_secs(5)).unwrap();
        assert_eq!((t.answer.as_str(), t.cost_usd), ("OK", Some(0.0002)));
        let auth = script("t-auth", r#"cat >/dev/null; echo '{"type":"result","is_error":true,"result":"Not logged in · Please run /login"}'; exit 1"#);
        assert_eq!(claude_cli_test(auth.to_str().unwrap(), "haiku", Duration::from_secs(5)).unwrap_err(), "Claude CLI is not logged in");
        let net = script("t-net", r#"cat >/dev/null; echo 'fetch failed: getaddrinfo ENOTFOUND api.anthropic.com' >&2; exit 1"#);
        assert_eq!(claude_cli_test(net.to_str().unwrap(), "haiku", Duration::from_secs(5)).unwrap_err(), "no internet");
        let hang = script("t-hang", "sleep 10");
        assert_eq!(claude_cli_test(hang.to_str().unwrap(), "haiku", Duration::from_millis(300)).unwrap_err(), "Claude CLI did not answer in time");
        assert_eq!(claude_cli_test("/nonexistent/claude", "haiku", Duration::from_secs(1)).unwrap_err(), "Claude CLI not found");
    }

    #[test]
    fn spawns_disable_the_users_claude_md_files() {
        let sc = script("env", r#"cat >/dev/null; echo "{\"type\":\"result\",\"is_error\":false,\"result\":\"md=$CLAUDE_CODE_DISABLE_CLAUDE_MDS\"}""#);
        let t = claude_cli_test(sc.to_str().unwrap(), "haiku", Duration::from_secs(5)).unwrap();
        assert_eq!(t.answer, "md=1");
        let a = base_args("haiku");
        assert!(a.contains(&"--setting-sources".to_string()) && a.contains(&"--tools".to_string()));
    }

    #[test]
    fn flags_never_include_bare() {
        let a = base_args("");
        assert!(a.contains(&"--no-session-persistence".to_string()) && !a.iter().any(|x| x == "--bare"));
        assert_eq!(&a[1..3], ["--model", "haiku"]);
    }

    #[test]
    fn plain_errors() {
        assert_eq!(plain_error("Invalid API key · Please run /login"), "Claude CLI is not logged in");
        assert_eq!(plain_error("API Error: 429 rate limit"), "Claude is busy or the usage limit is reached");
        assert!(plain_error("boom\nmore").starts_with("Claude CLI error: boom"));
    }
}
