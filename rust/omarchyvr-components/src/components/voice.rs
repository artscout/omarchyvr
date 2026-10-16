//! The voice processes of a desk session (moved from the launcher's `desk.rs` in P3): `omarchyvr-voice listen` (the mic,
//! the recogniser, the socket), `captions` (finals -> the WayVR caption panel) and `act` (voice commands). None of
//! them is an XR client.
use super::proc::ProcSpec;
use std::path::Path;

/// `voice.listen`.
pub fn listen_spec(bin: &Path, session_dir: &Path) -> ProcSpec {
    ProcSpec { name: "voice", program: bin.to_path_buf(), args: vec!["listen".into()], env: Vec::new(), log: session_dir.join("voice.log") }
}

/// `voice.captions`: socket -> WayVR panel `captions` through wayvrctl; types nothing.
pub fn captions_spec(bin: &Path, wayvrctl: &Path, session_dir: &Path) -> ProcSpec {
    ProcSpec {
        name: "voicecap",
        program: bin.to_path_buf(),
        args: vec!["captions".into(), "--wayvrctl".into(), wayvrctl.to_string_lossy().into_owned()],
        env: Vec::new(),
        log: session_dir.join("voicecap.log"),
    }
}

/// `voice.act`: "Pimax ..." finals -> intents -> actions; feedback as WayVR toasts.
pub fn act_spec(bin: &Path, wayvrctl: &Path, session_dir: &Path) -> ProcSpec {
    ProcSpec {
        name: "voiceact",
        program: bin.to_path_buf(),
        args: vec!["act".into(), "--wayvrctl".into(), wayvrctl.to_string_lossy().into_owned()],
        env: Vec::new(),
        log: session_dir.join("voiceact.log"),
    }
}

super::proc::client_component!(VoiceListen);
super::proc::client_component!(VoiceCaptions);
super::proc::client_component!(VoiceAct);
