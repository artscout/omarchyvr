//! C ABI of the in-hand detector (`include/pimax_inhand.h`), see [`crate::inhand`]. Every function tolerates NULL;
//! one object per controller, calls serialised by an internal mutex (reports on the session thread, queries on the
//! compositor and the driver's attach thread).
//!
//! Status file `$XDG_RUNTIME_DIR/pimax-ctrl-inhand` (written by [`pimax_inhand_publish`] when it changes; the app's
//! Home page and `pimax-ctrltrack live` read it):
//! ```text
//! pid=1234
//! mode=auto
//! left=held
//! left_why=motion
//! right=down
//! right_why=no reports
//! ```
//! Run-time mode change: `$XDG_RUNTIME_DIR/pimax-ctrl-inhand-mode` with `auto`, `always` or `off` (the app, the CLI
//! `omarchyvr controllers use ...` and voice write it); [`pimax_inhand_poll_mode`] reads and deletes it.

#![allow(non_camel_case_types, clippy::missing_safety_doc)]

use crate::inhand::{Config, InHand, Mode};
use std::os::raw::{c_char, c_int};
use std::path::PathBuf;
use std::sync::Mutex;

pub struct PimaxInHand {
    d: Mutex<InHand>,
}

/// `PIMAX_INHAND_MODE_*`.
pub const MODE_AUTO: c_int = 0;
pub const MODE_ALWAYS: c_int = 1;
pub const MODE_OFF: c_int = 2;

fn mode_to_c(m: Mode) -> c_int {
    match m {
        Mode::Auto => MODE_AUTO,
        Mode::Always => MODE_ALWAYS,
        Mode::Off => MODE_OFF,
    }
}

fn mode_from_c(m: c_int) -> Mode {
    match m {
        MODE_ALWAYS => Mode::Always,
        MODE_OFF => Mode::Off,
        _ => Mode::Auto,
    }
}

fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// `$XDG_RUNTIME_DIR/pimax-ctrl-inhand`.
pub fn status_path() -> PathBuf {
    runtime_dir().join("pimax-ctrl-inhand")
}

/// `$XDG_RUNTIME_DIR/pimax-ctrl-inhand-mode`.
pub fn request_path() -> PathBuf {
    runtime_dir().join("pimax-ctrl-inhand-mode")
}

/// The mode from `PIMAX_CTRL_INHAND` (auto | always | off; unset or invalid = `default_mode`).
#[no_mangle]
pub unsafe extern "C" fn pimax_inhand_mode_from_env(default_mode: c_int) -> c_int {
    std::env::var("PIMAX_CTRL_INHAND")
        .ok()
        .and_then(|v| Mode::parse(&v))
        .map(mode_to_c)
        .unwrap_or(default_mode)
}

/// "auto" / "always" / "off".
#[no_mangle]
pub extern "C" fn pimax_inhand_mode_name(mode: c_int) -> *const c_char {
    match mode_from_c(mode) {
        Mode::Auto => c"auto".as_ptr(),
        Mode::Always => c"always".as_ptr(),
        Mode::Off => c"off".as_ptr(),
    }
}

/// A new detector (thresholds from [`Config::from_env`]). `hand` is only for the log.
#[no_mangle]
pub extern "C" fn pimax_inhand_create(_hand: c_int) -> *mut PimaxInHand {
    Box::into_raw(Box::new(PimaxInHand {
        d: Mutex::new(InHand::new(Config::from_env())),
    }))
}

#[no_mangle]
pub unsafe extern "C" fn pimax_inhand_destroy(h: *mut PimaxInHand) {
    if !h.is_null() {
        drop(Box::from_raw(h));
    }
}

/// Every controller report: the 64 raw bytes (IMU) and the decoded inputs, at host CLOCK_MONOTONIC `host_ns`.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn pimax_inhand_report(
    h: *mut PimaxInHand,
    host_ns: u64,
    raw: *const u8,
    trigger: u8,
    grip: u8,
    stick_x: c_int,
    stick_y: c_int,
    buttons: u32,
    touch: u8,
) {
    if h.is_null() || raw.is_null() {
        return;
    }
    let raw = &*(raw as *const [u8; 64]);
    if let Ok(mut d) = (*h).d.lock() {
        d.report(
            host_ns, raw, trigger, grip, stick_x, stick_y, buttons, touch,
        );
    }
}

/// 1 when the controller is in the hand at `now_ns`, 0 when it lies somewhere (or sends nothing).
#[no_mangle]
pub unsafe extern "C" fn pimax_inhand_held(h: *mut PimaxInHand, now_ns: u64) -> c_int {
    if h.is_null() {
        return 0;
    }
    (*h).d
        .lock()
        .map(|mut d| d.held(now_ns) as c_int)
        .unwrap_or(0)
}

fn state_line(h: *mut PimaxInHand, mode: Mode, now_ns: u64) -> (String, String) {
    if mode == Mode::Off {
        return ("off".into(), "switched off".into());
    }
    if h.is_null() {
        return ("down".into(), "no controller".into());
    }
    // SAFETY: caller contract (valid or NULL).
    let Ok(mut d) = (unsafe { &*h }).d.lock() else {
        return ("down".into(), "poisoned".into());
    };
    let held = d.held(now_ns);
    let why = d.last_change().0.name().to_string();
    match (mode, held) {
        (Mode::Always, true) => ("held".into(), why),
        (Mode::Always, false) => ("shown".into(), why),
        (_, true) => ("held".into(), why),
        (_, false) => ("down".into(), why),
    }
}

/// Write the status file when it changed (cheap to call often). Returns 1 when written.
#[no_mangle]
pub unsafe extern "C" fn pimax_inhand_publish(
    mode: c_int,
    left: *mut PimaxInHand,
    right: *mut PimaxInHand,
    now_ns: u64,
) -> c_int {
    static LAST: Mutex<String> = Mutex::new(String::new());
    let m = mode_from_c(mode);
    let (l, lw) = state_line(left, m, now_ns);
    let (r, rw) = state_line(right, m, now_ns);
    let text = format!(
        "pid={}\nmode={}\nleft={l}\nleft_why={lw}\nright={r}\nright_why={rw}\n",
        std::process::id(),
        m.name()
    );
    let Ok(mut last) = LAST.lock() else { return 0 };
    if *last == text {
        return 0;
    }
    let path = status_path();
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, &text).is_ok() && std::fs::rename(&tmp, &path).is_ok() {
        *last = text;
        return 1;
    }
    0
}

/// A mode change asked for at run time (`$XDG_RUNTIME_DIR/pimax-ctrl-inhand-mode`): returns the new mode and
/// deletes the request, or -1 when there is none (an invalid request is deleted and ignored).
#[no_mangle]
pub unsafe extern "C" fn pimax_inhand_poll_mode() -> c_int {
    let p = request_path();
    let Ok(s) = std::fs::read_to_string(&p) else {
        return -1;
    };
    let _ = std::fs::remove_file(&p);
    Mode::parse(&s).map(mode_to_c).unwrap_or(-1)
}

/// Forget a stale request (call once at start, before the first poll).
#[no_mangle]
pub unsafe extern "C" fn pimax_inhand_clear_request() {
    let _ = std::fs::remove_file(request_path());
}

/// One line ("held (motion)", "down (still)") into `buf`; returns the length written.
#[no_mangle]
pub unsafe extern "C" fn pimax_inhand_describe(
    h: *mut PimaxInHand,
    now_ns: u64,
    buf: *mut c_char,
    len: usize,
) -> usize {
    if buf.is_null() || len == 0 {
        return 0;
    }
    let (s, w) = state_line(h, Mode::Auto, now_ns);
    let line = format!("{s} ({w})");
    let n = line.len().min(len - 1);
    std::ptr::copy_nonoverlapping(line.as_ptr(), buf as *mut u8, n);
    *buf.add(n) = 0;
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_is_harmless() {
        unsafe {
            pimax_inhand_report(std::ptr::null_mut(), 0, std::ptr::null(), 0, 0, 0, 0, 0, 0);
            assert_eq!(pimax_inhand_held(std::ptr::null_mut(), 0), 0);
            pimax_inhand_destroy(std::ptr::null_mut());
            assert_eq!(
                pimax_inhand_describe(std::ptr::null_mut(), 0, std::ptr::null_mut(), 0),
                0
            );
        }
    }

    #[test]
    fn a_button_press_shows_the_controller() {
        let h = pimax_inhand_create(0);
        let raw = [0u8; 64];
        unsafe {
            pimax_inhand_report(h, 1_000, raw.as_ptr(), 0, 0, 0, 0, 0, 0);
            assert_eq!(pimax_inhand_held(h, 2_000), 0);
            pimax_inhand_report(
                h,
                4_000_000,
                raw.as_ptr(),
                0,
                0,
                0,
                0,
                crate::report::BTN_A,
                0,
            );
            assert_eq!(pimax_inhand_held(h, 5_000_000), 1);
            let mut b = [0 as c_char; 64];
            let n = pimax_inhand_describe(h, 5_000_000, b.as_mut_ptr(), b.len());
            let s = std::ffi::CStr::from_ptr(b.as_ptr()).to_str().unwrap();
            assert_eq!((n, s), (12, "held (input)"));
            pimax_inhand_destroy(h);
        }
    }

    #[test]
    fn mode_names_round_trip() {
        for m in [MODE_AUTO, MODE_ALWAYS, MODE_OFF] {
            let s = unsafe { std::ffi::CStr::from_ptr(pimax_inhand_mode_name(m)) };
            assert_eq!(mode_to_c(Mode::parse(s.to_str().unwrap()).unwrap()), m);
        }
    }
}
