//! `omarchyvr import-previous`: bring back the user's own per-device data and settings of a previous install on this
//! PC (omarchyvr_config::import), and say what uses the shipped Dream Air reference instead (devicedata). Run by
//! `./install.sh` and by the app's automatic setup; never overwrites, never touches ai.toml.
//!
//! ```text
//! omarchyvr import-previous [--dry-run]   copy what is missing (dry run: only say what would be copied)
//! omarchyvr import-previous --status      what is the user's own, what is the reference, what an import would bring
//! omarchyvr import-previous --device-paths   the device-data paths the uninstaller keeps (relative to $HOME)
//! ```
use crate::args::Args;
use crate::{CliError, CliResult};
use omarchyvr_config::{devicedata, import};
use std::path::{Path, PathBuf};

/// Usage line.
pub const USAGE: &str = "usage: omarchyvr import-previous [--dry-run | --status | --device-paths]";

fn home() -> Result<PathBuf, CliError> {
    std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from).ok_or_else(|| CliError::new("HOME is not set"))
}

/// The plain status lines (also used by tests).
pub fn status_lines(home: &Path, env: &dyn Fn(&str) -> Option<String>, pending: &[String]) -> Vec<String> {
    let s = devicedata::Sources::read(home, env);
    let own = |p: &Option<PathBuf>, what: &str, reference: &str| match p {
        Some(p) => format!("{what}: your own ({})", p.display()),
        None => format!("{what}: {reference}"),
    };
    let mut v = vec![
        own(&s.calibration, "headset calibration", "the shipped Pimax Dream Air reference (Room setup reads your headset's own)"),
        own(&s.controllers, "controller models", "the shipped Pimax Dream Air reference (Pimax's default models)"),
        own(&s.lens, "lens profile", "the shipped Pimax Dream Air profile"),
        match s.gyro_measured {
            Some(true) => "gyro bias: measured on this PC".to_string(),
            Some(false) => "gyro bias: the Dream Air default until the headset rests once (then it is measured and saved)".to_string(),
            None => "gyro bias: none saved yet (the next start writes the Dream Air default)".to_string(),
        },
    ];
    if pending.is_empty() {
        v.push("previous install: nothing (more) to bring back".into());
    } else {
        v.push(format!("previous install: can bring back {}", pending.join("; ")));
    }
    v
}

/// One line for every Work and Play start: where the headset data comes from (the user's own or the shipped reference),
/// so the job log says when a reference is in use.
pub fn start_line(home: &Path, env: &dyn Fn(&str) -> Option<String>) -> String {
    let s = devicedata::Sources::read(home, env);
    let own = |p: &Option<PathBuf>| if p.is_some() { "your own" } else { "the shipped Dream Air reference" };
    format!(
        "headset data: calibration = {}, controller models = {}, lens = {}, gyro = {}",
        own(&s.calibration),
        own(&s.controllers),
        if s.lens.is_some() { "your own" } else { "the shipped Dream Air profile" },
        match s.gyro_measured {
            Some(true) => "measured on this PC",
            _ => "the Dream Air default until the headset rests once",
        }
    )
}

/// Entry point.
pub fn run(mut a: Args) -> CliResult {
    if matches!(a.0.first().map(String::as_str), Some("-h" | "--help" | "help")) {
        println!("{USAGE}");
        return Ok(());
    }
    let dry = a.flag("--dry-run");
    let status = a.flag("--status");
    let device_paths = a.flag("--device-paths");
    a.no_unknown_flags()?;
    if let Some(x) = a.0.first() {
        return Err(CliError::usage(format!("unexpected argument {x}\n{USAGE}")));
    }
    if device_paths {
        for p in import::device_paths() {
            println!("{p}");
        }
        return Ok(());
    }
    let home = home()?;
    let env = |k: &str| std::env::var(k).ok();
    let places = import::Places::for_home(&home);
    if status {
        for l in status_lines(&home, &env, &import::pending(&places)) {
            println!("{l}");
        }
        return Ok(());
    }
    println!("Import from your previous install{}:", if dry { " (dry run: nothing is changed)" } else { "" });
    let lines = import::run(&places, dry);
    for l in &lines {
        println!("  {}", l.describe());
    }
    let reference = devicedata::Sources::read(&home, &env).reference_in_use();
    if !reference.is_empty() {
        println!("Using the shipped Pimax Dream Air reference for: {} (your own data replaces it when it is found or read from the headset)", reference.join(", "));
    }
    match lines.iter().find(|l| matches!(l.state, import::State::Failed(_))) {
        Some(l) => Err(CliError::new(l.describe())),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_names_the_reference_in_plain_words() {
        let h = std::env::temp_dir().join(format!("omarchyvr-import-cmd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&h);
        std::fs::create_dir_all(&h).unwrap();
        let l = status_lines(&h, &|_| None, &[]);
        assert!(l[0].starts_with("headset calibration: the shipped Pimax Dream Air reference"), "{l:?}");
        assert!(l[1].starts_with("controller models: the shipped"));
        assert!(l[3].contains("none saved yet"));
        assert!(l[4].contains("nothing (more) to bring back"));
        let l = status_lines(&h, &|_| None, &["your IPD".to_string()]);
        assert!(l[4].ends_with("can bring back your IPD"));
        let _ = std::fs::remove_dir_all(&h);
    }
}
