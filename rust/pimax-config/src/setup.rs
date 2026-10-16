//! `setup.toml`: the launcher settings of a profile. A flat `key = value` subset of TOML (comments, strings, integers,
//! booleans); unknown keys are kept so newer tools do not lose data.
use std::collections::BTreeMap;

/// Launcher settings. `None` = not set, the launcher default applies.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Setup {
    /// Refresh rate, 72 or 90.
    pub fps: Option<u32>,
    /// Controller-driven hands (`PIMAX_CTRL_HANDS`).
    pub hands: Option<bool>,
    /// Controller pose mode (`ctrl_pose`).
    pub ctrl_pose: Option<String>,
    /// Backlight 0..=255 (only used once the flash question is settled).
    pub backlight: Option<u32>,
    /// DRM connector override.
    pub connector: Option<String>,
    /// Monado build directory.
    pub monado_build: Option<String>,
    /// Keys this version does not know, raw TOML values.
    pub extra: BTreeMap<String, String>,
}

impl Setup {
    /// Parse; errors name the line.
    pub fn parse(text: &str) -> Result<Setup, String> {
        let mut s = Setup::default();
        for (i, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (k, v) = line
                .split_once('=')
                .ok_or_else(|| format!("line {}: expected key = value", i + 1))?;
            let (k, v) = (k.trim(), strip_comment(v.trim()));
            let bad = |what: &str| format!("line {}: {k}: {what}", i + 1);
            match k {
                "fps" => {
                    let n: u32 = v.parse().map_err(|_| bad("not an integer"))?;
                    if n != 72 && n != 90 {
                        return Err(bad("must be 72 or 90"));
                    }
                    s.fps = Some(n);
                }
                "hands" => s.hands = Some(parse_bool(v).ok_or_else(|| bad("not true/false"))?),
                "backlight" => {
                    let n: u32 = v.parse().map_err(|_| bad("not an integer"))?;
                    if n > 255 {
                        return Err(bad("must be 0..=255"));
                    }
                    s.backlight = Some(n);
                }
                "ctrl_pose" => s.ctrl_pose = Some(parse_str(v).ok_or_else(|| bad("not a string"))?),
                "connector" => s.connector = Some(parse_str(v).ok_or_else(|| bad("not a string"))?),
                "monado_build" => {
                    s.monado_build = Some(parse_str(v).ok_or_else(|| bad("not a string"))?)
                }
                _ => {
                    s.extra.insert(k.to_string(), v.to_string());
                }
            }
        }
        Ok(s)
    }

    /// Serialise (stable key order).
    pub fn to_toml(&self) -> String {
        let mut o = String::from(
            "# Pimax Setup profile settings. Environment variables still override these.\n",
        );
        if let Some(v) = self.fps {
            o += &format!("fps = {v}\n");
        }
        if let Some(v) = self.hands {
            o += &format!("hands = {v}\n");
        }
        if let Some(v) = &self.ctrl_pose {
            o += &format!("ctrl_pose = {}\n", quote(v));
        }
        if let Some(v) = self.backlight {
            o += &format!("backlight = {v}\n");
        }
        if let Some(v) = &self.connector {
            o += &format!("connector = {}\n", quote(v));
        }
        if let Some(v) = &self.monado_build {
            o += &format!("monado_build = {}\n", quote(v));
        }
        for (k, v) in &self.extra {
            o += &format!("{k} = {v}\n");
        }
        o
    }
}

fn strip_comment(v: &str) -> &str {
    if let Some(rest) = v.strip_prefix('"') {
        // comment after the closing quote (first unescaped one)
        let b = rest.as_bytes();
        let mut i = 0;
        while i < b.len() {
            match b[i] {
                b'\\' => i += 1,
                b'"' => return v[..i + 2].trim(),
                _ => {}
            }
            i += 1;
        }
        return v;
    }
    v.split('#').next().unwrap_or("").trim()
}

fn parse_bool(v: &str) -> Option<bool> {
    match v {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn parse_str(v: &str) -> Option<String> {
    let inner = v.strip_prefix('"')?.strip_suffix('"')?;
    Some(inner.replace("\\\"", "\"").replace("\\\\", "\\"))
}

fn quote(v: &str) -> String {
    format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let s = Setup {
            fps: Some(90),
            hands: Some(false),
            ctrl_pose: Some("grip".into()),
            backlight: Some(200),
            connector: Some("DP-1".into()),
            monado_build: Some("/home/u/src/monado/build".into()),
            extra: [("future".to_string(), "[1, 2]".to_string())].into(),
        };
        assert_eq!(Setup::parse(&s.to_toml()).unwrap(), s);
    }

    #[test]
    fn rejects_out_of_range() {
        assert!(Setup::parse("fps = 60").unwrap_err().contains("72 or 90"));
        assert!(Setup::parse("backlight = 300").is_err());
        assert!(Setup::parse("hands = maybe").is_err());
        assert!(Setup::parse("garbage").is_err());
    }

    #[test]
    fn comments_are_ignored() {
        let s = Setup::parse("# hi\nfps = 72 # note\nconnector = \"DP-2\" # x\n").unwrap();
        assert_eq!(s.fps, Some(72));
        assert_eq!(s.connector.as_deref(), Some("DP-2"));
    }
}
