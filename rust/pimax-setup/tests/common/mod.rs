//! The test harness for the window: the real `PimaxApp` with demo data, its monitor and task threads running like
//! in the app, a machine whose every access is slowed down and recorded when it comes from the UI thread, and
//! egui_kittest driving input (clicks through AccessKit, typing, resizing) with every frame timed.
#![allow(dead_code)]
use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::Harness;
use eframe::egui::{self, accesskit::Role};
use pimax_config::Store;
use pimax_gui_core::app::AppCore;
use pimax_gui_core::machine::Machine;
use pimax_gui_core::probe::ProbeMachine;
use pimax_setup::app::PimaxApp;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Frame budget: what the window must never exceed. Debug builds (plain `cargo test`) are ~10x slower at layout,
/// so they get a looser number; the release run (`cargo test --release`) checks the real 50 ms.
pub fn budget() -> Duration {
    if cfg!(debug_assertions) {
        Duration::from_millis(400)
    } else {
        Duration::from_millis(50)
    }
}

/// One harness.
pub struct Ui {
    pub h: Harness<'static, PimaxApp>,
    pub probe: Arc<ProbeMachine>,
    pub times: Vec<Duration>,
    pub slowest: (Duration, String),
    pub dir: PathBuf,
    pub clicked: usize,
    /// read(2) calls made by the app during frames on the UI thread (the test's own measurements excluded).
    pub app_reads: u64,
}

impl Ui {
    /// The app on the demo machine (`running`: a desk session runs), every machine access delayed by `delay`.
    pub fn new(tag: &str, running: bool, delay: Duration) -> Ui {
        pimax_gui_core::cmd::set_dry_run(true);
        static HOOK: std::sync::Once = std::sync::Once::new();
        HOOK.call_once(|| pimax_gui_core::crash::install_panic_hook(panic_dir()));
        let dir = std::env::temp_dir().join(format!("pimax-ui-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("cfg")).unwrap();
        std::fs::write(dir.join("cfg/lens.conf"), "# headset serial SER1\ncant_deg = 2.0\n[left]\ncenter = 0.0271 0.009\n[right]\ncenter = -0.0271 0.009\n").unwrap();
        let store = Store::new(dir.join("cfg"), dir.join("cache"));
        let mut probe: Option<Arc<ProbeMachine>> = None;
        let mut core = AppCore::demo_with(
            running,
            store,
            |m: Arc<dyn Machine>| {
                let p = Arc::new(ProbeMachine::new(m, delay));
                probe = Some(p.clone());
                p
            },
            true,
        );
        core.paths.state_dir = dir.join("state");
        core.paths.gui_state = dir.join("cfg/gui-state.toml");
        // the walk through every page needs the advanced pages (Developer mode on); the simple flow has its own check
        core.state.dev_mode = true;
        let probe = probe.unwrap();
        core.start_threads();
        core.check_crashes();
        let app = PimaxApp::new(core);
        let h = Harness::builder()
            .with_size(egui::vec2(1280.0, 900.0))
            .build_ui_state(|ui, app: &mut PimaxApp| app.frame(ui), app);
        probe.watch_this_thread();
        Ui { h, probe, times: Vec::new(), slowest: (Duration::ZERO, String::new()), dir, clicked: 0, app_reads: 0 }
    }

    /// The app on THIS machine (read-only: /proc, /sys, journalctl -k, getcap, modinfo, `pimax desk help`), in dry
    /// run so nothing can be started or signalled; for measuring the real background cost (`PIMAX_SOAK_REAL=1`).
    pub fn new_real(tag: &str) -> Ui {
        pimax_gui_core::cmd::set_dry_run(true);
        let dir = std::env::temp_dir().join(format!("pimax-ui-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let env = |k: &str| std::env::var(k).ok();
        let mut paths = pimax_gui_core::paths::Paths::from_env(&env);
        paths.state_dir = dir.join("state");
        paths.gui_state = dir.join("gui-state.toml");
        let opts = pimax_gui_core::paths::doctor_opts(&paths, &env);
        let probe = Arc::new(ProbeMachine::new(Arc::new(pimax_gui_core::machine::RealMachine::from_env()), Duration::ZERO));
        let mut core = AppCore::new_background(
            probe.clone(),
            paths,
            opts,
            Arc::new(|k: &str| std::env::var(k).ok()),
            Store::new(dir.join("cfg"), dir.join("cache")),
        );
        core.start_threads();
        let app = PimaxApp::new(core);
        let h = Harness::builder()
            .with_size(egui::vec2(1280.0, 900.0))
            .build_ui_state(|ui, app: &mut PimaxApp| app.frame(ui), app);
        probe.watch_this_thread();
        Ui { h, probe, times: Vec::new(), slowest: (Duration::ZERO, String::new()), dir, clicked: 0, app_reads: 0 }
    }

    /// Run one frame (queued input included), timed.
    pub fn step(&mut self, what: &str) {
        // read(2) calls of this thread around the frame; the two measurements cost two read(2) calls each
        let r0 = thread_reads();
        let t = Instant::now();
        self.h.step();
        let e = t.elapsed();
        let d = thread_reads().saturating_sub(r0).saturating_sub(MEASURE_READS);
        if d > 0 {
            self.app_reads += d;
            eprintln!("frame '{what}' on {:?}: {d} read(2) calls on the UI thread", self.h.state().core.page);
        }
        self.times.push(e);
        if e > self.slowest.0 {
            self.slowest = (e, format!("{what} on {:?}", self.h.state().core.page));
        }
    }

    /// `n` frames, sleeping `gap` between them (background results arrive meanwhile).
    pub fn frames(&mut self, n: usize, gap: Duration, what: &str) {
        for _ in 0..n {
            self.step(what);
            if !gap.is_zero() {
                std::thread::sleep(gap);
            }
        }
    }

    /// Frames until `f` holds (or `timeout`).
    pub fn until(&mut self, timeout: Duration, what: &str, f: impl Fn(&PimaxApp) -> bool) -> bool {
        let t = Instant::now();
        while t.elapsed() < timeout {
            self.step(what);
            if f(self.h.state()) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// Wait for the first facts (the background check), with frames running meanwhile.
    pub fn wait_facts(&mut self) {
        assert!(self.until(Duration::from_secs(60), "waiting for facts", |a| a.core.facts.at != 0), "no facts after 60 s");
    }

    /// (role, label) of every enabled widget one can click or type into.
    pub fn widgets(&self) -> Vec<(Role, String)> {
        let mut v = Vec::new();
        for role in [Role::Button, Role::CheckBox, Role::RadioButton, Role::ComboBox, Role::Link, Role::Tab, Role::Slider, Role::TextInput] {
            for n in self.h.query_all_by_role(role) {
                let ak = n.accesskit_node();
                if ak.is_disabled() || ak.is_hidden() {
                    continue;
                }
                v.push((role, ak.label().unwrap_or_default()));
            }
        }
        v
    }

    /// Click the `k`-th enabled widget with this role and label (through AccessKit, so it works off-screen too).
    pub fn click(&mut self, role: Role, label: &str, k: usize) -> bool {
        let found = {
            let nodes: Vec<_> = self
                .h
                .query_all_by_role(role)
                .filter(|n| n.accesskit_node().label().unwrap_or_default() == label && !n.accesskit_node().is_disabled())
                .collect();
            match nodes.get(k) {
                Some(n) => {
                    if role == Role::TextInput {
                        n.focus();
                        n.type_text("eye");
                    } else {
                        n.click_accesskit();
                    }
                    true
                }
                None => false,
            }
        };
        if found {
            self.clicked += 1;
            self.step(&format!("click {role:?} {label:?}"));
            self.step("after the click");
        }
        found
    }

    /// Close whatever dialog is open (pkexec/SIGINT confirmation, report, crash), the way a user would.
    pub fn close_dialogs(&mut self) {
        for _ in 0..4 {
            let a = self.h.state();
            let (pending, report, recovery) = (a.core.pending.is_some(), a.ui.report_open, a.core.recovery.is_some());
            if !(pending || report || recovery) {
                return;
            }
            let label = if pending { "Cancel" } else { "Close" };
            if !self.click(Role::Button, label, 0) {
                self.h.key_press(egui::Key::Escape);
                self.step("escape");
            }
            self.frames(2, Duration::from_millis(10), "dialog closed");
        }
    }

    /// Click every widget of the current view once (labels in `skip` excepted), closing dialogs after each, then
    /// coming back to `page` if a click moved away. Returns the number of clicks.
    pub fn click_everything(&mut self, skip: &[&str]) -> usize {
        let page = self.h.state().core.page;
        let list = self.widgets();
        let mut seen: std::collections::HashMap<(Role, String), usize> = Default::default();
        let mut n = 0;
        for (role, label) in list {
            let k = seen.entry((role, label.clone())).or_insert(0);
            let idx = *k;
            *k += 1;
            if skip.iter().any(|s| label.starts_with(s)) {
                continue;
            }
            if self.click(role, &label, idx) {
                n += 1;
            }
            self.close_dialogs();
            if self.h.state().core.page != page {
                self.h.state_mut().core.page = page;
                self.step("back to the page");
            }
        }
        n
    }

    /// Resize the window and draw.
    pub fn resize(&mut self, w: f32, h: f32) {
        self.h.set_size(egui::vec2(w, h));
        self.step(&format!("resize {w}x{h}"));
    }

    /// Frame time statistics: (count, max, p99, mean).
    pub fn stats(&self) -> (usize, Duration, Duration, Duration) {
        let mut v = self.times.clone();
        v.sort();
        let n = v.len().max(1);
        let p99 = v.get((n * 99 / 100).min(n - 1)).copied().unwrap_or_default();
        let mean = v.iter().sum::<Duration>() / n as u32;
        (v.len(), v.last().copied().unwrap_or_default(), p99, mean)
    }

    /// The UI thread never touched the machine (nor read a file by itself), and no frame went over the budget.
    pub fn assert_responsive(&self) {
        let viol = self.probe.violations();
        assert!(viol.is_empty(), "the UI thread accessed the machine {} times, first: {:?}", viol.len(), &viol[..viol.len().min(10)]);
        eprintln!("read(2) calls of the app on the UI thread during {} frames: {}", self.times.len(), self.app_reads);
        assert_eq!(self.app_reads, 0, "the UI thread read files during frames");
        let (n, max, p99, mean) = self.stats();
        eprintln!("{n} frames: max {max:?} ({}), p99 {p99:?}, mean {mean:?}", self.slowest.1);
        assert!(max <= budget(), "a frame took {max:?} (> {:?}): {}", budget(), self.slowest.1);
    }
}

/// Resident memory and thread count of this process.
pub fn rss_threads() -> (u64, u64) {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let get = |k: &str| {
        s.lines()
            .find(|l| l.starts_with(k))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0)
    };
    (get("VmRSS:"), get("Threads:"))
}

/// Where the panic hook of the tests writes its reports.
pub fn panic_dir() -> PathBuf {
    std::env::temp_dir().join(format!("pimax-ui-panics-{}", std::process::id()))
}

/// Panics of the app (not the assertions of the test threads themselves): summary lines.
pub fn app_panics() -> Vec<String> {
    pimax_gui_core::crash::list(&panic_dir())
        .into_iter()
        .map(|(_, r, _)| r.summary)
        .filter(|s| !s.contains("thread 'every_page") && !s.contains("thread 'running_session") && !s.contains("thread 'soak"))
        .collect()
}

/// read(2) calls one measurement pair adds (two reads of /proc/thread-self/io).
pub const MEASURE_READS: u64 = 4;

/// read(2) calls of the calling thread so far (`syscr` of `/proc/thread-self/io`).
pub fn thread_reads() -> u64 {
    std::fs::read_to_string("/proc/thread-self/io")
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix("syscr: ").and_then(|v| v.trim().parse().ok()))
        .unwrap_or(0)
}

/// CPU time of this process so far (user + system), in seconds.
pub fn cpu_secs() -> f64 {
    let s = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let rest = s.rsplit_once(") ").map(|x| x.1).unwrap_or("");
    let f: Vec<&str> = rest.split_whitespace().collect();
    // fields after the name: state is 0, utime is 11, stime is 12
    let t = |i: usize| f.get(i).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    // SAFETY: sysconf(3) only reads a constant.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as f64;
    (t(11) + t(12)) / hz
}
