//! The camera broker (`pimax-cam broker`), the one process that owns the camera and feeds SLAM, passthrough and the
//! controller tracker (moved from the launcher's `sixdof.rs` in P3): export the Basalt config, start it, wait for its
//! socket; stop it last (after Monado).
use super::proc::{stop_client, ClientStop, Clock, ProcSpec, Procs, Spawner, CLIENT_GRACE};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// Pid file stem and log name.
pub const NAME: &str = "broker";

/// Where the broker's parts are.
pub struct Params<'a> {
    pub cam_bin: &'a Path,
    pub slam_bin: &'a Path,
    /// `omarchyvr-slam export --out` target.
    pub basalt_dir: &'a Path,
    /// The broker's socket.
    pub sock: &'a Path,
    pub session_dir: &'a Path,
}

pub fn spec(cam_bin: &Path, session_dir: &Path) -> ProcSpec {
    ProcSpec { name: NAME, program: cam_bin.to_path_buf(), args: vec!["broker".into()], env: vec![], log: session_dir.join("broker.log") }
}

/// Export the Basalt config, then start the broker and wait up to `settle` for its socket. Returns the broker pid.
/// `say(false, ..)` is progress, `say(true, ..)` a problem.
pub fn start(
    p: &Params,
    sp: &dyn Spawner,
    procs: &dyn Procs,
    clock: &dyn Clock,
    settle: Duration,
    say: &mut dyn FnMut(bool, &str),
) -> Result<u32, String> {
    std::fs::create_dir_all(p.basalt_dir).map_err(|e| format!("cannot create {}: {e}", p.basalt_dir.display()))?;
    let st = Command::new(p.slam_bin)
        .args(["export", "--out"])
        .arg(p.basalt_dir)
        .stdout(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("cannot run {} export: {e}", p.slam_bin.display()))?;
    if !st.success() {
        return Err(format!("{} export failed ({st})", p.slam_bin.display()));
    }
    let _ = std::fs::remove_file(p.sock);
    let spec = spec(p.cam_bin, p.session_dir);
    say(false, &format!("starting the camera broker ({})", spec.program.display()));
    let pid = sp.spawn(&spec).map_err(|e| format!("cannot start {}: {e}", spec.program.display()))?;
    let _ = std::fs::write(p.session_dir.join("broker.pid"), format!("{pid}\n"));
    let t0 = clock.now();
    while !p.sock.exists() {
        if !procs.is_alive(pid) || clock.now() - t0 > settle {
            let tail = std::fs::read_to_string(&spec.log).unwrap_or_default();
            let tail: Vec<&str> = tail.lines().rev().take(3).collect();
            stop(p.session_dir, p.cam_bin, procs, clock, say);
            return Err(format!(
                "camera broker did not come up ({}): {}",
                p.sock.display(),
                tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
            ));
        }
        clock.sleep(Duration::from_millis(200));
    }
    Ok(pid)
}

/// SIGINT the broker by pid (from its pid file), if there is one. `say(false, ..)` is progress, `say(true, ..)` a problem.
pub fn stop(session_dir: &Path, cam_bin: &Path, procs: &dyn Procs, clock: &dyn Clock, say: &mut dyn FnMut(bool, &str)) {
    let pf = session_dir.join("broker.pid");
    if !pf.exists() {
        return;
    }
    match stop_client(NAME, &pf, &super::proc::comm_of(cam_bin), procs, clock, CLIENT_GRACE, &mut |m| say(false, m)) {
        ClientStop::StillAlive => {
            say(true, &format!("WARNING: the camera broker is still running after SIGINT (pid file {})", pf.display()))
        }
        ClientStop::CannotSignal(m) => say(true, &m),
        ClientStop::NotRunning | ClientStop::Exited => {
            let _ = std::fs::remove_file(&pf);
        }
    }
}

super::proc::client_component!(Broker);
