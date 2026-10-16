//! The wrist performance readout (`omarchyvr-perf`): `desk start` runs it by default (`--no-perf`), `vr start --perf` too.
//!
//! * Monado writes frame statistics to `$XDG_RUNTIME_DIR/omarchyvr-perf.stats` when `PIMAX_PERF_FILE` is set (the
//!   launcher sets it; patch in `~/src/monado-eye`, `comp_multi_system.c`).
//! * `omarchyvr-perf run` turns that file into text and colours for the WayVR custom panel `perf` (`gui/perf.xml`, a
//!   `FollowHand Right` overlay, not grabbable) with `wayvrctl batch`.
//! * Desk mode already runs WayVR. In game mode (`vr start --perf`) the launcher starts a second, minimal WayVR
//!   (`--openxr --headless`: an `XR_EXTX_overlay` session without any screens, no watch, no keyboard) with its own
//!   config home that only holds the `perf` panel, so the game stays the main session.

use super::desk::{stop_client, ClientStop, ProcSpec, Spawner};
use super::config::{comm_of, StartOptions};
use super::host::{Clock, Procs};
use super::session::Deps;
use crate::args::Args;
use crate::{CliError, CliResult};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The wrist panel: four coloured single-line labels in a translucent box (10 cm wide on the wrist, see WayVR
/// `overlays/custom.rs`). `omarchyvr-perf` sets text and colour of `perf_l1..4`.
pub const PERF_XML: &str = r##"<layout>
  <elements>
    <div id="perf_outer" width="340" height="160">
      <rectangle id="perf_box" width="100%" height="100%" padding="10" flex_direction="column" justify_content="center" gap="2" color="#000000b4" round="12">
        <label id="perf_l1" size="30" weight="bold" color="#9aa0a6" text="perf: no data" />
        <label id="perf_l2" size="30" weight="bold" color="#9aa0a6" text=" " />
        <label id="perf_l3" size="30" weight="bold" color="#9aa0a6" text=" " />
        <label id="perf_l4" size="30" weight="bold" color="#9aa0a6" text=" " />
      </rectangle>
    </div>
  </elements>
</layout>
"##;

/// `conf.d` of the game-mode WayVR home: no screens (headless), no watch, no keyboard, game input untouched.
pub const VR_WAYVR_CONF: &str =
    "enable_watch: false\nenable_keyboard: false\nblock_game_input: false\nnotifications_sound_enabled: false\ncapture_method: screencopy\n";

/// Where Monado writes and `omarchyvr-perf` reads.
pub fn stats_file(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    if let Some(f) = env("PIMAX_PERF_FILE").filter(|v| !v.is_empty()) {
        return PathBuf::from(f);
    }
    PathBuf::from(
        env("XDG_RUNTIME_DIR")
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "/tmp".into()),
    )
    .join("omarchyvr-perf.stats")
}

/// What `desk start` / `vr start --perf` need to run the readout.
#[derive(Debug, Clone)]
pub struct PerfOptions {
    /// `omarchyvr-perf` binary.
    pub perf_bin: PathBuf,
    /// The stats file (also `PIMAX_PERF_FILE` for Monado).
    pub stats: PathBuf,
    /// `wayvrctl` binary.
    pub wayvrctl_bin: PathBuf,
    /// Game mode only: the minimal WayVR and its config home.
    pub wayvr_bin: PathBuf,
    pub wayvr_home: PathBuf,
}

/// Parse `--perf-bin` and set up `PIMAX_PERF_FILE` for Monado (the caller's own value wins).
pub fn parse(
    a: &mut Args,
    env: &dyn Fn(&str) -> Option<String>,
    vr: &mut StartOptions,
    home: &Path,
    wayvr_bin: &Path,
    wayvrctl_bin: &Path,
) -> Result<PerfOptions, CliError> {
    let perf_bin = a
        .opt("--perf-bin")?
        .map(PathBuf::from)
        .unwrap_or_else(|| super::config::tools_dir(home).join("omarchyvr-perf"));
    let stats = stats_file(env);
    if !vr.extra_env.iter().any(|(k, _)| k == "PIMAX_PERF_FILE") {
        vr.extra_env
            .push(("PIMAX_PERF_FILE".into(), stats.to_string_lossy().into_owned()));
    }
    Ok(PerfOptions {
        perf_bin,
        stats,
        wayvrctl_bin: wayvrctl_bin.to_path_buf(),
        wayvr_bin: wayvr_bin.to_path_buf(),
        wayvr_home: omarchyvr_config::paths::src_dir(&home).join("desk-out/wayvr-home-perf"),
    })
}

/// `gui/perf.xml` and `conf.d/40-panels.yaml` pieces: write the panel definition into a WayVR config home.
pub fn write_panel(home: &Path) -> io::Result<()> {
    let gui = home.join("config/wayvr/theme/gui");
    std::fs::create_dir_all(&gui)?;
    std::fs::write(gui.join("perf.xml"), PERF_XML)
}

/// The omarchyvr-perf client.
pub fn perf_spec(p: &PerfOptions, dir: &Path) -> ProcSpec {
    omarchyvr_components::components::perf::spec(&p.perf_bin, &p.stats, &p.wayvrctl_bin, dir)
}

/// Game mode: the minimal WayVR (overlay session, no screens) that carries only the wrist panel.
pub fn vr_wayvr_spec(p: &PerfOptions, o: &StartOptions, openvr_lib: Option<&Path>) -> ProcSpec {
    omarchyvr_components::components::wayvr::perf_spec(&omarchyvr_components::components::wayvr::Params {
        bin: &p.wayvr_bin,
        home: &p.wayvr_home,
        openvr_lib,
        runtime_json: &o.runtime_json(),
        session_dir: &o.session_dir,
    })
}

/// Create the game-mode config home (rewritten on every start).
pub fn ensure_vr_home(home: &Path) -> io::Result<()> {
    let conf_d = home.join("config/wayvr/conf.d");
    std::fs::create_dir_all(&conf_d)?;
    std::fs::create_dir_all(home.join("data"))?;
    std::fs::create_dir_all(home.join("state"))?;
    std::fs::write(conf_d.join("10-perf.yaml"), VR_WAYVR_CONF)?;
    std::fs::write(conf_d.join("40-panels.yaml"), "custom_panels:\n  - perf\n")?;
    write_panel(home)
}

/// `vr start --perf`: after the service is up, the minimal WayVR and the readout. Failures here only warn: the
/// game session itself does not depend on them.
pub fn vr_start_clients(p: &PerfOptions, o: &StartOptions, d: &Deps, sp: &dyn Spawner) {
    let _ = std::fs::remove_file(&p.stats);
    if let Err(e) = ensure_vr_home(&p.wayvr_home) {
        verr!("omarchyvr vr: --perf: cannot prepare {}: {e}", p.wayvr_home.display());
        return;
    }
    let openvr = PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("src/openvr/lib/linux64");
    let specs = [
        vr_wayvr_spec(p, o, openvr.join("libopenvr_api.so").exists().then_some(openvr.as_path())),
        perf_spec(p, &o.session_dir),
    ];
    for s in &specs {
        if !s.program.exists() {
            verr!("omarchyvr vr: --perf: {} does not exist, no wrist readout (cd ~/pimax-re/rust && cargo build --release -p omarchyvr-perf)", s.program.display());
            return;
        }
        match sp.spawn(s) {
            Ok(pid) => {
                let _ = std::fs::write(o.session_dir.join(format!("{}.pid", s.name)), format!("{pid}\n"));
                vout!("omarchyvr vr: --perf: {} running, pid {pid}, log {}", s.name, s.log.display());
            }
            Err(e) => {
                verr!("omarchyvr vr: --perf: cannot start {}: {e}", s.program.display());
                return;
            }
        }
        // WayVR must be up before the readout talks to it (the readout retries anyway)
        d.clock.sleep(Duration::from_secs(2));
    }
}

/// `vr stop`: the readout and its WayVR first (only when their pid files exist). Returns the WayVR if it is
/// still running after its whole wait (hung at exit): the caller ends it once Monado is gone
/// ([`super::desk::end_hung_wayvrs`]).
pub fn vr_stop_clients(p: &PerfOptions, session_dir: &Path, procs: &dyn Procs, clock: &dyn Clock) -> Vec<super::desk::HungWayvr> {
    let mut hung = Vec::new();
    for (name, bin) in [("perf", &p.perf_bin), ("perfwayvr", &p.wayvr_bin)] {
        let pf = session_dir.join(format!("{name}.pid"));
        if !pf.exists() {
            continue;
        }
        let mut info = |m: &str| vout!("omarchyvr vr: {m}");
        // the minimal WayVR must be gone before Monado gets SIGINT (Xid 31 at shutdown otherwise)
        let r = if name == "perfwayvr" {
            super::desk::stop_wayvr(name, &pf, &comm_of(bin), procs, clock, &mut info, &mut |m| verr!("omarchyvr vr: {m}"))
        } else {
            stop_client(name, &pf, &comm_of(bin), procs, clock, Duration::from_secs(10), &mut info)
        };
        match r {
            ClientStop::StillAlive if name == "perfwayvr" => {
                verr!("omarchyvr vr: WARNING: {name} is still running after SIGINT (hung at exit); it gets SIGTERM, then SIGKILL by pid, once Monado is gone");
                if let Some(pid) = super::session::read_pid(&pf) {
                    hung.push(super::desk::HungWayvr { name: name.to_string(), pid, comm: comm_of(bin), pid_file: pf.clone() });
                }
            }
            ClientStop::StillAlive => verr!("omarchyvr vr: WARNING: {name} is still running after SIGINT"),
            ClientStop::CannotSignal(m) => verr!("omarchyvr vr: {m}"),
            ClientStop::NotRunning | ClientStop::Exited => {
                let _ = std::fs::remove_file(&pf);
            }
        }
    }
    hung
}

/// `vr start --perf`: the normal start, then the clients when it worked.
pub fn vr_start(o: &StartOptions, p: &PerfOptions, d: &Deps, sp: &dyn Spawner, start: impl FnOnce() -> CliResult) -> CliResult {
    if o.dry_run {
        vout!("omarchyvr vr: --perf would also run, after the service: {} (minimal WayVR, no screens), {}", p.wayvr_bin.display(), p.perf_bin.display());
        vout!("       PIMAX_PERF_FILE={} for monado-service", p.stats.display());
        return start();
    }
    let r = start();
    if r.is_ok() {
        if o.wait {
            verr!("omarchyvr vr: --perf is ignored with --wait (the launcher stays in the foreground)");
        } else {
            vr_start_clients(p, o, d, sp);
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(k: &str) -> Option<String> {
        match k {
            "XDG_RUNTIME_DIR" => Some("/run/user/7".into()),
            _ => None,
        }
    }

    #[test]
    fn stats_file_is_in_the_runtime_dir_unless_overridden() {
        assert_eq!(stats_file(&env), PathBuf::from("/run/user/7/omarchyvr-perf.stats"));
        let own = |k: &str| (k == "PIMAX_PERF_FILE").then(|| "/x/p".to_string());
        assert_eq!(stats_file(&own), PathBuf::from("/x/p"));
        assert_eq!(stats_file(&|_: &str| None), PathBuf::from("/tmp/omarchyvr-perf.stats"));
    }

    #[test]
    fn the_panel_has_the_four_labels_the_helper_sets() {
        for id in pimax_perf_labels() {
            assert!(PERF_XML.contains(&format!("id=\"{id}\"")), "{id}");
        }
    }

    fn pimax_perf_labels() -> [&'static str; 4] {
        ["perf_l1", "perf_l2", "perf_l3", "perf_l4"]
    }

    #[test]
    fn game_mode_home_has_no_watch_keyboard_or_screens() {
        let d = crate::vr::test_dir("perf-home");
        ensure_vr_home(&d).unwrap();
        let c = std::fs::read_to_string(d.join("config/wayvr/conf.d/10-perf.yaml")).unwrap();
        assert!(c.contains("enable_watch: false") && c.contains("enable_keyboard: false") && c.contains("block_game_input: false"));
        assert!(std::fs::read_to_string(d.join("config/wayvr/conf.d/40-panels.yaml")).unwrap().contains("- perf"));
        assert!(d.join("config/wayvr/theme/gui/perf.xml").exists());
    }
}
