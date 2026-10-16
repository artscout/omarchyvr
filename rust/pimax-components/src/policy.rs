//! Careful restarts (plan section 4.1): per-class budgets, backoff, a circuit breaker with new-evidence reset,
//! flap detection, the global budget and exit classification. Pure: callers pass the time (seconds).
use crate::model::Class;

/// `N/<window>` with the window in s, m or h (`5/10m`, `8/600s`, `1/30m`).
pub fn parse_budget(s: &str) -> Option<(u32, f64)> {
    let (n, w) = s.trim().trim_matches('"').split_once('/')?;
    let n: u32 = n.trim().parse().ok()?;
    let w = w.trim();
    let (num, mul) = match w.chars().last()? {
        's' => (&w[..w.len() - 1], 1.0),
        'm' => (&w[..w.len() - 1], 60.0),
        'h' => (&w[..w.len() - 1], 3600.0),
        c if c.is_ascii_digit() => (w, 1.0),
        _ => return None,
    };
    let v: f64 = num.trim().parse().ok()?;
    (v > 0.0).then_some((n, v * mul))
}

/// `5/10m` for display.
pub fn budget_text(b: (u32, f64)) -> String {
    let (n, w) = b;
    if w >= 60.0 && (w % 60.0) == 0.0 {
        format!("{n}/{}m", (w / 60.0) as u64)
    } else {
        format!("{n}/{}s", w as u64)
    }
}

/// The budgets (user decision 2026-10-15: as proposed).
#[derive(Debug, Clone, PartialEq)]
pub struct Budgets {
    pub leaf: (u32, f64),
    pub xr_client: (u32, f64),
    pub core: (u32, f64),
    pub device: (u32, f64),
    pub global: (u32, f64),
}

impl Default for Budgets {
    fn default() -> Self {
        Budgets { leaf: (5, 600.0), xr_client: (3, 600.0), core: (2, 900.0), device: (1, 1800.0), global: (8, 600.0) }
    }
}

/// The rules of one class.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassPolicy {
    pub class: Class,
    pub max: u32,
    pub window_s: f64,
    /// Delay before the 1st, 2nd, ... restart within the window (the last value repeats).
    pub backoff_s: &'static [f64],
    /// Leaf breakers half-open after this long (time alone resets only leaf breakers).
    pub half_open_s: Option<f64>,
}

impl Budgets {
    pub fn policy(&self, c: Class) -> ClassPolicy {
        let (b, backoff_s, half): ((u32, f64), &'static [f64], Option<f64>) = match c {
            Class::Leaf => (self.leaf, &[2.0, 4.0, 8.0, 16.0, 60.0], Some(1800.0)),
            Class::XrClient => (self.xr_client, &[5.0, 15.0, 45.0], None),
            Class::Core => (self.core, &[30.0, 120.0], None),
            Class::Device => (self.device, &[0.0], None),
            Class::None => ((0, 600.0), &[0.0], None),
        };
        ClassPolicy { class: c, max: b.0, window_s: b.1, backoff_s, half_open_s: half }
    }
}

/// Circuit breaker.
#[derive(Debug, Clone, PartialEq)]
pub enum Breaker {
    Closed,
    Open { since: f64, why: String },
    /// One trial restart allowed; a success closes it, a failure opens it again.
    HalfOpen,
}

impl Breaker {
    pub fn name(&self) -> &'static str {
        match self {
            Breaker::Closed => "closed",
            Breaker::Open { .. } => "open",
            Breaker::HalfOpen => "half-open",
        }
    }
}

/// What the policy says about restarting now.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Now,
    /// Backoff: not before this time.
    At(f64),
    /// Breaker open (or the class never restarts).
    No(String),
}

/// Flapping: this many ok->down cycles within `FLAP_WINDOW_S` open the breaker.
pub const FLAP_CYCLES: usize = 3;
pub const FLAP_WINDOW_S: f64 = 300.0;
/// An up period shorter than this followed by a crash is a flap.
pub const FLAP_UP_S: f64 = 60.0;

/// Per-component restart bookkeeping.
#[derive(Debug, Clone, PartialEq)]
pub struct RestartState {
    /// Times of restarts that spent budget.
    pub spent: Vec<f64>,
    /// Times the component went from up to down (any reason but expected/user).
    pub downs: Vec<f64>,
    pub breaker: Breaker,
}

impl Default for RestartState {
    fn default() -> Self {
        RestartState { spent: Vec::new(), downs: Vec::new(), breaker: Breaker::Closed }
    }
}

impl RestartState {
    /// Restarts spent within the window ending at `now`.
    pub fn used(&self, p: &ClassPolicy, now: f64) -> u32 {
        self.spent.iter().filter(|&&t| now - t < p.window_s).count() as u32
    }

    /// Time passes: forget old entries; a leaf breaker half-opens after its timeout.
    pub fn tick(&mut self, p: &ClassPolicy, now: f64) {
        let keep = p.window_s.max(FLAP_WINDOW_S);
        self.spent.retain(|&t| now - t < keep);
        self.downs.retain(|&t| now - t < FLAP_WINDOW_S);
        if let (Breaker::Open { since, .. }, Some(h)) = (&self.breaker, p.half_open_s) {
            if now - since >= h {
                self.breaker = Breaker::HalfOpen;
            }
        }
    }

    /// May a crashed component restart now? `down_at`: when it went down.
    pub fn verdict(&self, p: &ClassPolicy, now: f64, down_at: f64) -> Verdict {
        if p.class == Class::None || p.max == 0 {
            return Verdict::No("not restarted automatically".into());
        }
        if let Breaker::Open { why, .. } = &self.breaker {
            return Verdict::No(why.clone());
        }
        let used = self.used(p, now);
        if used >= p.max && self.breaker != Breaker::HalfOpen {
            return Verdict::No(format!("budget {} used up", budget_text((p.max, p.window_s))));
        }
        let i = (used as usize).min(p.backoff_s.len().saturating_sub(1));
        let at = down_at + p.backoff_s.get(i).copied().unwrap_or(0.0);
        if now + 1e-9 >= at {
            Verdict::Now
        } else {
            Verdict::At(at)
        }
    }

    /// A budget-spending restart is being made. Opens the breaker when that used up the budget, so the next crash
    /// is not restarted.
    pub fn spend(&mut self, now: f64) {
        self.spent.push(now);
        if self.breaker == Breaker::HalfOpen {
            self.breaker = Breaker::Closed;
        }
    }

    /// The component crashed (or went down unexpectedly). Returns true when this opened the breaker (budget used up
    /// or flapping).
    pub fn crashed(&mut self, p: &ClassPolicy, now: f64) -> bool {
        self.crashed_after(p, now, 0.0)
    }

    /// As [`Self::crashed`], knowing how long it had been up: only short runs (< [`FLAP_UP_S`]) count as flapping,
    /// so a component that crashes now and then is governed by its budget, one that cannot stay up by the flap rule.
    pub fn crashed_after(&mut self, p: &ClassPolicy, now: f64, up_for: f64) -> bool {
        if up_for < FLAP_UP_S {
            self.downs.push(now);
        }
        self.downs.retain(|&t| now - t < FLAP_WINDOW_S);
        if matches!(self.breaker, Breaker::Open { .. }) {
            return false;
        }
        if self.breaker == Breaker::HalfOpen {
            self.breaker = Breaker::Open { since: now, why: "failed again after the half-open trial".into() };
            return true;
        }
        if p.class != Class::None && p.max > 0 && self.used(p, now) >= p.max {
            self.breaker = Breaker::Open {
                since: now,
                why: format!("restart budget {} used up", budget_text((p.max, p.window_s))),
            };
            return true;
        }
        if self.downs.len() >= FLAP_CYCLES && p.class != Class::None {
            self.breaker = Breaker::Open { since: now, why: format!("flapping: down {} times in 5 min", self.downs.len()) };
            return true;
        }
        false
    }

    /// New evidence (a replug, a user Fix or toggle, a user restart): close the breaker and forget the history.
    pub fn reset(&mut self) {
        *self = RestartState::default();
    }
}

/// The global budget over all automatic actions.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Global {
    pub times: Vec<f64>,
}

impl Global {
    pub fn used(&self, b: (u32, f64), now: f64) -> u32 {
        self.times.iter().filter(|&&t| now - t < b.1).count() as u32
    }
    pub fn allows(&self, b: (u32, f64), now: f64) -> bool {
        self.used(b, now) < b.0
    }
    /// Record an action; true when this one used the budget up (enter safe mode).
    pub fn spend(&mut self, b: (u32, f64), now: f64) -> bool {
        self.times.retain(|&t| now - t < b.1);
        self.times.push(now);
        self.times.len() as u32 >= b.0
    }
}

/// Why a process exited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitClass {
    /// pimaxd asked for it.
    Expected,
    /// A hard dependency went down within [`COLLATERAL_S`].
    Collateral,
    /// Exit 0, or SIGINT/SIGTERM/SIGHUP that pimaxd did not send, or a launcher stop.
    User,
    /// A signal such as SEGV or ABRT, or a non-zero exit. Only crashes spend budget.
    Crash,
    /// An adopted process with no exit status and no evidence: not restarted, the user is told.
    Unknown,
}

impl ExitClass {
    pub fn name(self) -> &'static str {
        match self {
            ExitClass::Expected => "expected",
            ExitClass::Collateral => "collateral",
            ExitClass::User => "user",
            ExitClass::Crash => "crash",
            ExitClass::Unknown => "unknown",
        }
    }
}

pub const COLLATERAL_S: f64 = 10.0;

/// The facts about one exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExitFacts {
    /// Exit code (children of pimaxd only).
    pub code: Option<i32>,
    /// Terminating signal (children of pimaxd only).
    pub signal: Option<i32>,
    /// pimaxd sent the stop.
    pub ours: bool,
    /// A hard dependency went down shortly before.
    pub dep_down: bool,
    /// The launcher's user-stop marker / stop.lock, or a client stop.
    pub user_stop: bool,
    /// Kernel log segfault/trap line or a crash pattern in the process log (adopted processes).
    pub crash_evidence: bool,
}

pub fn classify_exit(f: &ExitFacts) -> ExitClass {
    if f.ours {
        return ExitClass::Expected;
    }
    if f.user_stop {
        return ExitClass::User;
    }
    if f.dep_down {
        return ExitClass::Collateral;
    }
    match (f.code, f.signal) {
        (_, Some(s)) if s == libc::SIGINT || s == libc::SIGTERM || s == libc::SIGHUP => ExitClass::User,
        (_, Some(_)) => ExitClass::Crash,
        (Some(0), None) => {
            if f.crash_evidence {
                ExitClass::Crash
            } else {
                ExitClass::User
            }
        }
        (Some(_), None) => ExitClass::Crash,
        (None, None) => {
            if f.crash_evidence {
                ExitClass::Crash
            } else {
                ExitClass::Unknown
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn budgets_parse() {
        assert_eq!(parse_budget("5/10m"), Some((5, 600.0)));
        assert_eq!(parse_budget("\"1/30m\""), Some((1, 1800.0)));
        assert_eq!(parse_budget("8/600"), Some((8, 600.0)));
        assert_eq!(parse_budget("2/1h"), Some((2, 3600.0)));
        assert_eq!(parse_budget("x/10m"), None);
        assert_eq!(parse_budget("3/0m"), None);
        assert_eq!(budget_text((2, 900.0)), "2/15m");
        assert_eq!(budget_text((2, 90.0)), "2/90s");
    }

    #[test]
    fn leaf_backoff_budget_and_breaker() {
        let p = Budgets::default().policy(Class::Leaf);
        let mut s = RestartState::default();
        let mut t = 0.0;
        // crashes 100 s apart (5 fit in the 10 min window, fewer than 3 in 5 min: no flapping); the backoff grows
        // with the restarts already spent in the window
        for b in [2.0, 4.0, 8.0, 16.0, 60.0] {
            assert!(!s.crashed(&p, t));
            assert_eq!(s.verdict(&p, t, t), Verdict::At(t + b));
            assert_eq!(s.verdict(&p, t + b, t), Verdict::Now);
            s.spend(t + b);
            t += 100.0 + b;
            s.downs.clear();
        }
        assert!(s.crashed(&p, t), "the 6th crash within 10 min opens the breaker");
        assert!(matches!(s.verdict(&p, t + 999.0, t), Verdict::No(_)));
    }

    #[test]
    fn budget_opens_the_breaker_and_new_evidence_resets_it() {
        let p = Budgets::default().policy(Class::Core);
        let mut s = RestartState::default();
        assert!(!s.crashed(&p, 0.0));
        assert_eq!(s.verdict(&p, 0.0, 0.0), Verdict::At(30.0));
        s.spend(30.0);
        assert!(!s.crashed(&p, 400.0));
        assert_eq!(s.verdict(&p, 400.0, 400.0), Verdict::At(520.0));
        s.spend(520.0);
        // third crash within 15 min: the budget (2) is used up
        assert!(s.crashed(&p, 800.0));
        assert!(matches!(s.verdict(&p, 9999.0, 800.0), Verdict::No(_)));
        // core breakers do not half-open with time
        s.tick(&p, 99999.0);
        assert!(matches!(s.breaker, Breaker::Open { .. }));
        s.reset();
        assert_eq!(s.verdict(&p, 0.0, 0.0), Verdict::At(30.0));
    }

    #[test]
    fn leaf_breaker_half_opens_after_30_min() {
        let p = Budgets::default().policy(Class::Leaf);
        let mut s = RestartState { breaker: Breaker::Open { since: 0.0, why: "x".into() }, ..Default::default() };
        s.tick(&p, 1799.0);
        assert!(matches!(s.breaker, Breaker::Open { .. }));
        s.tick(&p, 1800.0);
        assert_eq!(s.breaker, Breaker::HalfOpen);
        assert_eq!(s.verdict(&p, 1800.0, 1798.0), Verdict::Now);
        assert!(s.crashed(&p, 1900.0), "a failed trial opens it again");
    }

    #[test]
    fn flapping_counts_as_failing() {
        let p = Budgets::default().policy(Class::XrClient);
        let mut s = RestartState::default();
        assert!(!s.crashed(&p, 0.0));
        s.spend(5.0);
        assert!(!s.crashed(&p, 60.0));
        s.spend(75.0);
        assert!(s.crashed(&p, 120.0), "3 downs in 5 min");
        let mut s = RestartState::default();
        for t in [0.0, 100.0, 200.0] {
            assert!(!s.crashed_after(&p, t, 90.0), "long runs are not flapping");
            s.downs.retain(|_| true);
        }
    }

    #[test]
    fn global_budget() {
        let mut g = Global::default();
        let b = (8, 600.0);
        for i in 0..7 {
            assert!(!g.spend(b, i as f64));
        }
        assert!(g.allows(b, 8.0));
        assert!(g.spend(b, 8.0));
        assert!(!g.allows(b, 9.0));
        assert!(g.allows(b, 700.0));
    }

    #[test]
    fn exits_are_classified() {
        let c = |f: ExitFacts| classify_exit(&f);
        assert_eq!(c(ExitFacts { ours: true, signal: Some(libc::SIGSEGV), ..Default::default() }), ExitClass::Expected);
        assert_eq!(c(ExitFacts { dep_down: true, code: Some(1), ..Default::default() }), ExitClass::Collateral);
        assert_eq!(c(ExitFacts { code: Some(0), ..Default::default() }), ExitClass::User);
        assert_eq!(c(ExitFacts { signal: Some(libc::SIGTERM), ..Default::default() }), ExitClass::User);
        assert_eq!(c(ExitFacts { signal: Some(libc::SIGABRT), ..Default::default() }), ExitClass::Crash);
        assert_eq!(c(ExitFacts { code: Some(101), ..Default::default() }), ExitClass::Crash);
        assert_eq!(c(ExitFacts::default()), ExitClass::Unknown);
        assert_eq!(c(ExitFacts { crash_evidence: true, ..Default::default() }), ExitClass::Crash);
        assert_eq!(c(ExitFacts { user_stop: true, ..Default::default() }), ExitClass::User);
    }

    proptest! {
        /// Whatever the crash times, a class never spends more restarts in any window than its budget.
        #[test]
        fn never_more_restarts_than_the_budget(gaps in proptest::collection::vec(0.0f64..400.0, 1..40), cls in 0usize..3) {
            let class = [Class::Leaf, Class::XrClient, Class::Core][cls];
            let p = Budgets::default().policy(class);
            let mut s = RestartState::default();
            let mut t = 0.0;
            let mut restarts: Vec<f64> = Vec::new();
            for g in gaps {
                t += g;
                s.tick(&p, t);
                s.crashed(&p, t);
                // restart as early as the policy allows
                let at = match s.verdict(&p, t, t) { Verdict::Now => Some(t), Verdict::At(a) => Some(a), Verdict::No(_) => None };
                if let Some(a) = at {
                    s.spend(a);
                    restarts.push(a);
                    t = t.max(a);
                }
            }
            for (i, &a) in restarts.iter().enumerate() {
                let n = restarts[..=i].iter().filter(|&&b| a - b < p.window_s).count() as u32;
                // a half-open trial may add one after the breaker's timeout (leaf only, 30 min > window)
                prop_assert!(n <= p.max, "{n} restarts within the window at {a}");
            }
        }
    }
}
