//! `pimax vr steam-setup [status|on|off]`: Steam games reach Monado without per-game launch options.
//!
//! Since pressure-vessel 0.20260925 Steam's game container shares Monado's socket (and the OpenXR runtime files) only
//! when `PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1` is in Steam's environment (LESSONS row 27). Games inherit Steam's
//! environment, so setting it once for Steam covers every game. On Omarchy Steam is started by the app launcher
//! (`gtk-launch steam.desktop` from Hyprland, which got its environment from the systemd user manager through uwsm):
//!
//! * a copy of Steam's desktop entry in `~/.local/share/applications/steam.desktop` (it hides the system one) whose
//!   `Exec=` lines start with `env PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1`: works from the next Steam start;
//! * `~/.config/environment.d/90-pimax-steam-vr.conf`: the systemd user manager reads it at login, so after the next
//!   log in every way of starting Steam (also `steam://` links and game shortcuts) has it;
//! * `systemctl --user set-environment` for programs the user manager starts before then (best effort).
//!
//! `off` removes only files that carry our marker. Nothing here touches hypr config. The variable only makes the
//! container import the host's OpenXR runtime files; games that do not use OpenXR ignore it.
use std::io;
use std::path::{Path, PathBuf};

/// The variable.
pub const VAR: &str = "PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES";
/// Marker key in our desktop entry and comment in our environment.d file.
pub const MARKER: &str = "X-Pimax-Steam-VR=1";
/// The system desktop entry we copy.
pub const SYSTEM_DESKTOP: &str = "/usr/share/applications/steam.desktop";

/// `~/.local/share/applications/steam.desktop`.
pub fn desktop_path(home: &Path) -> PathBuf {
    home.join(".local/share/applications/steam.desktop")
}

/// `~/.config/environment.d/90-pimax-steam-vr.conf`.
pub fn envd_path(home: &Path) -> PathBuf {
    home.join(".config/environment.d/90-pimax-steam-vr.conf")
}

/// The environment.d file text.
pub fn envd_text() -> String {
    format!("# {MARKER} written by the Pimax app (Set up Steam for VR games); `pimax vr steam-setup off` removes it.\n# Steam's game container shares Monado's socket only with this (LESSONS row 27).\n{VAR}=1\n")
}

/// Our desktop entry from the system one: every `Exec=` line gets `env VAR=1`, the marker goes into the main section.
pub fn desktop_text(system: &str) -> String {
    let mut out = String::new();
    let mut marked = false;
    for line in system.lines() {
        if let Some(rest) = line.strip_prefix("Exec=") {
            if rest.contains(VAR) {
                out.push_str(line);
            } else {
                out.push_str(&format!("Exec=env {VAR}=1 {rest}"));
            }
        } else {
            out.push_str(line);
        }
        out.push('\n');
        if !marked && line.trim() == "[Desktop Entry]" {
            out.push_str(MARKER);
            out.push('\n');
            marked = true;
        }
    }
    out
}

/// What is set up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct State {
    /// Our desktop entry is there.
    pub desktop: bool,
    /// Our environment.d file is there.
    pub envd: bool,
    /// Some other `steam.desktop` in ~/.local/share/applications (not ours): we leave it alone.
    pub foreign_desktop: bool,
    /// Steam runs now: Some(true) when it has the variable, Some(false) when it runs without it.
    pub running_has_var: Option<bool>,
}

impl State {
    /// Set up (files in place).
    pub fn configured(&self) -> bool {
        self.desktop || self.envd
    }

    /// One plain-words line.
    pub fn describe(&self) -> String {
        match (self.configured(), self.running_has_var) {
            (false, _) if self.foreign_desktop => "not set up (another steam.desktop in ~/.local/share/applications is left alone; environment.d is used instead)".into(),
            (false, _) => "not set up: Steam games cannot reach Monado without per-game launch options".into(),
            (true, Some(false)) => "set up, but the running Steam started before: quit Steam and start it again once (or log out and in)".into(),
            (true, Some(true)) => "active: every Steam game reaches Monado, no launch options needed".into(),
            (true, None) => "set up: active from the next Steam start".into(),
        }
    }
}

/// Read the state. `steam_env` gives the environment of the running Steam client (None: Steam does not run).
pub fn inspect(home: &Path, steam_env: Option<Vec<(String, String)>>) -> State {
    let d = std::fs::read_to_string(desktop_path(home)).ok();
    let ours = d.as_deref().is_some_and(|t| t.contains(MARKER));
    State {
        desktop: ours,
        envd: std::fs::read_to_string(envd_path(home)).is_ok_and(|t| t.contains(MARKER)),
        foreign_desktop: d.is_some() && !ours,
        running_has_var: steam_env.map(|e| e.iter().any(|(k, v)| k == VAR && v == "1")),
    }
}

fn write_atomic(p: &Path, text: &str) -> io::Result<()> {
    std::fs::create_dir_all(p.parent().unwrap())?;
    let tmp = p.with_file_name(format!(".{}.pimax-{}", p.file_name().unwrap().to_string_lossy(), std::process::id()));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, p).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Set it up. `system_desktop`: the text of Steam's system desktop entry (None: Steam's entry is missing, only
/// environment.d is written). Returns what was done, in plain words.
pub fn enable(home: &Path, system_desktop: Option<&str>) -> io::Result<Vec<String>> {
    let mut done = Vec::new();
    write_atomic(&envd_path(home), &envd_text())?;
    done.push(format!("wrote {} (read at the next log in)", envd_path(home).display()));
    let st = inspect(home, None);
    match system_desktop {
        Some(sys) if !st.foreign_desktop => {
            write_atomic(&desktop_path(home), &desktop_text(sys))?;
            done.push(format!("wrote {} (used from the next Steam start)", desktop_path(home).display()));
        }
        Some(_) => done.push(format!("left {} alone (not ours)", desktop_path(home).display())),
        None => done.push(format!("{SYSTEM_DESKTOP} not found: only the log-in setting was written")),
    }
    Ok(done)
}

/// Undo [`enable`] (only our files).
pub fn disable(home: &Path) -> io::Result<Vec<String>> {
    let st = inspect(home, None);
    let mut done = Vec::new();
    if st.desktop {
        std::fs::remove_file(desktop_path(home))?;
        done.push(format!("removed {}", desktop_path(home).display()));
    }
    if st.envd {
        std::fs::remove_file(envd_path(home))?;
        done.push(format!("removed {}", envd_path(home).display()));
    }
    if done.is_empty() {
        done.push("nothing to remove".into());
    }
    Ok(done)
}

/// Environment of the running Steam client of this user (comm `steam`), from /proc.
pub fn running_steam_env() -> Option<Vec<(String, String)>> {
    let uid = unsafe { libc::getuid() };
    for e in std::fs::read_dir("/proc").ok()?.flatten() {
        if e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()).is_none() {
            continue;
        }
        let dir = e.path();
        if std::fs::read_to_string(dir.join("comm")).ok().as_deref().map(str::trim) != Some("steam") {
            continue;
        }
        use std::os::unix::fs::MetadataExt;
        if std::fs::metadata(&dir).map(|m| m.uid()).ok() != Some(uid) {
            continue;
        }
        let Ok(raw) = std::fs::read(dir.join("environ")) else { continue };
        return Some(
            raw.split(|b| *b == 0)
                .filter_map(|kv| {
                    let s = String::from_utf8_lossy(kv);
                    s.split_once('=').map(|(k, v)| (k.to_string(), v.to_string()))
                })
                .collect(),
        );
    }
    None
}

/// `pimax vr steam-setup [status|on|off]`.
pub fn run(what: &str, home: &Path) -> Result<(), String> {
    match what {
        "status" => {
            let st = inspect(home, running_steam_env());
            vout!("Steam for VR games: {}", st.describe());
            if st.configured() { Ok(()) } else { Err(String::new()) }
        }
        "on" => {
            let sys = std::fs::read_to_string(SYSTEM_DESKTOP).ok();
            for l in enable(home, sys.as_deref()).map_err(|e| format!("could not set up Steam for VR games: {e}"))? {
                vout!("pimax vr: {l}");
            }
            // programs the user manager starts from now on (best effort; the files above are what lasts)
            let _ = std::process::Command::new("systemctl")
                .args(["--user", "set-environment", &format!("{VAR}=1")])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            let st = inspect(home, running_steam_env());
            vout!("Steam for VR games: {}", st.describe());
            if st.running_has_var == Some(false) {
                vout!("Once: quit Steam (Steam menu > Exit) and start it again, or log out and in. After that every game works without launch options.");
            }
            Ok(())
        }
        "off" => {
            for l in disable(home).map_err(|e| format!("could not undo the Steam setup: {e}"))? {
                vout!("pimax vr: {l}");
            }
            let _ = std::process::Command::new("systemctl")
                .args(["--user", "unset-environment", VAR])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            vout!("Steam for VR games: off. Restart Steam once; games then need the launch option again (pimax vr steam-options).");
            Ok(())
        }
        other => Err(format!("unknown steam-setup command {other} (status|on|off)")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SYS: &str = "[Desktop Entry]\nName=Steam\nExec=/usr/bin/steam %U\nActions=Store;\n\n[Desktop Action Store]\nName=Store\nExec=/usr/bin/steam steam://store\n";

    #[test]
    fn every_exec_line_gets_the_variable_and_the_marker_is_in_the_main_section() {
        let t = desktop_text(SYS);
        assert!(t.contains("Exec=env PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1 /usr/bin/steam %U\n"));
        assert!(t.contains("Exec=env PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1 /usr/bin/steam steam://store\n"));
        assert!(t.starts_with("[Desktop Entry]\nX-Pimax-Steam-VR=1\n"));
        assert_eq!(t.matches(MARKER).count(), 1);
        // idempotent on our own output
        assert_eq!(desktop_text(&t).matches("env PRESSURE").count(), 2);
    }

    #[test]
    fn on_then_off_is_reversible() {
        let h = crate::vr::test_dir("steam-setup");
        assert!(!inspect(&h, None).configured());
        enable(&h, Some(SYS)).unwrap();
        let st = inspect(&h, None);
        assert!(st.desktop && st.envd && !st.foreign_desktop);
        assert!(std::fs::read_to_string(envd_path(&h)).unwrap().contains("PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1\n"));
        assert!(st.describe().contains("next Steam start"));
        disable(&h).unwrap();
        assert!(!desktop_path(&h).exists() && !envd_path(&h).exists());
        assert_eq!(disable(&h).unwrap(), vec!["nothing to remove".to_string()]);
    }

    #[test]
    fn a_foreign_desktop_entry_is_left_alone() {
        let h = crate::vr::test_dir("steam-setup-foreign");
        std::fs::create_dir_all(desktop_path(&h).parent().unwrap()).unwrap();
        std::fs::write(desktop_path(&h), "[Desktop Entry]\nExec=my-steam\n").unwrap();
        let done = enable(&h, Some(SYS)).unwrap();
        assert!(done.iter().any(|l| l.contains("not ours")));
        assert_eq!(std::fs::read_to_string(desktop_path(&h)).unwrap(), "[Desktop Entry]\nExec=my-steam\n");
        assert!(inspect(&h, None).envd);
        disable(&h).unwrap();
        assert!(desktop_path(&h).exists(), "off never removes a file that is not ours");
    }

    #[test]
    fn running_steam_decides_the_restart_note() {
        let h = crate::vr::test_dir("steam-setup-running");
        enable(&h, Some(SYS)).unwrap();
        let without = inspect(&h, Some(vec![("HOME".into(), "/h".into())]));
        assert_eq!(without.running_has_var, Some(false));
        assert!(without.describe().contains("start it again"));
        let with = inspect(&h, Some(vec![(VAR.into(), "1".into())]));
        assert!(with.describe().starts_with("active"));
    }
}
