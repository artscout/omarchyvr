//! The encoder: an `ffmpeg` child fed raw RGBA frames on stdin, writing a fragmented mp4 (playable at any moment,
//! even if everything is killed). NVENC when available, libx264/libx265 on the CPU otherwise. The child runs at
//! nice 10 with idle-ish I/O priority so it never competes with the VR session.
use crate::settings::{Codec, EncoderPref, Quality, Settings};
use std::io::{self, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoder {
    Nvenc,
    Cpu,
}

impl Encoder {
    pub fn name(self, codec: Codec) -> &'static str {
        match (self, codec) {
            (Encoder::Nvenc, Codec::H264) => "h264_nvenc",
            (Encoder::Nvenc, Codec::Hevc) => "hevc_nvenc",
            (Encoder::Cpu, Codec::H264) => "libx264",
            (Encoder::Cpu, Codec::Hevc) => "libx265",
        }
    }
}

/// What `ffmpeg -encoders` offers (probed once per recording).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Available {
    pub h264_nvenc: bool,
    pub hevc_nvenc: bool,
    pub libx264: bool,
    pub libx265: bool,
}

impl Available {
    pub fn parse(encoders: &str) -> Available {
        let has = |n: &str| encoders.lines().any(|l| l.split_whitespace().nth(1) == Some(n));
        Available { h264_nvenc: has("h264_nvenc"), hevc_nvenc: has("hevc_nvenc"), libx264: has("libx264"), libx265: has("libx265") }
    }
    pub fn probe(ffmpeg: &Path) -> Option<Available> {
        let out = Command::new(ffmpeg).args(["-hide_banner", "-encoders"]).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        Some(Available::parse(&String::from_utf8_lossy(&out.stdout)))
    }
    fn nvenc(&self, c: Codec) -> bool {
        match c {
            Codec::H264 => self.h264_nvenc,
            Codec::Hevc => self.hevc_nvenc,
        }
    }
    fn cpu(&self, c: Codec) -> bool {
        match c {
            Codec::H264 => self.libx264,
            Codec::Hevc => self.libx265,
        }
    }
}

/// The encoders to try, in order. `gpu_ok`: the GPU gate is ok (at caution or worse, NVENC is not used: it is a GPU
/// client too).
pub fn plan(pref: EncoderPref, codec: Codec, avail: &Available, gpu_ok: bool) -> Result<Vec<Encoder>, String> {
    let mut v = Vec::new();
    let nv = avail.nvenc(codec) && gpu_ok;
    match pref {
        EncoderPref::Nvenc if !gpu_ok => return Err("the GPU safety gate is not ok: NVENC is not used (try --encoder cpu)".into()),
        EncoderPref::Nvenc if !avail.nvenc(codec) => return Err(format!("ffmpeg has no {}", Encoder::Nvenc.name(codec))),
        EncoderPref::Nvenc => v.push(Encoder::Nvenc),
        EncoderPref::Cpu => {}
        EncoderPref::Auto if nv => v.push(Encoder::Nvenc),
        EncoderPref::Auto => {}
    }
    if pref != EncoderPref::Nvenc && avail.cpu(codec) {
        v.push(Encoder::Cpu);
    }
    if v.is_empty() {
        return Err(format!("ffmpeg has neither {} nor {}", Encoder::Nvenc.name(codec), Encoder::Cpu.name(codec)));
    }
    Ok(v)
}

/// The quality knob: constant quality (NVENC `-cq`, x264/x265 `-crf`) plus a bitrate ceiling.
pub fn quality_args(enc: Encoder, codec: Codec, q: Quality) -> Vec<String> {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    match enc {
        Encoder::Nvenc => {
            let (cq, max) = match (codec, q) {
                (Codec::H264, Quality::Low) => ("30", "8M"),
                (Codec::H264, Quality::Medium) => ("25", "20M"),
                (Codec::H264, Quality::High) => ("20", "50M"),
                (Codec::Hevc, Quality::Low) => ("32", "6M"),
                (Codec::Hevc, Quality::Medium) => ("27", "15M"),
                (Codec::Hevc, Quality::High) => ("22", "40M"),
            };
            s(&["-preset", "p4", "-tune", "hq", "-rc", "vbr", "-cq", cq, "-b:v", "0", "-maxrate", max, "-bufsize", max])
        }
        Encoder::Cpu => {
            let crf = match (codec, q) {
                (Codec::H264, Quality::Low) => "28",
                (Codec::H264, Quality::Medium) => "23",
                (Codec::H264, Quality::High) => "19",
                (Codec::Hevc, Quality::Low) => "30",
                (Codec::Hevc, Quality::Medium) => "26",
                (Codec::Hevc, Quality::High) => "22",
            };
            let mut v = s(&["-preset", "veryfast", "-crf", crf, "-threads", "4"]);
            if codec == Codec::Hevc {
                v.extend(s(&["-x265-params", "log-level=error"]));
            }
            v
        }
    }
}

/// PulseAudio source names (PipeWire's pulse server understands them): what the default output plays, and the default mic.
pub const MONITOR_SRC: &str = "@DEFAULT_MONITOR@";
pub const MIC_SRC: &str = "@DEFAULT_SOURCE@";

/// The audio inputs (ffmpeg inputs 1 and 2): the output's monitor, then the mic. Empty without audio.
pub fn audio_inputs(s: &Settings) -> Vec<String> {
    let mut a = Vec::new();
    if !s.audio {
        return a;
    }
    let mut src = vec![MONITOR_SRC];
    if s.mic {
        src.push(MIC_SRC);
    }
    for d in src {
        a.extend(["-thread_queue_size", "1024", "-f", "pulse", "-i", d].iter().map(|x| x.to_string()));
    }
    a
}

/// Stream mapping and the audio codec: AAC 160k stereo 48 kHz (both sources mixed at full level, not halved).
pub fn audio_output(s: &Settings) -> Vec<String> {
    let v = |x: &[&str]| x.iter().map(|y| y.to_string()).collect::<Vec<_>>();
    if !s.audio {
        return v(&["-map", "0:v:0", "-an"]);
    }
    let mut a = if s.mic {
        v(&["-filter_complex", "[1:a][2:a]amix=inputs=2:duration=longest:normalize=0,aresample=async=1000,apad[a]", "-map", "0:v:0", "-map", "[a]"])
    } else {
        v(&["-af", "aresample=async=1000,apad", "-map", "0:v:0", "-map", "1:a:0"])
    };
    a.extend(v(&["-c:a", "aac", "-b:a", "160k", "-ar", "48000", "-ac", "2", "-shortest"]));
    a
}

/// A 0.3 s test read of a PulseAudio source: Ok, or why it cannot be recorded.
pub fn probe_audio(ffmpeg: &Path, src: &str) -> Result<(), String> {
    let mut child = Command::new(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-f", "pulse", "-i", src, "-t", "0.3", "-f", "null", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let t = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) if st.success() => return Ok(()),
            Ok(Some(_)) => {
                let mut err = String::new();
                if let Some(mut e) = child.stderr.take() {
                    let _ = io::Read::read_to_string(&mut e, &mut err);
                }
                return Err(err.lines().last().unwrap_or("ffmpeg failed").trim().to_string());
            }
            Ok(None) if t.elapsed() > Duration::from_secs(4) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("timed out".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// The full ffmpeg command line for raw RGBA `w`x`h` frames at `fps` on stdin -> `out`.
pub fn ffmpeg_args(enc: Encoder, s: &Settings, w: u32, h: u32, out: &Path) -> Vec<String> {
    let mut a: Vec<String> = ["-hide_banner", "-loglevel", "warning", "-nostdin", "-y"].iter().map(|x| x.to_string()).collect();
    a.extend(
        [
            "-f".into(),
            "rawvideo".into(),
            "-pix_fmt".into(),
            "rgba".into(),
            "-video_size".into(),
            format!("{w}x{h}"),
            "-framerate".into(),
            s.fps.to_string(),
            "-i".into(),
            "pipe:0".into(),
        ]
        .into_iter(),
    );
    a.extend(audio_inputs(s));
    a.extend(
        [
            "-c:v".into(),
            enc.name(s.codec).into(),
        ]
        .into_iter(),
    );
    a.extend(quality_args(enc, s.codec, s.quality));
    a.extend(audio_output(s));
    a.extend(
        [
            "-pix_fmt".into(),
            "yuv420p".into(),
            "-g".into(),
            (s.fps * 2).to_string(),
            // HEVC in mp4 must be tagged hvc1 for most players
            "-tag:v".into(),
            if s.codec == Codec::Hevc { "hvc1".into() } else { "avc1".into() },
            "-movflags".into(),
            "+frag_keyframe+empty_moov+default_base_moof".into(),
            "-f".into(),
            "mp4".into(),
            out.to_string_lossy().into_owned(),
        ]
        .into_iter(),
    );
    a
}

/// `ffmpeg` from `$PIMAX_FFMPEG` or the PATH.
pub fn ffmpeg_path() -> PathBuf {
    std::env::var_os("PIMAX_FFMPEG").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| "ffmpeg".into())
}

/// A running ffmpeg.
pub struct Ffmpeg {
    child: Child,
    stdin: Option<ChildStdin>,
    pub started: Instant,
    pub encoder: Encoder,
    pub out: PathBuf,
    pub frames: u64,
    /// The sound input never ends by itself: `finish` ends ffmpeg with SIGINT soon after the video is in.
    audio: bool,
}

impl Ffmpeg {
    pub fn spawn(ffmpeg: &Path, enc: Encoder, s: &Settings, w: u32, h: u32, out: &Path, log: &Path) -> io::Result<Ffmpeg> {
        let logf = std::fs::OpenOptions::new().create(true).append(true).open(log)?;
        let mut cmd = Command::new(ffmpeg);
        cmd.args(ffmpeg_args(enc, s, w, h, out)).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::from(logf));
        // SAFETY: setpriority/ioprio_set/prctl are async-signal-safe syscalls.
        unsafe {
            cmd.pre_exec(|| {
                libc::setpriority(libc::PRIO_PROCESS, 0, 10);
                // IOPRIO_CLASS_BE (2) level 7: the lowest best-effort I/O priority
                libc::syscall(libc::SYS_ioprio_set, 1, 0, (2 << 13) | 7);
                // die with the recorder (a crashed recorder must not leave an encoder behind)
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                Ok(())
            });
        }
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take();
        Ok(Ffmpeg { child, stdin, started: Instant::now(), encoder: enc, out: out.to_path_buf(), frames: 0, audio: s.audio })
    }

    /// Write one frame (blocks while ffmpeg is behind: the shared-memory ring then fills and the tap drops frames,
    /// the compositor never waits).
    pub fn write(&mut self, rgba: &[u8]) -> io::Result<()> {
        let w = self.stdin.as_mut().ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "closed"))?;
        w.write_all(rgba)?;
        self.frames += 1;
        Ok(())
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Exited already? (`Some(status text)`)
    pub fn exited(&mut self) -> Option<String> {
        self.child.try_wait().ok().flatten().map(|s| s.to_string())
    }

    /// Close stdin so ffmpeg writes the last fragment, wait up to `grace`; then SIGINT, then SIGKILL (the file
    /// stays playable up to the last complete fragment either way). Returns how it ended.
    pub fn finish(mut self, grace: Duration) -> String {
        drop(self.stdin.take());
        let t0 = Instant::now();
        let mut sent_int = false;
        // a live sound input never reaches its end: give the video 2 s to drain, then SIGINT (ffmpeg flushes and closes the file)
        let grace = if self.audio { grace.min(Duration::from_secs(2)) } else { grace };
        loop {
            if let Ok(Some(st)) = self.child.try_wait() {
                return if st.success() { "ok".into() } else { st.to_string() };
            }
            if t0.elapsed() > grace && !sent_int {
                // SAFETY: our own child.
                unsafe { libc::kill(self.child.id() as i32, libc::SIGINT) };
                sent_int = true;
            }
            if t0.elapsed() > grace + Duration::from_secs(5) {
                let _ = self.child.kill();
                let _ = self.child.wait();
                return "killed after the grace (the file is playable up to its last fragment)".into();
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENCODERS: &str = " V....D h264_nvenc           NVIDIA NVENC H.264 encoder (codec h264)\n V....D libx264              libx264 H.264\n V....D hevc_nvenc  x\n";

    #[test]
    fn parses_the_encoder_list() {
        let a = Available::parse(ENCODERS);
        assert_eq!(a, Available { h264_nvenc: true, hevc_nvenc: true, libx264: true, libx265: false });
    }

    #[test]
    fn auto_prefers_nvenc_with_a_cpu_fallback() {
        let a = Available::parse(ENCODERS);
        assert_eq!(plan(EncoderPref::Auto, Codec::H264, &a, true).unwrap(), vec![Encoder::Nvenc, Encoder::Cpu]);
        // GPU gate not ok: never NVENC automatically
        assert_eq!(plan(EncoderPref::Auto, Codec::H264, &a, false).unwrap(), vec![Encoder::Cpu]);
        assert!(plan(EncoderPref::Nvenc, Codec::H264, &a, false).is_err());
        assert_eq!(plan(EncoderPref::Cpu, Codec::H264, &a, true).unwrap(), vec![Encoder::Cpu]);
        // no libx265: hevc auto is NVENC only, cpu is an error
        assert_eq!(plan(EncoderPref::Auto, Codec::Hevc, &a, true).unwrap(), vec![Encoder::Nvenc]);
        assert!(plan(EncoderPref::Cpu, Codec::Hevc, &a, true).is_err());
        assert!(plan(EncoderPref::Auto, Codec::H264, &Available::default(), true).is_err());
    }

    #[test]
    fn quality_maps_to_cq_and_crf() {
        let cq = |q| quality_args(Encoder::Nvenc, Codec::H264, q);
        let pos = |v: &Vec<String>, k: &str| v[v.iter().position(|x| x == k).unwrap() + 1].clone();
        assert_eq!(pos(&cq(Quality::Low), "-cq"), "30");
        assert_eq!(pos(&cq(Quality::Medium), "-cq"), "25");
        assert_eq!(pos(&cq(Quality::High), "-cq"), "20");
        assert_eq!(pos(&quality_args(Encoder::Cpu, Codec::H264, Quality::Medium), "-crf"), "23");
        assert_eq!(pos(&quality_args(Encoder::Cpu, Codec::Hevc, Quality::High), "-crf"), "22");
    }

    #[test]
    fn the_command_line_writes_a_fragmented_mp4_from_raw_rgba() {
        let s = Settings { fps: 60, ..Settings::default() };
        let a = ffmpeg_args(Encoder::Nvenc, &s, 1168, 1080, Path::new("/v/x.mp4"));
        let j = a.join(" ");
        assert!(j.contains("-f rawvideo -pix_fmt rgba -video_size 1168x1080 -framerate 60 -i pipe:0"), "{j}");
        assert!(j.contains("-c:v h264_nvenc"), "{j}");
        assert!(j.contains("-movflags +frag_keyframe+empty_moov+default_base_moof -f mp4 /v/x.mp4"), "{j}");
        assert!(j.contains("-g 120"), "{j}");
        assert!(j.contains("-pix_fmt yuv420p"), "{j}");
        // sound is on by default: the output's monitor mapped as the audio stream, AAC
        assert!(j.contains("-f pulse -i @DEFAULT_MONITOR@"), "{j}");
        assert!(j.contains("-map 0:v:0 -map 1:a:0 -c:a aac -b:a 160k -ar 48000 -ac 2 -shortest"), "{j}");
        assert!(!j.contains("-an"), "{j}");
    }

    #[test]
    fn audio_off_and_mic_on_change_the_inputs_and_the_mapping() {
        let off = ffmpeg_args(Encoder::Cpu, &Settings { audio: false, ..Settings::default() }, 640, 480, Path::new("/v/x.mp4")).join(" ");
        assert!(!off.contains("pulse") && off.contains("-map 0:v:0 -an"), "{off}");
        let mic = ffmpeg_args(Encoder::Cpu, &Settings { mic: true, ..Settings::default() }, 640, 480, Path::new("/v/x.mp4")).join(" ");
        assert!(mic.contains("-i @DEFAULT_MONITOR@") && mic.contains("-i @DEFAULT_SOURCE@"), "{mic}");
        assert!(mic.contains("amix=inputs=2") && mic.contains("-map [a]"), "{mic}");
        // the mic alone never exists: it is a mix with the output
        let only_mic = audio_inputs(&Settings { audio: false, mic: true, ..Settings::default() });
        assert!(only_mic.is_empty());
    }

    #[test]
    fn the_default_high_quality_is_cq_20_with_a_50m_ceiling() {
        let q = quality_args(Encoder::Nvenc, Codec::H264, Settings::default().quality).join(" ");
        assert!(q.contains("-cq 20") && q.contains("-maxrate 50M"), "{q}");
    }
}
