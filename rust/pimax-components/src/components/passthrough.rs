//! `pimax-passthrough run`, the room view as the desk's main OpenXR app (moved from the launcher's `desk.rs` in P3).
use super::proc::ProcSpec;
use std::path::Path;

/// The spawn spec. `preset_args`: the preset's passthrough flags (`--gain 0.35`); `broker`: read the frames from the
/// camera broker (6DoF) instead of opening the camera.
pub fn spec(bin: &Path, preset_args: &[&str], runtime_json: &Path, broker: bool, session_dir: &Path) -> ProcSpec {
    let mut args = vec!["run".to_string()];
    args.extend(preset_args.iter().map(|s| s.to_string()));
    let mut env = super::proc::xr_env(runtime_json);
    if broker {
        args.extend(["--source".into(), "broker".into()]);
        env.push(("PIMAX_CAM_SOURCE".into(), "broker".into()));
    }
    ProcSpec { name: "passthrough", program: bin.to_path_buf(), args, env, log: session_dir.join("passthrough.log") }
}

super::proc::client_component!(Passthrough);

#[cfg(test)]
mod tests {
    #[test]
    fn broker_source() {
        let s = super::spec("/b/p".as_ref(), &["--gain", "0.35"], "/r".as_ref(), true, "/s".as_ref());
        assert_eq!(s.args, ["run", "--gain", "0.35", "--source", "broker"]);
        assert_eq!(s.env.last().unwrap(), &("PIMAX_CAM_SOURCE".to_string(), "broker".to_string()));
    }
}
