//! Background polling: [`Live`] every second, [`Facts`] every 10 s (the doctor checks every 60 s) or right away when
//! asked (after a job, on "Re-check"). Results go back over a channel; the UI thread never waits for `/proc` scans,
//! `journalctl` or `getcap`.
use crate::builds::MarkerCache;
use crate::facts::{collect_facts, collect_live, Facts, Live};
use crate::machine::Machine;
use crate::monlog::LogFollower;
use crate::paths::Paths;
use omarchyvr_doctor::Opts;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A result of the poller.
#[derive(Debug, Clone)]
pub enum Update {
    /// New live state.
    Live(Box<Live>),
    /// New facts.
    Facts(Box<Facts>),
}

/// Requests to the poller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// Collect the facts now (`doctor` = also the doctor checks; `caps` = ask the launcher again).
    Facts {
        /// Run the doctor checks too.
        doctor: bool,
        /// Re-read `omarchyvr desk help`.
        caps: bool,
    },
    /// End the thread.
    Stop,
}

/// Handle of the poller thread.
pub struct Monitor {
    tx: Sender<Request>,
    rx: Receiver<Update>,
}

/// Seconds between live updates, fact updates and doctor runs.
pub const LIVE_EVERY: Duration = Duration::from_secs(1);
/// Facts refresh period.
pub const FACTS_EVERY: Duration = Duration::from_secs(10);
/// Doctor refresh period.
pub const DOCTOR_EVERY: Duration = Duration::from_secs(60);

impl Monitor {
    /// Start polling.
    pub fn start(m: Arc<dyn Machine>, p: Paths, opts: Opts, cache: Arc<MarkerCache>, wake: Arc<dyn Fn() + Send + Sync>) -> Monitor {
        let (tx, req_rx) = channel::<Request>();
        let (up_tx, rx) = channel::<Update>();
        std::thread::Builder::new()
            .name("pimax-monitor".into())
            .spawn(move || {
                let mut follower = LogFollower::new(&p.session_dir.join("monado.log"));
                let mut last_facts: Option<Facts> = None;
                // the first round always collects everything (pending), so the timers can start now
                let mut t_facts = Instant::now();
                let mut t_doctor = Instant::now();
                let mut t_caps = Instant::now();
                let mut pending: Option<(bool, bool)> = Some((true, true));
                loop {
                    // a panic in one round (a strange /proc entry) is logged and the next round goes on
                    match crate::crash::catching(|| collect_live(m.as_ref(), &p, &mut follower)) {
                        Ok(live) => {
                            if up_tx.send(Update::Live(Box::new(live))).is_err() {
                                return;
                            }
                            wake();
                        }
                        Err(e) => {
                            log::error!("live status round failed: {}", crate::worker::panic_text(e.as_ref()));
                            follower = LogFollower::new(&p.session_dir.join("monado.log"));
                            std::thread::sleep(Duration::from_secs(2));
                        }
                    }
                    let due = t_facts.elapsed() >= FACTS_EVERY;
                    if due || pending.is_some() {
                        let (want_doc, want_caps) = pending.take().unwrap_or((false, false));
                        let doctor = want_doc || t_doctor.elapsed() >= DOCTOR_EVERY;
                        let caps = want_caps || t_caps.elapsed() >= Duration::from_secs(60);
                        let f = match crate::crash::catching(|| collect_facts(m.as_ref(), &p, &opts, &cache, last_facts.as_ref(), doctor, caps)) {
                            Ok(f) => f,
                            Err(e) => {
                                log::error!("facts round failed: {}", crate::worker::panic_text(e.as_ref()));
                                t_facts = Instant::now();
                                continue;
                            }
                        };
                        t_facts = Instant::now();
                        if doctor {
                            t_doctor = Instant::now();
                        }
                        if caps {
                            t_caps = Instant::now();
                        }
                        last_facts = Some(f.clone());
                        if up_tx.send(Update::Facts(Box::new(f))).is_err() {
                            return;
                        }
                        wake();
                    }
                    match req_rx.recv_timeout(LIVE_EVERY) {
                        Ok(Request::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                        Ok(Request::Facts { doctor, caps }) => {
                            let (d, c) = pending.unwrap_or((false, false));
                            pending = Some((d || doctor, c || caps));
                            // collect several requests in a row into one run
                            while let Ok(r) = req_rx.try_recv() {
                                match r {
                                    Request::Stop => return,
                                    Request::Facts { doctor, caps } => {
                                        let (d, c) = pending.unwrap_or((false, false));
                                        pending = Some((d || doctor, c || caps));
                                    }
                                }
                            }
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                    }
                }
            })
            .expect("spawn the monitor thread");
        Monitor { tx, rx }
    }

    /// Ask for fresh facts.
    pub fn request(&self, doctor: bool, caps: bool) {
        let _ = self.tx.send(Request::Facts { doctor, caps });
    }

    /// Updates since the last call.
    pub fn poll(&self) -> Vec<Update> {
        self.rx.try_iter().collect()
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        let _ = self.tx.send(Request::Stop);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo;

    #[test]
    fn delivers_live_and_facts_and_stops() {
        let (m, p) = demo::running_machine();
        let opts = demo::opts(&p);
        let mon = Monitor::start(Arc::new(m), p, opts, Arc::new(MarkerCache::default()), Arc::new(|| {}));
        let mut live = None;
        let mut facts = None;
        for _ in 0..300 {
            for u in mon.poll() {
                match u {
                    Update::Live(l) => live = Some(l),
                    Update::Facts(f) => facts = Some(f),
                }
            }
            if live.is_some() && facts.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(live.unwrap().service.unwrap().pid, 372460);
        assert!(facts.unwrap().report.is_some(), "the first facts include the doctor");
        mon.request(true, false);
        drop(mon);
    }
}
