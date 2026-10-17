//! The component model: ids, kinds, health, toggles, ladders, grace periods, desired state and intent.
//! Pure data, no I/O.
use std::collections::BTreeMap;

/// A component id (`usb.hid`, `monado`, `voice.listen`, ...).
pub type Id = &'static str;

/// What a component is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Device,
    Gate,
    Setting,
    Process,
    Feature,
    Job,
    Policy,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Device => "device",
            Kind::Gate => "gate",
            Kind::Setting => "setting",
            Kind::Process => "process",
            Kind::Feature => "feature",
            Kind::Job => "job",
            Kind::Policy => "policy",
        }
    }
}

/// How a change of the toggle applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    /// Not a user toggle (devices, gates, host settings).
    None,
    /// Applies at once.
    Live,
    /// Needs a restart of the named component (never applied automatically).
    Restart(Id),
}

/// Restart budget class (plan section 4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    /// Not restarted by omarchyvrd.
    None,
    /// voice.*, perf, ctrltrack, floor.follow, ai.server: 5 per 10 min.
    Leaf,
    /// wayvr, passthrough, voicehud: 3 per 10 min.
    XrClient,
    /// The Monado subtree and the broker: 2 per 15 min.
    Core,
    /// reset-headset: 1 per 30 min.
    Device,
}

impl Class {
    pub fn name(self) -> &'static str {
        match self {
            Class::None => "none",
            Class::Leaf => "leaf",
            Class::XrClient => "xr_client",
            Class::Core => "core",
            Class::Device => "device",
        }
    }
}

/// Remediation ladder steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Wait or observe.
    L0,
    /// Soft: re-apply a setting, reconnect.
    L1,
    /// Restart this process alone.
    L2,
    /// Restart the subtree in dependency order.
    L3,
    /// Device recovery with approved bytes only (reset-headset).
    L4,
    /// Tell the user, with a Fix button.
    L5,
}

/// One rung of a ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rung {
    pub level: Level,
    pub what: &'static str,
}

/// Grace periods in seconds (0 = none).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Grace {
    /// Until the component must be ready after a start.
    pub start_s: f64,
    /// Settle time after a start before the next action.
    pub settle_s: f64,
    /// Present this long before it counts as up (devices).
    pub present_s: f64,
    /// Absent this long before it counts as down (devices).
    pub absent_s: f64,
}

/// A dependency. `when` limits it to some configurations (the broker for Monado only with 6DoF).
#[derive(Clone, Copy)]
pub struct Dep {
    pub id: Id,
    pub hard: bool,
    pub when: Option<fn(&Desired) -> bool>,
}

impl std::fmt::Debug for Dep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}{}", self.id, if self.hard { "" } else { "(soft)" }, if self.when.is_some() { "?" } else { "" })
    }
}

impl Dep {
    pub const fn hard(id: Id) -> Dep {
        Dep { id, hard: true, when: None }
    }
    pub const fn soft(id: Id) -> Dep {
        Dep { id, hard: false, when: None }
    }
    pub const fn hard_if(id: Id, when: fn(&Desired) -> bool) -> Dep {
        Dep { id, hard: true, when: Some(when) }
    }
    pub fn active(&self, d: &Desired) -> bool {
        match self.when {
            None => true,
            Some(f) => f(d),
        }
    }
}

/// Health of one component.
#[derive(Debug, Clone, PartialEq)]
pub enum Health {
    /// Not wanted and not running.
    Off,
    /// Started; must be ready by `until` (seconds on the daemon clock).
    Starting { until: f64 },
    Ok,
    Degraded(String),
    /// A hard dependency is not up: waits for it and spends no budget.
    Blocked(String),
    Down(String),
    /// The breaker is open: needs the user (Fix).
    Failed(String),
    Unknown,
}

impl Health {
    pub fn name(&self) -> &'static str {
        match self {
            Health::Off => "off",
            Health::Starting { .. } => "starting",
            Health::Ok => "ok",
            Health::Degraded(_) => "degraded",
            Health::Blocked(_) => "blocked",
            Health::Down(_) => "down",
            Health::Failed(_) => "failed",
            Health::Unknown => "unknown",
        }
    }
    pub fn why(&self) -> Option<&str> {
        match self {
            Health::Degraded(w) | Health::Blocked(w) | Health::Down(w) | Health::Failed(w) => Some(w),
            _ => None,
        }
    }
    /// Good enough for dependents (ok or degraded).
    pub fn usable(&self) -> bool {
        matches!(self, Health::Ok | Health::Degraded(_))
    }
}

/// The one-word summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Overall {
    Ok,
    Degraded,
    Down,
    NeedsYou,
    NeedsReboot,
}

impl Overall {
    pub fn name(self) -> &'static str {
        match self {
            Overall::Ok => "ok",
            Overall::Degraded => "degraded",
            Overall::Down => "down",
            Overall::NeedsYou => "needs-you",
            Overall::NeedsReboot => "needs-reboot",
        }
    }
}

/// GPU safety gate level (plan section 4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum GateLevel {
    #[default]
    Ok,
    /// A WayVR Xid 31: warn only (user decision 2026-10-15), no automatic start of GPU clients.
    Caution,
    /// Any other GPU fault this boot: no GPU-client start without `--force`.
    NeedsReboot,
}

impl GateLevel {
    pub fn name(self) -> &'static str {
        match self {
            GateLevel::Ok => "ok",
            GateLevel::Caution => "caution",
            GateLevel::NeedsReboot => "needs-reboot",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "ok" => Some(GateLevel::Ok),
            "caution" => Some(GateLevel::Caution),
            "needs-reboot" => Some(GateLevel::NeedsReboot),
            _ => None,
        }
    }
}

/// Session mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Desk,
    /// `omarchyvr vr start`: Monado plus a game. Never stopped automatically.
    Vr,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Desk => "desk",
            Mode::Vr => "vr",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "desk" => Some(Mode::Desk),
            "vr" | "game" => Some(Mode::Vr),
            _ => None,
        }
    }
}

/// Eye tracking mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Eye {
    Off,
    #[default]
    Silent,
    Pointer,
}

impl Eye {
    pub fn name(self) -> &'static str {
        match self {
            Eye::Off => "off",
            Eye::Silent => "silent",
            Eye::Pointer => "pointer",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "off" | "false" => Some(Eye::Off),
            "silent" | "on" | "true" => Some(Eye::Silent),
            "pointer" => Some(Eye::Pointer),
            _ => None,
        }
    }
}

/// What "everything on" means (`[session]` + `[components]` of omarchyvrd.toml).
#[derive(Debug, Clone, PartialEq)]
pub struct Desired {
    pub mode: Mode,
    pub passthrough: bool,
    pub sixdof: bool,
    pub ctrltrack: bool,
    pub eye: Eye,
    pub eye_laser: bool,
    pub perf: bool,
    pub voice: bool,
    pub voicehud: bool,
    pub ai_server: bool,
    pub anchor: bool,
    pub floor_follow: bool,
    /// Camera hands (Mercury, `--hands`); needs 6DoF and the hands Monado build. Off by default.
    pub hands: bool,
    pub preset: String,
    /// VR screens (Hyprland outputs, `--screens N`, 1 to 6, default 6; desk only). Above 1 the screens watcher runs.
    pub screens: u8,
}

impl Default for Desired {
    fn default() -> Self {
        Desired {
            mode: Mode::Desk,
            passthrough: true,
            sixdof: true,
            ctrltrack: true,
            eye: Eye::Silent,
            eye_laser: true,
            perf: true,
            voice: false,
            voicehud: false,
            ai_server: false,
            anchor: true,
            floor_follow: false,
            hands: false,
            preset: "cinema".into(),
            screens: 6,
        }
    }
}

/// Who set the intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum By {
    #[default]
    None,
    User,
    Auto,
    Cli,
    /// A running session omarchyvrd found and adopted.
    Adopted,
}

impl By {
    pub fn name(self) -> &'static str {
        match self {
            By::None => "none",
            By::User => "user",
            By::Auto => "auto",
            By::Cli => "cli",
            By::Adopted => "adopted",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "none" => By::None,
            "user" | "gui" | "voice" => By::User,
            "auto" => By::Auto,
            "cli" => By::Cli,
            "adopted" | "launcher" => By::Adopted,
            _ => return None,
        })
    }
}

/// Runtime intent (`$XDG_RUNTIME_DIR/omarchyvrd/intent`): should a session run, who said so, per-session overrides.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Intent {
    /// None: nobody said anything this boot.
    pub session: Option<bool>,
    pub by: By,
    pub mode: Mode,
    /// Per-component on/off for this session only (a user toggle, a process the user closed).
    pub overrides: BTreeMap<String, bool>,
    /// When it was set (unix seconds).
    pub at: f64,
}

impl Intent {
    pub fn on(&self) -> bool {
        self.session == Some(true)
    }
    pub fn session_name(&self) -> &'static str {
        match self.session {
            Some(true) => "on",
            Some(false) => "off",
            None => "unset",
        }
    }
}
