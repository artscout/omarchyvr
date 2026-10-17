//! `omarchyvr-record run`: the recorder process (a leaf component of the session, pid file `record.pid`).
//!
//! Never hurts the session: it only reads the shared-memory ring (Monado's tap drops frames instead of waiting), runs
//! at nice 10 with the lowest best-effort I/O priority, and every way out (SIGINT/SIGTERM, Monado gone, disk almost
//! full, encoder error) closes the encoder's input so the fragmented mp4 is finished and playable. A clean stop for
//! any of those reasons exits 0 (omarchyvrd then counts it as a user stop and does not restart it); only a real crash
//! is non-zero.
use crate::encode::{self, Available, Encoder, Ffmpeg};
use crate::settings::{self, Settings};
use crate::shm::{self, Request, Ring};
use crate::status::{self, Status};
use omarchyvr_components::model::GateLevel;
use omarchyvr_components::util::{unix_now, Roots};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

/// Frames per file second -> how many times each delivered frame is written (constant frame rate output).
#[derive(Debug, Clone, Default)]
pub struct Pacer {
    fps: u32,
    t0: Option<u64>,
    next: u64,
    pub duplicated: u64,
    pub early: u64,
    /// Seconds not represented in the file (gaps longer than [`Pacer::MAX_COPIES`] frames).
    pub lost_s: f64,
}

impl Pacer {
    /// A gap is filled with at most this many copies of the next frame; a longer gap is cut out.
    pub const MAX_COPIES: u64 = 3;

    pub fn new(fps: u32) -> Pacer {
        Pacer { fps, ..Pacer::default() }
    }

    /// How many times to write the frame shown at `ts_ns` (0: it came too early, skip it).
    pub fn copies(&mut self, ts_ns: u64) -> u64 {
        let t0 = *self.t0.get_or_insert(ts_ns);
        if ts_ns < t0 {
            self.early += 1;
            return 0;
        }
        let n = ((ts_ns - t0) as f64 * self.fps as f64 / 1e9).round() as u64;
        if n < self.next {
            self.early += 1;
            return 0;
        }
        let gap = n - self.next + 1;
        let c = gap.min(Pacer::MAX_COPIES);
        if gap > c {
            self.lost_s += (gap - c) as f64 / self.fps as f64;
        }
        self.duplicated += c - 1;
        self.next = n + 1;
        c
    }
}

/// The GPU gate as omarchyvrd latched it (fresh and ok if omarchyvrd never wrote one).
pub fn load_gate(r: &Roots) -> omarchyvr_components::gpu::Gate {
    let boot = omarchyvr_components::gpu::boot_id(&r.proc_);
    omarchyvr_components::gpu::Gate::load(&r.omarchyvrd_dir().join("gpu-gate"), &boot)
}

/// The effective GPU gate level.
pub fn gate_level(r: &Roots) -> GateLevel {
    load_gate(r).effective()
}

/// The refusal text starts with this; the app shows its "Open System page" button for such a note.
pub const GATE_BLOCK_PREFIX: &str = "Recording is off: the GPU safety check wants a reboot";

/// Why recording is refused at needs-reboot, in plain words (the newest evidence, and the way forward).
pub fn gate_block_message(g: &omarchyvr_components::gpu::Gate) -> String {
    let why: String = g.evidence.last().map(|e| e.trim().chars().take(140).collect()).unwrap_or_else(|| "no detail".into());
    let way = if g.kill_only() {
        "Reboot the PC, or acknowledge it on the System page once a VR session has run cleanly for 5 minutes."
    } else {
        "Reboot the PC: the GPU logged an error this boot."
    };
    format!("{GATE_BLOCK_PREFIX} ({why}). {way}")
}

/// Is monado-service running (pid file + comm)?
pub fn monado_running(r: &Roots) -> bool {
    omarchyvr_components::spec::spec("monado")
        .and_then(|s| s.pid)
        .and_then(|p| omarchyvr_components::procs::from_pid_file(r, &p))
        .is_some()
}

/// Free bytes on the filesystem of `dir`.
pub fn free_bytes(dir: &Path) -> Option<u64> {
    let c = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes()).ok()?;
    // SAFETY: statvfs writes into st.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    (unsafe { libc::statvfs(c.as_ptr(), &mut st) } == 0).then(|| st.f_bavail as u64 * st.f_frsize as u64)
}

/// Refuse to start below this; stop when the free space drops below [`STOP_FREE`].
pub const START_FREE: u64 = 2 << 30;
pub const STOP_FREE: u64 = 1 << 30;

/// A file name that does not exist yet.
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    if !p.exists() {
        return p;
    }
    let stem = name.trim_end_matches(".mp4");
    (2..).map(|i| dir.join(format!("{stem}-{i}.mp4"))).find(|p| !p.exists()).unwrap()
}

/// Lower our own CPU and I/O priority (the encoder child inherits nothing: it sets its own).
fn be_nice() {
    // SAFETY: plain syscalls on this process.
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, 0, 10);
        libc::syscall(libc::SYS_ioprio_set, 1, 0, (2 << 13) | 7);
    }
}

struct Ctx<'a> {
    session_dir: &'a Path,
    status_path: PathBuf,
    st: Status,
}

impl Ctx<'_> {
    fn save(&mut self) {
        self.st.updated_unix = unix_now();
        self.st.save(&self.status_path);
    }
    /// A clean refusal or stop: status error/idle, an event, a notification. Exit code 0.
    fn fail(&mut self, text: &str) -> i32 {
        self.st.state = "error".into();
        self.st.note = text.into();
        self.save();
        status::event(self.session_dir, "critical", text, true);
        0
    }
}

/// The recorder. Returns the exit code.
pub fn run(s: &Settings, r: &Roots) -> i32 {
    be_nice();
    // SAFETY: async-signal-safe handler that only stores an atomic.
    unsafe {
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
    }
    let session_dir = r.session_dir.clone();
    let _ = std::fs::create_dir_all(&session_dir);
    let mut cx = Ctx {
        session_dir: &session_dir,
        status_path: status::status_path(&session_dir),
        st: Status {
            state: "starting".into(),
            pid: std::process::id(),
            started_unix: unix_now(),
            eyes: s.eyes.name().into(),
            fps: s.fps,
            quality: s.quality.name().into(),
            ..Status::default()
        },
    };
    cx.save();

    // Preconditions.
    if std::env::var_os("PIMAX_RECORD_NO_MONADO_CHECK").is_none() && !monado_running(r) {
        return cx.fail("no VR session (monado-service is not running)");
    }
    let gate_state = load_gate(r);
    let gate = gate_state.effective();
    if gate == GateLevel::NeedsReboot {
        return cx.fail(&gate_block_message(&gate_state));
    }
    if gate == GateLevel::Caution {
        // today's behaviour: NVENC is skipped at caution, the CPU encoder records; say what unlocks the GPU encoder
        status::event(&session_dir, "notice", "The GPU safety check is at caution and not acknowledged: recording on the CPU. Acknowledge it on the System page to use the GPU encoder.", false);
    }
    let ffmpeg = encode::ffmpeg_path();
    let Some(avail) = Available::probe(&ffmpeg) else {
        return cx.fail(&format!("cannot run {} (install ffmpeg)", ffmpeg.display()));
    };
    let mut plan = match encode::plan(s.encoder, s.codec, &avail, gate == GateLevel::Ok) {
        Ok(p) => p,
        Err(e) => return cx.fail(&e),
    };
    let dir = s.out_dir(&r.home);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return cx.fail(&format!("cannot create {}: {e}", dir.display()));
    }
    if free_bytes(&dir).is_some_and(|f| f < START_FREE) {
        return cx.fail(&format!("less than 2 GiB free in {}", dir.display()));
    }

    // The ring, and a heartbeat that keeps going while the encoder write blocks.
    let (max_h, scale) = s.request();
    let req = Request::new(s.eyes.count(), s.fps, max_h, scale);
    let ring_path = shm::default_path(&r.runtime);
    let ring = match Ring::create(&ring_path, req) {
        Ok(x) => Arc::new(x),
        Err(e) => return cx.fail(&format!("cannot create {}: {e}", ring_path.display())),
    };
    let hb_quit = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(AtomicU64::new(shm::monotonic_ns()));
    let encoder_pid = Arc::new(AtomicU64::new(0));
    let hb = {
        let (ring, quit, progress, encoder_pid) = (ring.clone(), hb_quit.clone(), progress.clone(), encoder_pid.clone());
        std::thread::spawn(move || {
            while !quit.load(Ordering::SeqCst) {
                ring.heartbeat();
                // encoder watchdog: a write blocked for 20 s (or 5 s after a stop request) means ffmpeg hung
                let stalled = shm::monotonic_ns().saturating_sub(progress.load(Ordering::SeqCst)) as f64 / 1e9;
                let pid = encoder_pid.load(Ordering::SeqCst) as i32;
                if pid > 0 && (stalled > 20.0 || (STOP.load(Ordering::SeqCst) && stalled > 5.0)) {
                    // SAFETY: our own ffmpeg child; the blocked write then fails with EPIPE.
                    unsafe { libc::kill(pid, libc::SIGKILL) };
                    encoder_pid.store(0, Ordering::SeqCst);
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        })
    };
    let finish_ring = |ring: &Ring| {
        ring.request_stop();
        // give Monado's worker a moment to detach before the file goes away
        let t = Instant::now();
        while ring.producer_pid() != 0 && t.elapsed() < Duration::from_secs(1) {
            std::thread::sleep(Duration::from_millis(20));
        }
        hb_quit.store(true, Ordering::SeqCst);
    };

    // Wait for Monado's tap to attach.
    let t0 = Instant::now();
    while ring.producer_pid() == 0 {
        if STOP.load(Ordering::SeqCst) {
            finish_ring(&ring);
            let _ = hb.join();
            cx.st.state = "idle".into();
            cx.st.note = "stopped before the first frame".into();
            cx.save();
            return 0;
        }
        if t0.elapsed() > Duration::from_secs(6) {
            finish_ring(&ring);
            let _ = hb.join();
            return cx.fail("Monado did not attach: this monado-service has no recorder tap (install the build with comp_recorder, docs/RECORD.md)");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let (w, h) = ring.size();
    eprintln!("omarchyvr-record: tap attached: {} ({w}x{h})", ring.producer_note());

    let out = unique_path(&dir, &settings::file_name(unix_now() as i64));
    let ff_log = session_dir.join("record-ffmpeg.log");
    let mut enc_i = 0usize;
    // Sound: only what can be read right now goes in; a missing output or mic never stops the video.
    let mut es = s.clone();
    let mut audio_note = String::new();
    if es.audio {
        if let Err(e) = encode::probe_audio(&ffmpeg, encode::MONITOR_SRC) {
            es.audio = false;
            es.mic = false;
            audio_note = format!("recording without sound: the audio output cannot be read ({e})");
        } else if es.mic {
            if let Err(e) = encode::probe_audio(&ffmpeg, encode::MIC_SRC) {
                es.mic = false;
                audio_note = format!("the microphone cannot be read, recording the output sound only ({e})");
            }
        }
    }
    let spawn = |enc: Encoder| Ffmpeg::spawn(&ffmpeg, enc, &es, w, h, &out, &ff_log);
    let mut ff = match spawn(plan[0]) {
        Ok(f) => f,
        Err(e) => {
            finish_ring(&ring);
            let _ = hb.join();
            return cx.fail(&format!("cannot start ffmpeg: {e}"));
        }
    };
    encoder_pid.store(ff.pid() as u64, Ordering::SeqCst);
    cx.st.state = "recording".into();
    cx.st.file = out.display().to_string();
    cx.st.encoder = plan[0].name(s.codec).into();
    cx.st.audio = if !es.audio { "" } else if es.mic { "output+mic" } else { "output" }.into();
    if !audio_note.is_empty() {
        status::event(&session_dir, "notice", &audio_note, true);
    }
    cx.st.width = w;
    cx.st.height = h;
    cx.save();
    status::event(&session_dir, "notice", &format!("recording to {}", out.display()), true);

    let mut pacer = Pacer::new(s.fps);
    let mut reason = String::from("stopped");
    let mut critical = false;
    let mut last_status = Instant::now();
    let mut write_failed = false;
    loop {
        progress.store(shm::monotonic_ns(), Ordering::SeqCst);
        if STOP.load(Ordering::SeqCst) {
            break;
        }
        if ring.producer_pid() == 0 || ring.producer_age_s().is_some_and(|a| a > 3.0) {
            reason = "Monado stopped (or detached the tap)".into();
            break;
        }
        if let Some(how) = ff.exited().or_else(|| write_failed.then(|| "its input closed".to_string())) {
            // an encoder that dies at once (NVENC unavailable, ...): the next one in the plan, same file name
            if ff.started.elapsed() < Duration::from_secs(5) && enc_i + 1 < plan.len() {
                enc_i += 1;
                let _ = std::fs::remove_file(&out);
                status::event(&session_dir, "notice", &format!("{} failed ({how}), falling back to {}", ff.encoder.name(s.codec), plan[enc_i].name(s.codec)), false);
                match spawn(plan[enc_i]) {
                    Ok(f) => {
                        ff = f;
                        encoder_pid.store(ff.pid() as u64, Ordering::SeqCst);
                        write_failed = false;
                        cx.st.encoder = plan[enc_i].name(s.codec).into();
                        pacer = Pacer::new(s.fps);
                        continue;
                    }
                    Err(e) => {
                        reason = format!("cannot start ffmpeg: {e}");
                        critical = true;
                        break;
                    }
                }
            }
            reason = format!("the encoder stopped ({how}; see {})", ff_log.display());
            critical = true;
            break;
        }
        if last_status.elapsed() >= Duration::from_secs(1) {
            last_status = Instant::now();
            if free_bytes(&dir).is_some_and(|f| f < STOP_FREE) {
                reason = format!("the disk is almost full (less than 1 GiB free in {})", dir.display());
                critical = true;
                break;
            }
            let k = ring.counters();
            cx.st.frames = ff.frames;
            cx.st.dropped_tap = k.dropped_busy;
            cx.st.dropped_ring = k.dropped_full;
            cx.st.duplicated = pacer.duplicated;
            cx.save();
        }
        match ring.take() {
            Some(f) => {
                let n = pacer.copies(f.timestamp_ns);
                for _ in 0..n {
                    if ff.write(f.pixels()).is_err() {
                        write_failed = true;
                        break;
                    }
                }
            }
            None => std::thread::sleep(Duration::from_millis(2)),
        }
    }
    plan.truncate(enc_i + 1);

    // Finalize: the tap detaches, the encoder writes its last fragment.
    cx.st.state = "finalizing".into();
    cx.save();
    finish_ring(&ring);
    let frames = ff.frames;
    let how = ff.finish(Duration::from_secs(15));
    encoder_pid.store(0, Ordering::SeqCst);
    let _ = hb.join();
    let k = ring.counters();
    drop(ring);
    cx.st.frames = frames;
    cx.st.dropped_tap = k.dropped_busy;
    cx.st.dropped_ring = k.dropped_full;
    cx.st.duplicated = pacer.duplicated;
    let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    let secs = frames as f64 / s.fps as f64;
    let mut text = format!(
        "saved {} ({:.0} s, {:.1} MB; {} dropped by the tap, {} repeated){}",
        out.display(),
        secs,
        size as f64 / 1e6,
        k.dropped_busy + k.dropped_full,
        pacer.duplicated,
        if how == "ok" { String::new() } else { format!("; encoder: {how}") }
    );
    if reason != "stopped" {
        text = format!("{reason}: {text}");
    }
    if frames == 0 {
        let _ = std::fs::remove_file(&out);
        text = format!("{reason}: nothing recorded");
        cx.st.file.clear();
    }
    cx.st.state = if critical { "error".into() } else { "idle".into() };
    cx.st.note = if reason == "stopped" { String::new() } else { reason };
    cx.save();
    status::event(&session_dir, if critical { "critical" } else { "notice" }, &text, true);
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reboot_refusal_is_plain_and_says_the_way_forward() {
        use omarchyvr_components::gpu::Gate;
        let mut g = Gate::new("b");
        g.raise_kill(GateLevel::NeedsReboot, "monado-service was SIGKILLed (pid 1)");
        let m = gate_block_message(&g);
        assert!(m.starts_with(GATE_BLOCK_PREFIX) && m.contains("SIGKILLed") && m.contains("acknowledge it on the System page"), "{m}");
        assert!(!m.contains("pimax health"));
        let mut f = Gate::new("b");
        f.raise(GateLevel::NeedsReboot, "kernel: Xid 79");
        let m = gate_block_message(&f);
        assert!(m.contains("Xid 79") && !m.contains("acknowledge"), "{m}");
    }

    const MS: u64 = 1_000_000;

    #[test]
    fn steady_frames_are_written_once() {
        let mut p = Pacer::new(30);
        let c: Vec<u64> = (0..10).map(|i| p.copies(1_000 * MS + i * 33_333_333)).collect();
        assert!(c.iter().all(|&n| n == 1), "{c:?}");
        assert_eq!(p.duplicated, 0);
    }

    #[test]
    fn a_dropped_frame_is_filled_by_repeating_the_next() {
        let mut p = Pacer::new(30);
        assert_eq!(p.copies(0), 1);
        assert_eq!(p.copies(33 * MS), 1);
        // frame 2 dropped by the tap: frame 3 is written twice
        assert_eq!(p.copies(100 * MS), 2);
        assert_eq!(p.duplicated, 1);
        // a frame faster than the rate is skipped
        assert_eq!(p.copies(105 * MS), 0);
        assert_eq!(p.early, 1);
    }

    #[test]
    fn a_long_gap_is_cut_not_filled() {
        let mut p = Pacer::new(30);
        p.copies(0);
        assert_eq!(p.copies(10_000 * MS), Pacer::MAX_COPIES);
        assert!((p.lost_s - (300.0 - 3.0) / 30.0).abs() < 0.05, "{}", p.lost_s);
        assert_eq!(p.copies(10_033 * MS), 1);
    }

    #[test]
    fn unique_names() {
        let d = std::env::temp_dir().join(format!("omarchyvr-record-uniq-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let a = unique_path(&d, "pimax-x.mp4");
        std::fs::write(&a, "").unwrap();
        assert_eq!(unique_path(&d, "pimax-x.mp4"), d.join("pimax-x-2.mp4"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
