//! Event sources (plan section 3.4), no root and no new dependencies:
//! * [`UeventSocket`]: a NETLINK_KOBJECT_UEVENT socket on the kernel multicast group 1. An event is only a hint:
//!   the caller re-scans sysfs (debounced) for the truth. If the bind fails the caller polls.
//! * [`PidWatch`]: pidfd_open(2) for every watched pid, one poll thread; an exit is reported once.
//! * [`KlogFollower`]: one `journalctl -k -f -n 0 -o short-monotonic` child; its lines are classified by
//!   [`crate::gpu`] (Xid, Idling, Stuck layer, segfaults, USB disconnects).
//! * [`Resume`]: CLOCK_BOOTTIME running ahead of CLOCK_MONOTONIC by more than 2 s between two ticks means the
//!   machine was suspended. No D-Bus.
use std::io::{BufRead, BufReader};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::Path;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// One event for a daemon loop.
#[derive(Debug, Clone, PartialEq)]
pub enum Ev {
    Uevent(Uevent),
    /// A watched pid exited.
    Exit(u32),
    /// A kernel log line.
    Kernel(String),
}

/// A parsed kobject uevent.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Uevent {
    pub action: String,
    pub devpath: String,
    pub subsystem: String,
    /// `PRODUCT=34a4/44/100` (usb devices).
    pub product: Option<String>,
    pub devtype: Option<String>,
}

impl Uevent {
    /// Kernel uevents look like `add@/devices/...\0ACTION=add\0DEVPATH=...\0SUBSYSTEM=usb\0...`. udev's own
    /// re-broadcasts (`libudev` magic) are on another group and are not parsed.
    pub fn parse(b: &[u8]) -> Option<Uevent> {
        let mut parts = b.split(|&c| c == 0).filter(|s| !s.is_empty());
        let head = std::str::from_utf8(parts.next()?).ok()?;
        let (action, devpath) = head.split_once('@')?;
        let mut u = Uevent { action: action.into(), devpath: devpath.into(), ..Default::default() };
        for p in parts {
            let Ok(s) = std::str::from_utf8(p) else { continue };
            match s.split_once('=') {
                Some(("SUBSYSTEM", v)) => u.subsystem = v.into(),
                Some(("PRODUCT", v)) => u.product = Some(v.into()),
                Some(("DEVTYPE", v)) => u.devtype = Some(v.into()),
                Some(("ACTION", v)) => u.action = v.into(),
                _ => {}
            }
        }
        Some(u)
    }

    /// Worth a re-scan: USB (any: the hub matters too), DRM connectors, video4linux, hidraw.
    pub fn relevant(&self) -> bool {
        matches!(self.subsystem.as_str(), "usb" | "drm" | "video4linux" | "hidraw")
    }

    /// The Pimax vendor (`PRODUCT=34a4/...`).
    pub fn is_pimax(&self) -> bool {
        self.product.as_deref().is_some_and(|p| p.starts_with("34a4/"))
    }

    pub fn summary(&self) -> String {
        format!("{} {} {}{}", self.action, self.subsystem, self.devpath, self.product.as_deref().map(|p| format!(" ({p})")).unwrap_or_default())
    }
}

/// The netlink uevent socket.
pub struct UeventSocket {
    fd: OwnedFd,
}

impl UeventSocket {
    pub fn open() -> std::io::Result<UeventSocket> {
        // SAFETY: plain socket/bind calls with a zeroed sockaddr_nl.
        unsafe {
            let fd = libc::socket(libc::AF_NETLINK, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, libc::NETLINK_KOBJECT_UEVENT);
            if fd < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let fd = OwnedFd::from_raw_fd(fd);
            let mut sa: libc::sockaddr_nl = std::mem::zeroed();
            sa.nl_family = libc::AF_NETLINK as u16;
            sa.nl_groups = 1;
            let r = libc::bind(fd.as_raw_fd(), (&sa as *const libc::sockaddr_nl).cast(), std::mem::size_of::<libc::sockaddr_nl>() as u32);
            if r < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(UeventSocket { fd })
        }
    }

    /// Block until one datagram arrives.
    pub fn recv(&self) -> std::io::Result<Vec<u8>> {
        let mut buf = vec![0u8; 8192];
        // SAFETY: valid buffer.
        let n = unsafe { libc::recv(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len(), 0) };
        if n < 0 {
            return Err(std::io::Error::last_os_error());
        }
        buf.truncate(n as usize);
        Ok(buf)
    }

    /// A thread sending relevant uevents to `tx` until the receiver is gone.
    pub fn spawn(self, tx: Sender<Ev>) {
        std::thread::Builder::new()
            .name("uevent".into())
            .spawn(move || loop {
                match self.recv() {
                    Ok(b) => {
                        if let Some(u) = Uevent::parse(&b).filter(Uevent::relevant) {
                            if tx.send(Ev::Uevent(u)).is_err() {
                                return;
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => std::thread::sleep(Duration::from_secs(1)),
                }
            })
            .ok();
    }
}

/// pidfd_open(2).
pub fn pidfd_open(pid: u32) -> std::io::Result<OwnedFd> {
    // SAFETY: the syscall returns a new fd or -1.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: a fresh fd we own.
    Ok(unsafe { OwnedFd::from_raw_fd(fd as RawFd) })
}

struct Watched {
    pid: u32,
    fd: OwnedFd,
}

/// Watches pids with pidfds in one thread.
#[derive(Clone)]
pub struct PidWatch {
    inner: Arc<Mutex<Vec<Watched>>>,
    wake: Arc<OwnedFd>,
}

impl PidWatch {
    /// Start the poll thread; exits go to `tx` as [`Ev::Exit`].
    pub fn spawn(tx: Sender<Ev>) -> std::io::Result<PidWatch> {
        let mut p = [0 as RawFd; 2];
        // SAFETY: pipe2 fills two fds.
        if unsafe { libc::pipe2(p.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: fresh fds we own.
        let (rd, wr) = unsafe { (OwnedFd::from_raw_fd(p[0]), OwnedFd::from_raw_fd(p[1])) };
        let w = PidWatch { inner: Arc::new(Mutex::new(Vec::new())), wake: Arc::new(wr) };
        let inner = w.inner.clone();
        std::thread::Builder::new().name("pidfd".into()).spawn(move || loop {
            let mut fds: Vec<libc::pollfd> = vec![libc::pollfd { fd: rd.as_raw_fd(), events: libc::POLLIN, revents: 0 }];
            let pids: Vec<u32> = {
                let g = inner.lock().unwrap();
                for x in g.iter() {
                    fds.push(libc::pollfd { fd: x.fd.as_raw_fd(), events: libc::POLLIN, revents: 0 });
                }
                g.iter().map(|x| x.pid).collect()
            };
            // SAFETY: valid pollfd array.
            let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 5000) };
            if n < 0 {
                continue;
            }
            if fds[0].revents != 0 {
                let mut b = [0u8; 64];
                // SAFETY: draining our pipe.
                while unsafe { libc::read(rd.as_raw_fd(), b.as_mut_ptr().cast(), b.len()) } > 0 {}
            }
            let dead: Vec<u32> = fds[1..].iter().zip(&pids).filter(|(f, _)| f.revents != 0).map(|(_, p)| *p).collect();
            if !dead.is_empty() {
                inner.lock().unwrap().retain(|x| !dead.contains(&x.pid));
                for p in dead {
                    if tx.send(Ev::Exit(p)).is_err() {
                        return;
                    }
                }
            }
        })?;
        Ok(w)
    }

    /// Watch `pid` (no-op if already watched). Errors when the process is gone or pidfd is unsupported.
    pub fn add(&self, pid: u32) -> std::io::Result<()> {
        let mut g = self.inner.lock().unwrap();
        if g.iter().any(|x| x.pid == pid) {
            return Ok(());
        }
        let fd = pidfd_open(pid)?;
        g.push(Watched { pid, fd });
        drop(g);
        // SAFETY: one byte to our own pipe.
        unsafe { libc::write(self.wake.as_raw_fd(), b"x".as_ptr().cast(), 1) };
        Ok(())
    }

    pub fn watched(&self) -> Vec<u32> {
        self.inner.lock().unwrap().iter().map(|x| x.pid).collect()
    }
}

/// The `journalctl -k -f` child.
pub struct KlogFollower {
    child: std::process::Child,
}

impl KlogFollower {
    /// Spawn the follower; each line goes to `tx` as [`Ev::Kernel`]. `cmd` replaces journalctl (tests).
    pub fn spawn(tx: Sender<Ev>, cmd: Option<&[&str]>) -> std::io::Result<KlogFollower> {
        let default = ["journalctl", "-k", "-f", "-n", "0", "-o", "short-monotonic", "-q", "--no-pager"];
        let argv: Vec<&str> = cmd.map(|c| c.to_vec()).unwrap_or_else(|| default.to_vec());
        let mut child = std::process::Command::new(argv[0])
            .args(&argv[1..])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        let out = child.stdout.take().expect("piped");
        std::thread::Builder::new().name("klog".into()).spawn(move || {
            for l in BufReader::new(out).lines() {
                let Ok(l) = l else { return };
                if tx.send(Ev::Kernel(l)).is_err() {
                    return;
                }
            }
        })?;
        Ok(KlogFollower { child })
    }

    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for KlogFollower {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Run a command with a deadline; stdout as text (None on failure or timeout).
pub fn run_capture(argv: &[&str], timeout: Duration) -> Option<String> {
    use std::io::Read;
    let mut c = std::process::Command::new(argv[0])
        .args(&argv[1..])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let mut out = c.stdout.take()?;
    let h = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let t0 = std::time::Instant::now();
    loop {
        match c.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if t0.elapsed() < timeout => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = c.kill();
                let _ = c.wait();
                return None;
            }
        }
    }
    h.join().ok()
}

/// GPU-relevant kernel lines of this boot (one `journalctl -k -b` read; empty if unavailable).
pub fn kernel_log_this_boot() -> Vec<String> {
    if std::env::var_os("PIMAX_NO_JOURNAL").is_some() {
        return vec![];
    }
    let out = run_capture(&["journalctl", "-k", "-b", "-q", "--no-pager", "-o", "short-monotonic"], Duration::from_secs(5)).unwrap_or_default();
    out.lines().filter(|l| l.contains("Xid") || l.contains("Idling display") || l.contains("Stuck layer")).map(str::to_string).collect()
}

pub fn read_intent(p: &Path) -> Option<crate::model::Intent> {
    crate::intent::read(p)
}

/// clock_gettime in seconds.
pub fn clock(id: libc::clockid_t) -> f64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: valid timespec.
    unsafe { libc::clock_gettime(id, &mut ts) };
    ts.tv_sec as f64 + ts.tv_nsec as f64 * 1e-9
}

/// Resume detection: BOOTTIME counts suspend, MONOTONIC does not.
#[derive(Debug, Clone, Default)]
pub struct Resume {
    last: Option<(f64, f64)>,
}

/// A suspend of more than this is a resume.
pub const RESUME_JUMP_S: f64 = 2.0;

impl Resume {
    /// Feed (monotonic, boottime); returns the time asleep when the machine resumed since the last call.
    pub fn check(&mut self, mono: f64, boot: f64) -> Option<f64> {
        let r = self.last.and_then(|(m0, b0)| {
            let slept = (boot - b0) - (mono - m0);
            (slept > RESUME_JUMP_S).then_some(slept)
        });
        self.last = Some((mono, boot));
        r
    }

    pub fn check_now(&mut self) -> Option<f64> {
        self.check(clock(libc::CLOCK_MONOTONIC), clock(libc::CLOCK_BOOTTIME))
    }
}

/// `pimax-health --watch`: re-render on uevents, process exits, kernel lines, resumes and every `interval` s;
/// print only when the output changed.
pub fn watch(r: &crate::util::Roots, json: bool, deep: bool, probe: bool, interval: f64, render: &dyn Fn(&crate::util::Roots, bool, bool, bool) -> String) -> i32 {
    let (tx, rx) = std::sync::mpsc::channel::<Ev>();
    let uevents = match UeventSocket::open() {
        Ok(s) => {
            s.spawn(tx.clone());
            true
        }
        Err(_) => false,
    };
    let pids = PidWatch::spawn(tx.clone()).ok();
    let _klog = KlogFollower::spawn(tx.clone(), None).ok();
    let stop = install_term_flag();
    let mut resume = Resume::default();
    let mut last = String::new();
    let mut why = String::from("start");
    if !json {
        eprintln!("watching (uevents {}, pidfd {}, kernel log {}); Ctrl-C to stop", if uevents { "on" } else { "off: polling" }, if pids.is_some() { "on" } else { "off" }, if _klog.is_some() { "on" } else { "off" });
    }
    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
        if let Some(s) = resume.check_now() {
            why = format!("resumed after {s:.0} s");
        }
        let out = render(r, json, deep, probe);
        if out != last {
            if json {
                println!("{out}");
            } else {
                println!("--- {} ({why})\n{out}", local_hms());
            }
            last = out;
        }
        if let Some(pw) = &pids {
            for s in crate::spec::SPECS.iter().filter_map(|s| s.pid) {
                if let Some(i) = crate::procs::from_pid_file(r, &s) {
                    let _ = pw.add(i.pid);
                }
            }
        }
        match rx.recv_timeout(Duration::from_secs_f64(interval)) {
            Ok(Ev::Uevent(u)) => {
                why = format!("uevent {}", u.summary());
                // the hint; the truth comes from a re-scan once the burst settled
                std::thread::sleep(Duration::from_millis(500));
                while rx.try_recv().is_ok() {}
            }
            Ok(Ev::Exit(p)) => why = format!("pid {p} exited"),
            Ok(Ev::Kernel(l)) => {
                if crate::gpu::kernel_line(&l).is_none() && crate::gpu::crash_line(&l).is_none() && crate::gpu::usb_disconnect_line(&l).is_none() {
                    continue;
                }
                why = format!("kernel: {}", l.trim());
            }
            Err(_) => why = "tick".into(),
        }
    }
    0
}

fn install_term_flag() -> &'static std::sync::atomic::AtomicBool {
    static FLAG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    extern "C" fn on(_: libc::c_int) {
        FLAG.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    // SAFETY: an async-signal-safe handler storing an atomic.
    unsafe {
        libc::signal(libc::SIGINT, on as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on as *const () as libc::sighandler_t);
    }
    &FLAG
}

/// `HH:MM:SS` local time.
pub fn local_hms() -> String {
    // SAFETY: time + localtime_r with valid pointers.
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Built from the 2026-10-15 09:21:43 drop (logs-hang-0711/kern.log: usb 2-4 disconnect, then the camera).
    pub const REMOVE_CAM: &[u8] = b"remove@/devices/pci0000:00/0000:00:14.0/usb2/2-4/2-4.1\0ACTION=remove\0DEVPATH=/devices/pci0000:00/0000:00:14.0/usb2/2-4/2-4.1\0SUBSYSTEM=usb\0DEVTYPE=usb_device\0PRODUCT=34a4/14/100\0SEQNUM=5123\0";
    pub const ADD_HUB: &[u8] = b"add@/devices/pci0000:00/0000:00:14.0/usb2/2-4\0ACTION=add\0SUBSYSTEM=usb\0DEVTYPE=usb_device\0PRODUCT=5e3/620/9324\0SEQNUM=5130\0";
    pub const DRM: &[u8] = b"change@/devices/pci0000:00/0000:00:01.0/0000:01:00.0/drm/card1\0ACTION=change\0SUBSYSTEM=drm\0HOTPLUG=1\0SEQNUM=5140\0";

    #[test]
    fn uevents_parse() {
        let u = Uevent::parse(REMOVE_CAM).unwrap();
        assert_eq!(u.action, "remove");
        assert_eq!(u.subsystem, "usb");
        assert_eq!(u.product.as_deref(), Some("34a4/14/100"));
        assert!(u.relevant() && u.is_pimax());
        let h = Uevent::parse(ADD_HUB).unwrap();
        assert!(h.relevant() && !h.is_pimax());
        let d = Uevent::parse(DRM).unwrap();
        assert_eq!(d.subsystem, "drm");
        assert!(d.relevant());
        assert!(Uevent::parse(b"libudev\0\xfe\xed").is_none());
        assert!(!Uevent::parse(b"add@/x\0SUBSYSTEM=net\0").unwrap().relevant());
    }

    #[test]
    fn pidfd_reports_a_child_exit() {
        let (tx, rx) = std::sync::mpsc::channel();
        let w = PidWatch::spawn(tx).unwrap();
        let mut c = std::process::Command::new("sleep").arg("0.2").spawn().unwrap();
        w.add(c.id()).unwrap();
        w.add(c.id()).unwrap();
        assert_eq!(w.watched().len(), 1);
        let ev = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(ev, Ev::Exit(c.id()));
        let _ = c.wait();
        assert!(w.watched().is_empty());
        assert!(pidfd_open(c.id()).is_err(), "reaped");
    }

    #[test]
    fn klog_follower_streams_lines() {
        let (tx, rx) = std::sync::mpsc::channel();
        let _k = KlogFollower::spawn(tx, Some(&["printf", "a Xid 13\\nb\\n"])).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), Ev::Kernel("a Xid 13".into()));
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), Ev::Kernel("b".into()));
    }

    #[test]
    fn resume_is_a_boottime_jump() {
        let mut r = Resume::default();
        assert_eq!(r.check(100.0, 100.0), None);
        assert_eq!(r.check(101.0, 101.5), None, "jitter");
        assert_eq!(r.check(102.0, 3702.5), Some(3600.0));
        assert_eq!(r.check(103.0, 3703.5), None);
        // the real clocks do not jump within a test
        let mut r = Resume::default();
        assert_eq!(r.check_now(), None);
        assert_eq!(r.check_now(), None);
    }

    #[test]
    fn run_capture_times_out() {
        assert_eq!(run_capture(&["echo", "hi"], Duration::from_secs(5)).as_deref(), Some("hi\n"));
        assert_eq!(run_capture(&["sleep", "5"], Duration::from_millis(100)), None);
        assert_eq!(run_capture(&["/nonexistent/x"], Duration::from_secs(1)), None);
    }
}
