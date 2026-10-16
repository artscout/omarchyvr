//! What the recorder tells the world: `<session dir>/record.status` (key=value, rewritten atomically about every
//! second and at every state change; read by `omarchyvr record status`, the app and voice), `<session dir>/record.events`
//! (one JSON object per line, the omarchyvrd event shape) and a desktop notification for the important ones.
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Status {
    /// starting | recording | finalizing | idle | error
    pub state: String,
    pub pid: u32,
    pub file: String,
    pub started_unix: f64,
    pub eyes: String,
    pub fps: u32,
    pub quality: String,
    pub encoder: String,
    /// What is mixed into the sound: "" (none), "output" or "output+mic".
    pub audio: String,
    pub width: u32,
    pub height: u32,
    /// Frames written to the encoder (duplicates included: the file is constant frame rate).
    pub frames: u64,
    /// Frames the tap dropped because its GPU readback was still busy (the compositor never waits).
    pub dropped_tap: u64,
    /// Frames the tap dropped because the ring was full (recorder or encoder behind).
    pub dropped_ring: u64,
    /// Frames repeated to keep the frame rate (the compositor delivered none in time).
    pub duplicated: u64,
    /// Last error or stop reason (empty while recording normally).
    pub note: String,
    pub updated_unix: f64,
}

impl Status {
    pub fn to_text(&self) -> String {
        format!(
            "state={}\npid={}\nfile={}\nstarted_unix={:.1}\neyes={}\nfps={}\nquality={}\nencoder={}\naudio={}\nwidth={}\nheight={}\n\
             frames={}\ndropped_tap={}\ndropped_ring={}\nduplicated={}\nnote={}\nupdated_unix={:.1}\n",
            self.state,
            self.pid,
            self.file,
            self.started_unix,
            self.eyes,
            self.fps,
            self.quality,
            self.encoder,
            self.audio,
            self.width,
            self.height,
            self.frames,
            self.dropped_tap,
            self.dropped_ring,
            self.duplicated,
            self.note.replace('\n', " "),
            self.updated_unix
        )
    }

    pub fn parse(text: &str) -> Status {
        let mut s = Status::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let n = || v.trim().parse::<u64>().unwrap_or(0);
            let f = || v.trim().parse::<f64>().unwrap_or(0.0);
            match k {
                "state" => s.state = v.into(),
                "pid" => s.pid = n() as u32,
                "file" => s.file = v.into(),
                "started_unix" => s.started_unix = f(),
                "eyes" => s.eyes = v.into(),
                "fps" => s.fps = n() as u32,
                "quality" => s.quality = v.into(),
                "encoder" => s.encoder = v.into(),
                "audio" => s.audio = v.into(),
                "width" => s.width = n() as u32,
                "height" => s.height = n() as u32,
                "frames" => s.frames = n(),
                "dropped_tap" => s.dropped_tap = n(),
                "dropped_ring" => s.dropped_ring = n(),
                "duplicated" => s.duplicated = n(),
                "note" => s.note = v.into(),
                "updated_unix" => s.updated_unix = f(),
                _ => {}
            }
        }
        s
    }

    pub fn load(p: &Path) -> Option<Status> {
        std::fs::read_to_string(p).ok().map(|t| Status::parse(&t))
    }

    pub fn save(&self, p: &Path) {
        let _ = omarchyvr_components::util::write_atomic(p, self.to_text().as_bytes());
    }

    /// Seconds recorded so far (or in total, once idle).
    pub fn seconds(&self) -> f64 {
        if self.fps == 0 {
            0.0
        } else {
            self.frames as f64 / self.fps as f64
        }
    }

    /// One line for humans.
    pub fn summary(&self) -> String {
        let secs = self.seconds();
        let len = format!("{}:{:02}", (secs / 60.0) as u64, secs as u64 % 60);
        match self.state.as_str() {
            "recording" => format!(
                "recording {len} to {} ({} eye(s), {}x{} at {} fps, {} quality, {}, sound: {}; dropped {} + {}, repeated {})",
                self.file,
                if self.eyes == "both" { 2 } else { 1 },
                self.width,
                self.height,
                self.fps,
                self.quality,
                self.encoder,
                if self.audio.is_empty() { "none" } else { &self.audio },
                self.dropped_tap,
                self.dropped_ring,
                self.duplicated
            ),
            "starting" => "starting (waiting for Monado's recorder tap)".into(),
            "finalizing" => format!("finishing {}", self.file),
            "error" => format!("not recording: {}", self.note),
            _ if !self.file.is_empty() => format!("not recording (last: {} {len}{})", self.file, if self.note.is_empty() { String::new() } else { format!(", {}", self.note) }),
            _ => "not recording".into(),
        }
    }
}

pub fn status_path(session_dir: &Path) -> PathBuf {
    session_dir.join("record.status")
}

pub fn events_path(session_dir: &Path) -> PathBuf {
    session_dir.join("record.events")
}

fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// The omarchyvrd event line shape: `{"event":E,"t":T,"id":"record","text":...}` (`t` is unix time here).
pub fn event_line(event: &str, unix: f64, text: &str) -> String {
    format!("{{\"event\":{},\"t\":{:.1},\"id\":\"record\",\"text\":{}}}", json_str(event), unix, json_str(text))
}

/// Append an event (`notice` or `critical`) and, if `notify`, show a desktop notification.
pub fn event(session_dir: &Path, event: &str, text: &str, notify: bool) {
    eprintln!("omarchyvr-record: {text}");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(events_path(session_dir)) {
        let _ = writeln!(f, "{}", event_line(event, omarchyvr_components::util::unix_now(), text));
    }
    if notify && std::env::var_os("PIMAX_RECORD_NO_NOTIFY").is_none() {
        let urgency = if event == "critical" { "critical" } else { "normal" };
        let _ = std::process::Command::new("notify-send")
            .args(["-a", "omarchyvr", "-u", urgency, "-i", "media-record", "Pimax recording", text])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(|mut c| std::thread::spawn(move || c.wait()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_summarises() {
        let s = Status {
            state: "recording".into(),
            pid: 42,
            file: "/v/pimax-1.mp4".into(),
            eyes: "left".into(),
            fps: 30,
            quality: "medium".into(),
            encoder: "h264_nvenc".into(),
            width: 1168,
            height: 1080,
            frames: 90,
            note: "a\nb".into(),
            ..Status::default()
        };
        let back = Status::parse(&s.to_text());
        assert_eq!(back.note, "a b");
        assert_eq!(Status { note: "a b".into(), ..s.clone() }, back);
        assert!(back.summary().starts_with("recording 0:03 to /v/pimax-1.mp4 (1 eye(s), 1168x1080 at 30 fps"), "{}", back.summary());
        let idle = Status { state: "idle".into(), note: "Monado stopped".into(), ..back };
        assert!(idle.summary().contains("last: /v/pimax-1.mp4 0:03, Monado stopped"), "{}", idle.summary());
    }

    #[test]
    fn events_are_json_lines() {
        assert_eq!(event_line("notice", 1.0, "saved \"x\"\n"), r#"{"event":"notice","t":1.0,"id":"record","text":"saved \"x\"\n"}"#);
    }
}
