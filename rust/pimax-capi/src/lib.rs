//! Drop-in replacement for `libpimax` (`libpimax/include/pimax.h`).
//!
//! Same symbol names, struct layouts, callback signatures and return conventions (0 / length / `-errno`).
//! Differences from the C library, all deliberate and documented in `analysis/rust-port.md`:
//! * `pimax_controller_report.battery` is `byte46 & 0x7f` (percent); the C library returned the raw byte, whose
//!   bit 7 means "charging". `raw[46]` still holds the raw byte.
//! * `on_controller_status` is also called for the second 0x37 message of a packet (the C library ignored it).
//! * The keepalive is sent in the Windows wire form `58 04 39 00 RR 00` (C: `58 05 39 00 RR 00`).
//! * Extra exported symbol `pimax_open_with_fds` (not in pimax.h) for tests.
#![allow(non_camel_case_types, clippy::missing_safety_doc)]

use pimax::{Backend, Config, Handler, Link, Session, State};
// Re-exported so the `#[no_mangle]` pimax_hands_* functions stay in this library's staticlib/cdylib.
pub use pimax_hands;
// Same for the eye gaze session (pimax-eye/include/pimax_eye.h): the ABI lives in `eye`.
pub mod eye;
pub use pimax_eye;
use pimax_proto as proto;
use std::fs::File;
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::raw::{c_char, c_int, c_void};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub const PIMAX_NUM_IF: usize = 4;
pub const PIMAX_PATH_MAX: usize = 64;
pub const PIMAX_REPORT_SIZE: usize = 64;

pub const PIMAX_DISCONNECTED: c_int = 0;
pub const PIMAX_CONNECTED: c_int = 1;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct pimax_imu_sample {
    pub ts_us: u32,
    pub raw_accel: [i16; 3],
    pub raw_gyro: [i16; 3],
    pub accel_g: [f32; 3],
    pub gyro_dps: [f32; 3],
    pub ipd_mm: f32,
    pub sample_in_report: c_int,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct pimax_controller_report {
    pub slot: u8,
    pub ts_us: u32,
    pub stick_x: c_int,
    pub stick_y: c_int,
    pub trigger: u8,
    pub grip: u8,
    pub battery: u8,
    pub buttons: u32,
    pub touch: u8,
    pub raw_accel: [i16; 3],
    pub raw_gyro: [i16; 3],
    pub raw: [u8; PIMAX_REPORT_SIZE],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct pimax_controller_status {
    pub slot: u8,
    pub battery: u8,
    pub fw: [u8; 2],
    pub raw: [u8; PIMAX_REPORT_SIZE],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct pimax_callbacks {
    pub user: *mut c_void,
    pub on_state: Option<unsafe extern "C" fn(*mut c_void, c_int)>,
    pub on_imu: Option<unsafe extern "C" fn(*mut c_void, *const pimax_imu_sample)>,
    pub on_controller: Option<unsafe extern "C" fn(*mut c_void, *const pimax_controller_report)>,
    pub on_controller_status:
        Option<unsafe extern "C" fn(*mut c_void, *const pimax_controller_status)>,
    pub on_log: Option<unsafe extern "C" fn(*mut c_void, *const c_char)>,
    pub on_reply: Option<unsafe extern "C" fn(*mut c_void, *const u8)>,
}

/// Opaque handle.
pub struct pimax_t {
    session: Session,
    /// Set when pimax_close starts: the session thread then stops calling into the application, so a callback
    /// that needs a lock the closing thread holds (or state it is tearing down) cannot stall or race the join.
    closing: Arc<AtomicBool>,
}

fn to_imu(s: &proto::ImuSample) -> pimax_imu_sample {
    pimax_imu_sample {
        ts_us: s.ts_us,
        raw_accel: s.raw_accel,
        raw_gyro: s.raw_gyro,
        accel_g: s.accel_g,
        gyro_dps: s.gyro_dps,
        ipd_mm: s.ipd_mm,
        sample_in_report: s.sample_in_report,
    }
}

fn to_ctl(c: &proto::ControllerReport) -> pimax_controller_report {
    pimax_controller_report {
        slot: c.slot,
        ts_us: c.ts_us,
        stick_x: c.stick_x,
        stick_y: c.stick_y,
        trigger: c.trigger,
        grip: c.grip,
        battery: c.battery,
        buttons: c.buttons,
        touch: c.touch,
        raw_accel: c.raw_accel,
        raw_gyro: c.raw_gyro,
        raw: c.raw,
    }
}

struct CHandler(pimax_callbacks, Arc<AtomicBool>);
// SAFETY: the C side promises (pimax.h) that the callbacks and `user` may be used from the session thread.
unsafe impl Send for CHandler {}

impl CHandler {
    fn closing(&self) -> bool {
        self.1.load(Ordering::Acquire)
    }
}

impl Handler for CHandler {
    fn on_state(&mut self, s: State) {
        if self.closing() {
            return;
        }
        if let Some(f) = self.0.on_state {
            // SAFETY: callback supplied by the caller of pimax_open, called with its own `user`.
            unsafe {
                f(
                    self.0.user,
                    if s == State::Connected {
                        PIMAX_CONNECTED
                    } else {
                        PIMAX_DISCONNECTED
                    },
                )
            }
        }
    }
    fn on_imu(&mut self, s: &proto::ImuSample) {
        if self.closing() {
            return;
        }
        if let Some(f) = self.0.on_imu {
            let c = to_imu(s);
            // SAFETY: as above; `c` lives across the call.
            unsafe { f(self.0.user, &c) }
        }
    }
    fn on_controller(&mut self, r: &proto::ControllerReport) {
        if self.closing() {
            return;
        }
        if let Some(f) = self.0.on_controller {
            let c = to_ctl(r);
            // SAFETY: as above.
            unsafe { f(self.0.user, &c) }
        }
    }
    fn on_status(&mut self, s: &proto::ControllerStatus) {
        if self.closing() {
            return;
        }
        if let Some(f) = self.0.on_controller_status {
            let c = pimax_controller_status {
                slot: s.slot,
                battery: s.battery,
                fw: s.fw,
                raw: s.raw,
            };
            // SAFETY: as above.
            unsafe { f(self.0.user, &c) }
        }
    }
    fn on_log(&mut self, line: &str) {
        if self.closing() {
            return;
        }
        if let Some(f) = self.0.on_log {
            let mut buf = [0u8; 513];
            let n = line.len().min(511);
            buf[..n].copy_from_slice(&line.as_bytes()[..n]);
            // SAFETY: `buf` is NUL terminated and lives across the call.
            unsafe { f(self.0.user, buf.as_ptr().cast()) }
        }
    }
    fn on_reply(&mut self, r: &proto::Report) {
        if self.closing() {
            return;
        }
        if let Some(f) = self.0.on_reply {
            // SAFETY: `r` points at 64 valid bytes.
            unsafe { f(self.0.user, r.as_ptr()) }
        }
    }
}

fn set_nonblock(fd: c_int) {
    // SAFETY: fcntl on a descriptor the caller handed over.
    unsafe {
        let fl = libc::fcntl(fd, libc::F_GETFL);
        if fl >= 0 {
            libc::fcntl(fd, libc::F_SETFL, fl | libc::O_NONBLOCK);
        }
    }
}

fn set_errno(e: i32) {
    // SAFETY: __errno_location returns the calling thread's errno slot.
    unsafe { *libc::__errno_location() = e }
}

unsafe fn handler_from(cb: *const pimax_callbacks, closing: Arc<AtomicBool>) -> CHandler {
    // SAFETY: caller passes NULL or a valid pimax_callbacks.
    CHandler(
        if cb.is_null() {
            unsafe { std::mem::zeroed() }
        } else {
            unsafe { *cb }
        },
        closing,
    )
}

fn boxed(s: std::io::Result<Session>, closing: Arc<AtomicBool>) -> *mut pimax_t {
    match s {
        Ok(session) => Box::into_raw(Box::new(pimax_t { session, closing })),
        Err(e) => {
            set_errno(e.raw_os_error().unwrap_or(libc::ENOMEM));
            std::ptr::null_mut()
        }
    }
}

/// Fill `paths[n]` with "/dev/hidrawN" for USB interface n. Returns a bitmask of the interfaces found.
#[no_mangle]
pub unsafe extern "C" fn pimax_find_interfaces(paths: *mut [c_char; PIMAX_PATH_MAX]) -> c_int {
    let found = pimax::discovery::find_interfaces(&pimax::discovery::Roots::from_env());
    let mut mask = 0;
    for (i, p) in found.iter().enumerate() {
        let Some(p) = p else { continue };
        mask |= 1 << i;
        if paths.is_null() {
            continue;
        }
        let b = p.as_os_str().as_encoded_bytes();
        let n = b.len().min(PIMAX_PATH_MAX - 1);
        // SAFETY: caller provides char paths[4][64].
        let dst = unsafe { &mut *paths.add(i) };
        for (d, s) in dst.iter_mut().zip(b[..n].iter().chain(std::iter::once(&0))) {
            *d = *s as c_char;
        }
    }
    mask
}

/// Start the session thread. `cb` may be NULL. Returns NULL (errno set) on failure.
#[no_mangle]
pub unsafe extern "C" fn pimax_open(cb: *const pimax_callbacks) -> *mut pimax_t {
    // SAFETY: forwarded contract.
    let closing = Arc::new(AtomicBool::new(false));
    let h = unsafe { handler_from(cb, closing.clone()) };
    boxed(Session::open(Config::default(), h), closing)
}

struct FdBackend(Option<Link>);
impl Backend for FdBackend {
    fn connect(&mut self) -> std::io::Result<Option<Link>> {
        Ok(self.0.take())
    }
}

/// Not part of pimax.h: run a session on already-open descriptors (tests, fake devices).
/// `read_fds[n]` is the descriptor to read for interface n (negative = absent), `ctl_write_fd` receives commands.
/// The library takes ownership of all descriptors and makes them non-blocking.
#[doc(hidden)]
#[no_mangle]
pub unsafe extern "C" fn pimax_open_with_fds(
    cb: *const pimax_callbacks,
    read_fds: *const c_int,
    ctl_write_fd: c_int,
) -> *mut pimax_t {
    let mut reads: [Option<File>; PIMAX_NUM_IF] = Default::default();
    for (i, r) in reads.iter_mut().enumerate() {
        // SAFETY: caller passes four descriptors it hands over.
        let fd = unsafe { *read_fds.add(i) };
        if fd >= 0 {
            set_nonblock(fd);
            *r = Some(File::from(unsafe { OwnedFd::from_raw_fd(fd) }));
        }
    }
    set_nonblock(ctl_write_fd);
    // SAFETY: ownership transferred.
    let ctl = File::from(unsafe { OwnedFd::from_raw_fd(ctl_write_fd) });
    // SAFETY: forwarded contract.
    let closing = Arc::new(AtomicBool::new(false));
    let h = unsafe { handler_from(cb, closing.clone()) };
    boxed(
        Session::open_with_backend(Config::default(), FdBackend(Some(Link { reads, ctl })), h),
        closing,
    )
}

#[no_mangle]
pub unsafe extern "C" fn pimax_close(h: *mut pimax_t) {
    if !h.is_null() {
        // SAFETY: h came from pimax_open and is closed once.
        let b = unsafe { Box::from_raw(h) };
        b.closing.store(true, Ordering::Release);
        drop(b); // wakes the session thread (eventfd in the ppoll set) and joins it
    }
}

unsafe fn sess<'a>(h: *mut pimax_t) -> &'a Session {
    // SAFETY: caller passes a live handle.
    unsafe { &(*h).session }
}

#[no_mangle]
pub unsafe extern "C" fn pimax_is_connected(h: *mut pimax_t) -> c_int {
    c_int::from(unsafe { sess(h) }.is_connected())
}

/// 0 on success, -ETIMEDOUT otherwise.
#[no_mangle]
pub unsafe extern "C" fn pimax_wait_connected(h: *mut pimax_t, timeout_ms: c_int) -> c_int {
    match unsafe { sess(h) }.wait_connected(Duration::from_millis(timeout_ms.max(0) as u64)) {
        Ok(()) => 0,
        Err(_) => -libc::ETIMEDOUT,
    }
}

#[no_mangle]
pub unsafe extern "C" fn pimax_set_video_rate(h: *mut pimax_t, fps: u8) {
    unsafe { sess(h) }.set_video_rate(fps)
}

unsafe fn data_slice<'a>(data: *const c_void, len: usize) -> &'a [u8] {
    if data.is_null() || len == 0 {
        &[]
    } else {
        // SAFETY: caller guarantees `len` readable bytes.
        unsafe { std::slice::from_raw_parts(data.cast(), len) }
    }
}

#[no_mangle]
pub unsafe extern "C" fn pimax_send(
    h: *mut pimax_t,
    cmd: u8,
    opt: u8,
    data: *const c_void,
    len: usize,
) -> c_int {
    match unsafe { sess(h) }.send(cmd, opt, unsafe { data_slice(data, len) }) {
        Ok(()) => 0,
        Err(e) => e.to_errno(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn pimax_request(
    h: *mut pimax_t,
    cmd: u8,
    opt: u8,
    data: *const c_void,
    len: usize,
    reply: *mut u8,
    reply_cap: usize,
    timeout_ms: c_int,
) -> c_int {
    let t = Duration::from_millis(timeout_ms.max(0) as u64);
    match unsafe { sess(h) }.request(cmd, opt, unsafe { data_slice(data, len) }, t) {
        Ok(d) => {
            if !reply.is_null() && reply_cap > 0 {
                let n = d.len().min(reply_cap);
                // SAFETY: reply has reply_cap writable bytes.
                unsafe { std::ptr::copy_nonoverlapping(d.as_ptr(), reply, n) };
            }
            d.len() as c_int
        }
        Err(e) => e.to_errno(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn pimax_read_serial(
    h: *mut pimax_t,
    opt: u8,
    out: *mut c_char,
    cap: usize,
    timeout_ms: c_int,
) -> c_int {
    let mut r = [0u8; PIMAX_REPORT_SIZE];
    let arg = [0x1eu8];
    let n = unsafe {
        pimax_request(
            h,
            proto::cmd::SERIAL,
            opt,
            arg.as_ptr().cast(),
            1,
            r.as_mut_ptr(),
            r.len(),
            timeout_ms,
        )
    };
    if n < 0 {
        return n;
    }
    let n = n as usize;
    let mut sl = if n > 0 { r[0] as usize } else { 0 };
    if sl > n.saturating_sub(1) {
        sl = n.saturating_sub(1);
    }
    if sl >= cap {
        sl = cap.saturating_sub(1);
    }
    if cap > 0 && !out.is_null() {
        // SAFETY: out has cap writable bytes and sl < cap.
        unsafe {
            std::ptr::copy_nonoverlapping(r.as_ptr().add(1), out.cast::<u8>(), sl);
            *out.add(sl) = 0;
        }
    }
    sl as c_int
}

#[no_mangle]
pub unsafe extern "C" fn pimax_read_version(
    h: *mut pimax_t,
    opt: u8,
    out: *mut u8,
    cap: usize,
    timeout_ms: c_int,
) -> c_int {
    unsafe {
        pimax_request(
            h,
            proto::cmd::VERSION,
            opt,
            std::ptr::null(),
            0,
            out,
            cap,
            timeout_ms,
        )
    }
}

#[no_mangle]
pub unsafe extern "C" fn pimax_controller_attach(h: *mut pimax_t, slot: u8) -> c_int {
    match unsafe { sess(h) }.controller_attach(slot) {
        Ok(()) => 0,
        Err(e) => e.to_errno(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn pimax_video_prepare(h: *mut pimax_t, fps: u8) -> c_int {
    match unsafe { sess(h) }.video_prepare(fps) {
        Ok(()) => 0,
        Err(e) => e.to_errno(),
    }
}

/// Controller vibration from an OpenXR haptic value (amplitude 0..1, duration ns, -1 = minimum; frequency is not used
/// by the hardware). Queued for the session thread: `58 09 07 SS AMP DUR` at most once per 8 ms per hand, newest wins.
/// Returns 0 when queued, 1 for a stop (amplitude 0: nothing is sent, a waiting pulse is dropped), -EINVAL for a slot
/// other than 2/3.
#[no_mangle]
pub unsafe extern "C" fn pimax_haptic(
    h: *mut pimax_t,
    slot: u8,
    amplitude: f32,
    duration_ns: i64,
) -> c_int {
    let pulse = proto::haptic_from_openxr(amplitude, duration_ns);
    if !unsafe { sess(h) }.haptic(slot, pulse) {
        return -libc::EINVAL;
    }
    c_int::from(pulse.is_none())
}

/// Take a test pulse request (`$XDG_RUNTIME_DIR/pimax-haptic-request`, written by `pimax controllers buzz` / the app)
/// and queue it. Returns the slot (2/3) when one was sent, 0 when there was none, -EINVAL for a malformed request
/// (removed).
#[no_mangle]
pub unsafe extern "C" fn pimax_haptic_poll_request(h: *mut pimax_t) -> c_int {
    match pimax::haptics::take_request(&pimax::haptics::request_path()) {
        Ok(Some((slot, amp, ms))) => {
            unsafe { sess(h) }.haptic(slot, Some((amp, ms)));
            c_int::from(slot)
        }
        Ok(None) => 0,
        Err(_) => -libc::EINVAL,
    }
}

/// Vibration commands written and requests replaced before they went out (either pointer may be NULL).
#[no_mangle]
pub unsafe extern "C" fn pimax_haptic_counts(h: *mut pimax_t, sent: *mut u64, replaced: *mut u64) {
    let (s, r) = unsafe { sess(h) }.haptic_counts();
    if !sent.is_null() {
        // SAFETY: caller passes a writable u64 or NULL.
        unsafe { *sent = s };
    }
    if !replaced.is_null() {
        // SAFETY: as above.
        unsafe { *replaced = r };
    }
}

/// Returns 0 if the buffer is not an IMU report. `out` receives two samples.
#[no_mangle]
pub unsafe extern "C" fn pimax_decode_imu(r: *const u8, out: *mut pimax_imu_sample) -> c_int {
    // SAFETY: r points at 64 bytes.
    let rep: &proto::Report = unsafe { &*r.cast::<proto::Report>() };
    match proto::decode_imu(rep) {
        Some(s) => {
            // SAFETY: out points at two samples.
            unsafe {
                *out = to_imu(&s[0]);
                *out.add(1) = to_imu(&s[1]);
            }
            1
        }
        None => 0,
    }
}

/// Returns 0 if the buffer is not a controller report.
#[no_mangle]
pub unsafe extern "C" fn pimax_decode_controller(
    r: *const u8,
    out: *mut pimax_controller_report,
) -> c_int {
    // SAFETY: r points at 64 bytes.
    let rep: &proto::Report = unsafe { &*r.cast::<proto::Report>() };
    match proto::decode_controller(rep) {
        Some(c) => {
            // SAFETY: out points at one report.
            unsafe { *out = to_ctl(&c) };
            1
        }
        None => 0,
    }
}
