//! The only `unsafe` code of the I/O layer: thin wrappers over `ppoll`, `eventfd` and `socketpair`.
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::time::Duration;

pub use libc::{POLLERR, POLLHUP, POLLIN, POLLNVAL};

/// One entry for [`ppoll`].
pub type PollFd = libc::pollfd;

/// Construct a poll entry waiting for input.
pub fn pollin(fd: RawFd) -> PollFd {
    libc::pollfd {
        fd,
        events: POLLIN,
        revents: 0,
    }
}

/// `ppoll` with nanosecond timeout (`None` = block). Returns the number of ready fds; EINTR is reported as 0.
pub fn ppoll(fds: &mut [PollFd], timeout: Option<Duration>) -> io::Result<usize> {
    let ts = timeout.map(|d| libc::timespec {
        tv_sec: d.as_secs() as libc::time_t,
        tv_nsec: i64::from(d.subsec_nanos()) as _,
    });
    let tsp = ts
        .as_ref()
        .map_or(std::ptr::null(), |t| t as *const libc::timespec);
    // SAFETY: `fds` is a valid slice of pollfd for its whole length; `tsp` is null or points to a live timespec.
    let rc = unsafe {
        libc::ppoll(
            fds.as_mut_ptr(),
            fds.len() as libc::nfds_t,
            tsp,
            std::ptr::null(),
        )
    };
    if rc < 0 {
        let e = io::Error::last_os_error();
        if e.kind() == io::ErrorKind::Interrupted {
            return Ok(0);
        }
        return Err(e);
    }
    Ok(rc as usize)
}

/// Poll any number of files for input; returns which are readable (or hung up).
pub fn wait_readable(files: &[&File], timeout: Option<Duration>) -> io::Result<Vec<bool>> {
    let mut p: Vec<PollFd> = files.iter().map(|f| pollin(f.as_raw_fd())).collect();
    ppoll(&mut p, timeout)?;
    Ok(p.iter()
        .map(|x| x.revents & (POLLIN | POLLHUP | POLLERR) != 0)
        .collect())
}

/// Non-blocking eventfd used to wake the session thread.
pub fn eventfd() -> io::Result<File> {
    // SAFETY: plain syscall without pointers.
    let fd = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is a freshly created descriptor we own.
    Ok(File::from(unsafe { OwnedFd::from_raw_fd(fd) }))
}

/// Non-blocking `SOCK_SEQPACKET` socketpair: message boundaries like hidraw.
pub fn seqpacket_pair() -> io::Result<(File, File)> {
    let mut sv = [0 as RawFd; 2];
    // SAFETY: `sv` has room for the two descriptors socketpair writes.
    let rc = unsafe {
        libc::socketpair(
            libc::AF_UNIX,
            libc::SOCK_SEQPACKET | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
            sv.as_mut_ptr(),
        )
    };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: both descriptors were just created and are owned here.
    unsafe {
        Ok((
            File::from(OwnedFd::from_raw_fd(sv[0])),
            File::from(OwnedFd::from_raw_fd(sv[1])),
        ))
    }
}

/// Monotonic CPU time of the calling process (user + system) in seconds, from `/proc/self/stat`.
pub fn process_cpu_seconds() -> Option<f64> {
    proc_cpu_seconds("/proc/self/stat")
}

/// CPU seconds of `/proc/<pid>/stat`-style file (utime+stime, fields 14 and 15).
pub fn proc_cpu_seconds(path: &str) -> Option<f64> {
    let s = std::fs::read_to_string(path).ok()?;
    let rest = &s[s.rfind(')')? + 2..];
    let f: Vec<&str> = rest.split(' ').collect();
    // after the comm field: state is index 0, utime index 11, stime index 12
    let ut: f64 = f.get(11)?.parse().ok()?;
    let st: f64 = f.get(12)?.parse().ok()?;
    // SAFETY: sysconf has no memory arguments.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
    Some((ut + st) / hz)
}

static TERM_FLAG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

extern "C" fn on_term(_sig: libc::c_int) {
    TERM_FLAG.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Install SIGINT/SIGTERM handlers that set the returned flag (no SA_RESTART, so blocking calls return EINTR).
pub fn install_term_flag() -> &'static std::sync::atomic::AtomicBool {
    // SAFETY: the handler only stores to an atomic, which is async-signal-safe.
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = on_term as extern "C" fn(libc::c_int) as usize;
        libc::sigemptyset(&mut sa.sa_mask);
        libc::sigaction(libc::SIGINT, &sa, std::ptr::null_mut());
        libc::sigaction(libc::SIGTERM, &sa, std::ptr::null_mut());
    }
    &TERM_FLAG
}

/// Block SIGINT/SIGTERM in the calling thread (call before spawning threads so they inherit the mask).
pub fn block_term_signals() {
    // SAFETY: plain signal-mask manipulation on a local set.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::sigaddset(&mut set, libc::SIGTERM);
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
    }
}

/// Sleep until SIGINT or SIGTERM arrives (after [`block_term_signals`]); zero CPU while waiting.
pub fn wait_for_term() {
    // SAFETY: sigwait on a local set containing blocked signals.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::sigaddset(&mut set, libc::SIGTERM);
        let mut sig = 0;
        libc::sigwait(&set, &mut sig);
    }
}

/// Local wall-clock time as `HH:MM:SS`.
pub fn local_hms() -> String {
    // SAFETY: time/localtime_r with local buffers.
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
    }
}
