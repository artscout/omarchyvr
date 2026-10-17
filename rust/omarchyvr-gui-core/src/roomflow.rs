//! "Room setup", the one guided flow behind the third big button (docs/ROOM-SETUP.md):
//! (a) the headset calibration is there, or it is read from the headset (asleep headset: the wake guide),
//! (b) the room setup (Fast by default, Full as an option, guided in the headset as before),
//! (c) a short summary with ticks.
//!
//! Pure state machine: what the PC sees comes in through [`FlowEnv`], what the app must do comes out as an [`Effect`].
//! No new HID commands: the read is `omarchyvr calib --save-config` (calibration.rs) and the room setup is the
//! existing Fast/Full job (roomsetup.rs).
use crate::headset_choice::HeadsetUsb;

/// What the flow sees.
pub trait FlowEnv {
    /// The factory calibration file is there.
    fn calibration_present(&self) -> bool;
    /// The headset's HID chip state from USB.
    fn chip(&self) -> HeadsetUsb;
}

/// What the app must do after a step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Nothing.
    None,
    /// Start `omarchyvr calib --save-config`.
    ReadCalibration,
    /// Start the room setup job.
    StartRoomSetup {
        /// Full instead of Fast.
        full: bool,
    },
}

/// Where the flow is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage {
    /// Not opened.
    Closed,
    /// The calibration is missing: "Read it from the headset".
    NeedCalibration,
    /// The headset sleeps: the wake guide runs, the read starts by itself when it is awake.
    Asleep,
    /// The read runs.
    Reading,
    /// The read failed (plain words).
    ReadFailed(String),
    /// The calibration is there: pick Fast or Full.
    Choose,
    /// The room setup runs in the headset.
    Mapping {
        /// Full setup.
        full: bool,
    },
    /// Done (or the room setup failed).
    Summary,
}

/// The flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomFlow {
    /// The stage.
    pub stage: Stage,
    /// Full setup chosen (Fast is the default).
    pub full: bool,
    /// The calibration was read by this flow (not already there).
    pub read_now: bool,
    /// The room setup finished: `None` = not run, `Some(ok)`.
    pub room_ok: Option<bool>,
}

impl Default for RoomFlow {
    fn default() -> Self {
        RoomFlow { stage: Stage::Closed, full: false, read_now: false, room_ok: None }
    }
}

impl RoomFlow {
    /// The page opens (the third button): start at the first step that is not done.
    pub fn open(&mut self, env: &dyn FlowEnv) {
        if matches!(self.stage, Stage::Reading | Stage::Mapping { .. }) {
            return; // a step runs: keep showing it
        }
        self.read_now = false;
        self.room_ok = None;
        self.stage = if env.calibration_present() { Stage::Choose } else { Stage::NeedCalibration };
    }

    /// "Read the calibration from the headset".
    pub fn read_pressed(&mut self, env: &dyn FlowEnv) -> Effect {
        if !matches!(self.stage, Stage::NeedCalibration | Stage::ReadFailed(_) | Stage::Asleep) {
            return Effect::None;
        }
        if env.calibration_present() {
            self.stage = Stage::Choose;
            return Effect::None;
        }
        match env.chip() {
            HeadsetUsb::Awake => {
                self.stage = Stage::Reading;
                Effect::ReadCalibration
            }
            HeadsetUsb::Asleep => {
                self.stage = Stage::Asleep;
                Effect::None
            }
            HeadsetUsb::Absent => {
                self.stage = Stage::ReadFailed("The headset is not connected. Plug in its USB cable and its power cable, then try again.".into());
                Effect::None
            }
        }
    }

    /// Called often: an asleep headset that wakes starts the read by itself.
    pub fn tick(&mut self, env: &dyn FlowEnv) -> Effect {
        if self.stage == Stage::Asleep && env.chip() == HeadsetUsb::Awake {
            self.stage = Stage::Reading;
            return Effect::ReadCalibration;
        }
        Effect::None
    }

    /// The read job ended.
    pub fn read_done(&mut self, ok: bool, env: &dyn FlowEnv) {
        if self.stage != Stage::Reading {
            return;
        }
        if ok && env.calibration_present() {
            self.read_now = true;
            self.stage = Stage::Choose;
        } else {
            self.stage = Stage::ReadFailed("The headset did not give its calibration. Make sure it is awake (press its power button once) and nothing else is using it, then try again.".into());
        }
    }

    /// "Fast setup" / "Full setup".
    pub fn choose(&mut self, full: bool, env: &dyn FlowEnv) -> Effect {
        if self.stage != Stage::Choose && self.stage != Stage::Summary {
            return Effect::None;
        }
        if !env.calibration_present() {
            self.stage = Stage::NeedCalibration;
            return Effect::None;
        }
        self.full = full;
        self.room_ok = None;
        self.stage = Stage::Mapping { full };
        Effect::StartRoomSetup { full }
    }

    /// The room setup job ended.
    pub fn room_done(&mut self, ok: bool) {
        if matches!(self.stage, Stage::Mapping { .. }) {
            self.room_ok = Some(ok);
            self.stage = Stage::Summary;
        }
    }

    /// Back to the first step (after a failure or "Do it again").
    pub fn restart(&mut self, env: &dyn FlowEnv) {
        self.stage = Stage::Closed;
        self.open(env);
    }

    /// Should the wake guide be fed now?
    pub fn wants_wake(&self) -> bool {
        self.stage == Stage::Asleep
    }

    /// The ticks of the summary: (text, done).
    pub fn ticks(&self, env: &dyn FlowEnv) -> Vec<(String, bool)> {
        let cal = env.calibration_present();
        vec![
            (if self.read_now { "Headset calibration read from the headset".to_string() } else { "Headset calibration in place".to_string() }, cal),
            (format!("Room mapped ({} setup)", if self.full { "Full" } else { "Fast" }), self.room_ok == Some(true)),
            ("Floor, play area, centre and forward saved".to_string(), self.room_ok == Some(true)),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct Fake {
        calib: Cell<bool>,
        chip: Cell<HeadsetUsb>,
    }
    impl FlowEnv for Fake {
        fn calibration_present(&self) -> bool {
            self.calib.get()
        }
        fn chip(&self) -> HeadsetUsb {
            self.chip.get()
        }
    }
    fn fake(calib: bool, chip: HeadsetUsb) -> Fake {
        Fake { calib: Cell::new(calib), chip: Cell::new(chip) }
    }

    #[test]
    fn calibration_present_goes_straight_to_the_room_setup() {
        let e = fake(true, HeadsetUsb::Awake);
        let mut f = RoomFlow::default();
        f.open(&e);
        assert_eq!(f.stage, Stage::Choose);
        assert_eq!(f.choose(false, &e), Effect::StartRoomSetup { full: false });
        assert_eq!(f.stage, Stage::Mapping { full: false });
        f.room_done(true);
        assert_eq!(f.stage, Stage::Summary);
        assert!(f.ticks(&e).iter().all(|(_, ok)| *ok));
    }

    #[test]
    fn missing_then_read_ok() {
        let e = fake(false, HeadsetUsb::Awake);
        let mut f = RoomFlow::default();
        f.open(&e);
        assert_eq!(f.stage, Stage::NeedCalibration);
        assert_eq!(f.choose(false, &e), Effect::None, "no room setup without the calibration");
        assert_eq!(f.read_pressed(&e), Effect::ReadCalibration);
        assert_eq!(f.stage, Stage::Reading);
        e.calib.set(true);
        f.read_done(true, &e);
        assert_eq!(f.stage, Stage::Choose);
        assert!(f.read_now);
        assert!(f.ticks(&e)[0].0.contains("read from the headset"));
    }

    #[test]
    fn missing_headset_asleep_shows_the_wake_guide_then_reads_by_itself() {
        let e = fake(false, HeadsetUsb::Asleep);
        let mut f = RoomFlow::default();
        f.open(&e);
        assert_eq!(f.read_pressed(&e), Effect::None);
        assert_eq!(f.stage, Stage::Asleep);
        assert!(f.wants_wake());
        assert_eq!(f.tick(&e), Effect::None);
        e.chip.set(HeadsetUsb::Awake);
        assert_eq!(f.tick(&e), Effect::ReadCalibration);
        assert_eq!(f.stage, Stage::Reading);
        assert!(!f.wants_wake());
    }

    #[test]
    fn read_fails_in_plain_words_and_can_be_retried() {
        let e = fake(false, HeadsetUsb::Awake);
        let mut f = RoomFlow::default();
        f.open(&e);
        f.read_pressed(&e);
        f.read_done(false, &e);
        let Stage::ReadFailed(m) = &f.stage else { panic!("{:?}", f.stage) };
        assert!(m.contains("power button") && !m.contains("omarchyvr calib"));
        assert_eq!(f.read_pressed(&e), Effect::ReadCalibration);
    }

    #[test]
    fn headset_absent_is_a_plain_failure() {
        let e = fake(false, HeadsetUsb::Absent);
        let mut f = RoomFlow::default();
        f.open(&e);
        assert_eq!(f.read_pressed(&e), Effect::None);
        assert!(matches!(&f.stage, Stage::ReadFailed(m) if m.contains("not connected")));
    }

    #[test]
    fn a_failed_room_setup_shows_unticked_and_can_run_again() {
        let e = fake(true, HeadsetUsb::Awake);
        let mut f = RoomFlow::default();
        f.open(&e);
        f.choose(true, &e);
        f.room_done(false);
        assert_eq!(f.stage, Stage::Summary);
        let t = f.ticks(&e);
        assert!(t[0].1 && !t[1].1);
        assert_eq!(f.choose(true, &e), Effect::StartRoomSetup { full: true });
    }

    #[test]
    fn opening_again_keeps_a_running_step() {
        let e = fake(false, HeadsetUsb::Awake);
        let mut f = RoomFlow::default();
        f.open(&e);
        f.read_pressed(&e);
        f.open(&e);
        assert_eq!(f.stage, Stage::Reading);
    }
}
