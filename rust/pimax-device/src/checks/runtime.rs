//! Runtime checks.

use crate::check::{Area, Check, Ctx, Finding, FixRef, Severity};
use std::path::PathBuf;

/// The variable Steam's game container needs to share Monado's socket (LESSONS row 27).
pub const PV_VAR: &str = "PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES";
/// Marker of the files `pimax vr steam-setup on` writes (pimax-tools `vr::steam_setup::MARKER`).
pub const MARKER: &str = "X-Pimax-Steam-VR=1";

/// `runtime.steam_vr_games`: Steam itself has `PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1`, so every Steam game reaches
/// Monado without per-game launch options (`pimax vr steam-setup`).
pub struct SteamVrGames;

impl Check for SteamVrGames {
    fn id(&self) -> &'static str {
        "runtime.steam_vr_games"
    }
    fn area(&self) -> Area {
        Area::Runtime
    }
    fn run(&self, cx: &Ctx) -> Vec<Finding> {
        let title = "Steam set up for VR games";
        let Some(home) = cx.host.env("HOME").map(PathBuf::from) else { return Vec::new() };
        let steam = cx.host.exists(std::path::Path::new("/usr/bin/steam")) || cx.host.exists(&home.join(".local/share/Steam/steam.sh"));
        if !steam {
            return vec![self.finding(Severity::Info, title, "Steam is not installed (only needed for Steam games)", None)];
        }
        let ours = |p: PathBuf| cx.host.read_str(&p).is_some_and(|t| t.contains(MARKER));
        let configured = ours(home.join(".local/share/applications/steam.desktop")) || ours(home.join(".config/environment.d/90-pimax-steam-vr.conf"));
        if !configured {
            return vec![self.finding(
                Severity::Warn,
                title,
                "Steam games cannot reach Monado unless every game gets a launch option. Fix sets it once for Steam; then restart Steam once.",
                Some(FixRef::new("runtime.steam_vr_games", &[])),
            )];
        }
        // the running Steam client: started before the setup, it lacks the variable until it is restarted
        let user = cx.host.env("USER").unwrap_or_default();
        let running = if user.is_empty() { None } else { cx.host.run("pgrep", &["-x", "-u", &user, "steam"]) };
        let pid = running.filter(|o| o.code == Some(0)).and_then(|o| o.stdout.lines().next().map(|l| l.trim().to_string()));
        let has = pid.and_then(|p| cx.host.read(&PathBuf::from(format!("/proc/{p}/environ"))))
            .map(|raw| raw.split(|b| *b == 0).any(|kv| kv == format!("{PV_VAR}=1").as_bytes()));
        match has {
            Some(false) => vec![self.finding(Severity::Warn, title, "set up, but Steam was started before: quit Steam (Steam menu > Exit) and start it again once, or log out and in", None)],
            _ => vec![self.finding(Severity::Ok, title, "every Steam game reaches Monado, no launch options needed", None)],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pimax_doctor::fake::FakeHost;
    use pimax_doctor::Opts;

    fn run(h: &FakeHost) -> Finding {
        let opts = Opts::from_env(&|k| (k == "HOME").then(|| "/home/u".to_string()));
        let cx = Ctx { host: h, opts: &opts, module: None };
        SteamVrGames.run(&cx).remove(0)
    }

    fn host() -> FakeHost {
        let mut h = FakeHost::default();
        h.env.insert("HOME".into(), "/home/u".into());
        h.env.insert("USER".into(), "u".into());
        h.file("/usr/bin/steam", "#!/bin/sh");
        h
    }

    #[test]
    fn not_set_up_offers_the_fix() {
        let f = run(&host());
        assert_eq!(f.severity, Severity::Warn);
        assert_eq!(f.fix.as_ref().unwrap().id, "runtime.steam_vr_games");
        let p = f.fix.unwrap().plan().unwrap();
        assert_eq!(p.commands, vec!["pimax vr steam-setup on".to_string()]);
        assert_eq!(p.undo, vec!["pimax vr steam-setup off".to_string()]);
    }

    #[test]
    fn set_up_is_ok_unless_the_running_steam_lacks_the_variable() {
        let mut h = host();
        h.file("/home/u/.config/environment.d/90-pimax-steam-vr.conf", format!("# {MARKER}\n{PV_VAR}=1\n"));
        assert_eq!(run(&h).severity, Severity::Ok);
        h.cmd("pgrep -x -u u steam", 0, "123\n");
        h.file("/proc/123/environ", "HOME=/home/u\0");
        let f = run(&h);
        assert_eq!(f.severity, Severity::Warn);
        assert!(f.detail.contains("start it again"));
        h.file("/proc/123/environ", format!("HOME=/home/u\0{PV_VAR}=1\0"));
        assert_eq!(run(&h).severity, Severity::Ok);
    }

    #[test]
    fn no_steam_is_only_info() {
        let mut h = FakeHost::default();
        h.env.insert("HOME".into(), "/home/u".into());
        assert_eq!(run(&h).severity, Severity::Info);
    }
}
