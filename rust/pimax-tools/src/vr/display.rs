//! "Brightness" (Home > Quality / Performance): the panel backlight and the image brightness.
//!
//! * Backlight, 20..=100 % (default 100 = the panel's maximum, `nvt_set_bl_brightness` 255). Sent exactly as the Windows
//!   capture `20261013-124541-ipd-brightness-hub2.pcap` shows (analysis/controllers-settings.md section 2): three
//!   packets on the command channel, 1 ms apart: `58 0d 26 00 'pbrg' 00000000 01 PCT`, `58 04 2a 00 PCT`
//!   (eSpiCmdPanelBrightnessAdj) and `58 0b 27 00 'pbrg' 00000000`. The panel keeps the value it was given last (it
//!   came up at 239 after a Windows session, 255 on Linux), so Play sends it again at every start, and the app sends it
//!   live when the slider moves. It is the only HID traffic of this module.
//! * Image brightness (`standard` | `bright` | `brighter`, default bright): `XRT_COMPOSITOR_OUTPUT_GAMMA` of the Monado
//!   compositor (encoded^g after the sRGB encode, g < 1 lifts the mid tones; black and white stay put, so nothing
//!   blows out; analysis/overnight/polish.md). One compositor produces the final image on both game runtimes (Monado
//!   OpenXR and SteamVR through driver_monado), so one env variable covers both; it is read at service start.
//!   Under SteamVR the final image comes from SteamVR's own vrcompositor (Direct mode), not Monado's, so there the level
//!   becomes `steamvr.hmdDisplayColorGainR/G/B` = 1.12 / 1.25 in steamvr.vrsettings, written at Play while SteamVR is stopped.
//!   Measured on the sRGB path: sRGB content passes unchanged, so "standard" is correct but not bright.
//!
//! Settings: ~/.config/pimax/display.toml (`image = "bright"`, `backlight = 100`), shared by the app and the CLI
//! (`pimax vr display [status|image standard|bright|brighter|backlight N|apply]`).
use pimax::{Config, NoopHandler, Session};
use pimax_proto::{build_command, Report};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub fn settings_path(home: &Path) -> PathBuf {
    home.join(".config/pimax/display.toml")
}

/// Image levels: (saved value, label, OUTPUT_GAMMA, what it does).
pub const IMAGES: [(&str, &str, Option<&str>, &str); 3] = [
    ("standard", "Standard", None, "Colours exactly as the game sends them."),
    ("bright", "Bright", Some("0.85"), "Lifts the mid tones (grey 128 shows as 142); black and white stay as they are. With SteamVR: colour gain 1.12."),
    ("brighter", "Brighter", Some("0.75"), "Lifts them more (grey 128 shows as 156); black and white stay as they are. With SteamVR: colour gain 1.25."),
];

pub const MIN_BACKLIGHT: u8 = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub image: String,
    pub backlight: u8,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { image: "bright".into(), backlight: 100 }
    }
}

fn value<'a>(t: &'a str, key: &str) -> Option<&'a str> {
    t.lines().find_map(|l| {
        let l = l.split('#').next()?.trim();
        Some(l.strip_prefix(key)?.trim().strip_prefix('=')?.trim().trim_matches('"'))
    })
}

pub fn parse_settings(t: &str) -> Settings {
    let mut s = Settings::default();
    if let Some(i) = value(t, "image").filter(|i| IMAGES.iter().any(|x| x.0 == *i)) {
        s.image = i.to_string();
    }
    if let Some(b) = value(t, "backlight").and_then(|b| b.parse::<u8>().ok()).filter(|b| (MIN_BACKLIGHT..=100).contains(b)) {
        s.backlight = b;
    }
    s
}

pub fn load(home: &Path) -> Settings {
    std::fs::read_to_string(settings_path(home)).map(|t| parse_settings(&t)).unwrap_or_default()
}

pub fn save(home: &Path, s: &Settings) -> std::io::Result<()> {
    let p = settings_path(home);
    std::fs::create_dir_all(p.parent().unwrap())?;
    std::fs::write(p, format!("# Brightness (Pimax app). image = standard | bright | brighter; backlight = 20..100 (%)\nimage = \"{}\"\nbacklight = {}\n", s.image, s.backlight))
}

/// `XRT_COMPOSITOR_OUTPUT_GAMMA` for a saved image level (None = standard: not set).
pub fn gamma_env(image: &str) -> Option<String> {
    IMAGES.iter().find(|x| x.0 == image).and_then(|x| x.2).map(String::from)
}

/// SteamVR path: `steamvr.hmdDisplayColorGainR/G/B` (read by vrcompositor, default 1.0) for a saved image level. SteamVR's own
/// compositor makes the final image there (Direct mode), so XRT_COMPOSITOR_OUTPUT_GAMMA does nothing; a gain also clips the
/// brightest 10 to 20 % of the values, so the steps are small.
pub fn steamvr_gain(image: &str) -> f64 {
    match image {
        "bright" => 1.12,
        "brighter" => 1.25,
        _ => 1.0,
    }
}

/// The three packets of the capture for `pct` (clamped to 20..=100).
pub fn backlight_packets(pct: u8) -> [Report; 3] {
    let pct = pct.clamp(MIN_BACKLIGHT, 100);
    let key = *b"pbrg";
    let mut d1 = [0u8; 10];
    d1[..4].copy_from_slice(&key);
    d1[8] = 0x01;
    d1[9] = pct;
    let mut d3 = [0u8; 8];
    d3[..4].copy_from_slice(&key);
    // all payloads are short constants
    let f = |c: u8, d: &[u8]| build_command(c, 0, d).unwrap_or([0; 64]);
    [f(0x26, &d1), f(0x2a, &[pct]), f(0x27, &d3)]
}

/// The panel MCU's backlight value for a percentage (`floor(pct * 255 / 100)`, as its log prints it).
pub fn backlight_value(pct: u8) -> u32 {
    pct.clamp(MIN_BACKLIGHT, 100) as u32 * 255 / 100
}

/// Send the backlight to the headset now (1 ms between the packets).
pub fn apply_backlight(pct: u8) -> Result<(), String> {
    let s = Session::open(Config::passive(), NoopHandler).map_err(|e| format!("headset not reachable: {e}"))?;
    for (i, p) in backlight_packets(pct).iter().enumerate() {
        if i > 0 {
            std::thread::sleep(Duration::from_millis(1));
        }
        s.send_report(p).map_err(|e| format!("backlight write failed: {e}"))?;
    }
    std::thread::sleep(Duration::from_millis(20));
    s.close();
    Ok(())
}

/// At Play: the saved backlight, sent again once the session is up. A plain-words line, never an error for VR.
pub fn on_start(home: &Path) -> Option<String> {
    on_start_for(home, false)
}

/// [`on_start`] for "Games run with: SteamVR": vrserver (the Pimax driver inside it) owns the headset's HID from the
/// start, so the write from here usually cannot open it. At 100 % (the level the panel is at after every Linux
/// panel-on, 255) there is nothing to send and nothing is said; below 100 % a failure is explained in plain words.
pub fn on_start_steamvr(home: &Path) -> Option<String> {
    on_start_for(home, true)
}

fn on_start_for(home: &Path, steamvr: bool) -> Option<String> {
    let s = load(home);
    if steamvr && s.backlight == 100 {
        return None;
    }
    match apply_backlight(s.backlight) {
        Ok(()) => (s.backlight != 100).then(|| format!("backlight {} %", s.backlight)),
        // already at the panel's default level: a failed re-send changes nothing, so say nothing
        Err(_) if s.backlight == 100 => None,
        Err(_) if steamvr => Some(format!("backlight {} % not applied: SteamVR's driver holds the headset (it stays at 100 %)", s.backlight)),
        Err(e) => Some(format!("backlight not set ({e})")),
    }
}

pub fn cli(what: &[String], home: &Path) -> Result<(), String> {
    let mut s = load(home);
    match what.first().map(String::as_str).unwrap_or("status") {
        "image" => {
            let i = what.get(1).cloned().unwrap_or_default();
            if !IMAGES.iter().any(|x| x.0 == i) {
                return Err("image: standard | bright | brighter".into());
            }
            s.image = i;
            save(home, &s).map_err(|e| e.to_string())?;
            println!("Image brightness: {} (next start)", s.image);
        }
        "backlight" => {
            let b: u8 = what.get(1).and_then(|b| b.trim_end_matches('%').parse().ok()).ok_or("backlight: 20..100 (%)")?;
            if !(MIN_BACKLIGHT..=100).contains(&b) {
                return Err("backlight: 20..100 (%)".into());
            }
            s.backlight = b;
            save(home, &s).map_err(|e| e.to_string())?;
            match apply_backlight(b) {
                Ok(()) => println!("Backlight: {b} % (panel value {}), sent to the headset", backlight_value(b)),
                Err(e) => println!("Backlight: {b} % saved; not sent now ({e}); Play sends it"),
            }
        }
        "apply" => {
            apply_backlight(s.backlight)?;
            println!("Backlight: {} % sent", s.backlight);
        }
        "status" => {
            println!("Image brightness: {}", s.image);
            println!("Backlight: {} %", s.backlight);
        }
        w => return Err(format!("unknown choice {w} (status|image standard|bright|brighter|backlight 20..100|apply)")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn steamvr_start_is_quiet_at_the_default_backlight() {
        let h = crate::vr::test_dir("display-steamvr-quiet");
        assert_eq!(super::on_start_steamvr(&h), None, "100 % = nothing to send, nothing to say");
    }

    use super::*;

    fn hex(r: &Report) -> String {
        let n = r.iter().rposition(|b| *b != 0).map_or(0, |i| i + 1);
        r[..n].iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn packets_equal_the_windows_capture() {
        // t=7.3727 / .3741 / .3751 of 20261013-124541-ipd-brightness-hub2.pcap (value 0x4e = 78 %)
        let p = backlight_packets(78);
        assert_eq!(hex(&p[0]), "58 0d 26 00 70 62 72 67 00 00 00 00 01 4e");
        assert_eq!(hex(&p[1]), "58 04 2a 00 4e");
        assert_eq!(hex(&p[2]), "58 0b 27 00 70 62 72 67");
        assert_eq!(p[2][11], 0);
        // the other captured values: 0x5e = 94 % -> panel 239, 0x33 = 51 % -> 130
        assert_eq!(hex(&backlight_packets(94)[1]), "58 04 2a 00 5e");
        assert_eq!(backlight_value(94), 239);
        assert_eq!(backlight_value(51), 130);
        assert_eq!(backlight_value(100), 255);
    }

    #[test]
    fn changing_one_setting_keeps_the_other() {
        let d = std::env::temp_dir().join(format!("pimax-display-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        save(&d, &Settings { image: "brighter".into(), backlight: 70 }).unwrap();
        cli(&["image".into(), "standard".into()], &d).unwrap();
        assert_eq!(load(&d), Settings { image: "standard".into(), backlight: 70 });
        assert!(cli(&["image".into(), "neon".into()], &d).is_err());
        assert!(cli(&["backlight".into(), "5".into()], &d).is_err());
        assert_eq!(load(&d), Settings { image: "standard".into(), backlight: 70 });
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn clamped_and_parsed() {
        assert_eq!(backlight_packets(0)[1][4], 20);
        assert_eq!(backlight_packets(250)[1][4], 100);
        assert_eq!(parse_settings(""), Settings::default());
        assert_eq!(Settings::default().image, "bright");
        let s = parse_settings("image = \"brighter\" # x\nbacklight = 80\n");
        assert_eq!((s.image.as_str(), s.backlight), ("brighter", 80));
        assert_eq!(parse_settings("image = \"neon\"\nbacklight = 5").clone(), Settings::default());
        assert_eq!(gamma_env("bright").as_deref(), Some("0.85"));
        assert_eq!(gamma_env("standard"), None);
        assert_eq!((steamvr_gain("standard"), steamvr_gain("bright"), steamvr_gain("brighter")), (1.0, 1.12, 1.25));
    }
}
