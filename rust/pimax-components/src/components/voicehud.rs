//! `pimax-voice-hud`, the head-locked XR_EXTX_overlay HUD (moved from the launcher's `desk.rs` in P3).
use super::proc::ProcSpec;
use std::path::Path;

pub fn spec(bin: &Path, runtime_json: &Path, session_dir: &Path) -> ProcSpec {
    ProcSpec {
        name: "voicehud",
        program: bin.to_path_buf(),
        args: Vec::new(),
        env: super::proc::xr_env(runtime_json),
        log: session_dir.join("voicehud.log"),
    }
}

super::proc::client_component!(VoiceHud);
