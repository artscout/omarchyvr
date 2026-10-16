//! Offline end-to-end test without a headset or a GPU: a fake producer (doing what Monado's tap does: attach, publish
//! frames at the requested rate with monotonic timestamps, heartbeat) feeds the real `omarchyvr-record run`, which
//! encodes with x264 on the CPU (niced, 240 lines). Checks the mp4 with ffprobe. Skipped if ffmpeg/ffprobe/libx264
//! are missing.
use omarchyvr_record::shm::{self, Ring};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn have_tools() -> bool {
    let ok = |c: &str, a: &[&str]| Command::new(c).args(a).stdout(Stdio::piped()).stderr(Stdio::null()).output().map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).contains("libx264") || c == "ffprobe" && o.status.success()).unwrap_or(false);
    ok("ffmpeg", &["-hide_banner", "-encoders"]) && ok("ffprobe", &["-version"])
}

struct T {
    dir: PathBuf,
}

impl T {
    fn new(name: &str) -> T {
        let dir = std::env::temp_dir().join(format!("omarchyvr-record-it-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for d in ["session", "run", "out", "cfg", "omarchyvrd"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        T { dir }
    }
    fn ring(&self) -> PathBuf {
        self.dir.join("run/rec.shm")
    }
    fn recorder(&self, extra: &[&str]) -> Child {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_omarchyvr-record"));
        // XDG_RUNTIME_DIR below is a temp dir: point PulseAudio (PipeWire) at the real socket so the sound can be read
        if let Some(real) = std::env::var_os("XDG_RUNTIME_DIR") {
            let sock = Path::new(&real).join("pulse/native");
            if sock.exists() {
                cmd.env("PULSE_SERVER", format!("unix:{}", sock.display()));
            }
        }
        cmd.args(["run", "--encoder", "cpu", "--max-height", "240", "--fps", "30", "--dir"])
            .arg(self.dir.join("out"))
            .args(extra)
            .env("PIMAX_RECORD_SHM", self.ring())
            .env("PIMAX_VR_SESSION_DIR", self.dir.join("session"))
            .env("XDG_RUNTIME_DIR", self.dir.join("run"))
            .env("XDG_CONFIG_HOME", self.dir.join("cfg"))
            .env("PIMAXD_DIR", self.dir.join("omarchyvrd"))
            .env("PIMAX_RECORD_NO_MONADO_CHECK", "1")
            .env("PIMAX_RECORD_NO_NOTIFY", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    }
    fn status(&self) -> omarchyvr_record::status::Status {
        omarchyvr_record::status::Status::load(&self.dir.join("session/record.status")).unwrap_or_default()
    }
    fn mp4s(&self) -> Vec<PathBuf> {
        std::fs::read_dir(self.dir.join("out")).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "mp4")).collect()
    }
}

impl Drop for T {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Wait for the ring, attach, publish `secs` of frames at `fps` (moving bars), optionally dropping every 7th
/// frame as a busy tap would. Returns the ring so the caller decides when the producer "dies".
fn produce(path: &Path, secs: f64, fps: u32, drop_some: bool) -> Ring {
    let t = Instant::now();
    while !path.exists() {
        assert!(t.elapsed() < Duration::from_secs(5), "the recorder never created the ring");
        std::thread::sleep(Duration::from_millis(10));
    }
    let ring = Ring::open(path).unwrap();
    let h = ring.hdr();
    let (w, ht) = shm::pick_eye_extent((400, 360), h.max_height, h.scale_permille, h.eyes, h.slot_size).unwrap();
    ring.attach_producer(w, ht, "fake producer");
    let n = (secs * fps as f64) as u64;
    let t0 = shm::monotonic_ns();
    for i in 0..n {
        let ts = t0 + i * 1_000_000_000 / fps as u64;
        while shm::monotonic_ns() < ts {
            std::thread::sleep(Duration::from_millis(1));
        }
        ring.producer_heartbeat();
        if drop_some && i % 7 == 3 {
            continue;
        }
        ring.publish(w, ht, i, ts, &mut |b| {
            for (k, px) in b.chunks_mut(4).enumerate() {
                let x = (k as u32 % w + i as u32 * 4) % w;
                px.copy_from_slice(&[(x * 255 / w) as u8, 128, (255 - x * 255 / w) as u8, 255]);
            }
        });
    }
    ring
}

fn probe(p: &Path) -> (String, f64, u64) {
    let o = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-count_frames", "-show_entries", "stream=codec_name,nb_read_frames:format=duration", "-of", "default=nw=1"])
        .arg(p)
        .output()
        .unwrap();
    let t = String::from_utf8_lossy(&o.stdout).to_string();
    let get = |k: &str| t.lines().find_map(|l| l.strip_prefix(&format!("{k}="))).unwrap_or("").to_string();
    (get("codec_name"), get("duration").parse().unwrap_or(0.0), get("nb_read_frames").parse().unwrap_or(0))
}

fn wait_exit(c: &mut Child, secs: u64) -> i32 {
    let t = Instant::now();
    loop {
        if let Some(s) = c.try_wait().unwrap() {
            return s.code().unwrap_or(-1);
        }
        assert!(t.elapsed() < Duration::from_secs(secs), "the recorder did not exit");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn sigint_finishes_a_playable_constant_rate_mp4() {
    if !have_tools() {
        eprintln!("skipped: ffmpeg with libx264 and ffprobe needed");
        return;
    }
    let t = T::new("sigint");
    let mut rec = t.recorder(&[]);
    let ring = produce(&t.ring(), 3.0, 30, true);
    assert_eq!(t.status().state, "recording");
    // SIGINT as `omarchyvr record stop` sends it, while the producer is still attached
    unsafe { libc::kill(rec.id() as i32, libc::SIGINT) };
    assert_eq!(wait_exit(&mut rec, 30), 0);
    assert!(ring.stop_requested(), "the recorder told the tap to detach");
    let st = t.status();
    assert_eq!(st.state, "idle", "{st:?}");
    let files = t.mp4s();
    assert_eq!(files.len(), 1);
    let (codec, dur, frames) = probe(&files[0]);
    assert_eq!(codec, "h264");
    // 3 s at 30 fps; the dropped frames were filled by repeats (constant rate)
    // (the first ~0.3 s is lost to opening the sound input when audio is on)
    assert!((75..=92).contains(&frames), "{frames} frames");
    assert!((dur - 3.0).abs() < 0.5, "{dur} s");
    assert!(st.duplicated >= 10, "{st:?}");
    // default settings: 30 fps, and sound when the machine has an audio output to read (else a notice, never a failure)
    let streams = Command::new("ffprobe").args(["-v", "error", "-show_entries", "stream=codec_type,codec_name,avg_frame_rate", "-of", "csv=p=0"]).arg(&files[0]).output().unwrap();
    let streams = String::from_utf8_lossy(&streams.stdout).to_string();
    assert!(streams.contains("h264,video,30/1"), "{streams}");
    assert_eq!(streams.contains("aac"), !st.audio.is_empty(), "audio stream vs status {st:?}: {streams}");
    eprintln!("streams: {streams} audio={:?}", st.audio);
    assert!(!t.ring().exists(), "the ring file is removed");
    assert!(std::fs::read_to_string(t.dir.join("session/record.events")).unwrap().contains("saved "));
}

#[test]
fn monado_going_away_finishes_the_file_by_itself() {
    if !have_tools() {
        return;
    }
    let t = T::new("gone");
    let mut rec = t.recorder(&["--quality", "low"]);
    let ring = produce(&t.ring(), 1.5, 30, false);
    // the producer dies: no detach, its heartbeat just stops
    drop(ring);
    assert_eq!(wait_exit(&mut rec, 30), 0);
    let st = t.status();
    assert_eq!(st.state, "idle");
    assert!(st.note.contains("Monado stopped"), "{st:?}");
    let (codec, dur, _) = probe(&t.mp4s()[0]);
    assert_eq!(codec, "h264");
    assert!((dur - 1.5).abs() < 0.5, "{dur}");
}

#[test]
fn without_a_tap_it_gives_up_cleanly() {
    if !have_tools() {
        return;
    }
    let t = T::new("notap");
    let mut rec = t.recorder(&[]);
    assert_eq!(wait_exit(&mut rec, 15), 0, "a clean refusal is exit 0 (omarchyvrd does not restart it)");
    let st = t.status();
    assert_eq!(st.state, "error");
    assert!(st.note.contains("no recorder tap"), "{st:?}");
    assert!(t.mp4s().is_empty());
    assert!(!t.ring().exists());
}
