//! `pimax-ctrltrack live`: optical controller tracking on the camera broker's LED frames (moved from the launcher's
//! `sixdof.rs` in P3). Not an XR client: it writes the pose file Monado reads (PIMAX_CTRL_TRACK=1).
use super::proc::ProcSpec;
use std::path::Path;

/// Pid file stem and log name.
pub const NAME: &str = "ctrltrack";

pub fn spec(bin: &Path, broker_sock: &Path, session_dir: &Path) -> ProcSpec {
    ProcSpec {
        name: NAME,
        program: bin.to_path_buf(),
        args: vec!["live".into(), "--source".into(), format!("broker:{}", broker_sock.display())],
        env: Vec::new(),
        log: session_dir.join("ctrltrack.log"),
    }
}

super::proc::client_component!(Ctrltrack);
