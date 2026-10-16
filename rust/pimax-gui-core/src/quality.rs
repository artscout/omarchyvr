//! Render quality: the Quality / Balanced / Performance presets of the Display page, the
//! render resolution they give in plain words, and the frame health of the running session (Monado's
//! `pimax-perf.stats`).
//!
//! The preset is the saved supersampling percentage (`scale_pct` in gui-state.toml), which the launcher passes on as
//! `XRT_COMPOSITOR_SCALE_PERCENTAGE`. Monado multiplies the panel size per eye by it for the size it recommends to
//! OpenXR apps (xrizer passes that to the game) and the Pimax SteamVR driver does the same for
//! `GetRecommendedRenderTargetSize`. Both sides of the eye scale by the same factor, so 70 % is about half the pixels.

/// What Monado reports per eye after the 90/270 degree pre-rotation (monado.log: `display 3840x3552`).
pub const NATIVE_EYE: (u32, u32) = (3840, 3552);

/// Compositor time budget choices (saved value, label); the same list as `pimax_tools::vr::gamemode::BUDGETS`.
/// Image brightness levels (saved value, label, what it does); the same list as `pimax_tools::vr::display::IMAGES`.
pub const IMAGES: [(&str, &str, &str); 3] = [
    ("standard", "Standard", "Colours exactly as the game sends them."),
    ("bright", "Bright", "Lifts the mid tones (grey 128 shows as 142); black and white stay as they are. With SteamVR: colour gain 1.12."),
    ("brighter", "Brighter", "Lifts them more (grey 128 shows as 156); black and white stay as they are. With SteamVR: colour gain 1.25."),
];
/// Lowest backlight the slider allows (percent).
pub const MIN_BACKLIGHT: u8 = 20;

pub const BUDGETS: [(&str, &str); 3] = [("auto", "Auto"), ("6", "6 ms"), ("8", "8 ms")];

/// The presets, best looking first: name, percentage, what it is for.
pub const PRESETS: [(&str, u16, &str); 3] = [
    ("Quality", 100, "the panel's own resolution: sharpest, the most GPU time"),
    ("Balanced", 85, "about 72 % of the pixels: nearly as sharp, a good start for heavy games"),
    ("Performance", 70, "about half the pixels: for games that stutter or feel delayed"),
];

/// The preset of a percentage (None = a custom value from the developer slider).
pub fn preset_of(pct: u16) -> Option<&'static str> {
    PRESETS.iter().find(|p| p.1 == pct).map(|p| p.0)
}

/// Per-eye render size at `pct` percent, rounded as Monado does (`(uint32_t)(w * scale)`, scale clamped to 2.0).
pub fn eye_size(pct: u16) -> (u32, u32) {
    let s = f64::from(pct.min(200)) / 100.0;
    ((f64::from(NATIVE_EYE.0) * s) as u32, (f64::from(NATIVE_EYE.1) * s) as u32)
}

/// "3264x3019 per eye (85 %, 72 % of the pixels)".
pub fn describe(pct: u16) -> String {
    let (w, h) = eye_size(pct);
    let px = (f64::from(pct.min(200)) / 100.0).powi(2) * 100.0;
    if pct == 100 {
        format!("{w}x{h} per eye (100 %, the panel's own resolution)")
    } else {
        format!("{w}x{h} per eye ({pct} %, {px:.0} % of the pixels)")
    }
}

/// One snapshot of Monado's frame statistics (`$XDG_RUNTIME_DIR/pimax-perf.stats`; counters since the compositor started).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PerfSnap {
    /// Display period (ms).
    pub period_ms: f64,
    /// Compositor frames per second over the last half second.
    pub comp_fps: f64,
    /// Compositor frames.
    pub frames: u64,
    /// Frames whose compositor commit (render, present and the wait for the GPU queue) returned after the predicted
    /// display time. It counts latency of the compositor's own work, not frames the headset missed.
    pub late: u64,
    /// Display periods the compositor skipped: the headset showed the previous image again (a real dropped frame).
    pub skipped: u64,
    /// The apps (overlay, fps, GPU ms per frame); the game (not an overlay) first when there is one.
    pub apps: Vec<(bool, f64, f64)>,
}

impl PerfSnap {
    /// Parse the file text (`key=value` lines and `app NAME k=v ...` lines).
    pub fn parse(text: &str) -> Option<PerfSnap> {
        let mut s = PerfSnap::default();
        let mut seen = false;
        for l in text.lines() {
            if let Some(rest) = l.strip_prefix("app ") {
                let (mut overlay, mut fps, mut gpu) = (false, 0.0, 0.0);
                for kv in rest.split_whitespace() {
                    match kv.split_once('=') {
                        Some(("overlay", v)) => overlay = v == "1",
                        Some(("fps", v)) => fps = v.parse().unwrap_or(0.0),
                        Some(("gpu_ms", v)) => gpu = v.parse().unwrap_or(0.0),
                        _ => {}
                    }
                }
                s.apps.push((overlay, fps, gpu));
                continue;
            }
            let Some((k, v)) = l.split_once('=') else { continue };
            match k.trim() {
                "period_ms" => s.period_ms = v.trim().parse().ok()?,
                "comp_fps" => s.comp_fps = v.trim().parse().ok()?,
                "comp_frames" => {
                    s.frames = v.trim().parse().ok()?;
                    seen = true;
                }
                "comp_late" => s.late = v.trim().parse().ok()?,
                "comp_skipped" => s.skipped = v.trim().parse().ok()?,
                _ => {}
            }
        }
        // the game before the desk overlay (WayVR)
        s.apps.sort_by_key(|a| a.0);
        seen.then_some(s)
    }

    /// Dropped frames in percent of all frames (the headset showed an image twice).
    pub fn dropped_pct(&self) -> f64 {
        if self.frames == 0 { 0.0 } else { self.skipped as f64 * 100.0 / (self.frames + self.skipped) as f64 }
    }

    /// Late compositor commits in percent of all frames.
    pub fn late_pct(&self) -> f64 {
        if self.frames == 0 { 0.0 } else { self.late as f64 * 100.0 / self.frames as f64 }
    }

    /// One plain line for the session card: "72 fps, 0.0 % dropped frames, game 71 fps (GPU 6.2 ms)".
    pub fn summary(&self) -> String {
        let mut s = format!("headset {:.0} fps, {:.1} % dropped frames, {:.0} % late", self.comp_fps, self.dropped_pct(), self.late_pct());
        if let Some((_, fps, gpu)) = self.apps.first().filter(|a| !a.0) {
            let budget = self.period_ms;
            let warn = if budget > 0.0 && *gpu > budget * 0.9 { " - over the frame budget" } else { "" };
            s.push_str(&format!(", game {fps:.0} fps (GPU {gpu:.1} ms of {budget:.1}{warn})"));
        }
        s
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn brightness_lists_match_the_tools() {
        for (a, b) in super::IMAGES.iter().zip(pimax_tools::vr::display::IMAGES.iter()) {
            assert_eq!((a.0, a.1, a.2), (b.0, b.1, b.3));
        }
        assert_eq!(super::MIN_BACKLIGHT, pimax_tools::vr::display::MIN_BACKLIGHT);
    }

    use super::*;

    #[test]
    fn presets_give_the_sizes_monado_recommends() {
        assert_eq!(eye_size(100), (3840, 3552));
        assert_eq!(eye_size(85), (3264, 3019));
        assert_eq!(eye_size(70), (2688, 2486));
        assert_eq!(eye_size(250), eye_size(200)); // Monado clamps to 200 %
        assert_eq!(preset_of(85), Some("Balanced"));
        assert_eq!(preset_of(90), None);
        assert_eq!(describe(70), "2688x2486 per eye (70 %, 49 % of the pixels)");
        assert_eq!(describe(100), "3840x3552 per eye (100 %, the panel's own resolution)");
    }

    #[test]
    fn perf_stats_split_dropped_from_late() {
        let t = "v=1\nt_ms=14749666\nperiod_ms=13.889\ncomp_fps=72.0\ncomp_frames=14061\ncomp_late=12959\ncomp_skipped=2\n\
                 app overlay=1 active=1 fps=72.0 frames=14058 cpu_ms=1.51 gpu_ms=0.11\n\
                 app overlay=0 active=1 fps=70.4 frames=9000 cpu_ms=4.0 gpu_ms=12.9\n";
        let s = PerfSnap::parse(t).unwrap();
        assert_eq!((s.frames, s.late, s.skipped), (14061, 12959, 2));
        assert!(s.dropped_pct() < 0.02);
        assert!((s.late_pct() - 92.2).abs() < 0.1);
        assert!(!s.apps[0].0);
        assert_eq!(s.summary(), "headset 72 fps, 0.0 % dropped frames, 92 % late, game 70 fps (GPU 12.9 ms of 13.9 - over the frame budget)");
        assert!(PerfSnap::parse("garbage").is_none());
    }
}
