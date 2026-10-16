//! In-process fake headset: seqpacket socketpairs behave like hidraw (message boundaries, poll, hang-up on unplug).
//! Used by the test suite, the benchmarks and the C-ABI tests; never touches real hardware.
use crate::session::{Backend, Link};
use crate::sys;
use pimax_proto::{Report, NUM_IF, REPORT_SIZE};
use std::fs::File;
use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct Ends {
    if_dev: [Option<File>; NUM_IF],
    ctl_dev: File,
}

#[derive(Default)]
struct FakeState {
    plugged: bool,
    ends: Option<Ends>,
    connects: u64,
}

/// The "device side" handle of a fake headset.
#[derive(Clone)]
pub struct FakeDevice {
    st: Arc<Mutex<FakeState>>,
}

/// The [`Backend`] half; give it to `Session::open_with_backend`.
pub struct FakeBackend {
    st: Arc<Mutex<FakeState>>,
}

/// A plugged-in fake headset and its backend.
pub fn pair() -> (FakeBackend, FakeDevice) {
    let st = Arc::new(Mutex::new(FakeState {
        plugged: true,
        ..Default::default()
    }));
    (FakeBackend { st: st.clone() }, FakeDevice { st })
}

impl Backend for FakeBackend {
    fn connect(&mut self) -> io::Result<Option<Link>> {
        let mut s = self.st.lock().unwrap_or_else(|p| p.into_inner());
        if !s.plugged {
            return Ok(None);
        }
        let mut reads: [Option<File>; NUM_IF] = Default::default();
        let mut devs: [Option<File>; NUM_IF] = Default::default();
        let (h0, d0) = sys::seqpacket_pair()?;
        let (ctl_h, ctl_d) = sys::seqpacket_pair()?;
        reads[0] = Some(h0);
        devs[0] = Some(d0);
        for i in 1..NUM_IF {
            let (h, d) = sys::seqpacket_pair()?;
            reads[i] = Some(h);
            devs[i] = Some(d);
        }
        s.ends = Some(Ends {
            if_dev: devs,
            ctl_dev: ctl_d,
        });
        s.connects += 1;
        Ok(Some(Link { reads, ctl: ctl_h }))
    }
}

impl FakeDevice {
    /// Number of sessions the host has opened.
    pub fn connects(&self) -> u64 {
        self.st.lock().unwrap_or_else(|p| p.into_inner()).connects
    }

    /// Simulate unplugging: the host sees hang-up on every interface and discovery finds nothing.
    pub fn unplug(&self) {
        let mut s = self.st.lock().unwrap_or_else(|p| p.into_inner());
        s.plugged = false;
        s.ends = None;
    }

    /// Plug it back in; the next connect attempt succeeds.
    pub fn plug(&self) {
        self.st.lock().unwrap_or_else(|p| p.into_inner()).plugged = true;
    }

    fn with_ends<T>(&self, f: impl FnOnce(&mut Ends) -> T) -> Option<T> {
        let mut s = self.st.lock().unwrap_or_else(|p| p.into_inner());
        s.ends.as_mut().map(f)
    }

    /// Wait until the host has connected (device ends exist).
    pub fn wait_host(&self, timeout: Duration) -> bool {
        let end = Instant::now() + timeout;
        while Instant::now() < end {
            if self.with_ends(|_| ()).is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        false
    }

    /// Deliver a report on interface `iface` (0..3) as if the device had sent it.
    pub fn send_report(&self, iface: usize, r: &Report) -> io::Result<()> {
        self.with_ends(|e| {
            let f: &File = e.if_dev[iface].as_ref().unwrap_or(&e.ctl_dev);
            (&*f).write(r).map(|_| ())
        })
        .unwrap_or_else(|| Err(io::ErrorKind::NotConnected.into()))
    }

    /// Next command the host wrote to IF0 (report id stripped), waiting up to `timeout`.
    pub fn recv_command(&self, timeout: Duration) -> Option<Report> {
        let end = Instant::now() + timeout;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            let r = self.with_ends(|e| {
                let mut buf = [0u8; REPORT_SIZE + 1];
                match (&e.ctl_dev).read(&mut buf) {
                    Ok(n) if n == REPORT_SIZE + 1 => {
                        let mut r = [0u8; REPORT_SIZE];
                        r.copy_from_slice(&buf[1..]);
                        Some(Some(r))
                    }
                    Ok(_) => Some(None),
                    Err(_) => None,
                }
            });
            match r {
                Some(Some(Some(r))) => return Some(r),
                Some(Some(None)) => continue,
                _ => {}
            }
            if left.is_zero() {
                return None;
            }
            std::thread::sleep(Duration::from_millis(1).min(left));
        }
    }

    /// Collect every command written until `timeout` expires.
    pub fn drain_commands(&self, timeout: Duration) -> Vec<Report> {
        let end = Instant::now() + timeout;
        let mut v = vec![];
        while let Some(r) = self.recv_command(end.saturating_duration_since(Instant::now())) {
            v.push(r);
        }
        v
    }
}
