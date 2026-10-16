//! SIGINT / SIGTERM / SIGHUP set a flag; the loops poll it (every frame, or every 20 ms while waiting).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Set by the signal handler.
pub static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

/// Install the handlers. `SA_RESTART` stays on so that blocking calls inside the OpenXR runtime library are restarted
/// instead of failing with EINTR; the loops notice the flag within a frame.
pub fn install() {
    // SAFETY: the handler only stores to an atomic; the sigaction struct is zero-initialised and filled before use.
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = on_signal as *const () as libc::sighandler_t;
        sa.sa_flags = libc::SA_RESTART;
        libc::sigemptyset(&mut sa.sa_mask);
        for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            libc::sigaction(sig, &sa, std::ptr::null_mut());
        }
    }
}

pub fn stopped() -> bool {
    STOP.load(Ordering::Relaxed)
}

/// Sleep `total` in slices of at most 20 ms, calling `tick` each slice; returns early when a stop was requested.
pub fn wait(total: Duration, tick: impl FnMut(Instant)) {
    wait_unless(total, &STOP, tick);
}

/// [`wait`] with an explicit stop flag.
pub fn wait_unless(total: Duration, stop: &AtomicBool, mut tick: impl FnMut(Instant)) {
    let end = Instant::now() + total;
    loop {
        let now = Instant::now();
        tick(now);
        if stop.load(Ordering::Relaxed) || now >= end {
            return;
        }
        std::thread::sleep((end - now).min(Duration::from_millis(20)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wait_ticks_and_returns_after_the_time() {
        let t = Instant::now();
        let mut ticks = 0;
        let never = AtomicBool::new(false);
        wait_unless(Duration::from_millis(100), &never, |_| ticks += 1);
        assert!(t.elapsed() >= Duration::from_millis(100));
        assert!(t.elapsed() < Duration::from_millis(400));
        assert!(ticks >= 4, "{ticks}");
        // an already set flag ends the wait after the first tick
        let stop = AtomicBool::new(true);
        let t = Instant::now();
        let mut ticks = 0;
        wait_unless(Duration::from_secs(5), &stop, |_| ticks += 1);
        assert_eq!(ticks, 1);
        assert!(t.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn the_handler_sets_the_flag() {
        // run in the same process: raise SIGHUP after installing, the default action would kill the test
        install();
        assert!(!stopped() || STOP.load(Ordering::Relaxed));
        STOP.store(false, Ordering::Relaxed);
        // SAFETY: raising a signal that this process handles.
        unsafe {
            libc::raise(libc::SIGHUP);
        }
        assert!(stopped());
        STOP.store(false, Ordering::Relaxed);
    }
}
