//! Is a controller in the hand or lying somewhere? (PIMAX_CTRL_INHAND, docs/CONTROLLERS-INHAND.md)
//!
//! In Work (the WayVR desk) the controllers often lie on the desk. The optical tracker then loses or misreads them
//! and their models and rays jump around the scene. This detector looks at the controller's own IMU and inputs and
//! says "held" or "down"; the Monado driver reports no pose (and inactive inputs) while a controller is down.
//!
//! Signals, from the session 2026-10-16-1818 (left controller, 518 s, gyro 0.122 deg/s per LSB, accel ~1024 LSB/g;
//! fixtures `rust/fixtures/inhand_*.csv.gz`):
//! * the IMU samples (~490 per second) are averaged over 20 ms blocks (removes vibration above ~50 Hz, keeps the
//!   hand's tremor), and the block means are looked at over 0.5 s windows;
//! * **still window**: on every gyro and accel axis the standard deviation of the block means is under 2.5 LSB.
//!   Lying on the desk: gyro p50 0.74 / p95 2.6 LSB, accel p50 0.54 / p95 1.4 LSB, 94 % of the windows still.
//!   In the hand, even when quiet: gyro p5 2.6 / p50 8.4 LSB, accel p5 3.0 / p50 6.2 LSB, 2 % still;
//! * **put down**: 8 of the last 10 windows (5 s) still, none of them with real motion (gyro std >= 8 LSB), and no
//!   input change for 5 s. In the hand at most 4 of 10 windows were ever still;
//! * **picked up**: the gyro (block mean minus the bias learned while still) above 30 LSB (3.7 deg/s) on some axis
//!   for 6 blocks in a row (120 ms), or the controller tilted more than 10 deg from where it lay for 3 blocks
//!   (60 ms), or any input change (button, trigger, grip, stick, capacitive touch). On the desk the biggest block was
//!   28 LSB for one block (a knock at 70.7 s) and the tilt never passed 1.7 deg;
//! * **no reports** for 1.5 s (switched off, asleep, out of range): down.
//!
//! Inputs count when they change, not while they are held: a controller lying on its trigger stays down.

/// Thresholds; [`Config::default`] holds the values tuned on the recording.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Config {
    /// IMU averaging block (ms).
    pub block_ms: u64,
    /// Blocks per stillness window (25 x 20 ms = 0.5 s).
    pub window_blocks: usize,
    /// A window is still when every gyro and accel axis' block-mean standard deviation is below this (LSB).
    pub still_lsb: f64,
    /// A window with a gyro standard deviation at or above this (LSB) is real motion: no put-down in that span.
    pub motion_lsb: f64,
    /// Windows looked at for a put-down (10 x 0.5 s = 5 s).
    pub down_windows: usize,
    /// Still windows among them needed for a put-down.
    pub down_still: usize,
    /// Pick-up: gyro block mean minus bias above this on some axis (LSB) ...
    pub move_lsb: f64,
    /// ... for this many blocks in a row (6 x 20 ms = 120 ms).
    pub move_blocks: u32,
    /// Pick-up: tilt from the resting accel direction above this (deg) ...
    pub tilt_deg: f64,
    /// ... for this many blocks in a row.
    pub tilt_blocks: u32,
    /// No report for this long (s): down.
    pub no_report_s: f64,
    /// Input changes keep (or make) the controller held for this long (s).
    pub input_hold_s: f64,
    /// Analog change that counts as input (0..255 trigger / grip, -128..127 stick).
    pub analog_step: i32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            block_ms: 20,
            window_blocks: 25,
            still_lsb: 2.5,
            motion_lsb: 8.0,
            down_windows: 10,
            down_still: 8,
            move_lsb: 30.0,
            move_blocks: 6,
            tilt_deg: 10.0,
            tilt_blocks: 3,
            no_report_s: 1.5,
            input_hold_s: 5.0,
            analog_step: 24,
        }
    }
}

impl Config {
    /// Defaults, `PIMAX_CTRL_INHAND_DOWN_S` (seconds of stillness before a put-down, 1..60, default 5) on top.
    pub fn from_env() -> Self {
        let mut c = Config::default();
        if let Some(s) = std::env::var("PIMAX_CTRL_INHAND_DOWN_S")
            .ok()
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|s| (1.0..=60.0).contains(s))
        {
            let win_s = c.block_ms as f64 * c.window_blocks as f64 / 1000.0;
            c.down_windows = (s / win_s).round().max(2.0) as usize;
            c.down_still = ((c.down_windows as f64) * 0.8).ceil() as usize;
            c.input_hold_s = s;
        }
        c
    }
}

/// What the switch in the app says (PIMAX_CTRL_INHAND).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    /// "When picked up": shown only while held (Work's default).
    #[default]
    Auto,
    /// "Always": shown whenever reported (Play's default, games expect controllers).
    Always,
    /// "Off": never shown, inputs inactive.
    Off,
}

impl Mode {
    /// `auto` / `when-held` / `held`, `always` / `on`, `off` / `never`.
    pub fn parse(s: &str) -> Option<Mode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" | "when-held" | "when_held" | "held" | "picked-up" => Some(Mode::Auto),
            "always" | "on" | "1" => Some(Mode::Always),
            "off" | "never" | "0" => Some(Mode::Off),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Mode::Auto => "auto",
            Mode::Always => "always",
            Mode::Off => "off",
        }
    }
}

/// Why the state last changed (for the log and the app).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    Start,
    Motion,
    Tilt,
    Input,
    Still,
    NoReports,
}

impl Why {
    pub fn name(self) -> &'static str {
        match self {
            Why::Start => "start",
            Why::Motion => "motion",
            Why::Tilt => "tilt",
            Why::Input => "input",
            Why::Still => "still",
            Why::NoReports => "no reports",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Inputs {
    trigger: i32,
    grip: i32,
    stick_x: i32,
    stick_y: i32,
    buttons: u32,
    touch: u8,
}

/// One controller's detector. Feed it every IMU sample and every report's inputs; ask [`InHand::held`].
#[derive(Clone, Debug)]
pub struct InHand {
    pub cfg: Config,
    held: bool,
    why: Why,
    changed_ns: u64,
    last_report_ns: Option<u64>,
    last_input_ns: Option<u64>,
    inputs: Option<Inputs>,
    // current block
    blk_start: Option<u64>,
    blk_n: u32,
    blk_g: [f64; 3],
    blk_a: [f64; 3],
    // current window (block means)
    win: Vec<([f64; 3], [f64; 3])>,
    // last windows: (still, gyro std)
    ring: std::collections::VecDeque<(bool, f64)>,
    bias: Option<[f64; 3]>,
    rest_a: Option<[f64; 3]>,
    move_run: u32,
    tilt_run: u32,
    /// Transitions so far (held, down).
    pub counts: (u32, u32),
}

impl Default for InHand {
    fn default() -> Self {
        InHand::new(Config::default())
    }
}

fn std3(v: &[([f64; 3], [f64; 3])], gyro: bool) -> f64 {
    let n = v.len().max(1) as f64;
    (0..3)
        .map(|k| {
            let x = |e: &([f64; 3], [f64; 3])| if gyro { e.0[k] } else { e.1[k] };
            let m = v.iter().map(x).sum::<f64>() / n;
            (v.iter().map(|e| (x(e) - m).powi(2)).sum::<f64>() / n).sqrt()
        })
        .fold(0.0, f64::max)
}

fn angle_deg(a: [f64; 3], b: [f64; 3]) -> f64 {
    let na = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    let nb = (b[0] * b[0] + b[1] * b[1] + b[2] * b[2]).sqrt();
    if na < 1e-6 || nb < 1e-6 {
        return 0.0;
    }
    let c = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2]) / (na * nb);
    c.clamp(-1.0, 1.0).acos().to_degrees()
}

impl InHand {
    /// Starts "down": a controller shows once it is picked up (or a button is pressed).
    pub fn new(cfg: Config) -> Self {
        InHand {
            cfg,
            held: false,
            why: Why::Start,
            changed_ns: 0,
            last_report_ns: None,
            last_input_ns: None,
            inputs: None,
            blk_start: None,
            blk_n: 0,
            blk_g: [0.0; 3],
            blk_a: [0.0; 3],
            win: Vec::with_capacity(cfg.window_blocks),
            ring: std::collections::VecDeque::with_capacity(cfg.down_windows),
            bias: None,
            rest_a: None,
            move_run: 0,
            tilt_run: 0,
            counts: (0, 0),
        }
    }

    fn set(&mut self, held: bool, why: Why, t: u64) {
        if self.held == held {
            return;
        }
        self.held = held;
        self.why = why;
        self.changed_ns = t;
        self.move_run = 0;
        self.tilt_run = 0;
        self.ring.clear();
        if held {
            self.counts.0 += 1;
        } else {
            self.counts.1 += 1;
        }
    }

    /// A report arrived at `t_ns` (keeps the no-report timeout away).
    pub fn report_seen(&mut self, t_ns: u64) {
        self.last_report_ns = Some(t_ns);
    }

    /// The inputs of a report: trigger and grip 0..255, stick -128..127, `buttons` (PIMAX_BTN_*), `touch` (byte 49).
    #[allow(clippy::too_many_arguments)]
    pub fn input(
        &mut self,
        t_ns: u64,
        trigger: u8,
        grip: u8,
        stick_x: i32,
        stick_y: i32,
        buttons: u32,
        touch: u8,
    ) {
        self.report_seen(t_ns);
        let now = Inputs {
            trigger: trigger as i32,
            grip: grip as i32,
            stick_x,
            stick_y,
            buttons,
            // the capacitive bits the protocol knows (A/X, B/Y, stick); others are unknown
            touch: touch
                & (crate::report::TOUCH_A | crate::report::TOUCH_B | crate::report::TOUCH_STICK),
        };
        let Some(old) = self.inputs.replace(now) else {
            return; // first report: nothing to compare with
        };
        let step = self.cfg.analog_step;
        let changed = now.buttons != old.buttons
            || now.touch != old.touch
            || (now.trigger - old.trigger).abs() >= step
            || (now.grip - old.grip).abs() >= step
            || (now.stick_x - old.stick_x).abs() >= step
            || (now.stick_y - old.stick_y).abs() >= step;
        if changed {
            self.last_input_ns = Some(t_ns);
            self.set(true, Why::Input, t_ns);
        } else if step > 1 {
            // slow analog drift: compare with the value at the last counted change
            self.inputs = Some(Inputs {
                trigger: old.trigger,
                grip: old.grip,
                stick_x: old.stick_x,
                stick_y: old.stick_y,
                ..now
            });
        }
    }

    /// One IMU sample (raw LSB) at `t_ns`.
    pub fn sample(&mut self, t_ns: u64, g: [i16; 3], a: [i16; 3]) {
        self.report_seen(t_ns);
        let start = *self.blk_start.get_or_insert(t_ns);
        if t_ns >= start + self.cfg.block_ms * 1_000_000 || t_ns < start {
            if self.blk_n > 0 {
                let n = self.blk_n as f64;
                let gm = self.blk_g.map(|x| x / n);
                let am = self.blk_a.map(|x| x / n);
                self.block(t_ns, gm, am);
            }
            self.blk_start = Some(t_ns);
            self.blk_n = 0;
            self.blk_g = [0.0; 3];
            self.blk_a = [0.0; 3];
        }
        self.blk_n += 1;
        for k in 0..3 {
            self.blk_g[k] += g[k] as f64;
            self.blk_a[k] += a[k] as f64;
        }
    }

    fn block(&mut self, t: u64, g: [f64; 3], a: [f64; 3]) {
        let bias = *self.bias.get_or_insert(g);
        let rest = *self.rest_a.get_or_insert(a);
        self.win.push((g, a));
        if self.win.len() >= self.cfg.window_blocks {
            let gs = std3(&self.win, true);
            let as_ = std3(&self.win, false);
            let still = gs < self.cfg.still_lsb && as_ < self.cfg.still_lsb;
            if still {
                let n = self.win.len() as f64;
                let mut gm = [0.0; 3];
                let mut am = [0.0; 3];
                for (wg, wa) in &self.win {
                    for k in 0..3 {
                        gm[k] += wg[k] / n;
                        am[k] += wa[k] / n;
                    }
                }
                let b = self.bias.unwrap_or(gm);
                self.bias = Some([
                    0.8 * b[0] + 0.2 * gm[0],
                    0.8 * b[1] + 0.2 * gm[1],
                    0.8 * b[2] + 0.2 * gm[2],
                ]);
                if !self.held {
                    self.rest_a = Some(am);
                }
            }
            self.win.clear();
            if self.ring.len() == self.cfg.down_windows {
                self.ring.pop_front();
            }
            self.ring.push_back((still, gs));
            if self.held && self.ring.len() == self.cfg.down_windows {
                let n_still = self.ring.iter().filter(|w| w.0).count();
                let calm = self.ring.iter().all(|w| w.1 < self.cfg.motion_lsb);
                let quiet_input = self
                    .last_input_ns
                    .is_none_or(|i| t.saturating_sub(i) as f64 * 1e-9 >= self.cfg.input_hold_s);
                if n_still >= self.cfg.down_still && calm && quiet_input {
                    self.set(false, Why::Still, t);
                    self.rest_a = Some(a);
                }
            }
        }
        if !self.held {
            let mo = (0..3).map(|k| (g[k] - bias[k]).abs()).fold(0.0, f64::max);
            self.move_run = if mo > self.cfg.move_lsb {
                self.move_run + 1
            } else {
                0
            };
            self.tilt_run = if angle_deg(a, rest) > self.cfg.tilt_deg {
                self.tilt_run + 1
            } else {
                0
            };
            if self.move_run >= self.cfg.move_blocks {
                self.set(true, Why::Motion, t);
            } else if self.tilt_run >= self.cfg.tilt_blocks {
                self.set(true, Why::Tilt, t);
            }
        }
    }

    /// One raw 64-byte controller report (IMU at bytes 16..41, see [`crate::fuse::report_gyro`]) plus its decoded
    /// inputs, received at `host_ns`.
    #[allow(clippy::too_many_arguments)]
    pub fn report(
        &mut self,
        host_ns: u64,
        raw: &[u8; 64],
        trigger: u8,
        grip: u8,
        stick_x: i32,
        stick_y: i32,
        buttons: u32,
        touch: u8,
    ) {
        let (_, g, n) = crate::fuse::report_gyro(raw);
        let a = crate::fuse::report_accel(raw);
        for k in 0..n {
            // the second sample is 2 ms after the first (device clock); host time is close enough for 20 ms blocks
            self.sample(
                host_ns.saturating_sub(((n - 1 - k) as u64) * 2_000_000),
                g[k],
                a[k],
            );
        }
        self.input(host_ns, trigger, grip, stick_x, stick_y, buttons, touch);
    }

    /// Held at `now_ns`? (No report for [`Config::no_report_s`]: down.)
    pub fn held(&mut self, now_ns: u64) -> bool {
        if self.held {
            let silent = self
                .last_report_ns
                .is_none_or(|r| now_ns.saturating_sub(r) as f64 * 1e-9 > self.cfg.no_report_s);
            if silent {
                self.set(false, Why::NoReports, now_ns);
            }
        }
        self.held
    }

    /// The state without the timeout check.
    pub fn is_held(&self) -> bool {
        self.held
    }

    /// Why the state last changed, and when.
    pub fn last_change(&self) -> (Why, u64) {
        (self.why, self.changed_ns)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_parse() {
        assert_eq!(Mode::parse("when-held"), Some(Mode::Auto));
        assert_eq!(Mode::parse(" Always "), Some(Mode::Always));
        assert_eq!(Mode::parse("off"), Some(Mode::Off));
        assert_eq!(Mode::parse("x"), None);
        for m in [Mode::Auto, Mode::Always, Mode::Off] {
            assert_eq!(Mode::parse(m.name()), Some(m));
        }
    }

    fn still_for(d: &mut InHand, t0: u64, secs: f64) -> u64 {
        let mut t = t0;
        let n = (secs * 500.0) as u64;
        for i in 0..n {
            t = t0 + i * 2_000_000;
            let j = (i % 3) as i16 - 1;
            d.sample(t, [-5 + j, j, -4], [-17, 669 + j, -726]);
        }
        t
    }

    #[test]
    fn starts_down_and_a_press_picks_up() {
        let mut d = InHand::default();
        let t = still_for(&mut d, 1, 1.0);
        assert!(!d.held(t));
        d.input(t, 0, 0, 0, 0, 0, 0);
        assert!(!d.held(t), "the first report is only a reference");
        d.input(t + 4_000_000, 0, 0, 0, 0, crate::report::BTN_A, 0);
        assert!(d.held(t + 4_000_000));
        assert_eq!(d.last_change().0, Why::Input);
    }

    #[test]
    fn lying_on_the_trigger_is_not_holding_it() {
        let mut d = InHand::default();
        let mut t = 0;
        for i in 0..2000u64 {
            t = i * 4_000_000;
            d.input(t, 200, 0, 0, 0, 0, 0); // constant: pressed by the desk
        }
        assert!(!d.held(t));
    }

    #[test]
    fn silence_puts_it_down() {
        let mut d = InHand::default();
        d.input(0, 0, 0, 0, 0, 0, 0);
        d.input(1_000_000, 0, 0, 0, 0, crate::report::BTN_B, 0);
        assert!(d.held(1_000_000));
        assert!(d.held(2_000_000_000 - 600_000_000));
        assert!(!d.held(3_000_000_000));
        assert_eq!(d.last_change().0, Why::NoReports);
    }

    #[test]
    fn env_down_seconds() {
        // not via the environment (tests run in parallel): the same arithmetic
        let c = Config::default();
        assert_eq!(
            c.down_windows as u64 * c.window_blocks as u64 * c.block_ms,
            5000
        );
    }
}
