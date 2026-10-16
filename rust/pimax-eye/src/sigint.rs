//! Ctrl-C flag. The handler only sets an atomic; the main loops poll it between USB transfers (all of which have
//! timeouts), then drop the device, which releases the interface.
#![allow(unsafe_code)]

use std::sync::atomic::{AtomicBool, Ordering};

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

/// Install the SIGINT / SIGTERM handlers.
pub fn install() {
    let h = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
    // SAFETY: the handler is async-signal-safe (one atomic store).
    unsafe {
        libc::signal(libc::SIGINT, h);
        libc::signal(libc::SIGTERM, h);
    }
}

/// Whether a stop was requested.
pub fn stopped() -> bool {
    STOP.load(Ordering::SeqCst)
}
