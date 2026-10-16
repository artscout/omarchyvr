//! Auto-exposure (AE) for the tracking cameras (6DoF phase 2; evidence and frame layout in
//! `analysis/slam-exposure.md`).
//!
//! On Windows the host pushes exposure and gain for the four cameras with HID command `0x51` on the headset
//! (34a4:0044), about 12.5 times a second:
//!
//! ```text
//! 58 19 51 00 | 01 32 | e0 e1 e2 e3 | g0 g1 g2 g3 | led_e led_g      (u16 little endian)
//! ```
//!
//! `e`/`g` are the tracking-exposure values of cameras 0..3 (the lower pair 0,1 and the upper pair 2,3 always carry
//! equal values), `led_e`/`led_g` the short controller-LED exposure (constant 40/62). The camera frame metadata
//! echoes the values in effect, so the loop can be closed on what each image was really taken with. On Linux
//! nobody sends it and the cameras stay at the power-on default 10410/449.
//!
//! This module is pure logic: statistics of the tracking tiles -> a new setting per camera pair -> the command
//! payload. Sending is the driver's job and is OFF unless `PIMAX_CAM_AE=1` (needs the user's approval: the frame is
//! derived from captures). `PIMAX_CAM_AE=dry` runs the controller and only logs.
//!
//! Control law (per pair, both tiles pooled): histogram on a subsampled grid inside the fisheye image circle,
//! `m` = mean of the pixels below the 98th percentile (light sources and LED blobs do not drive the loop), error
//! `ln(target / m)` with a deadband, damped multiplicative step `r = exp(k * err)` clamped to `max_step`, applied to
//! the exposure*gain product **of the analysed frame** (from its metadata, so a frame taken before the last
//! command was applied gives the same answer again instead of a second step: no wind-up from the 1-3 frame
//! latency). The product is split exposure first (up to `exposure_max`, the motion-blur cap), then gain.

use crate::camera::FrameInfo;

/// HID command id of the exposure push.
pub const AE_CMD: u8 = 0x51;
/// Option byte of the exposure push.
pub const AE_OPT: u8 = 0x00;
/// Payload length (`01 32` + 10 u16): `58 19 51 00` + 22 bytes, LL = 22 + 3 = 0x19.
pub const AE_PAYLOAD_LEN: usize = 22;
/// The two constant leading payload bytes Windows always sends.
pub const AE_HEAD: [u8; 2] = [0x01, 0x32];
/// Linux power-on default of the tracking exposure (no AE).
pub const POWER_ON_EXPOSURE: u16 = 10410;
/// Windows' exposure ladder (gain 13): geometric, ratio ~1.0302 per step, anchored at 1682; observed values
/// 1682..4108 (`analysis/slam-exposure.md`). Exposure unit = microseconds [inferred].
pub const LADDER_ANCHOR: f64 = 1682.0;
/// Ladder ratio.
pub const LADDER_RATIO: f64 = 1.030_212;
/// Gain codes Windows used (both pairs), ascending.
pub const GAIN_CODES: [u16; 29] = [
    13, 15, 17, 19, 21, 23, 26, 29, 32, 35, 39, 43, 46, 53, 59, 71, 78, 79, 87, 95, 105, 116, 128,
    141, 155, 171, 188, 207, 215,
];

/// Windows' exposure values at gain 13, ascending (observed 1682..4108), extended below 1682 with the same ratio
/// down to Windows' observed minimum ~1140.
pub const EXPOSURE_LADDER: [u16; 44] = [
    1142, 1177, 1212, 1249, 1287, 1326, 1366, 1407, 1449, 1493, 1538, 1585, 1633, 1682, 1733, 1785,
    1839, 1895, 1952, 2011, 2072, 2135, 2200, 2266, 2334, 2405, 2478, 2553, 2630, 2709, 2791, 2875,
    2962, 3051, 3143, 3238, 3336, 3437, 3541, 3648, 3758, 3871, 3988, 4108,
];

/// Nearest ladder value (geometric extension outside the table).
pub fn ladder(e: f64) -> f64 {
    let (lo, hi) = (
        EXPOSURE_LADDER[0] as f64,
        EXPOSURE_LADDER[EXPOSURE_LADDER.len() - 1] as f64,
    );
    if e < lo || e > hi {
        let k = ((e.max(1.0) / LADDER_ANCHOR).ln() / LADDER_RATIO.ln()).round();
        return (LADDER_ANCHOR * LADDER_RATIO.powf(k)).round();
    }
    let mut best = lo;
    for &v in &EXPOSURE_LADDER {
        if (v as f64 - e).abs() < (best - e).abs() {
            best = v as f64;
        }
    }
    best
}

/// Largest ladder value <= `e` (geometric extension below the table).
pub fn ladder_down(e: f64) -> f64 {
    match EXPOSURE_LADDER.iter().rev().find(|&&v| v as f64 <= e) {
        Some(&v) => v as f64,
        None => {
            let k = ((e.max(1.0) / LADDER_ANCHOR).ln() / LADDER_RATIO.ln() + 1e-9).floor();
            (LADDER_ANCHOR * LADDER_RATIO.powf(k)).round()
        }
    }
}

/// Linux power-on default of the tracking gain (no AE).
pub const POWER_ON_GAIN: u16 = 449;

/// Exposure and gain of the four cameras plus the LED-frame pair (raw units, as on the wire).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AeSetting {
    /// Tracking exposure of cameras 0..3.
    pub exposure: [u16; 4],
    /// Tracking gain of cameras 0..3.
    pub gain: [u16; 4],
    /// LED-frame exposure (Windows: 40).
    pub led_exposure: u16,
    /// LED-frame gain (Windows: 62).
    pub led_gain: u16,
}

impl AeSetting {
    /// The setting a frame's metadata reports (LED pair: the given defaults, the metadata does not carry it).
    pub fn from_info(fi: &FrameInfo, led_exposure: u16, led_gain: u16) -> AeSetting {
        AeSetting {
            exposure: fi.exposure,
            gain: fi.gain,
            led_exposure,
            led_gain,
        }
    }
}

/// Command payload (22 bytes after `58 19 51 00`).
pub fn payload(s: &AeSetting) -> [u8; AE_PAYLOAD_LEN] {
    let mut p = [0u8; AE_PAYLOAD_LEN];
    p[..2].copy_from_slice(&AE_HEAD);
    let words = s
        .exposure
        .into_iter()
        .chain(s.gain)
        .chain([s.led_exposure, s.led_gain]);
    for (i, w) in words.enumerate() {
        p[2 + 2 * i..4 + 2 * i].copy_from_slice(&w.to_le_bytes());
    }
    p
}

/// Parse a payload (or a whole `58 19 51 00 ..` report). `None` if it is not an exposure push.
pub fn parse_payload(b: &[u8]) -> Option<AeSetting> {
    let p = if b.len() >= 4 && b[0] == 0x58 && b[1] == 0x19 && b[2] == AE_CMD && b[3] == AE_OPT {
        &b[4..]
    } else {
        b
    };
    if p.len() < AE_PAYLOAD_LEN || p[..2] != AE_HEAD {
        return None;
    }
    let w = |i: usize| u16::from_le_bytes([p[2 + 2 * i], p[3 + 2 * i]]);
    Some(AeSetting {
        exposure: [w(0), w(1), w(2), w(3)],
        gain: [w(4), w(5), w(6), w(7)],
        led_exposure: w(8),
        led_gain: w(9),
    })
}

/// A borrowed 640x480 tile inside a raw frame.
#[derive(Clone, Copy)]
pub struct TileView<'a> {
    /// Raw frame bytes.
    pub data: &'a [u8],
    /// Row stride of `data`.
    pub stride: usize,
    /// First column of the tile.
    pub x0: usize,
    /// First row of the tile.
    pub y0: usize,
    /// Tile width.
    pub w: usize,
    /// Tile height.
    pub h: usize,
}

impl TileView<'_> {
    /// Pixel (x, y) of the tile (no bounds check beyond the slice's own).
    #[inline]
    pub fn px(&self, x: usize, y: usize) -> u8 {
        self.data[(self.y0 + y) * self.stride + self.x0 + x]
    }
}

/// Brightness statistics of one tile (or a pooled pair).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TileStats {
    /// Mean of all sampled pixels.
    pub mean: f64,
    /// Mean of the pixels at or below the `trim` percentile (the AE input).
    pub trimmed_mean: f64,
    /// Median.
    pub p50: f64,
    /// 95th percentile.
    pub p95: f64,
    /// Fraction of samples at or above the saturation level.
    pub sat_frac: f64,
    /// Fraction of samples at or below 8 (black).
    pub dark_frac: f64,
    /// Samples used.
    pub n: u32,
}

/// 256-bin histogram.
#[derive(Clone, Debug)]
pub struct Histogram {
    /// Counts.
    pub bins: [u32; 256],
}

impl Default for Histogram {
    fn default() -> Self {
        Histogram { bins: [0; 256] }
    }
}

impl Histogram {
    /// Total count.
    pub fn total(&self) -> u64 {
        self.bins.iter().map(|&c| c as u64).sum()
    }

    /// Add another histogram.
    pub fn merge(&mut self, o: &Histogram) {
        for (a, b) in self.bins.iter_mut().zip(o.bins.iter()) {
            *a += b;
        }
    }

    /// Value at quantile `q` (0..1).
    pub fn quantile(&self, q: f64) -> f64 {
        let n = self.total();
        if n == 0 {
            return 0.0;
        }
        let want = (q.clamp(0.0, 1.0) * n as f64).ceil().max(1.0) as u64;
        let mut acc = 0u64;
        for (v, &c) in self.bins.iter().enumerate() {
            acc += c as u64;
            if acc >= want {
                return v as f64;
            }
        }
        255.0
    }

    /// Statistics; `trim` = percentile (0..1) above which pixels are left out of the trimmed mean.
    pub fn stats(&self, trim: f64, sat_level: u8) -> TileStats {
        let n = self.total();
        if n == 0 {
            return TileStats::default();
        }
        let cut = self.quantile(trim) as usize;
        let (mut sum, mut tsum, mut tn, mut sat, mut dark) = (0f64, 0f64, 0u64, 0u64, 0u64);
        for (v, &c) in self.bins.iter().enumerate() {
            let c = c as u64;
            sum += (v as u64 * c) as f64;
            if v <= cut {
                tsum += (v as u64 * c) as f64;
                tn += c;
            }
            if v >= sat_level as usize {
                sat += c;
            }
            if v <= 8 {
                dark += c;
            }
        }
        TileStats {
            mean: sum / n as f64,
            trimmed_mean: if tn > 0 { tsum / tn as f64 } else { 0.0 },
            p50: self.quantile(0.5),
            p95: self.quantile(0.95),
            sat_frac: sat as f64 / n as f64,
            dark_frac: dark as f64 / n as f64,
            n: n as u32,
        }
    }
}

/// Sampling mask: pixels inside an ellipse centred on the tile (the fisheye image circle; the corners are dark
/// lens border and would drag the mean down).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mask {
    /// Centre x (tile pixels).
    pub cx: f64,
    /// Centre y.
    pub cy: f64,
    /// Radius x.
    pub rx: f64,
    /// Radius y.
    pub ry: f64,
}

impl Default for Mask {
    /// Image rows 24..489 of the frame are lit (camera-frames.md); a 0.95 x half-width / 0.97 x half-height
    /// ellipse keeps the dark corners out without losing the scene.
    fn default() -> Self {
        Mask {
            cx: 319.5,
            cy: 239.5,
            rx: 320.0 * 0.95,
            ry: 240.0 * 0.97,
        }
    }
}

impl Mask {
    /// Inside?
    #[inline]
    pub fn inside(&self, x: f64, y: f64) -> bool {
        let dx = (x - self.cx) / self.rx;
        let dy = (y - self.cy) / self.ry;
        dx * dx + dy * dy <= 1.0
    }
}

/// Histogram of a tile on a `step`-pixel grid inside `mask`.
pub fn tile_histogram(t: &TileView, step: usize, mask: &Mask) -> Histogram {
    let mut h = Histogram::default();
    let step = step.max(1);
    let mut y = step / 2;
    while y < t.h {
        let mut x = step / 2;
        while x < t.w {
            if mask.inside(x as f64, y as f64) {
                h.bins[t.px(x, y) as usize] += 1;
            }
            x += step;
        }
        y += step;
    }
    h
}

/// Controller parameters (raw units as on the wire).
#[derive(Clone, Debug, PartialEq)]
pub struct AeParams {
    /// Target of the trimmed mean (0..255).
    pub target: f64,
    /// No change while `|ln(m / target)|` is below this (0.15 = +-16 %).
    pub deadband: f64,
    /// Damping of the log-domain step (1 = one-step model correction).
    pub k: f64,
    /// Largest brightness ratio per update (both directions).
    pub max_step: f64,
    /// Largest ratio per update while the image is mostly saturated or black (faster recovery).
    pub max_step_far: f64,
    /// Exposure range.
    pub exposure_min: u16,
    /// Exposure cap (motion blur); gain is used above it.
    pub exposure_max: u16,
    /// Gain range.
    pub gain_min: u16,
    /// Gain cap.
    pub gain_max: u16,
    /// Pixel level counted as saturated.
    pub sat_level: u8,
    /// Above this saturated fraction the loop pulls down by at least `sat_pull` even if the mean looks fine.
    pub sat_max_frac: f64,
    /// Minimum pull-down (log) when too much is saturated.
    pub sat_pull: f64,
    /// Percentile for the trimmed mean.
    pub trim: f64,
    /// Sampling grid step (pixels).
    pub step: usize,
    /// Command period (Windows: 80 ms = 12.5 Hz).
    pub period_ns: u64,
    /// LED-frame pair sent unchanged.
    pub led_exposure: u16,
    /// LED-frame gain.
    pub led_gain: u16,
    /// Camera pairs controlled together (stereo matching wants equal brightness within a pair).
    pub pairs: [[usize; 2]; 2],
    /// Control the upper pair (cameras 2, 3) too.
    pub upper: bool,
    /// Quantise exposure to Windows' ladder and gain to the codes Windows used.
    pub quantize: bool,
    /// Exposure up to which the split prefers exposure over gain (the normal ceiling). Above it gain comes first and
    /// exposure only grows (up to `exposure_max`) once the gain is at its cap.
    pub exposure_soft: u16,
    /// The automatic extended range (low light).
    pub light: LightParams,
}

impl Default for AeParams {
    /// Windows' behaviour from the captures (`analysis/slam-exposure.md`): target mean ~62/255, one 3 % ladder
    /// step per command typical (small steps keep the optical flow's brightness constancy), exposure ladder
    /// 1140..4108 at gain 13, gain codes 13..215 only above the exposure ceiling. The floor goes below Windows'
    /// indoor minimum (100: the same sensors run 40 µs LED exposures every other frame) for sunlit rooms. A step never moves brightness
    /// the wrong way, so a scene too dark even for the power-on setting keeps it instead of trading it for the cap.
    fn default() -> Self {
        AeParams {
            target: 64.0,
            deadband: 0.08,
            k: 0.5,
            max_step: LADDER_RATIO * LADDER_RATIO * LADDER_RATIO,
            max_step_far: 2.0,
            exposure_min: 100,
            exposure_max: 4108,
            gain_min: 13,
            gain_max: 215,
            sat_level: 250,
            sat_max_frac: 0.05,
            sat_pull: 0.15,
            trim: 0.98,
            step: 4,
            period_ns: 80_000_000,
            led_exposure: 40,
            led_gain: 62,
            pairs: [[0, 1], [2, 3]],
            upper: true,
            quantize: true,
            exposure_soft: 4108,
            light: LightParams::default(),
        }
    }
}

fn env_f64(name: &str) -> Option<f64> {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok())
}

impl AeParams {
    /// Defaults with `PIMAX_CAM_AE_TARGET`, `_EXP_MAX`, `_EXP_MIN`, `_GAIN_MAX`, `_GAIN_MIN`, `_PERIOD_MS`,
    /// `_K`, `_DEADBAND`, `_UPPER` (0/1) overrides. Values are clamped to sane ranges.
    pub fn from_env() -> AeParams {
        let mut p = AeParams::default();
        if let Some(v) = env_f64("PIMAX_CAM_AE_TARGET") {
            p.target = v.clamp(20.0, 200.0);
        }
        if let Some(v) = env_f64("PIMAX_CAM_AE_EXP_MAX") {
            p.exposure_max = v.clamp(100.0, 10410.0) as u16;
        }
        if let Some(v) = env_f64("PIMAX_CAM_AE_EXP_MIN") {
            p.exposure_min = v.clamp(1.0, p.exposure_max as f64) as u16;
        }
        if let Some(v) = env_f64("PIMAX_CAM_AE_GAIN_MAX") {
            p.gain_max = v.clamp(1.0, 449.0) as u16;
        }
        if let Some(v) = env_f64("PIMAX_CAM_AE_GAIN_MIN") {
            p.gain_min = v.clamp(1.0, p.gain_max as f64) as u16;
        }
        if let Some(v) = env_f64("PIMAX_CAM_AE_PERIOD_MS") {
            p.period_ns = (v.clamp(40.0, 2000.0) * 1e6) as u64;
        }
        if let Some(v) = env_f64("PIMAX_CAM_AE_K") {
            p.k = v.clamp(0.1, 1.0);
        }
        if let Some(v) = env_f64("PIMAX_CAM_AE_DEADBAND") {
            p.deadband = v.clamp(0.0, 1.0);
        }
        if let Some(v) = env_f64("PIMAX_CAM_AE_UPPER") {
            p.upper = v != 0.0;
        }
        if let Some(v) = env_f64("PIMAX_CAM_AE_QUANTIZE") {
            p.quantize = v != 0.0;
        }
        if let Some(v) = env_f64("PIMAX_CAM_AE_EXTEND") {
            p.light.enabled = v != 0.0;
        }
        p.exposure_soft = p.exposure_max.min(p.exposure_soft);
        // An explicit exposure cap from the environment is also the soft one.
        if env_f64("PIMAX_CAM_AE_EXP_MAX").is_some() {
            p.exposure_soft = p.exposure_max;
        }
        p
    }

    fn clamp_setting(&self, e: f64, g: f64) -> (u16, u16) {
        (
            e.round()
                .clamp(self.exposure_min as f64, self.exposure_max as f64) as u16,
            g.round().clamp(self.gain_min as f64, self.gain_max as f64) as u16,
        )
    }

    /// Split a brightness product `e * g` into (exposure, gain): exposure first up to the cap, then gain (as
    /// Windows does). Gain is taken as linear in its raw code (Windows lowers e by ~7 % when g steps 13 -> 15,
    /// consistent with that). With `quantize`, values snap to Windows' ladder / gain codes; above the exposure
    /// cap the smallest gain code that reaches the product is used and the exposure fine-tunes below the cap.
    pub fn split(&self, product: f64) -> (u16, u16) {
        let g_min = self.gain_min.max(1) as f64;
        let (e_lo, e_hi) = (self.exposure_min as f64, self.exposure_max as f64);
        let e_soft = (self.exposure_soft as f64).clamp(e_lo, e_hi);
        if !self.quantize {
            let e = (product / g_min).clamp(e_lo, e_soft);
            let g = (product / e.max(1.0)).clamp(g_min, self.gain_max as f64);
            // Gain exhausted: the exposure may go above the soft cap (extended range).
            let e = if product / g > e { product / g } else { e };
            return self.clamp_setting(e, g);
        }
        if product / g_min <= e_soft {
            let e = ladder(product / g_min).clamp(e_lo, e_soft);
            return self.clamp_setting(e, g_min);
        }
        let mut codes: Vec<u16> = GAIN_CODES
            .iter()
            .copied()
            .filter(|&g| g >= self.gain_min && g <= self.gain_max)
            .collect();
        // The power-on gain (Windows' 449) is the one code above the ladder; only when the cap allows it.
        if self.gain_max >= POWER_ON_GAIN && self.gain_min <= POWER_ON_GAIN {
            codes.push(POWER_ON_GAIN);
        }
        let need = product / e_soft;
        if let Some(g) = codes.iter().copied().find(|&g| g as f64 >= need) {
            let e = ladder_down(product / g as f64).clamp(e_lo, e_soft);
            return self.clamp_setting(e, g as f64);
        }
        // Every gain code is used up: the exposure grows past the soft cap (exposure last), up to `exposure_max`.
        let g = codes.last().copied().unwrap_or(self.gain_min) as f64;
        let e = ladder(product / g).clamp(e_lo, e_hi);
        self.clamp_setting(e, g)
    }
}

/// Thresholds of the low-light governor (extended range and the "tracking light low" warning). Means are the
/// trimmed mean of the tracking tiles, 0..255.
#[derive(Clone, Debug, PartialEq)]
pub struct LightParams {
    /// Automatic extended range on (`PIMAX_CAM_AE_EXTEND=0` turns it off; the warning still works).
    pub enabled: bool,
    /// Extend while the loop sits at its ceiling with a mean below this ...
    pub extend_mean: f64,
    /// ... for this long (ns).
    pub extend_ns: u64,
    /// Back to the normal range when the mean is above this ...
    pub retract_mean: f64,
    /// ... or the loop needs less than half of the normal ceiling (mean at least this) ...
    pub settled_mean: f64,
    /// ... for this long (ns).
    pub retract_ns: u64,
    /// Warn when even the full extended range leaves the mean below this ...
    pub low_mean: f64,
    /// ... for this long (ns).
    pub low_ns: u64,
    /// The warning clears when the mean is above this ...
    pub clear_mean: f64,
    /// ... for this long (ns).
    pub clear_ns: u64,
    /// Extended exposure cap (the camera's power-on value, which Windows holds all session).
    pub ext_exposure: u16,
    /// Extended gain cap.
    pub ext_gain: u16,
}

impl Default for LightParams {
    fn default() -> Self {
        LightParams {
            enabled: true,
            extend_mean: 20.0,
            extend_ns: 5_000_000_000,
            retract_mean: 80.0,
            settled_mean: 40.0,
            retract_ns: 8_000_000_000,
            low_mean: 15.0,
            low_ns: 10_000_000_000,
            clear_mean: 25.0,
            clear_ns: 3_000_000_000,
            ext_exposure: POWER_ON_EXPOSURE,
            ext_gain: POWER_ON_GAIN,
        }
    }
}

/// A change of the low-light governor, for the driver to log and show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightEvent {
    /// Range extended to `stage` (1 = gain up to the extended cap, 2 = exposure too).
    Extended(u8),
    /// Back to the normal range.
    Retracted,
    /// The cameras see too little light even with the full extended range.
    LowRaised,
    /// Light recovered.
    LowCleared,
}

/// Low-light governor state (pure, driven by host time).
#[derive(Clone, Debug, Default)]
pub struct LightGov {
    /// 0 normal, 1 gain extended, 2 gain and exposure extended.
    pub stage: u8,
    /// "Tracking light low" warning raised.
    pub low: bool,
    starved: Option<u64>,
    easy: Option<u64>,
    dark: Option<u64>,
    clear: Option<u64>,
    last_ns: Option<u64>,
}

fn held(t: &mut Option<u64>, now_ns: u64, cond: bool, ns: u64) -> bool {
    if !cond {
        *t = None;
        return false;
    }
    let since = *t.get_or_insert(now_ns);
    now_ns.saturating_sub(since) >= ns
}

impl LightGov {
    /// One tracking frame. `at_ceiling`: every controlled pair already uses the caps of the current stage;
    /// `settled`: the loop needs less than half of the normal ceiling; `top`: the highest stage that exists.
    pub fn step(
        &mut self,
        lp: &LightParams,
        now_ns: u64,
        mean: f64,
        at_ceiling: bool,
        settled: bool,
        top: u8,
    ) -> Vec<LightEvent> {
        let mut ev = Vec::new();
        // A gap in the tracking frames (camera restart) makes the timers stale.
        if self.last_ns.is_some_and(|t| now_ns.saturating_sub(t) > 2_000_000_000) {
            self.starved = None;
            self.easy = None;
            self.dark = None;
            self.clear = None;
        }
        self.last_ns = Some(now_ns);
        let starving = self.stage < top && at_ceiling && mean < lp.extend_mean;
        if held(&mut self.starved, now_ns, starving, lp.extend_ns) {
            self.stage += 1;
            self.starved = None;
            ev.push(LightEvent::Extended(self.stage));
        }
        let easy = self.stage > 0 && (mean > lp.retract_mean || (settled && mean >= lp.settled_mean));
        if held(&mut self.easy, now_ns, easy, lp.retract_ns) {
            self.stage = 0;
            self.easy = None;
            self.starved = None;
            ev.push(LightEvent::Retracted);
        }
        // The warning: the full range is in use (or none exists) and the image is still dark.
        let full = self.stage >= top;
        if !self.low {
            if held(&mut self.dark, now_ns, full && at_ceiling && mean < lp.low_mean, lp.low_ns) {
                self.low = true;
                self.dark = None;
                self.clear = None;
                ev.push(LightEvent::LowRaised);
            }
        } else if held(&mut self.clear, now_ns, mean > lp.clear_mean || !full, lp.clear_ns) {
            self.low = false;
            self.clear = None;
            self.dark = None;
            ev.push(LightEvent::LowCleared);
        }
        ev
    }
}

/// What one update decided.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AeDecision {
    /// Pooled statistics per pair.
    pub stats: [TileStats; 2],
    /// Log error per pair (`ln(target / m)`, after the saturation rule), 0 inside the deadband.
    pub err: [f64; 2],
    /// The target setting changed in this update.
    pub changed: bool,
    /// A command should go out now (period elapsed).
    pub send: bool,
}

/// The controller.
#[derive(Clone, Debug)]
pub struct AutoExposure {
    /// Parameters.
    pub p: AeParams,
    target: Option<AeSetting>,
    last_send_ns: Option<u64>,
    /// Commands handed out for sending.
    pub sends: u64,
    /// Updates that changed the target.
    pub changes: u64,
    /// Tracking frames analysed.
    pub frames: u64,
    /// Low-light governor.
    pub light: LightGov,
    base_exposure_max: u16,
    base_gain_max: u16,
    events: Vec<LightEvent>,
}

impl AutoExposure {
    /// New controller.
    pub fn new(p: AeParams) -> AutoExposure {
        AutoExposure {
            target: None,
            last_send_ns: None,
            sends: 0,
            changes: 0,
            frames: 0,
            light: LightGov::default(),
            base_exposure_max: p.exposure_max,
            base_gain_max: p.gain_max,
            events: Vec::new(),
            p,
        }
    }

    /// Highest range stage that exists (0: the normal ceiling is already the extended one, or the feature is off).
    pub fn top_stage(&self) -> u8 {
        let l = &self.p.light;
        if l.enabled && (self.base_exposure_max < l.ext_exposure || self.base_gain_max < l.ext_gain) {
            2
        } else {
            0
        }
    }

    /// (exposure cap, gain cap) of a range stage: 0 normal, 1 gain first, 2 exposure last.
    pub fn caps(&self, stage: u8) -> (u16, u16) {
        let l = &self.p.light;
        match stage {
            0 => (self.base_exposure_max, self.base_gain_max),
            1 => (self.base_exposure_max, self.base_gain_max.max(l.ext_gain)),
            _ => (
                self.base_exposure_max.max(l.ext_exposure),
                self.base_gain_max.max(l.ext_gain),
            ),
        }
    }

    /// Light events since the last call (for the driver to log and show).
    pub fn take_events(&mut self) -> Vec<LightEvent> {
        std::mem::take(&mut self.events)
    }

    /// Feed the governor with one tracking frame and apply the caps of its stage.
    fn govern(&mut self, setting: &AeSetting, stats: &[TileStats; 2], now_ns: u64) {
        let pairs: Vec<usize> = (0..if self.p.upper { 2 } else { 1 }).collect();
        let mean = pairs.iter().map(|&k| stats[k].trimmed_mean).sum::<f64>() / pairs.len() as f64;
        let (ce, cg) = self.caps(self.light.stage);
        let (ne, ng) = self.caps(0);
        let at_ceiling = pairs.iter().all(|&k| {
            let c = self.p.pairs[k][0];
            setting.gain[c] >= cg && setting.exposure[c] as f64 >= 0.97 * ce as f64
        });
        // "settled": every controlled pair needs less than half of the normal ceiling product.
        let half = 0.5 * ne as f64 * ng as f64;
        let settled = pairs.iter().all(|&k| {
            let c = self.p.pairs[k][0];
            (setting.exposure[c] as f64 * setting.gain[c] as f64) <= half
        });
        let top = self.top_stage();
        let before = self.light.stage;
        let ev = self.light.step(&self.p.light, now_ns, mean, at_ceiling, settled, top);
        if self.light.stage != before {
            let (e, g) = self.caps(self.light.stage);
            self.p.exposure_max = e;
            self.p.gain_max = g;
        }
        self.events.extend(ev);
    }

    /// Current target (None before the first frame).
    pub fn target(&self) -> Option<AeSetting> {
        self.target
    }

    /// Log-domain error of one pair from its pooled statistics (0 inside the deadband).
    pub fn error(&self, s: &TileStats) -> f64 {
        if s.n == 0 {
            return 0.0;
        }
        let m = s.trimmed_mean.max(1.0);
        let mut err = (self.p.target / m).ln();
        if s.sat_frac > self.p.sat_max_frac {
            err = err.min(-self.p.sat_pull);
        }
        if err.abs() < self.p.deadband {
            0.0
        } else {
            err
        }
    }

    /// New (exposure, gain) for a pair whose frame was taken with `(e, g)` and shows statistics `s`.
    pub fn step_pair(&self, e: u16, g: u16, s: &TileStats) -> (u16, u16, f64) {
        let err = self.error(s);
        if err == 0.0 {
            return (e, g, 0.0);
        }
        let far =
            s.sat_frac > 0.5 || s.dark_frac > 0.8 || s.trimmed_mean > 240.0 || s.trimmed_mean < 8.0;
        let lim = if far {
            self.p.max_step_far
        } else {
            self.p.max_step
        }
        .max(1.0)
        .ln();
        let r = (self.p.k * err).clamp(-lim, lim).exp();
        let base = e.max(1) as f64 * g.max(1) as f64;
        let (ne, ng) = self.p.split(base * r);
        let new = ne as f64 * ng as f64;
        // The caps can make the split darker than the frame's own setting (e.g. the power-on 10410/449 in a dark
        // room): never step against the error.
        if (err > 0.0 && new < base) || (err < 0.0 && new > base) {
            return (e, g, err);
        }
        (ne, ng, err)
    }

    /// One tracking frame: `setting` = what the frame was taken with (its metadata), `stats` = pooled statistics
    /// of each pair, `now_ns` = host time. Returns the decision; the setting to send is [`AutoExposure::target`].
    pub fn update(
        &mut self,
        setting: &AeSetting,
        stats: [TileStats; 2],
        now_ns: u64,
    ) -> AeDecision {
        self.frames += 1;
        self.govern(setting, &stats, now_ns);
        let prev = self.target;
        let mut next = prev.unwrap_or(AeSetting {
            led_exposure: self.p.led_exposure,
            led_gain: self.p.led_gain,
            ..*setting
        });
        let mut errs = [0.0; 2];
        for (k, pair) in self.p.pairs.iter().enumerate() {
            if k == 1 && !self.p.upper {
                continue;
            }
            let [a, b] = *pair;
            // The pair runs on one setting; take the frame's value of the first camera.
            let (e, g, err) = self.step_pair(setting.exposure[a], setting.gain[a], &stats[k]);
            errs[k] = err;
            if err != 0.0 || prev.is_none() {
                // Outside the deadband (or first frame): the new value is relative to what this frame used.
                next.exposure[a] = e;
                next.exposure[b] = e;
                next.gain[a] = g;
                next.gain[b] = g;
            }
        }
        next.led_exposure = self.p.led_exposure;
        next.led_gain = self.p.led_gain;
        let changed = prev != Some(next);
        if changed {
            self.changes += 1;
        }
        self.target = Some(next);
        let due = match self.last_send_ns {
            None => true,
            Some(t) => now_ns.saturating_sub(t) >= self.p.period_ns,
        };
        let send = due || (changed && prev.is_none());
        if send {
            self.last_send_ns = Some(now_ns);
            self.sends += 1;
        }
        AeDecision {
            stats,
            err: errs,
            changed,
            send,
        }
    }

    /// Pool the tiles of each pair of a tracking frame into one histogram per pair.
    pub fn frame_stats(&self, tiles: &[TileView; 4], mask: &Mask) -> [TileStats; 2] {
        let mut out = [TileStats::default(); 2];
        for (k, pair) in self.p.pairs.iter().enumerate() {
            let mut h = Histogram::default();
            for &t in pair {
                h.merge(&tile_histogram(&tiles[t], self.p.step, mask));
            }
            out[k] = h.stats(self.p.trim, self.p.sat_level);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_matches_the_windows_frame() {
        // camera-frames.md: 58 19 51 00 01 32 47 0c 47 0c 57 08 57 08 0d 00 0d 00 0d 00 0d 00 28 00 3e 00
        let s = AeSetting {
            exposure: [0x0c47, 0x0c47, 0x0857, 0x0857],
            gain: [13; 4],
            led_exposure: 40,
            led_gain: 62,
        };
        let p = payload(&s);
        let want = [
            0x01, 0x32, 0x47, 0x0c, 0x47, 0x0c, 0x57, 0x08, 0x57, 0x08, 0x0d, 0x00, 0x0d, 0x00,
            0x0d, 0x00, 0x0d, 0x00, 0x28, 0x00, 0x3e, 0x00,
        ];
        assert_eq!(p, want);
        let mut report = vec![0x58, 0x19, 0x51, 0x00];
        report.extend_from_slice(&p);
        assert_eq!(parse_payload(&report), Some(s));
        assert_eq!(parse_payload(&p), Some(s));
        assert_eq!(parse_payload(&[0x58, 0x04, 0x39, 0x00, 0x48]), None);
    }

    #[test]
    fn histogram_stats() {
        let mut h = Histogram::default();
        h.bins[10] = 50;
        h.bins[100] = 48;
        h.bins[255] = 2;
        let s = h.stats(0.98, 250);
        assert_eq!(s.n, 100);
        assert!((s.mean - (500.0 + 4800.0 + 510.0) / 100.0).abs() < 1e-9);
        // The two saturated pixels sit above the 98th percentile: trimmed mean leaves them out.
        assert!((s.trimmed_mean - (500.0 + 4800.0) / 98.0).abs() < 1e-9);
        assert_eq!(s.p50, 10.0);
        assert!((s.sat_frac - 0.02).abs() < 1e-12);
        assert_eq!(h.quantile(0.51), 100.0);
    }

    #[test]
    fn split_prefers_exposure() {
        let p = AeParams::default();
        // On the ladder at gain 13 (2200 and 3143 are Windows values).
        assert_eq!(p.split(2200.0 * 13.0), (2200, 13));
        assert_eq!(p.split(3150.0 * 13.0), (3143, 13));
        // Above the exposure cap: smallest gain code that reaches the product, exposure below the cap.
        let (e, g) = p.split(4500.0 * 13.0);
        assert_eq!(g, 15);
        assert!(
            e <= p.exposure_max
                && (e as f64 * 15.0 - 4500.0 * 13.0).abs() / (4500.0 * 13.0) < 0.035
        );
        // Clamped at both ends.
        assert_eq!(p.split(1.0), (p.exposure_min, p.gain_min));
        assert_eq!(p.split(1e12), (p.exposure_max, p.gain_max));
        // Without quantisation: plain exposure-first split.
        let q = AeParams {
            quantize: false,
            ..AeParams::default()
        };
        assert_eq!(q.split(2000.0 * 13.0), (2000, 13));
        // The ladder reproduces Windows' observed values.
        for w in [
            1682.0, 1733.0, 1785.0, 2011.0, 2478.0, 3051.0, 3988.0, 4108.0,
        ] {
            assert_eq!(ladder(w), w);
        }
    }

    /// A simulated sensor: pixel = clamp(radiance * e * g * c), with a textured scene and a bright light blob.
    fn render(radiance: f64, e: u16, g: u16, blob: bool) -> Vec<u8> {
        let mut img = vec![0u8; 640 * 480];
        let c = 1.0 / (2000.0 * 13.0);
        for y in 0..480 {
            for x in 0..640 {
                let tex =
                    0.6 + 0.4 * (((x / 16 + y / 16) % 2) as f64) + 0.1 * ((x as f64 * 0.1).sin());
                let mut v = radiance * tex * e as f64 * g as f64 * c;
                if blob && (x as i32 - 500).pow(2) + (y as i32 - 100).pow(2) < 900 {
                    v = 1e4;
                }
                img[y * 640 + x] = v.round().clamp(0.0, 255.0) as u8;
            }
        }
        img
    }

    fn view(d: &[u8]) -> TileView<'_> {
        TileView {
            data: d,
            stride: 640,
            x0: 0,
            y0: 0,
            w: 640,
            h: 480,
        }
    }

    fn stats_of(ae: &AutoExposure, img0: &[u8], img1: &[u8]) -> [TileStats; 2] {
        let tiles = [view(img0), view(img1), view(img0), view(img1)];
        ae.frame_stats(&tiles, &Mask::default())
    }

    /// Closed loop with a 2-frame application latency (the camera applies a command two tracking frames later),
    /// starting at the Linux power-on default in a bright room: converges into the deadband without
    /// oscillating, keeps the pair equal and stays within the limits.
    #[test]
    fn converges_from_power_on_with_latency() {
        for &(radiance, blob) in &[(90.0, true), (30.0, false), (300.0, false), (6.0, false)] {
            let mut ae = AutoExposure::new(AeParams::default());
            let mut applied = AeSetting {
                exposure: [POWER_ON_EXPOSURE; 4],
                gain: [POWER_ON_GAIN; 4],
                led_exposure: 40,
                led_gain: 62,
            };
            // Measured on Windows: a command sent after tracking frame i is in effect from tracking frame i + 2.
            let mut pipeline: Vec<(usize, AeSetting)> = Vec::new();
            let mut means = Vec::new();
            let mut t = 0u64;
            for i in 0..150usize {
                while pipeline.first().is_some_and(|(f, _)| *f <= i) {
                    applied = pipeline.remove(0).1;
                }
                let img = render(radiance, applied.exposure[0], applied.gain[0], blob);
                let st = stats_of(&ae, &img, &img);
                means.push(st[0].trimmed_mean);
                let d = ae.update(&applied, st, t);
                if d.send {
                    pipeline.push((i + 2, ae.target().unwrap()));
                }
                let s = ae.target().unwrap();
                assert_eq!(s.exposure[0], s.exposure[1]);
                assert_eq!(s.gain[2], s.gain[3]);
                assert!(s.exposure[0] <= ae.p.exposure_max && s.gain[0] <= ae.p.gain_max);
                t += 40_000_000;
            }
            // Too dark even for the caps: keeps the brighter power-on setting or ends at the caps.
            let at_caps = (applied.exposure[0] == ae.p.exposure_max
                && applied.gain[0] == ae.p.gain_max)
                || (applied.exposure[0] == ae.p.exposure_min && applied.gain[0] == ae.p.gain_min)
                || applied.exposure[0] == POWER_ON_EXPOSURE;
            let in_band = |m: f64| (m / ae.p.target).ln().abs() < 0.25;
            // Settles within 80 tracking frames (3.2 s) from the power-on default (or ends at the caps when the
            // scene is too dark for any setting).
            let settled = means.iter().position(|&m| in_band(m));
            eprintln!(
                "radiance {radiance}: settled after {settled:?} tracking frames, final {:?}",
                applied
            );
            assert!(
                at_caps || settled.is_some_and(|i| i < 80),
                "radiance {radiance}: {means:?}"
            );
            // Then steady: no limit cycle in the last 20 frames.
            let tail = &means[means.len() - 20..];
            let (lo, hi) = tail
                .iter()
                .fold((f64::MAX, f64::MIN), |(a, b), &m| (a.min(m), b.max(m)));
            assert!(hi - lo < 1.0, "radiance {radiance}: not steady {tail:?}");
            assert!(at_caps || in_band(*tail.last().unwrap()));
        }
    }

    #[test]
    fn deadband_and_period() {
        let mut ae = AutoExposure::new(AeParams::default());
        let s = AeSetting {
            exposure: [2000; 4],
            gain: [13; 4],
            led_exposure: 40,
            led_gain: 62,
        };
        let st = TileStats {
            trimmed_mean: 66.0,
            n: 1000,
            ..Default::default()
        };
        let d = ae.update(&s, [st, st], 0);
        assert!(d.send); // first update always sends
        assert_eq!(ae.target().unwrap().exposure, [2000; 4]);
        let d = ae.update(&s, [st, st], 40_000_000);
        assert!(!d.send && !d.changed);
        let d = ae.update(&s, [st, st], 80_000_000);
        assert!(d.send && !d.changed); // periodic push like Windows
                                       // A darker scene raises the exposure of both pairs.
        let dark = TileStats {
            trimmed_mean: 35.0,
            n: 1000,
            ..Default::default()
        };
        let d = ae.update(&s, [dark, st], 120_000_000);
        assert!(d.changed && d.err[0] > 0.0 && d.err[1] == 0.0);
        let t = ae.target().unwrap();
        assert!(t.exposure[0] > 2000 && t.exposure[0] == t.exposure[1]);
        assert_eq!(t.exposure[2], 2000);
        // Mostly saturated with an acceptable trimmed mean still pulls down.
        let sat = TileStats {
            trimmed_mean: 95.0,
            sat_frac: 0.2,
            n: 1000,
            ..Default::default()
        };
        let (e, _, err) = ae.step_pair(2000, 13, &sat);
        assert!(err < 0.0 && e < 2000);
        // Too dark even at the power-on setting: keep it (the exposure cap would make it darker).
        let black = TileStats {
            trimmed_mean: 7.0,
            n: 1000,
            ..Default::default()
        };
        assert_eq!(
            ae.step_pair(POWER_ON_EXPOSURE, POWER_ON_GAIN, &black).0,
            POWER_ON_EXPOSURE
        );
        // Bright at the power-on setting: exposure drops to the cap first, gain carries the rest.
        let bright = TileStats {
            trimmed_mean: 240.0,
            sat_frac: 0.6,
            n: 1000,
            ..Default::default()
        };
        let (e, g, _) = ae.step_pair(POWER_ON_EXPOSURE, POWER_ON_GAIN, &bright);
        assert!(
            e <= ae.p.exposure_max
                && (e as f64 * g as f64) < POWER_ON_EXPOSURE as f64 * POWER_ON_GAIN as f64
        );
    }

    // ---- low-light governor: extended range and the "tracking light low" warning ----

    const S: u64 = 1_000_000_000;

    /// Closed-loop run in a scene of constant `radiance` (2-frame latency, 40 ms tracking period) starting from
    /// frame `from` of `ae`'s clock; returns the applied setting and checks the invariants on every frame.
    fn run_scene(
        ae: &mut AutoExposure,
        applied: &mut AeSetting,
        pipeline: &mut Vec<(usize, AeSetting)>,
        radiance: f64,
        frames: usize,
        t: &mut u64,
        events: &mut Vec<(u64, LightEvent)>,
    ) {
        // Commands still in flight from the previous call land at once.
        if let Some((_, last)) = pipeline.pop() {
            *applied = last;
            pipeline.clear();
        }
        for i in 0..frames {
            while pipeline.first().is_some_and(|(f, _)| *f <= i) {
                *applied = pipeline.remove(0).1;
            }
            let img = render(radiance, applied.exposure[0], applied.gain[0], false);
            let st = stats_of(ae, &img, &img);
            let d = ae.update(applied, st, *t);
            for ev in ae.take_events() {
                events.push((*t, ev));
            }
            if d.send {
                pipeline.push((i + 2, ae.target().unwrap()));
            }
            let s = ae.target().unwrap();
            // The LED frames are never touched.
            assert_eq!((s.led_exposure, s.led_gain), (40, 62));
            // Gain before exposure: above the normal exposure ceiling the gain is at its cap.
            if s.exposure[0] > 4108 {
                assert_eq!(s.gain[0], POWER_ON_GAIN, "exposure {} with gain {}", s.exposure[0], s.gain[0]);
            }
            // The exposure stays far below the tracking frame period (40 ms at 25 Hz).
            assert!((s.exposure[0] as u64) * 1000 < 40_000_000);
            assert!(s.exposure[0] <= 10410 && s.gain[0] <= 449);
            *t += 40_000_000;
        }
    }

    fn power_on() -> AeSetting {
        AeSetting {
            exposure: [POWER_ON_EXPOSURE; 4],
            gain: [POWER_ON_GAIN; 4],
            led_exposure: 40,
            led_gain: 62,
        }
    }

    #[test]
    fn extends_gain_first_then_exposure_and_retracts() {
        let mut ae = AutoExposure::new(AeParams::default());
        // Start at the normal ceiling in a room too dark for it (mean ~8 there).
        let mut applied = AeSetting {
            exposure: [4108; 4],
            gain: [215; 4],
            led_exposure: 40,
            led_gain: 62,
        };
        let (mut pipe, mut ev, mut t) = (Vec::new(), Vec::new(), 0u64);
        run_scene(&mut ae, &mut applied, &mut pipe, 0.3, 900, &mut t, &mut ev);
        let ext: Vec<(u64, LightEvent)> = ev.iter().copied().filter(|e| matches!(e.1, LightEvent::Extended(_))).collect();
        assert_eq!(ext.len(), 2, "{ev:?}");
        assert_eq!(ext[0].1, LightEvent::Extended(1));
        assert_eq!(ext[1].1, LightEvent::Extended(2));
        // Not before 5 s at the ceiling, and the second step needs another 5 s at the stage-1 ceiling.
        assert!(ext[0].0 >= 5 * S && ext[0].0 < 8 * S, "{ext:?}");
        assert!(ext[1].0 - ext[0].0 >= 5 * S, "{ext:?}");
        assert_eq!(ae.light.stage, 2);
        assert_eq!((ae.p.exposure_max, ae.p.gain_max), (10410, 449));
        // Stage 1 (gain only) reached the gain cap before any exposure above 4108 was needed.
        assert!(applied.exposure[0] > 4108 || applied.gain[0] == 449);
        // The scene is bright enough with the extended range (mean ~45): no warning.
        assert!(!ae.light.low && !ev.iter().any(|e| e.1 == LightEvent::LowRaised));

        // Light comes back (lamp on): the loop settles low and after 8 s the normal range returns.
        run_scene(&mut ae, &mut applied, &mut pipe, 30.0, 600, &mut t, &mut ev);
        assert_eq!(ae.light.stage, 0, "{ev:?}");
        assert!(ev.iter().any(|e| e.1 == LightEvent::Retracted));
        assert_eq!((ae.p.exposure_max, ae.p.gain_max), (4108, 215));
        assert!(applied.exposure[0] <= 4108 && applied.gain[0] <= 215, "{applied:?}");
    }

    #[test]
    fn gain_goes_to_the_extended_cap_before_exposure_passes_the_normal_one() {
        let p = AeParams {
            exposure_max: 10410,
            gain_max: 449,
            ..AeParams::default()
        };
        // Products between the normal ceiling and 449 x 4108: gain 449, exposure below 4108.
        let (e, g) = p.split(4108.0 * 300.0);
        assert_eq!(g, 449);
        assert!(e <= 4108 && (e as f64 * 449.0 - 4108.0 * 300.0).abs() / (4108.0 * 300.0) < 0.035);
        // Normal-range products ignore the extended exposure (no blur for no reason).
        let (e, g) = p.split(3000.0 * 13.0);
        assert_eq!((e, g), (ladder(3000.0) as u16, 13));
        let (e, g) = p.split(4108.0 * 215.0);
        assert!(e <= 4108 && g <= 449);
        // Beyond 449 x 4108 the exposure grows, up to 10410.
        let (e, g) = p.split(6000.0 * 449.0);
        assert_eq!(g, 449);
        assert!(e > 4108 && e <= 10410);
        assert_eq!(p.split(1e12), (10410, 449));
    }

    /// Feed the governor `secs` seconds of 25 Hz tracking frames with a constant state.
    #[allow(clippy::too_many_arguments)]
    fn tick(
        lp: &LightParams,
        g: &mut LightGov,
        t: &mut u64,
        ev: &mut Vec<LightEvent>,
        secs: f64,
        mean: f64,
        at: bool,
        settled: bool,
    ) {
        for _ in 0..(secs * 25.0).round() as u64 {
            ev.extend(g.step(lp, *t, mean, at, settled, 2));
            *t += 40_000_000;
        }
    }

    #[test]
    fn governor_timers_and_hysteresis() {
        let lp = LightParams::default();
        let mut g = LightGov::default();
        let mut t = 0u64;
        let mut all = Vec::new();
        // 4 s dark at the ceiling: not yet.
        tick(&lp, &mut g, &mut t, &mut all, 4.0, 10.0, true, false);
        assert_eq!(g.stage, 0);
        // Not at the ceiling (the loop still has room): the timer does not run.
        tick(&lp, &mut g, &mut t, &mut all, 10.0, 10.0, false, false);
        assert_eq!(g.stage, 0);
        tick(&lp, &mut g, &mut t, &mut all, 6.0, 10.0, true, false);
        assert_eq!(g.stage, 1);
        tick(&lp, &mut g, &mut t, &mut all, 6.0, 10.0, true, false);
        assert_eq!(g.stage, 2);
        assert_eq!(all, vec![LightEvent::Extended(1), LightEvent::Extended(2)]);
        // A short bright spell does not retract; a long one does.
        tick(&lp, &mut g, &mut t, &mut all, 5.0, 90.0, false, false);
        tick(&lp, &mut g, &mut t, &mut all, 1.0, 30.0, false, false);
        assert_eq!(g.stage, 2);
        tick(&lp, &mut g, &mut t, &mut all, 9.0, 90.0, false, false);
        assert_eq!(g.stage, 0);
        assert_eq!(all.last(), Some(&LightEvent::Retracted));
        // A gap in the frames (> 2 s) resets a running timer.
        let mut g = LightGov::default();
        let mut t = 0u64;
        tick(&lp, &mut g, &mut t, &mut all, 4.0, 10.0, true, false);
        t += 3 * S;
        tick(&lp, &mut g, &mut t, &mut all, 4.0, 10.0, true, false);
        assert_eq!(g.stage, 0);
    }

    #[test]
    fn low_light_warning_raises_and_clears() {
        let lp = LightParams::default();
        let mut g = LightGov::default();
        g.stage = 2;
        let mut t = 0u64;
        let mut ev = Vec::new();
        // 9 s at mean 12 with the full range at its ceiling: not yet; 10 s: raised.
        tick(&lp, &mut g, &mut t, &mut ev, 9.0, 12.0, true, false);
        assert!(!g.low);
        tick(&lp, &mut g, &mut t, &mut ev, 1.5, 12.0, true, false);
        assert!(g.low);
        // Mean 20: above the raise level but not above the clear level (25): stays raised.
        tick(&lp, &mut g, &mut t, &mut ev, 10.0, 20.0, true, false);
        assert!(g.low);
        // Light recovers (> 25 for 3 s): cleared.
        tick(&lp, &mut g, &mut t, &mut ev, 2.0, 40.0, true, false);
        assert!(g.low);
        tick(&lp, &mut g, &mut t, &mut ev, 1.5, 40.0, true, false);
        assert!(!g.low);
        assert_eq!(ev, vec![LightEvent::LowRaised, LightEvent::LowCleared]);
        // Dark but the loop is not at its ceiling (still adjusting): no warning.
        let mut g = LightGov { stage: 2, ..LightGov::default() };
        ev.clear();
        tick(&lp, &mut g, &mut t, &mut ev, 20.0, 5.0, false, false);
        assert!(!g.low);
        // Without an extended range (feature off, top stage 0) the warning still works on the normal ceiling.
        let mut g = LightGov::default();
        for _ in 0..(11 * 25) {
            ev.extend(g.step(&lp, t, 8.0, true, false, 0));
            t += 40_000_000;
        }
        assert!(g.low && g.stage == 0);
    }

    #[test]
    fn closed_loop_warning_in_a_very_dark_room_and_clear_when_lit() {
        let mut ae = AutoExposure::new(AeParams::default());
        let mut applied = power_on();
        let (mut pipe, mut ev, mut t) = (Vec::new(), Vec::new(), 0u64);
        // Radiance so low that even 10410 us at gain 449 gives a mean below 15.
        run_scene(&mut ae, &mut applied, &mut pipe, 0.08, 1100, &mut t, &mut ev);
        assert_eq!(ae.light.stage, 2);
        assert!(ae.light.low, "{ev:?}");
        assert!(ev.iter().any(|e| e.1 == LightEvent::LowRaised));
        let raised = ev.iter().find(|e| e.1 == LightEvent::LowRaised).unwrap().0;
        // After the extension (2 x 5 s from the start) plus 10 s dark: not earlier than 10 s.
        assert!(raised >= 10 * S);
        // A lamp is switched on: the warning clears.
        run_scene(&mut ae, &mut applied, &mut pipe, 20.0, 400, &mut t, &mut ev);
        assert!(!ae.light.low, "{ev:?}");
        assert!(ev.iter().any(|e| e.1 == LightEvent::LowCleared));
    }

    #[test]
    fn extension_is_off_with_the_env_switch_or_when_the_caps_are_already_the_largest() {
        let off = AeParams {
            light: LightParams { enabled: false, ..LightParams::default() },
            ..AeParams::default()
        };
        assert_eq!(AutoExposure::new(off).top_stage(), 0);
        let big = AeParams {
            exposure_max: 10410,
            gain_max: 449,
            ..AeParams::default()
        };
        assert_eq!(AutoExposure::new(big).top_stage(), 0);
        assert_eq!(AutoExposure::new(AeParams::default()).top_stage(), 2);
        // Normal caps are Windows' 4108 / 215 and the extended ones the power-on values.
        let ae = AutoExposure::new(AeParams::default());
        assert_eq!(ae.caps(0), (4108, 215));
        assert_eq!(ae.caps(1), (4108, 449));
        assert_eq!(ae.caps(2), (10410, 449));
    }
}
