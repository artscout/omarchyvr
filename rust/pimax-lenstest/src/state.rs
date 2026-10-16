//! What is on screen and what changes it: pattern, step, eye select, square direction, on-screen text, the timed
//! auto mode. Pure state machine, driven by [`Event`]s (controllers, keyboard) and [`State::tick`].

use crate::patterns::align::{Align, EyeCal};
use crate::patterns::{Dir, Pattern};
use crate::tune::Tune;

/// Which eyes show the pattern; the other eye is black.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EyeSelect {
    Both,
    Left,
    Right,
}

impl EyeSelect {
    pub fn shows(self, eye: usize) -> bool {
        match self {
            EyeSelect::Both => true,
            EyeSelect::Left => eye == 0,
            EyeSelect::Right => eye == 1,
        }
    }

    pub fn next(self) -> EyeSelect {
        match self {
            EyeSelect::Both => EyeSelect::Left,
            EyeSelect::Left => EyeSelect::Right,
            EyeSelect::Right => EyeSelect::Both,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            EyeSelect::Both => "both",
            EyeSelect::Left => "left",
            EyeSelect::Right => "right",
        }
    }

    pub fn from_name(s: &str) -> Option<EyeSelect> {
        match s.to_ascii_lowercase().as_str() {
            "both" | "b" => Some(EyeSelect::Both),
            "left" | "l" => Some(EyeSelect::Left),
            "right" | "r" => Some(EyeSelect::Right),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    NextPattern,
    PrevPattern,
    NextStep,
    PrevStep,
    /// Jump to the n-th pattern of the cycle (0 based).
    Goto(usize),
    CycleEye,
    ToggleAuto,
    ToggleHud,
    CycleDir,
    /// Thumbstick up (+1) or down (-1): square direction, or the vertical offset in the align pattern.
    StickVert(i8),
    /// Align pattern: change the vergence by this many units of 0.05 deg.
    AlignVerg(i32),
    /// Align pattern: change the vertical offset by this many units of 0.05 deg.
    AlignVert(i32),
    /// Reset the align offsets and the distortion tuning.
    AlignReset,
    /// Live distortion tuning: change dk1 and the residual scale by this many key steps.
    Tune(i32, i32),
    /// Live distortion tuning: change the mid-zone term dk2 by this many key steps.
    Tune2(i32),
    /// Swim pattern: put the room around the head's current position and heading.
    Recenter,
    /// Gaze pattern: accept (write) the fitted eye correction.
    Accept,
    Quit,
}

/// Everything that decides the pixels of the two eye images. When it changes the images are rendered again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderKey {
    pub pattern: Pattern,
    pub step: usize,
    pub eye: EyeSelect,
    pub dir: Dir,
    pub hud: bool,
    pub align: Align,
    pub tune: Tune,
}

#[derive(Clone, Debug)]
pub struct State {
    pub patterns: Vec<Pattern>,
    pub idx: usize,
    pub step: usize,
    pub eye: EyeSelect,
    /// Seconds per step in auto mode.
    pub auto_secs: f32,
    pub auto_on: bool,
    /// On-screen instructions wanted (patterns that measure glare fade them out, see `hud_visible`).
    pub hud: bool,
    pub dir: Dir,
    /// Stereo alignment offsets (align pattern).
    pub align: Align,
    /// Lens centre and gain per eye, for the align conversion.
    pub align_cal: [EyeCal; 2],
    /// Live distortion tuning (pre-warp of the grid and swim images).
    pub tune: Tune,
    /// Centre gain per eye, `base_g * scale` (normalises the tuning warp).
    pub tune_g: [f32; 2],
    /// The swim room was asked to re-centre (cleared by the client loop).
    pub recenter: bool,
    /// Seconds since the displayed pattern or step changed.
    pub since_change: f32,
    pub quit: bool,
}

/// Seconds after which the glare patterns hide the on-screen text.
pub const HUD_HIDE_AFTER: f32 = 4.0;

/// Number of steps of a pattern, given the direction of the square (supplied by the caller who knows the FOV).
pub type StepsFn<'a> = &'a dyn Fn(Pattern, Dir) -> usize;

impl State {
    pub fn new(patterns: Vec<Pattern>) -> State {
        assert!(!patterns.is_empty(), "at least one pattern");
        State {
            patterns,
            idx: 0,
            step: 0,
            eye: EyeSelect::Both,
            auto_secs: 6.0,
            auto_on: false,
            hud: true,
            dir: Dir::Right,
            align: Align::default(),
            align_cal: EyeCal::DEFAULT,
            tune: Tune::default(),
            tune_g: [0.65307; 2],
            recenter: false,
            since_change: 0.0,
            quit: false,
        }
    }

    pub fn pattern(&self) -> Pattern {
        self.patterns[self.idx]
    }

    fn changed(&mut self) {
        self.since_change = 0.0;
    }

    pub fn handle(&mut self, ev: Event, steps: StepsFn<'_>) {
        let n = self.patterns.len();
        match ev {
            Event::NextPattern => {
                self.idx = (self.idx + 1) % n;
                self.step = 0;
                self.changed();
            }
            Event::PrevPattern => {
                self.idx = (self.idx + n - 1) % n;
                self.step = 0;
                self.changed();
            }
            Event::Goto(i) => {
                self.idx = i.min(n - 1);
                self.step = 0;
                self.changed();
            }
            Event::NextStep if self.pattern() == Pattern::Align => self.align.add_verg(1),
            Event::PrevStep if self.pattern() == Pattern::Align => self.align.add_verg(-1),
            Event::ToggleAuto if self.pattern() == Pattern::Align => self.align = Align::default(),
            Event::StickVert(s) if self.pattern() == Pattern::Align => {
                self.align.add_vert(i32::from(s))
            }
            Event::StickVert(_) => self.handle(Event::CycleDir, steps),
            Event::AlignVerg(n) => self.align.add_verg(n),
            Event::AlignVert(n) => self.align.add_vert(n),
            Event::AlignReset => {
                self.align = Align::default();
                self.tune = Tune::default();
            }
            Event::Tune(dk, sc) => self.tune.add(dk, sc),
            Event::Tune2(n) => self.tune.add2(n),
            Event::Recenter => self.recenter = true,
            Event::Accept => {}
            Event::NextStep => {
                let count = steps(self.pattern(), self.dir).max(1);
                self.step = (self.step + 1) % count;
                self.changed();
            }
            Event::PrevStep => {
                let count = steps(self.pattern(), self.dir).max(1);
                self.step = (self.step + count - 1) % count;
                self.changed();
            }
            Event::CycleEye => {
                self.eye = self.eye.next();
                self.changed();
            }
            Event::ToggleAuto => {
                self.auto_on = !self.auto_on;
                self.since_change = 0.0;
            }
            Event::ToggleHud => {
                self.hud = !self.hud;
                self.changed();
            }
            Event::CycleDir => {
                self.dir = self.dir.next();
                self.step = 0;
                self.changed();
            }
            Event::Quit => self.quit = true,
        }
    }

    /// Advance the clock; in auto mode move on to the next step, or to the next pattern after the last step.
    pub fn tick(&mut self, dt: f32, steps: StepsFn<'_>) {
        self.since_change += dt;
        if self.auto_on && self.auto_secs > 0.0 && self.since_change >= self.auto_secs {
            self.advance(steps);
        }
    }

    /// Next step; after the last step of a pattern the next pattern (wraps around).
    pub fn advance(&mut self, steps: StepsFn<'_>) {
        let count = steps(self.pattern(), self.dir).max(1);
        if self.step + 1 < count {
            self.step += 1;
        } else {
            self.idx = (self.idx + 1) % self.patterns.len();
            self.step = 0;
        }
        self.changed();
    }

    pub fn hud_visible(&self) -> bool {
        self.hud && (!self.pattern().hides_hud() || self.since_change < HUD_HIDE_AFTER)
    }

    pub fn key(&self) -> RenderKey {
        RenderKey {
            pattern: self.pattern(),
            step: self.step,
            eye: self.eye,
            dir: self.dir,
            hud: self.hud_visible(),
            align: self.align,
            tune: self.tune,
        }
    }

    /// One line for the terminal and the on-screen text.
    pub fn status_line(&self, step_name: &str, steps: usize) -> String {
        let step = if steps > 1 {
            format!(" step {}/{} {}", self.step + 1, steps, step_name)
        } else {
            String::new()
        };
        format!(
            "[{}/{}] {}{} | eye {} | auto {}",
            self.idx + 1,
            self.patterns.len(),
            self.pattern().title(),
            step,
            self.eye.name(),
            if self.auto_on {
                format!("{:.0}s", self.auto_secs)
            } else {
                "off".to_string()
            }
        )
    }
}

pub const CONTROLS_HELP: &str = "controls: A/X next pattern, B/Y previous, stick left/right step (square position, channel), \
stick up/down square direction, stick click eye (both/left/right), trigger auto on/off, grip text on/off; \
keyboard (this terminal): n p . , e a h d 1-9 q; align pattern: [ ] vergence 0.05 deg, { } 0.25 deg, i k vertical; \
grid and swim: ; ' distortion dk1 -/+0.01 (: \" 0.05), o l mid-zone dk2 +/-0.01 (O L 0.05), - = residual scale -/+0.5% (_ + 2%), c re-centre the swim room; r resets all";

#[cfg(test)]
mod tests {
    use super::*;

    fn steps(p: Pattern, _d: Dir) -> usize {
        match p {
            Pattern::Channels => 4,
            Pattern::Square100 => 6,
            _ => 1,
        }
    }

    fn st() -> State {
        State::new(vec![Pattern::Grid, Pattern::Channels, Pattern::Square100])
    }

    #[test]
    fn patterns_cycle_both_ways_and_reset_the_step() {
        let mut s = st();
        s.handle(Event::NextPattern, &steps);
        assert_eq!(s.pattern(), Pattern::Channels);
        s.handle(Event::NextStep, &steps);
        s.handle(Event::NextStep, &steps);
        assert_eq!(s.step, 2);
        s.handle(Event::NextPattern, &steps);
        assert_eq!((s.pattern(), s.step), (Pattern::Square100, 0));
        s.handle(Event::NextPattern, &steps);
        assert_eq!(s.pattern(), Pattern::Grid);
        s.handle(Event::PrevPattern, &steps);
        assert_eq!(s.pattern(), Pattern::Square100);
        s.handle(Event::Goto(1), &steps);
        assert_eq!(s.pattern(), Pattern::Channels);
        s.handle(Event::Goto(99), &steps);
        assert_eq!(s.pattern(), Pattern::Square100);
    }

    #[test]
    fn steps_wrap_in_both_directions() {
        let mut s = st();
        s.handle(Event::Goto(1), &steps);
        s.handle(Event::PrevStep, &steps);
        assert_eq!(s.step, 3);
        s.handle(Event::NextStep, &steps);
        assert_eq!(s.step, 0);
        // a pattern with one step stays on step 0
        s.handle(Event::Goto(0), &steps);
        s.handle(Event::NextStep, &steps);
        s.handle(Event::PrevStep, &steps);
        assert_eq!(s.step, 0);
    }

    #[test]
    fn eye_hud_dir_auto_quit() {
        let mut s = st();
        let order: Vec<EyeSelect> = (0..4)
            .map(|_| {
                s.handle(Event::CycleEye, &steps);
                s.eye
            })
            .collect();
        assert_eq!(
            order,
            vec![
                EyeSelect::Left,
                EyeSelect::Right,
                EyeSelect::Both,
                EyeSelect::Left
            ]
        );
        assert!(s.eye.shows(0) && !s.eye.shows(1));
        assert!(EyeSelect::Both.shows(0) && EyeSelect::Both.shows(1));
        s.handle(Event::ToggleHud, &steps);
        assert!(!s.hud);
        s.handle(Event::Goto(2), &steps);
        s.handle(Event::NextStep, &steps);
        s.handle(Event::CycleDir, &steps);
        assert_eq!((s.dir, s.step), (Dir::Left, 0));
        s.handle(Event::ToggleAuto, &steps);
        assert!(s.auto_on);
        assert!(!s.quit);
        s.handle(Event::Quit, &steps);
        assert!(s.quit);
        assert_eq!(EyeSelect::from_name("R"), Some(EyeSelect::Right));
        assert_eq!(EyeSelect::from_name("x"), None);
    }

    #[test]
    fn auto_mode_walks_steps_then_patterns_and_wraps() {
        let mut s = st();
        s.auto_on = true;
        s.auto_secs = 2.0;
        let mut seen = vec![(s.pattern(), s.step)];
        for _ in 0..24 {
            s.tick(2.0, &steps);
            seen.push((s.pattern(), s.step));
        }
        assert_eq!(seen[1], (Pattern::Channels, 0));
        assert_eq!(seen[2], (Pattern::Channels, 1));
        assert_eq!(seen[5], (Pattern::Square100, 0));
        assert_eq!(seen[10], (Pattern::Square100, 5));
        assert_eq!(seen[11], (Pattern::Grid, 0)); // wrapped
                                                  // not before the time is up
        let mut t = st();
        t.auto_on = true;
        t.auto_secs = 2.0;
        t.tick(1.9, &steps);
        assert_eq!(t.pattern(), Pattern::Grid);
        t.tick(0.2, &steps);
        assert_eq!(t.pattern(), Pattern::Channels);
        // auto off: time passes, nothing moves
        let mut u = st();
        u.tick(100.0, &steps);
        assert_eq!(u.pattern(), Pattern::Grid);
        assert!((u.since_change - 100.0).abs() < 1e-3);
    }

    #[test]
    fn glare_patterns_hide_the_text_after_a_while_and_the_key_follows() {
        let mut s = st();
        s.handle(Event::Goto(2), &steps); // square100: hides the text
        assert!(s.hud_visible());
        let k0 = s.key();
        s.tick(HUD_HIDE_AFTER + 0.1, &steps);
        assert!(!s.hud_visible());
        assert_ne!(s.key(), k0);
        assert!(!s.key().hud);
        s.handle(Event::NextStep, &steps); // a change shows it again
        assert!(s.hud_visible());
        // the grid keeps its text; hud off hides it everywhere
        s.handle(Event::Goto(0), &steps);
        s.tick(100.0, &steps);
        assert!(s.hud_visible());
        s.handle(Event::ToggleHud, &steps);
        assert!(!s.hud_visible());
    }

    #[test]
    fn align_keys_and_controller_events() {
        let mut s = State::new(vec![Pattern::Align, Pattern::Grid]);
        s.handle(Event::AlignVerg(1), &steps);
        s.handle(Event::AlignVerg(5), &steps);
        s.handle(Event::AlignVert(-1), &steps);
        assert_eq!((s.align.verg, s.align.vert), (6, -1));
        assert_eq!(s.key().align, s.align);
        let k0 = s.key();
        // stick left/right = fine vergence, up/down = vertical, trigger = reset, in the align pattern
        s.handle(Event::PrevStep, &steps);
        s.handle(Event::StickVert(1), &steps);
        assert_eq!((s.align.verg, s.align.vert), (5, 0));
        assert_eq!(
            s.dir,
            Dir::Right,
            "stick up/down does not turn the square here"
        );
        assert_ne!(s.key(), k0);
        s.handle(Event::NextStep, &steps);
        assert_eq!(s.align.verg, 6);
        s.handle(Event::ToggleAuto, &steps);
        assert_eq!(s.align, Align::default());
        assert!(!s.auto_on);
        s.handle(Event::AlignVerg(3), &steps);
        s.handle(Event::AlignReset, &steps);
        assert_eq!(s.align, Align::default());
        // elsewhere stick up/down is the square direction again and the values survive a pattern change
        s.handle(Event::AlignVerg(3), &steps);
        s.handle(Event::NextPattern, &steps);
        s.handle(Event::StickVert(-1), &steps);
        assert_eq!(s.dir, Dir::Left);
        assert_eq!(s.align.verg, 3);
        // limits
        s.handle(Event::AlignVerg(100_000), &steps);
        assert_eq!(s.align.verg, crate::patterns::align::MAX_UNITS);
    }

    #[test]
    fn tune_and_recenter_events() {
        let mut s = st();
        let k0 = s.key();
        s.handle(Event::Tune(2, 0), &steps);
        s.handle(Event::Tune(0, -3), &steps);
        assert_eq!((s.tune.dk, s.tune.sc), (2, -3));
        s.handle(Event::Tune2(4), &steps);
        assert_eq!(s.tune.dk2, 4);
        assert_ne!(s.key(), k0);
        s.handle(Event::Recenter, &steps);
        assert!(s.recenter);
        s.handle(Event::AlignReset, &steps);
        assert!(s.tune.is_zero());
        assert_eq!(s.key(), k0);
    }

    #[test]
    fn status_line() {
        let mut s = st();
        assert_eq!(
            s.status_line("", 1),
            "[1/3] STRAIGHT-LINE GRID | eye both | auto off"
        );
        s.handle(Event::Goto(1), &steps);
        s.handle(Event::NextStep, &steps);
        s.auto_on = true;
        assert_eq!(
            s.status_line("RED ONLY", 4),
            "[2/3] PER-CHANNEL GRIDS step 2/4 RED ONLY | eye both | auto 6s"
        );
    }

    #[test]
    #[should_panic(expected = "at least one pattern")]
    fn empty_cycle_is_rejected() {
        let _ = State::new(vec![]);
    }
}
