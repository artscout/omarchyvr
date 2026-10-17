//! The Work first-run questions (`~/.config/omarchyvr/work-setup.toml`; docs/WORK-FIRST-RUN.md).
//!
//! The first time the user presses Work, the app asks two questions before VR starts: the voice assistant (install
//! Lara, and which "brain" answers free-form requests) and speech to text. The answers themselves are saved where the
//! rest of the stack reads them: ai.toml (brain, Lara, the speech engine; [`crate::ai`]) and the app's switches (voice
//! on or off). This file only remembers that the questions were answered, and what was answered, so the next Work
//! press starts at once and the AI page can offer "change these answers". Play never reads it.
//!
//! `key = value` lines, written atomically (a temporary file, then rename); a missing or unreadable file means "not
//! answered yet".
use std::path::{Path, PathBuf};

/// The file name in the config directory (`~/.config/omarchyvr`).
pub const FILE: &str = "work-setup.toml";

/// Who answers free-form voice requests (what the command list does not know).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Brain {
    /// The Claude Code program installed on this PC, with its own sign-in (no key in our files).
    ClaudeCli,
    /// Claude through the user's own Anthropic API key (stored in ai.toml, mode 600).
    ClaudeKey,
    /// Qwen3.5-4B on this PC (the processor by default; the graphics card only when chosen).
    Local,
    /// Any OpenAI-compatible API: base URL, key and model.
    Compatible,
    /// No brain: only the built-in voice commands (nothing leaves the PC).
    None,
}

impl Brain {
    pub const ALL: [Brain; 5] = [Brain::ClaudeCli, Brain::ClaudeKey, Brain::Local, Brain::Compatible, Brain::None];

    pub fn id(self) -> &'static str {
        match self {
            Brain::ClaudeCli => "claude-cli",
            Brain::ClaudeKey => "claude-key",
            Brain::Local => "local",
            Brain::Compatible => "compatible",
            Brain::None => "none",
        }
    }

    pub fn parse(s: &str) -> Option<Brain> {
        Brain::ALL.into_iter().find(|b| b.id() == s.trim())
    }

    /// Lara can sort the commands for this brain (it needs a model that acts afterwards; the Claude CLI reads every
    /// command itself).
    pub fn takes_lara(self) -> bool {
        matches!(self, Brain::ClaudeKey | Brain::Local | Brain::Compatible)
    }
}

/// Speech to text in Work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speech {
    /// Parakeet on the processor (the default: no video memory).
    Cpu,
    /// Whisper large-v3-turbo on the graphics card (opt-in; about 0.9 GB of video memory, Work only).
    Gpu,
    /// No speech: no voice commands and no subtitles in Work.
    Off,
}

impl Speech {
    pub const ALL: [Speech; 3] = [Speech::Cpu, Speech::Gpu, Speech::Off];

    pub fn id(self) -> &'static str {
        match self {
            Speech::Cpu => "cpu",
            Speech::Gpu => "gpu",
            Speech::Off => "off",
        }
    }

    pub fn parse(s: &str) -> Option<Speech> {
        Speech::ALL.into_iter().find(|b| b.id() == s.trim())
    }
}

/// The record.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkSetup {
    /// The questions were answered (the next Work press starts at once).
    pub done: bool,
    /// When (unix seconds).
    pub at: u64,
    /// Lara was asked for.
    pub lara: bool,
    pub brain: Option<Brain>,
    /// The local model may use the graphics card (default: the processor).
    pub local_gpu: bool,
    pub speech: Option<Speech>,
}

impl WorkSetup {
    /// Parse the file text (unknown keys and bad values are ignored).
    pub fn parse(text: &str) -> WorkSetup {
        let mut s = WorkSetup::default();
        for l in text.lines() {
            let l = l.split('#').next().unwrap_or("").trim();
            let Some((k, v)) = l.split_once('=') else { continue };
            let v = v.trim().trim_matches('"');
            let on = matches!(v, "true" | "yes" | "1");
            match k.trim() {
                "done" => s.done = on,
                "at" => s.at = v.parse().unwrap_or(0),
                "lara" => s.lara = on,
                "brain" => s.brain = Brain::parse(v),
                "local_gpu" => s.local_gpu = on,
                "speech" => s.speech = Speech::parse(v),
                _ => {}
            }
        }
        s
    }

    /// The file text (never a key: those live in ai.toml only).
    pub fn render(&self) -> String {
        let mut o = String::from("# Work first-run answers (written by the omarchyvr app; docs/WORK-FIRST-RUN.md). The settings themselves are in ai.toml.\n");
        o += &format!("done = {}\nat = {}\nlara = {}\nlocal_gpu = {}\n", self.done, self.at, self.lara, self.local_gpu);
        if let Some(b) = self.brain {
            o += &format!("brain = \"{}\"\n", b.id());
        }
        if let Some(sp) = self.speech {
            o += &format!("speech = \"{}\"\n", sp.id());
        }
        o
    }

    /// The file in a config directory.
    pub fn path(config_dir: &Path) -> PathBuf {
        config_dir.join(FILE)
    }

    /// Read it (missing or unreadable: not answered).
    pub fn load(path: &Path) -> WorkSetup {
        std::fs::read_to_string(path).map(|t| WorkSetup::parse(&t)).unwrap_or_default()
    }

    /// Write it atomically (a temporary file in the same directory, then rename).
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let dir = path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let tmp = dir.join(format!(".{FILE}.tmp{}", std::process::id()));
        std::fs::write(&tmp, self.render()).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("{}: {e}", path.display())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_record_round_trips_and_a_missing_file_means_not_answered() {
        let d = std::env::temp_dir().join(format!("omarchyvr-work-setup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let p = WorkSetup::path(&d);
        assert!(!WorkSetup::load(&p).done, "no file: the questions come");
        let s = WorkSetup { done: true, at: 1792000000, lara: true, brain: Some(Brain::Local), local_gpu: false, speech: Some(Speech::Cpu) };
        s.save(&p).unwrap();
        assert_eq!(WorkSetup::load(&p), s);
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("brain = \"local\"") && text.contains("speech = \"cpu\""), "{text}");
        assert!(!text.contains("key"), "never a key here");
        // bad values are ignored, not fatal
        let b = WorkSetup::parse("done = true\nbrain = \"martian\"\nspeech = 3\n");
        assert!(b.done && b.brain.is_none() && b.speech.is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn lara_needs_a_brain_that_acts() {
        assert!(Brain::Local.takes_lara() && Brain::ClaudeKey.takes_lara() && Brain::Compatible.takes_lara());
        assert!(!Brain::ClaudeCli.takes_lara(), "the Claude CLI reads every command itself");
        assert!(!Brain::None.takes_lara());
        for b in Brain::ALL {
            assert_eq!(Brain::parse(b.id()), Some(b));
        }
        for s in Speech::ALL {
            assert_eq!(Speech::parse(s.id()), Some(s));
        }
    }
}
