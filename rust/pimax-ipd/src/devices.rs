//! [`Devices`] implementations: the real eye chip and headset, and a simulation.

use crate::Devices;
use pimax_eye::ipd::{self, Busy, Measure, Measurement};
use pimax_eye::live::{Config, State, Tracker};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The real headset. Opens the eye chip directly (Monado must run without eye tracking).
pub struct Real {
    cfg: Option<Config>,
    tracker: Option<Tracker>,
    info: Option<PathBuf>,
    cmd: Option<PathBuf>,
    apply: bool,
    last_seq: Option<u64>,
}

impl Real {
    pub fn new(usb_device: Option<PathBuf>, apply: bool) -> Real {
        let mut cfg = Config::from_env();
        if usb_device.is_some() {
            cfg.usb_device = usb_device;
        }
        Real { cfg: Some(cfg), tracker: None, info: None, cmd: None, apply, last_seq: None }
    }
}

impl Devices for Real {
    fn prepare(&mut self) -> Result<Option<f32>, String> {
        let cfg = self.cfg.take().ok_or("already prepared")?;
        let eye_node = cfg
            .usb_device
            .clone()
            .or_else(|| pimax_eye::usbfs::find_eye_chips().first().map(|d| d.devnode.clone()));
        match ipd::busy_check(Path::new("/proc"), eye_node.as_deref()) {
            Busy::Refuse(m) => return Err(m),
            Busy::Warn(m) => eprintln!("warning: {m}"),
            Busy::Free => {}
        }
        let (hc, dev) = (Path::new("/sys/class/hidraw"), Path::new("/dev"));
        self.info = ipd::headset_node(hc, dev, "02");
        self.cmd = ipd::headset_node(hc, dev, "00");
        if self.apply && self.cmd.is_none() {
            return Err("no hidraw node for interface 0 of 34a4:0044: nothing sent".into());
        }
        let lens0 = self.info.as_ref().and_then(|n| ipd::read_lens_ipd(n, Duration::from_millis(500)));
        let tr = Tracker::start(cfg);
        let t0 = Instant::now();
        while tr.state() != State::Streaming {
            if t0.elapsed().as_secs() >= 15 || pimax_eye::sigint::stopped() {
                tr.stop();
                return Err("No gaze stream after 15 s. Eye chip absent or busy: start Monado without eye tracking, stop other pimax-eye.".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        self.tracker = Some(tr);
        Ok(lens0)
    }

    fn collect(&mut self, seconds: f64, tick: &mut dyn FnMut(f32), stop: &dyn Fn() -> bool) -> Measurement {
        let mut me = Measure::default();
        if let Some(tr) = self.tracker.take() {
            let t0 = Instant::now();
            // Drop the samples from the countdown: only what arrives from now on counts.
            for s in tr.samples_since(self.last_seq) {
                self.last_seq = Some(s.seq);
            }
            while t0.elapsed().as_secs_f64() < seconds && !stop() {
                std::thread::sleep(Duration::from_millis(50));
                for s in tr.samples_since(self.last_seq) {
                    self.last_seq = Some(s.seq);
                    me.push(&s);
                }
                tick((t0.elapsed().as_secs_f64() / seconds).min(1.0) as f32);
            }
            tr.stop();
        }
        me.result()
    }

    fn send(&mut self, mm: f32) -> Result<String, String> {
        let node = self.cmd.clone().ok_or("no command node")?;
        let rep = ipd::motor_report(mm);
        ipd::send_motor(&node, mm).map_err(|e| format!("{}: {e}", node.display()))?;
        Ok(format!(
            "sent once to {}: {} -> lens IPD {mm:.2} mm",
            node.display(),
            rep[..7].iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
        ))
    }

    fn verify(&mut self, target: f32, timeout: Duration, tick: &mut dyn FnMut(f32)) -> Option<f32> {
        let info = self.info.clone()?;
        let t0 = Instant::now();
        let mut fin = None;
        while t0.elapsed() < timeout {
            if let Some(v) = ipd::read_lens_ipd(&info, Duration::from_millis(500)) {
                fin = Some(v);
                if ipd::arrived(v, target) {
                    break;
                }
            }
            tick((t0.elapsed().as_secs_f32() / timeout.as_secs_f32()).min(1.0));
        }
        fin
    }
}

/// Fake measurement and lens for `--simulate` and tests: the lens "moves" to whatever is sent.
pub struct Sim {
    pub lens: Option<f32>,
    pub field34: f32,
    pub origin: f32,
    pub sent: Vec<f32>,
    pub fail: Option<String>,
}

impl Sim {
    pub fn new(lens: Option<f32>, field34: f32, origin: f32) -> Sim {
        Sim { lens, field34, origin, sent: vec![], fail: None }
    }
}

impl Devices for Sim {
    fn prepare(&mut self) -> Result<Option<f32>, String> {
        match &self.fail {
            Some(e) => Err(e.clone()),
            None => Ok(self.lens),
        }
    }

    fn collect(&mut self, _seconds: f64, tick: &mut dyn FnMut(f32), _stop: &dyn Fn() -> bool) -> Measurement {
        for i in 0..=4 {
            tick(i as f32 / 4.0);
        }
        Measurement { seen: 400, valid: 390, good: 380, field34_mm: self.field34, origin_mm: self.origin }
    }

    fn send(&mut self, mm: f32) -> Result<String, String> {
        self.sent.push(mm);
        self.lens = Some(mm);
        Ok(format!("simulated: would send lens IPD {mm:.2} mm"))
    }

    fn verify(&mut self, _target: f32, _timeout: Duration, tick: &mut dyn FnMut(f32)) -> Option<f32> {
        tick(0.5);
        self.lens
    }
}
