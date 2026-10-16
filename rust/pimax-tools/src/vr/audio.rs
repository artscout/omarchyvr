//! Sound and microphone while VR runs (docs/AUDIO.md).
//!
//! Speakers: the Dream Air's USB audio is a microphone only (no OUT endpoint). Its speakers are DisplayPort audio
//! through the NVIDIA HDA codec, exactly as on Windows ("Pimax m (NVIDIA High Definition Audio)"). NVKMS fills the
//! ELD when the headset's head is lit (Monado or SteamVR direct mode), so the card gets a port named "Pimax"
//! (`device.product.name`), e.g. `hdmi-output-1` in profile `output:hdmi-stereo-extra1`. WirePlumber does not switch
//! card profiles by itself (`api.acp.auto-profile = false`) and the projector's port has the higher priority, so the
//! card stays on `output:hdmi-stereo` and no Pimax sink exists. At start we select the profile of the Pimax port
//! (the old one is saved), make its sink the default output, and put both back at Stop. The port appears a few
//! seconds after the headset lights up (SteamVR: after its compositor starts), so a small background helper
//! (`pimax vr audio follow DIR`) waits up to [`FOLLOW_SECS`] for it.
//!
//! Microphone: the USB source whose names mention Pimax becomes the default input while VR runs and the previous
//! default comes back at Stop. `pimax-voice listen` already captures "Pimax" by default (PipeWire converts the
//! mic's 2 ch 48 kHz to the 1 ch 16 kHz that speech-to-text wants). Sunshine and other devices are never chosen.
//!
//! Settings: ~/.config/pimax/audio.toml (`headset_sound`, `headset_mic`, both default true). Only `pactl`,
//! `pw-record` and `pw-play` are used; nothing is root-owned.
use std::path::{Path, PathBuf};
use std::process::Command;

/// How long the background helper waits for the headset speaker to appear, seconds.
pub const FOLLOW_SECS: u64 = 60;

/// Settings file.
pub fn settings_path(home: &Path) -> PathBuf {
    home.join(".config/pimax/audio.toml")
}

fn parse_key(t: &str, key: &str) -> Option<bool> {
    t.lines().find_map(|l| {
        let l = l.split('#').next()?.trim();
        let rest = l.strip_prefix(key)?.trim();
        let v = rest.strip_prefix('=')?.trim().trim_matches('"');
        match v {
            "true" | "on" => Some(true),
            "false" | "off" => Some(false),
            _ => None,
        }
    })
}

/// Parse the "Sound in headset" switch.
pub fn parse_settings(t: &str) -> Option<bool> {
    parse_key(t, "headset_sound")
}

/// Parse the "Headset microphone" switch.
pub fn parse_mic(t: &str) -> Option<bool> {
    parse_key(t, "headset_mic")
}

fn read(home: &Path) -> String {
    std::fs::read_to_string(settings_path(home)).unwrap_or_default()
}

/// The saved sound switch (default on).
pub fn enabled(home: &Path) -> bool {
    parse_settings(&read(home)).unwrap_or(true)
}

/// The saved microphone switch (default on).
pub fn mic_enabled(home: &Path) -> bool {
    parse_mic(&read(home)).unwrap_or(true)
}

fn write(home: &Path, sound: bool, mic: bool) -> std::io::Result<()> {
    let p = settings_path(home);
    std::fs::create_dir_all(p.parent().unwrap())?;
    std::fs::write(
        p,
        format!("# Sound and microphone in the headset (Pimax app). true | false\nheadset_sound = {sound}\nheadset_mic = {mic}\n"),
    )
}

/// Save the sound switch (the microphone switch is kept).
pub fn save(home: &Path, on: bool) -> std::io::Result<()> {
    write(home, on, mic_enabled(home))
}

/// Save the microphone switch (the sound switch is kept).
pub fn save_mic(home: &Path, on: bool) -> std::io::Result<()> {
    write(home, enabled(home), on)
}

/// One sink or source from `pactl list sinks|sources`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sink {
    /// Node name.
    pub name: String,
    /// All the text of its block, lower case (description, alsa names, ELD monitor names ...).
    pub text: String,
}

/// Split `pactl list sinks` (or `sources`) output into blocks.
pub fn parse_sinks(out: &str) -> Vec<Sink> {
    let mut v: Vec<Sink> = Vec::new();
    for l in out.lines() {
        if l.starts_with("Sink #") || l.starts_with("Source #") {
            v.push(Sink { name: String::new(), text: String::new() });
        } else if let Some(s) = v.last_mut() {
            if let Some(n) = l.trim().strip_prefix("Name:") {
                s.name = n.trim().to_string();
            }
            s.text.push_str(&l.to_lowercase());
            s.text.push('\n');
        }
    }
    v.retain(|s| !s.name.is_empty());
    v
}

fn is_sunshine(name: &str) -> bool {
    name.to_lowercase().contains("sunshine")
}

/// The headset's own sink: a sink whose names mention Pimax.
pub fn headset_sink(sinks: &[Sink]) -> Option<&Sink> {
    sinks.iter().find(|s| !is_sunshine(&s.name) && s.text.contains("pimax"))
}

/// The headset microphone: a real (not monitor) source whose names mention Pimax.
pub fn headset_mic(sources: &[Sink]) -> Option<&Sink> {
    let monitor = |s: &Sink| {
        s.name.ends_with(".monitor")
            || s.text.lines().any(|l| l.trim().strip_prefix("monitor of sink:").is_some_and(|v| v.trim() != "n/a"))
    };
    sources.iter().find(|s| !is_sunshine(&s.name) && !monitor(s) && s.text.contains("pimax"))
}

/// What to do at start: the headset sink to make default, or None. Never any other output.
pub fn choose(sinks: &[Sink], default: &str) -> Option<String> {
    headset_sink(sinks).filter(|h| h.name != default).map(|h| h.name.clone())
}

/// One card port from `pactl list cards`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Port {
    pub name: String,
    pub available: bool,
    /// `device.product.name` (the monitor name from the ELD for HDMI/DP ports).
    pub product: String,
    pub profiles: Vec<String>,
}

/// One card from `pactl list cards`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Card {
    pub name: String,
    pub active: String,
    pub ports: Vec<Port>,
}

fn tabs(l: &str) -> usize {
    l.chars().take_while(|c| *c == '\t').count()
}

/// Parse `LC_ALL=C pactl list cards`.
pub fn parse_cards(out: &str) -> Vec<Card> {
    let mut v: Vec<Card> = Vec::new();
    let mut in_ports = false;
    for l in out.lines() {
        if l.starts_with("Card #") {
            v.push(Card::default());
            in_ports = false;
            continue;
        }
        let Some(c) = v.last_mut() else { continue };
        let t = l.trim();
        match tabs(l) {
            1 => {
                in_ports = t == "Ports:";
                if let Some(n) = t.strip_prefix("Name:") {
                    c.name = n.trim().into();
                } else if let Some(n) = t.strip_prefix("Active Profile:") {
                    c.active = n.trim().into();
                }
            }
            2 if in_ports => {
                if let Some((name, rest)) = t.split_once(": ") {
                    c.ports.push(Port { name: name.into(), available: !rest.contains("not available"), ..Port::default() });
                }
            }
            n if in_ports && n >= 3 => {
                let Some(p) = c.ports.last_mut() else { continue };
                if let Some(x) = t.strip_prefix("device.product.name = ") {
                    p.product = x.trim_matches('"').into();
                } else if let Some(x) = t.strip_prefix("Part of profile(s):") {
                    p.profiles = x.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                }
            }
            _ => {}
        }
    }
    v.retain(|c| !c.name.is_empty());
    v
}

/// The card and profile that carry the headset speaker: an available port whose product name mentions Pimax;
/// its stereo output profile is preferred.
pub fn headset_port(cards: &[Card]) -> Option<(&Card, &Port, String)> {
    cards.iter().find_map(|c| {
        let p = c.ports.iter().find(|p| p.available && p.product.to_lowercase().contains("pimax"))?;
        let prof = p
            .profiles
            .iter()
            .find(|x| x.starts_with("output:hdmi-stereo"))
            .or_else(|| p.profiles.first())?
            .clone();
        Some((c, p, prof))
    })
}

/// The sink PipeWire makes for `profile` on `card` (`alsa_card.X` + `output:Y` -> `alsa_output.X.Y`).
pub fn sink_for(card: &str, profile: &str) -> String {
    format!(
        "alsa_output.{}.{}",
        card.strip_prefix("alsa_card.").unwrap_or(card),
        profile.strip_prefix("output:").unwrap_or(profile)
    )
}

/// The speaker step at a given moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Speaker {
    /// The default output is already the headset.
    Already(String),
    /// Make this existing sink the default.
    Default(String),
    /// Switch `card` from `from` to `to` (then `sink` appears and becomes the default).
    Profile { card: String, from: String, to: String, sink: String },
    /// No headset speaker (yet).
    Missing,
}

/// Decide the speaker step (pure).
pub fn plan_speaker(sinks: &[Sink], cards: &[Card], default: &str) -> Speaker {
    if let Some(h) = headset_sink(sinks) {
        return if h.name == default { Speaker::Already(h.name.clone()) } else { Speaker::Default(h.name.clone()) };
    }
    if let Some((c, _, to)) = headset_port(cards) {
        if c.active != to {
            return Speaker::Profile { card: c.name.clone(), from: c.active.clone(), sink: sink_for(&c.name, &to), to };
        }
        // profile already active but no sink with Pimax in its names: its sink by name
        let s = sink_for(&c.name, &to);
        if sinks.iter().any(|x| x.name == s) {
            return if s == default { Speaker::Already(s) } else { Speaker::Default(s) };
        }
    }
    Speaker::Missing
}

/// The microphone step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mic {
    Already(String),
    Default(String),
    Missing,
}

/// Decide the microphone step (pure).
pub fn plan_mic(sources: &[Sink], default: &str) -> Mic {
    match headset_mic(sources) {
        Some(m) if m.name == default => Mic::Already(m.name.clone()),
        Some(m) => Mic::Default(m.name.clone()),
        None => Mic::Missing,
    }
}

/// Plain-words line when the headset has no speaker output.
pub const NO_SPEAKER: &str =
    "No headset speaker yet (it appears over the video cable a few seconds after the headset lights up)";

fn pactl(args: &[&str]) -> Option<String> {
    let o = Command::new("pactl").env("LC_ALL", "C").args(args).output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).into_owned())
}

fn prev_file(session_dir: &Path) -> PathBuf {
    session_dir.join("audio-prev-default.txt")
}
fn prev_profile_file(session_dir: &Path) -> PathBuf {
    session_dir.join("audio-prev-profile.txt")
}
fn prev_source_file(session_dir: &Path) -> PathBuf {
    session_dir.join("audio-prev-source.txt")
}
fn follow_marker(session_dir: &Path) -> PathBuf {
    session_dir.join("audio-follow.on")
}

/// Write `v` to `p` unless a value from this session is already saved (the first one is the one to restore).
fn save_once(p: &Path, v: &str) {
    if !p.exists() {
        let _ = std::fs::write(p, v);
    }
}

/// One attempt at the speaker; Some(line) when done (switched or already), None when the headset speaker is missing.
fn speaker_once(session_dir: &Path) -> Option<String> {
    let sinks = parse_sinks(&pactl(&["list", "sinks"])?);
    let cards = parse_cards(&pactl(&["list", "cards"]).unwrap_or_default());
    let default = pactl(&["get-default-sink"]).unwrap_or_default().trim().to_string();
    match plan_speaker(&sinks, &cards, &default) {
        Speaker::Already(_) => Some("sound: already on the headset".into()),
        Speaker::Default(s) => {
            save_once(&prev_file(session_dir), &default);
            pactl(&["set-default-sink", &s])?;
            Some(format!("sound: default output is now {s} (the headset); {default} comes back at Stop"))
        }
        Speaker::Profile { card, from, to, sink } => {
            save_once(&prev_profile_file(session_dir), &format!("{card} {from}"));
            save_once(&prev_file(session_dir), &default);
            pactl(&["set-card-profile", &card, &to])?;
            for _ in 0..10 {
                let sinks = parse_sinks(&pactl(&["list", "sinks"]).unwrap_or_default());
                if sinks.iter().any(|x| x.name == sink) {
                    pactl(&["set-default-sink", &sink])?;
                    return Some(format!(
                        "sound: the headset speaker is on ({card} profile {to}, sink {sink}); {default} and profile {from} come back at Stop"
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
            Some(format!("sound: switched {card} to {to} but its sink did not appear"))
        }
        Speaker::Missing => None,
    }
}

fn mic_once(session_dir: &Path) -> String {
    let Some(out) = pactl(&["list", "sources"]) else {
        return "microphone: pactl not available".into();
    };
    let default = pactl(&["get-default-source"]).unwrap_or_default().trim().to_string();
    match plan_mic(&parse_sinks(&out), &default) {
        Mic::Already(_) => "microphone: already the headset microphone".into(),
        Mic::Default(m) => {
            save_once(&prev_source_file(session_dir), &default);
            if pactl(&["set-default-source", &m]).is_none() {
                return format!("microphone: could not select {m}");
            }
            format!("microphone: default input is now {m} (the headset); {default} comes back at Stop")
        }
        Mic::Missing => "microphone: no headset microphone found (is the headset on USB?)".into(),
    }
}

/// Start the background helper that waits for the speaker (detached, own session; log in the session dir).
fn spawn_follow(session_dir: &Path) -> Result<u32, String> {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let log = std::fs::OpenOptions::new().create(true).append(true).open(session_dir.join("audio.log"));
    let mut c = Command::new(exe);
    c.args(["vr", "audio", "follow"]).arg(session_dir).stdin(std::process::Stdio::null());
    if let Ok(l) = log {
        if let Ok(l2) = l.try_clone() {
            c.stdout(l2);
        }
        c.stderr(l);
    }
    // SAFETY: setsid(2) is async-signal-safe; the helper outlives the launcher.
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    c.spawn().map(|ch| ch.id()).map_err(|e| e.to_string())
}

/// At start (Play, Work, SteamVR). Returns plain-words lines.
pub fn on_start(home: &Path, session_dir: &Path) -> Option<String> {
    let mut lines = Vec::new();
    if mic_enabled(home) {
        lines.push(mic_once(session_dir));
    }
    if enabled(home) {
        match speaker_once(session_dir) {
            Some(l) => lines.push(l),
            None => {
                let _ = std::fs::write(follow_marker(session_dir), "");
                match spawn_follow(session_dir) {
                    Ok(pid) => lines.push(format!(
                        "sound: waiting up to {FOLLOW_SECS} s for the headset speaker (pid {pid}, log {})",
                        session_dir.join("audio.log").display()
                    )),
                    Err(e) => lines.push(format!("sound: {NO_SPEAKER}; cannot wait for it: {e}")),
                }
            }
        }
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// `pimax vr audio follow DIR`: wait for the headset speaker while the session's marker exists.
pub fn follow(session_dir: &Path) {
    let marker = follow_marker(session_dir);
    for _ in 0..FOLLOW_SECS {
        if !marker.exists() {
            println!("sound: the session stopped before the headset speaker appeared");
            return;
        }
        if let Some(l) = speaker_once(session_dir) {
            println!("{l}");
            let _ = std::fs::remove_file(&marker);
            return;
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    let _ = std::fs::remove_file(&marker);
    println!("sound: {NO_SPEAKER}; default output left as it is");
}

/// At stop: the card profile, the default output and the default input go back (if they still exist).
pub fn on_stop(session_dir: &Path) -> Option<String> {
    let _ = std::fs::remove_file(follow_marker(session_dir));
    let mut lines = Vec::new();
    if let Ok(t) = std::fs::read_to_string(prev_profile_file(session_dir)) {
        let _ = std::fs::remove_file(prev_profile_file(session_dir));
        if let Some((card, prof)) = t.trim().split_once(' ') {
            if pactl(&["set-card-profile", card, prof]).is_some() {
                lines.push(format!("sound: {card} back to profile {prof}"));
            }
        }
    }
    if let Ok(prev) = std::fs::read_to_string(prev_file(session_dir)) {
        let _ = std::fs::remove_file(prev_file(session_dir));
        let prev = prev.trim().to_string();
        // the restored profile's sink needs a moment to come back
        for _ in 0..10 {
            let sinks = parse_sinks(&pactl(&["list", "sinks"]).unwrap_or_default());
            if !prev.is_empty() && sinks.iter().any(|s| s.name == prev) {
                if pactl(&["set-default-sink", &prev]).is_some() {
                    lines.push(format!("sound: default output back to {prev}"));
                }
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
    }
    if let Ok(prev) = std::fs::read_to_string(prev_source_file(session_dir)) {
        let _ = std::fs::remove_file(prev_source_file(session_dir));
        let prev = prev.trim();
        let srcs = parse_sinks(&pactl(&["list", "sources"]).unwrap_or_default());
        if !prev.is_empty() && srcs.iter().any(|s| s.name == prev) && pactl(&["set-default-source", prev]).is_some() {
            lines.push(format!("microphone: default input back to {prev}"));
        }
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// Level of a recording.
#[derive(Debug, Clone, PartialEq)]
pub struct Level {
    pub rms_db: f32,
    pub peak_db: f32,
    pub channels: u16,
    /// Every frame has the same value on all channels.
    pub identical: bool,
    pub seconds: f32,
}

/// Read a 16-bit PCM WAV and measure it (`None`: not a 16-bit PCM WAV). Tolerates a data size of 0 or too big
/// (a recorder stopped by a signal): the rest of the file is used.
pub fn wav_level(b: &[u8]) -> Option<Level> {
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return None;
    }
    let (mut i, mut ch, mut bits, mut rate) = (12usize, 0u16, 0u16, 0u32);
    while i + 8 <= b.len() {
        let id = &b[i..i + 4];
        let sz = u32::from_le_bytes(b[i + 4..i + 8].try_into().ok()?) as usize;
        let body = i + 8;
        if id == b"fmt " && body + 16 <= b.len() {
            ch = u16::from_le_bytes([b[body + 2], b[body + 3]]);
            rate = u32::from_le_bytes(b[body + 4..body + 8].try_into().ok()?);
            bits = u16::from_le_bytes([b[body + 14], b[body + 15]]);
        } else if id == b"data" {
            if bits != 16 || ch == 0 {
                return None;
            }
            let end = if sz == 0 || body + sz > b.len() { b.len() } else { body + sz };
            let s: Vec<i16> = b[body..end].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
            let frames = s.len() / ch as usize;
            if frames == 0 {
                return Some(Level { rms_db: -120.0, peak_db: -120.0, channels: ch, identical: true, seconds: 0.0 });
            }
            let (mut sum, mut peak) = (0f64, 0i32);
            let mut identical = true;
            for f in s.chunks_exact(ch as usize) {
                for &x in f {
                    sum += (x as f64) * (x as f64);
                    peak = peak.max((x as i32).abs());
                }
                if f.iter().any(|&x| x != f[0]) {
                    identical = false;
                }
            }
            let db = |v: f64| if v <= 0.0 { -120.0 } else { (20.0 * (v / 32768.0).log10()).max(-120.0) as f32 };
            let rms = (sum / (frames * ch as usize) as f64).sqrt();
            return Some(Level {
                rms_db: db(rms),
                peak_db: db(peak as f64),
                channels: ch,
                identical,
                seconds: frames as f32 / rate.max(1) as f32,
            });
        }
        i = body + sz + (sz & 1);
    }
    None
}

/// The plain-words verdict of a microphone level.
pub fn verdict(l: &Level) -> &'static str {
    if l.peak_db <= -80.0 {
        "silent (only zeros): the headset microphone sends zeros while the headset display is off; start VR (Play or Work) and try again"
    } else if l.peak_db >= -0.5 {
        "too loud (clipping): lower the microphone volume"
    } else if l.rms_db < -60.0 && l.peak_db < -40.0 {
        "very quiet: speak closer or raise the microphone volume"
    } else {
        "good"
    }
}

/// `pimax vr audio mic-test`: record 3 s from the headset mic (else the default input), print the level, play it
/// back on the current default output.
pub fn mic_test() -> Result<(), String> {
    let srcs = parse_sinks(&pactl(&["list", "sources"]).unwrap_or_default());
    let default = pactl(&["get-default-source"]).unwrap_or_default().trim().to_string();
    let (target, label) = match headset_mic(&srcs) {
        Some(m) => (m.name.clone(), "headset microphone"),
        None => (default.clone(), "default input (no headset microphone found)"),
    };
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let wav = dir.join("pimax-mic-test.wav");
    let _ = std::fs::remove_file(&wav);
    println!("Recording 3 seconds from the {label} ({target}): speak now ...");
    let mut c = Command::new("timeout");
    c.args(["-s", "INT", "3", "pw-record", "--rate", "48000", "--channels", "2", "--format", "s16"]);
    if !target.is_empty() {
        c.args(["--target", &target]);
    }
    let st = c.arg(&wav).status().map_err(|e| format!("cannot run pw-record: {e}"))?;
    // timeout exits 124 when it stopped the recorder (the normal case)
    if !st.success() && st.code() != Some(124) {
        return Err(format!("Microphone test: recording failed ({st})"));
    }
    let b = std::fs::read(&wav).map_err(|e| format!("Microphone test: no recording ({e})"))?;
    let l = wav_level(&b).ok_or("Microphone test: the recording is not a 16-bit WAV")?;
    println!(
        "Microphone test: {label}, level {:.0} dBFS (peak {:.0} dBFS, {:.1} s, {} ch{}): {}",
        l.rms_db,
        l.peak_db,
        l.seconds,
        l.channels,
        if l.channels > 1 && l.identical { ", channels identical" } else { "" },
        verdict(&l)
    );
    let out = pactl(&["get-default-sink"]).unwrap_or_default().trim().to_string();
    println!("Playing it back on {out} ...");
    let _ = Command::new("pw-play").arg(&wav).status();
    let _ = std::fs::remove_file(&wav);
    Ok(())
}

/// A short soft chime (660 Hz then 880 Hz, 0.35 s each, about -20 dBFS, 10 ms fades) as a 48 kHz 16-bit stereo WAV.
pub fn chime_wav() -> Vec<u8> {
    let rate = 48000u32;
    let (amp, part) = (3277.0f32, (rate as f32 * 0.35) as usize);
    let fade = (rate / 100) as usize;
    let mut s: Vec<i16> = Vec::with_capacity(part * 4);
    for f in [660.0f32, 880.0] {
        for i in 0..part {
            let env = (i.min(part - 1 - i) as f32 / fade as f32).min(1.0);
            let v = (amp * env * (2.0 * std::f32::consts::PI * f * i as f32 / rate as f32).sin()) as i16;
            s.push(v);
            s.push(v);
        }
    }
    let n = (s.len() * 2) as u32;
    let mut b = Vec::with_capacity(44 + n as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + n).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * 4).to_le_bytes());
    b.extend_from_slice(&4u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&n.to_le_bytes());
    for v in s {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b
}

/// Where the speaker test plays now (pure): the headset sink, or the plain-words reason it cannot.
pub fn speaker_test_target(sinks: &[Sink], cards: &[Card], default: &str) -> Result<String, &'static str> {
    match plan_speaker(sinks, cards, default) {
        Speaker::Already(s) | Speaker::Default(s) => Ok(s),
        Speaker::Profile { .. } => {
            Err("the headset speaker is there but not selected: turn \"Sound in headset\" on, then Stop and Play again")
        }
        Speaker::Missing => Err(
            "no headset speaker: it exists only while VR runs (Play, Work or SteamVR), a few seconds after the headset lights up",
        ),
    }
}

/// `pimax vr audio speaker-test`: play a short chime on the headset speaker only (never on another output).
pub fn speaker_test() -> Result<(), String> {
    let sinks = parse_sinks(&pactl(&["list", "sinks"]).unwrap_or_default());
    let cards = parse_cards(&pactl(&["list", "cards"]).unwrap_or_default());
    let default = pactl(&["get-default-sink"]).unwrap_or_default().trim().to_string();
    let sink = match speaker_test_target(&sinks, &cards, &default) {
        Ok(s) => s,
        Err(why) => {
            println!("Speaker test: {why}");
            return Err(format!("Speaker test: {why}"));
        }
    };
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let wav = dir.join("pimax-speaker-test.wav");
    std::fs::write(&wav, chime_wav()).map_err(|e| format!("Speaker test: cannot write the sound ({e})"))?;
    println!("Playing a short chime on the headset speaker ({sink}) ...");
    let st = Command::new("pw-play").args(["--target", &sink]).arg(&wav).status();
    let _ = std::fs::remove_file(&wav);
    match st {
        Ok(s) if s.success() => {
            println!("Speaker test: played on the headset speaker; you should have heard two soft notes");
            Ok(())
        }
        Ok(s) => Err(format!("Speaker test: playing failed ({s})")),
        Err(e) => Err(format!("Speaker test: cannot run pw-play ({e})")),
    }
}

/// `pimax vr audio [status|on|off|mic-on|mic-off|mic-test|speaker-test|follow DIR]`.
pub fn cli(what: &str, arg: Option<&str>, home: &Path) -> Result<(), String> {
    match what {
        "on" | "off" => {
            save(home, what == "on").map_err(|e| e.to_string())?;
            println!("Sound in headset: {what}");
            Ok(())
        }
        "mic-on" | "mic-off" => {
            save_mic(home, what == "mic-on").map_err(|e| e.to_string())?;
            println!("Headset microphone: {}", &what[4..]);
            Ok(())
        }
        "mic-test" => mic_test(),
        "speaker-test" => speaker_test(),
        "follow" => {
            follow(Path::new(arg.ok_or("follow needs the session dir")?));
            Ok(())
        }
        "status" => {
            println!("Sound in headset: {}", if enabled(home) { "on" } else { "off" });
            println!("Headset microphone: {}", if mic_enabled(home) { "on" } else { "off" });
            if let Some(o) = pactl(&["list", "sinks"]) {
                let sinks = parse_sinks(&o);
                let cards = parse_cards(&pactl(&["list", "cards"]).unwrap_or_default());
                let d = pactl(&["get-default-sink"]).unwrap_or_default();
                println!("Default output: {}", d.trim());
                let h = match plan_speaker(&sinks, &cards, d.trim()) {
                    Speaker::Already(s) | Speaker::Default(s) => s,
                    Speaker::Profile { card, to, .. } => format!("present on {card} (profile {to}, selected at Play)"),
                    Speaker::Missing => NO_SPEAKER.into(),
                };
                println!("Headset output: {h}");
            }
            if let Some(o) = pactl(&["list", "sources"]) {
                let d = pactl(&["get-default-source"]).unwrap_or_default();
                println!("Default input: {}", d.trim());
                println!(
                    "Headset microphone device: {}",
                    headset_mic(&parse_sinks(&o)).map(|s| s.name.as_str()).unwrap_or("not found")
                );
            }
            Ok(())
        }
        w => Err(format!("unknown choice {w} (on|off|mic-on|mic-off|mic-test|speaker-test|status)")),
    }
}

/// "headset" / "other" for the app, from `pactl get-default-source`.
pub fn mic_kind(default_source: &str) -> &'static str {
    let d = default_source.to_lowercase();
    if d.contains("pimax") && !d.ends_with(".monitor") {
        "headset"
    } else {
        "other"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUT: &str = "Sink #57\n\tName: alsa_output.pci-0000_01_00.1.hdmi-stereo\n\tDescription: GB202 HDMI Audio Digital Stereo (HDMI)\n\talsa.name = \"HDMI 0 [EPSON PJ]\"\nSink #75\n\tName: sink-sunshine-stereo\n\tDescription: Sunshine\n";

    // `pactl list cards` of the NVIDIA HDA card while Monado drives the headset (analysis/overnight/desk.md section 7)
    const CARDS: &str = "Card #48\n\tName: alsa_card.pci-0000_01_00.1\n\tDriver: alsa\n\tProperties:\n\t\tapi.acp.auto-profile = \"false\"\n\tProfiles:\n\t\toff: Off (sinks: 0, sources: 0, priority: 0, available: yes)\n\t\toutput:hdmi-stereo: Digital Stereo (HDMI) Output (sinks: 1, sources: 0, priority: 5900, available: yes)\n\t\toutput:hdmi-stereo-extra1: Digital Stereo (HDMI 2) Output (sinks: 1, sources: 0, priority: 5700, available: yes)\n\tActive Profile: output:hdmi-stereo\n\tPorts:\n\t\thdmi-output-0: HDMI / DisplayPort (type: HDMI, priority: 5900, latency offset: 0 usec, availability group: Legacy 1, available)\n\t\t\tProperties:\n\t\t\t\tport.type = \"hdmi\"\n\t\t\t\tdevice.product.name = \"EPSON PJ\"\n\t\t\tPart of profile(s): output:hdmi-stereo\n\t\thdmi-output-1: HDMI / DisplayPort 2 (type: HDMI, priority: 5800, latency offset: 0 usec, availability group: Legacy 2, available)\n\t\t\tProperties:\n\t\t\t\tport.type = \"hdmi\"\n\t\t\t\tdevice.product.name = \"Pimax\"\n\t\t\tPart of profile(s): output:hdmi-stereo-extra1, output:hdmi-surround-extra1, output:hdmi-surround71-extra1\n\t\thdmi-output-2: HDMI / DisplayPort 3 (type: HDMI, priority: 5700, latency offset: 0 usec, availability group: Legacy 3, not available)\n\t\t\tProperties:\n\t\t\t\tport.type = \"hdmi\"\n\t\t\tPart of profile(s): output:hdmi-stereo-extra2\nCard #49\n\tName: alsa_card.usb-Pimax_Corporation_Pimax_Dream_Air_P511-04\n\tActive Profile: input:analog-stereo\n\tPorts:\n\t\tanalog-input-mic: Microphone (type: Mic, priority: 8700, latency offset: 0 usec, availability unknown)\n\t\t\tPart of profile(s): input:analog-stereo\n";

    const SOURCES: &str = "Source #57\n\tName: alsa_output.pci-0000_01_00.1.hdmi-stereo.monitor\n\tDescription: Monitor of GB202 HDMI Audio\nSource #58\n\tName: alsa_input.usb-Pimax_Corporation_Pimax_Dream_Air_P511X0X000000000002-04.analog-stereo\n\tDescription: Pimax Dream Air Analog Stereo\n\tMonitor of Sink: n/a\nSource #75\n\tName: sink-sunshine-stereo.monitor\n\tDescription: Monitor of Sunshine\n";

    #[test]
    fn settings() {
        assert_eq!(parse_settings("headset_sound = false # x"), Some(false));
        assert_eq!(parse_settings("# c\nheadset_sound=true"), Some(true));
        assert_eq!(parse_settings("x"), None);
        assert_eq!(parse_mic("headset_sound = true\nheadset_mic = false\n"), Some(false));
        assert_eq!(parse_mic("headset_sound = true\n"), None);
    }

    #[test]
    fn no_headset_sink_changes_nothing() {
        let s = parse_sinks(OUT);
        assert_eq!(s.len(), 2);
        assert_eq!(choose(&s, "sink-sunshine-stereo"), None);
        assert_eq!(choose(&s, "alsa_output.pci-0000_00_1f.3.analog-stereo"), None);
        assert_eq!(plan_speaker(&s, &[], "sink-sunshine-stereo"), Speaker::Missing);
    }

    #[test]
    fn headset_sink_wins() {
        let o = format!("{OUT}Sink #90\n\tName: alsa_output.pci-0000_01_00.1.hdmi-stereo-extra1\n\talsa.name = \"HDMI 1 [Pimax]\"\n");
        let s = parse_sinks(&o);
        assert_eq!(choose(&s, "sink-sunshine-stereo"), Some("alsa_output.pci-0000_01_00.1.hdmi-stereo-extra1".into()));
        assert_eq!(choose(&s, "alsa_output.pci-0000_01_00.1.hdmi-stereo-extra1"), None);
    }

    #[test]
    fn cards_and_the_pimax_port() {
        let c = parse_cards(CARDS);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].active, "output:hdmi-stereo");
        assert_eq!(c[0].ports.len(), 3);
        assert_eq!(c[0].ports[1].product, "Pimax");
        assert!(c[0].ports[1].available && !c[0].ports[2].available);
        assert_eq!(c[0].ports[1].profiles[0], "output:hdmi-stereo-extra1");
        let (card, port, prof) = headset_port(&c).unwrap();
        assert_eq!(
            (card.name.as_str(), port.name.as_str(), prof.as_str()),
            ("alsa_card.pci-0000_01_00.1", "hdmi-output-1", "output:hdmi-stereo-extra1")
        );
        assert_eq!(sink_for(&card.name, &prof), "alsa_output.pci-0000_01_00.1.hdmi-stereo-extra1");
    }

    #[test]
    fn speaker_plan_switches_the_profile_then_uses_the_sink() {
        let c = parse_cards(CARDS);
        let s = parse_sinks(OUT);
        assert_eq!(
            plan_speaker(&s, &c, "sink-sunshine-stereo"),
            Speaker::Profile {
                card: "alsa_card.pci-0000_01_00.1".into(),
                from: "output:hdmi-stereo".into(),
                to: "output:hdmi-stereo-extra1".into(),
                sink: "alsa_output.pci-0000_01_00.1.hdmi-stereo-extra1".into()
            }
        );
        // after the switch: the sink exists (its names need not mention Pimax)
        let mut c2 = c.clone();
        c2[0].active = "output:hdmi-stereo-extra1".into();
        let s2 = parse_sinks(
            "Sink #91\n\tName: alsa_output.pci-0000_01_00.1.hdmi-stereo-extra1\n\tDescription: GB202 HDMI 2\nSink #75\n\tName: sink-sunshine-stereo\n",
        );
        assert_eq!(plan_speaker(&s2, &c2, "sink-sunshine-stereo"), Speaker::Default("alsa_output.pci-0000_01_00.1.hdmi-stereo-extra1".into()));
        assert_eq!(
            plan_speaker(&s2, &c2, "alsa_output.pci-0000_01_00.1.hdmi-stereo-extra1"),
            Speaker::Already("alsa_output.pci-0000_01_00.1.hdmi-stereo-extra1".into())
        );
        // headset dark: the port is not available, nothing is switched
        let dark = CARDS.replace("Legacy 2, available", "Legacy 2, not available");
        assert_eq!(plan_speaker(&s, &parse_cards(&dark), "sink-sunshine-stereo"), Speaker::Missing);
    }

    #[test]
    fn mic_plan_picks_the_headset_never_a_monitor() {
        let s = parse_sinks(SOURCES);
        assert_eq!(s.len(), 3);
        let mic = "alsa_input.usb-Pimax_Corporation_Pimax_Dream_Air_P511X0X000000000002-04.analog-stereo";
        assert_eq!(plan_mic(&s, "sink-sunshine-stereo.monitor"), Mic::Default(mic.into()));
        assert_eq!(plan_mic(&s, mic), Mic::Already(mic.into()));
        let monitors_only = parse_sinks("Source #1\n\tName: x.pimax.monitor\n\tMonitor of Sink: x.pimax\nSource #2\n\tName: y\n\tDescription: pimax\n\tMonitor of Sink: y-sink\n");
        assert_eq!(plan_mic(&monitors_only, ""), Mic::Missing);
        assert_eq!(mic_kind(mic), "headset");
        assert_eq!(mic_kind("sink-sunshine-stereo.monitor"), "other");
    }

    fn wav(ch: u16, samples: &[i16], data_size: Option<u32>) -> Vec<u8> {
        let mut b = Vec::new();
        let n = (samples.len() * 2) as u32;
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + n).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&ch.to_le_bytes());
        b.extend_from_slice(&48000u32.to_le_bytes());
        b.extend_from_slice(&(48000 * 2 * ch as u32).to_le_bytes());
        b.extend_from_slice(&(2 * ch).to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data_size.unwrap_or(n).to_le_bytes());
        for s in samples {
            b.extend_from_slice(&s.to_le_bytes());
        }
        b
    }

    #[test]
    fn levels_and_verdicts() {
        let z = wav_level(&wav(2, &vec![0; 9600], None)).unwrap();
        assert!(z.peak_db <= -80.0 && z.identical && (z.seconds - 0.1).abs() < 1e-3);
        assert!(verdict(&z).starts_with("silent"));
        // a -20 dBFS square wave on both channels, data size 0 (recorder stopped by a signal)
        let a = 3277i16;
        let sq: Vec<i16> = (0..9600).map(|i| if (i / 2 / 24) % 2 == 0 { a } else { -a }).collect();
        let l = wav_level(&wav(2, &sq, Some(0))).unwrap();
        assert!((l.rms_db + 20.0).abs() < 0.1 && l.identical, "{l:?}");
        assert_eq!(verdict(&l), "good");
        let clip = wav_level(&wav(1, &[32767, -32768, 0, 0], None)).unwrap();
        assert!(verdict(&clip).starts_with("too loud"));
        assert!(wav_level(b"not a wav").is_none());
    }

    #[test]
    fn speaker_test_plays_only_on_the_headset() {
        let c = parse_cards(CARDS);
        let s = parse_sinks(OUT);
        // headset lit but profile not selected: refuse with a reason, never play on the projector
        assert!(speaker_test_target(&s, &c, "sink-sunshine-stereo").unwrap_err().contains("not selected"));
        assert!(speaker_test_target(&s, &[], "sink-sunshine-stereo").unwrap_err().contains("only while VR runs"));
        let mut c2 = c.clone();
        c2[0].active = "output:hdmi-stereo-extra1".into();
        let s2 = parse_sinks("Sink #91\n\tName: alsa_output.pci-0000_01_00.1.hdmi-stereo-extra1\n");
        assert_eq!(speaker_test_target(&s2, &c2, "x").unwrap(), "alsa_output.pci-0000_01_00.1.hdmi-stereo-extra1");
        let l = wav_level(&chime_wav()).unwrap();
        assert!((l.seconds - 0.7).abs() < 0.01 && l.channels == 2 && l.identical, "{l:?}");
        assert!(l.peak_db < -19.0 && l.peak_db > -21.0 && verdict(&l) == "good", "{l:?}");
    }

    #[test]
    fn save_and_read() {
        let h = crate::vr::test_dir("audio-settings");
        assert!(enabled(&h) && mic_enabled(&h));
        save(&h, false).unwrap();
        assert!(!enabled(&h) && mic_enabled(&h));
        save_mic(&h, false).unwrap();
        assert!(!enabled(&h) && !mic_enabled(&h));
        save(&h, true).unwrap();
        assert!(enabled(&h) && !mic_enabled(&h));
    }
}
