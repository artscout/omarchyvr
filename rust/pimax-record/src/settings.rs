//! Recording settings: `~/.config/pimax/record.toml` (a plain `key = value` file), overridden per recording by the
//! command line (`--eyes both --fps 60 ...`).
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eyes {
    /// The left eye, lens-undistorted (what the compositor squashed before the lens distortion).
    Left,
    /// Both eyes side by side.
    Both,
}

impl Eyes {
    pub fn name(self) -> &'static str {
        match self {
            Eyes::Left => "left",
            Eyes::Both => "both",
        }
    }
    pub fn parse(s: &str) -> Option<Eyes> {
        match s {
            "left" | "one" | "1" => Some(Eyes::Left),
            "both" | "two" | "2" | "sbs" => Some(Eyes::Both),
            _ => None,
        }
    }
    pub fn count(self) -> u32 {
        match self {
            Eyes::Left => 1,
            Eyes::Both => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quality {
    Low,
    Medium,
    High,
}

impl Quality {
    pub fn name(self) -> &'static str {
        match self {
            Quality::Low => "low",
            Quality::Medium => "medium",
            Quality::High => "high",
        }
    }
    pub fn parse(s: &str) -> Option<Quality> {
        match s {
            "low" => Some(Quality::Low),
            "medium" | "med" => Some(Quality::Medium),
            "high" => Some(Quality::High),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    H264,
    Hevc,
}

impl Codec {
    pub fn name(self) -> &'static str {
        match self {
            Codec::H264 => "h264",
            Codec::Hevc => "hevc",
        }
    }
    pub fn parse(s: &str) -> Option<Codec> {
        match s {
            "h264" | "avc" => Some(Codec::H264),
            "hevc" | "h265" => Some(Codec::Hevc),
            _ => None,
        }
    }
}

/// Which encoder ffmpeg uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncoderPref {
    /// NVENC when ffmpeg has it and the GPU gate is ok, else the CPU.
    Auto,
    Nvenc,
    /// libx264 / libx265 on the CPU (niced).
    Cpu,
}

impl EncoderPref {
    pub fn name(self) -> &'static str {
        match self {
            EncoderPref::Auto => "auto",
            EncoderPref::Nvenc => "nvenc",
            EncoderPref::Cpu => "cpu",
        }
    }
    pub fn parse(s: &str) -> Option<EncoderPref> {
        match s {
            "auto" => Some(EncoderPref::Auto),
            "nvenc" | "gpu" => Some(EncoderPref::Nvenc),
            "cpu" | "x264" | "x265" | "software" => Some(EncoderPref::Cpu),
            _ => None,
        }
    }
}

/// Frame rates offered (the panel runs at 72 or 90 Hz; the tap paces to the request).
pub const FPS_CHOICES: [u32; 3] = [30, 60, 72];

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub eyes: Eyes,
    pub fps: u32,
    pub quality: Quality,
    /// 0 = fit `max_height`; else a fraction (0.1 to 1.0) of the eye's resolution, still capped by `max_height`.
    pub scale: f32,
    pub max_height: u32,
    pub codec: Codec,
    pub encoder: EncoderPref,
    /// Output directory (`~` is expanded).
    pub dir: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            eyes: Eyes::Left,
            fps: 30,
            quality: Quality::Medium,
            scale: 0.0,
            max_height: 1080,
            codec: Codec::H264,
            encoder: EncoderPref::Auto,
            dir: "~/Videos/pimax".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SettingsError(pub String);

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn bad(k: &str, v: &str, want: &str) -> SettingsError {
    SettingsError(format!("{k} = {v}: expected {want}"))
}

impl Settings {
    /// Apply one `key = value` (from the file or a `--key value` flag).
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), SettingsError> {
        let v = value.trim().trim_matches('"');
        match key.trim().replace('-', "_").as_str() {
            "eyes" => self.eyes = Eyes::parse(v).ok_or_else(|| bad(key, v, "left or both"))?,
            "fps" => {
                let n: u32 = v.parse().map_err(|_| bad(key, v, "30, 60 or 72"))?;
                if !FPS_CHOICES.contains(&n) {
                    return Err(bad(key, v, "30, 60 or 72"));
                }
                self.fps = n;
            }
            "quality" => self.quality = Quality::parse(v).ok_or_else(|| bad(key, v, "low, medium or high"))?,
            "scale" => {
                let f: f32 = v.parse().map_err(|_| bad(key, v, "0 (auto) or 0.1 to 1.0"))?;
                if !(f == 0.0 || (0.1..=1.0).contains(&f)) {
                    return Err(bad(key, v, "0 (auto) or 0.1 to 1.0"));
                }
                self.scale = f;
            }
            "max_height" => {
                let n: u32 = v.parse().map_err(|_| bad(key, v, "240 to 2160"))?;
                if !(240..=2160).contains(&n) {
                    return Err(bad(key, v, "240 to 2160"));
                }
                self.max_height = n;
            }
            "codec" => self.codec = Codec::parse(v).ok_or_else(|| bad(key, v, "h264 or hevc"))?,
            "encoder" => self.encoder = EncoderPref::parse(v).ok_or_else(|| bad(key, v, "auto, nvenc or cpu"))?,
            "dir" => {
                if v.is_empty() {
                    return Err(bad(key, v, "a directory"));
                }
                self.dir = v.to_string();
            }
            other => return Err(SettingsError(format!("unknown setting {other}"))),
        }
        Ok(())
    }

    /// Parse the file's text. Comments (`#`), blank lines and `[section]` headers are ignored; unknown keys are an
    /// error (a typo must not silently record at the wrong settings).
    pub fn parse(text: &str) -> Result<Settings, SettingsError> {
        let mut s = Settings::default();
        for (i, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() || line.starts_with('[') {
                continue;
            }
            let (k, v) = line
                .split_once('=')
                .ok_or_else(|| SettingsError(format!("line {}: expected key = value", i + 1)))?;
            s.set(k, v).map_err(|e| SettingsError(format!("line {}: {e}", i + 1)))?;
        }
        Ok(s)
    }

    pub fn to_toml(&self) -> String {
        format!(
            "# pimax record settings (pimax record --help). Written by `pimax record start --save` and the app.\n\
             eyes = \"{}\"        # left (one eye, lens-undistorted) | both (side by side)\n\
             fps = {}             # 30 | 60 | 72\n\
             quality = \"{}\"   # low | medium | high\n\
             scale = {}            # 0 = fit max_height; 0.1 to 1.0 = fraction of the eye's resolution\n\
             max_height = {}\n\
             codec = \"{}\"        # h264 | hevc\n\
             encoder = \"{}\"      # auto (NVENC, else CPU) | nvenc | cpu\n\
             dir = \"{}\"\n",
            self.eyes.name(),
            self.fps,
            self.quality.name(),
            self.scale,
            self.max_height,
            self.codec.name(),
            self.encoder.name(),
            self.dir
        )
    }

    /// The settings file, or the defaults if it does not exist.
    pub fn load(path: &Path) -> Result<Settings, SettingsError> {
        match std::fs::read_to_string(path) {
            Ok(t) => Settings::parse(&t).map_err(|e| SettingsError(format!("{}: {e}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(e) => Err(SettingsError(format!("{}: {e}", path.display()))),
        }
    }

    /// Write atomically (temp file + rename).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, self.to_toml())?;
        std::fs::rename(&tmp, path)
    }

    /// `dir` with `~` expanded.
    pub fn out_dir(&self, home: &Path) -> PathBuf {
        match self.dir.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None if self.dir == "~" => home.to_path_buf(),
            None => PathBuf::from(&self.dir),
        }
    }

    /// Per-eye output height the tap is asked for, as the request's `(max_height, scale_permille)`.
    pub fn request(&self) -> (u32, u32) {
        (self.max_height, (self.scale * 1000.0).round() as u32)
    }

    /// The command-line form (what `start` passes to `run`, so a pimaxd respawn records with the same settings).
    pub fn to_args(&self) -> Vec<String> {
        vec![
            "--eyes".into(),
            self.eyes.name().into(),
            "--fps".into(),
            self.fps.to_string(),
            "--quality".into(),
            self.quality.name().into(),
            "--scale".into(),
            self.scale.to_string(),
            "--max-height".into(),
            self.max_height.to_string(),
            "--codec".into(),
            self.codec.name().into(),
            "--encoder".into(),
            self.encoder.name().into(),
            "--dir".into(),
            self.dir.clone(),
        ]
    }
}

/// `~/.config/pimax/record.toml`.
pub fn settings_path(config_dir: &Path) -> PathBuf {
    config_dir.join("record.toml")
}

/// `pimax-YYYYmmdd-HHMMSS.mp4` in local time.
pub fn file_name(unix: i64) -> String {
    // SAFETY: localtime_r writes into the tm we own.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let t = unix as libc::time_t;
    unsafe { libc::localtime_r(&t, &mut tm) };
    format!(
        "pimax-{:04}{:02}{:02}-{:02}{:02}{:02}.mp4",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_one_eye_30_fps_medium() {
        let s = Settings::default();
        assert_eq!((s.eyes, s.fps, s.quality, s.codec, s.encoder), (Eyes::Left, 30, Quality::Medium, Codec::H264, EncoderPref::Auto));
        assert_eq!(s.request(), (1080, 0));
    }

    #[test]
    fn round_trips_through_the_file() {
        let mut s = Settings::default();
        s.set("eyes", "both").unwrap();
        s.set("fps", "72").unwrap();
        s.set("quality", "high").unwrap();
        s.set("scale", "0.5").unwrap();
        s.set("max-height", "1440").unwrap();
        s.set("codec", "hevc").unwrap();
        s.set("encoder", "cpu").unwrap();
        s.set("dir", "/data/vr").unwrap();
        assert_eq!(Settings::parse(&s.to_toml()).unwrap(), s);
        assert_eq!(s.request(), (1440, 500));
    }

    #[test]
    fn rejects_bad_values_and_unknown_keys() {
        let mut s = Settings::default();
        assert!(s.set("fps", "45").is_err());
        assert!(s.set("fps", "x").is_err());
        assert!(s.set("eyes", "three").is_err());
        assert!(s.set("quality", "ultra").is_err());
        assert!(s.set("scale", "1.5").is_err());
        assert!(s.set("scale", "0.05").is_err());
        assert!(s.set("max_height", "100").is_err());
        assert!(s.set("colour", "red").is_err());
        assert_eq!(s, Settings::default(), "a rejected value changes nothing");
        let e = Settings::parse("fps = 30\nfsp = 60\n").unwrap_err();
        assert!(e.0.contains("line 2"), "{e}");
    }

    #[test]
    fn parses_comments_quotes_and_sections() {
        let s = Settings::parse("# hi\n[record]\neyes = \"both\" # side by side\n\nfps=60\n").unwrap();
        assert_eq!((s.eyes, s.fps), (Eyes::Both, 60));
    }

    #[test]
    fn missing_file_means_defaults_and_save_is_atomic() {
        let d = std::env::temp_dir().join(format!("pimax-record-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let p = settings_path(&d);
        assert_eq!(Settings::load(&p).unwrap(), Settings::default());
        let s = Settings { fps: 60, ..Settings::default() };
        s.save(&p).unwrap();
        assert_eq!(Settings::load(&p).unwrap(), s);
        assert!(!p.with_extension("toml.tmp").exists());
        std::fs::write(&p, "fps = 61\n").unwrap();
        assert!(Settings::load(&p).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn expands_the_home_dir_and_names_files_by_time() {
        let s = Settings::default();
        assert_eq!(s.out_dir(Path::new("/home/u")), PathBuf::from("/home/u/Videos/pimax"));
        let n = file_name(1_792_032_882);
        assert!(n.starts_with("pimax-20") && n.ends_with(".mp4") && n.len() == "pimax-20261015-123456.mp4".len(), "{n}");
    }

    #[test]
    fn args_round_trip() {
        let s = Settings { eyes: Eyes::Both, fps: 60, scale: 0.75, ..Settings::default() };
        let a = s.to_args();
        let mut t = Settings::default();
        for kv in a.chunks(2) {
            t.set(kv[0].trim_start_matches("--"), &kv[1]).unwrap();
        }
        assert_eq!(t, s);
    }
}
