//! `omarchyvr-perf run`: Monado's stats file -> the wrist panel `perf` of WayVR through wayvrctl (moved from the
//! launcher's `perf.rs` in P3). Not an XR client.
use super::proc::ProcSpec;
use std::path::Path;

pub fn spec(bin: &Path, stats: &Path, wayvrctl: &Path, session_dir: &Path) -> ProcSpec {
    ProcSpec {
        name: "perf",
        program: bin.to_path_buf(),
        args: vec![
            "run".into(),
            "--file".into(),
            stats.to_string_lossy().into_owned(),
            "--wayvrctl".into(),
            wayvrctl.to_string_lossy().into_owned(),
        ],
        env: Vec::new(),
        log: session_dir.join("perf.log"),
    }
}

super::proc::client_component!(Perf);
