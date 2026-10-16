# Recording (basic)

`pimax record` records what the headset shows into `~/Videos/pimax/pimax-YYYYmmdd-HHMMSS.mp4`. You can record one
eye (the default) or both eyes side by side, at 30, 60 or 72 fps, in low, medium or high quality. **Defaults
(2026-10-16): 30 fps, high quality, with sound.**

```
pimax record start [--eyes left|both] [--fps 30|60|72] [--quality low|medium|high]
                   [--scale F] [--max-height N] [--codec h264|hevc] [--encoder auto|nvenc|cpu] [--dir DIR] [--save]
pimax record stop        # finishes the file (always playable) and prints where it is
pimax record status
```

- **Defaults** (30 fps, high, sound on, microphone off) come from `~/.config/pimax/record.toml`, a plain `key = value` file: `eyes`, `fps`, `quality`, `scale`,
  `max_height`, `codec`, `encoder`, `dir`, `audio` and `mic`. A file from before `audio` existed that says medium quality takes
  the new default (high) once; after that the file keeps what you chose. Use `--save` to write the given flags there; the app's Record card writes
  there too.
- **Eyes:**
  - `left` records the left eye after layer composition and before the lens distortion. It is "what you see", without
    the barrel warp.
  - `both` puts the two eyes side by side.
- **Size:** each eye is fit into `max_height` (default 1080), keeping its aspect ratio. `scale` (0.1 to 1.0) takes a
  fraction of the eye's own resolution instead; `max_height` still caps it.
- **Quality** (default high): NVENC uses `-cq` 30, 25 or 20 for low, medium and high (HEVC: 32, 27, 22) with a bitrate ceiling. The
  CPU encoder (x264, or x265 for HEVC) uses `-crf` 28, 23 or 19 (HEVC: 30, 26, 22).
- **Encoder:** `auto` uses NVENC (`h264_nvenc` or `hevc_nvenc`) when ffmpeg has it and the GPU gate is ok. Otherwise
  it uses libx264 on the CPU. If NVENC dies within 5 s of starting, the recorder falls back to the CPU encoder and
  keeps the same file name.
- **Sound** (`--audio on|off`, default on; the app's "Sound" switch): what you hear in the headset, which is the monitor of the
  default PulseAudio/PipeWire output (`@DEFAULT_MONITOR@`), mixed down to AAC 160 kbit/s, 48 kHz stereo in the same mp4.
  - **Microphone** (`--mic on|off`, default off; the "Microphone" switch): the default input (`@DEFAULT_SOURCE@`) is
    mixed in with the output at full level (`amix`). Off by default so a recording never carries your voice unless you
    choose it.
  - Both sources are test-read for 0.3 s before the recording starts. If the output cannot be read, the video is
    recorded without sound; if only the mic cannot, the output sound is recorded alone. Either way a notice says so
    and VR is never affected. The first ~0.3 s of a recording with sound is not captured (ffmpeg opens the sound
    input first); the sound is padded with silence so it never cuts the video short.
  - ffmpeg ends itself when the video ends (`-shortest`); `pimax record status` shows `sound: output` or `output+mic`.
- **Voice** (works in Work and Play; the same start and stop as the app's buttons, with the saved settings):
  "Max, start recording", "record this", "begin recording", "record the video" start it and answer **Recording**;
  "Max, stop recording", "stop the recording", "end the recording" stop it and answer **Saved: pimax-YYYYmmdd-HHMMSS.mp4**
  (or, in plain words, why nothing was saved). "Stop recording" never stops the VR session: that needs a stop verb plus
  a VR word (`Actor::consent`), and any recording word blocks it. The voice commands are `pimax record start --brief`
  and `pimax record stop --brief`. A failure (for example "no VR session") is spoken in plain words and VR keeps running.

## Record card on the Recording tab (normal mode)

The Record cards have their own "Recording" page in the app's sidebar (they were on Home before 2026-10-16). The separate
"Video recording ... coming soon" placeholder row stays hidden: it was only a stub, and this card is the real thing.

- **Video** (Eyes, FPS, Quality, Sound, Microphone, Start/Stop recording): the section above. Needs a running session.
- **Record tracking** (Scenario, Length, Record tracking / Stop, Open folder): one folder per take for comparing with
  Windows. It never writes into the session and never blocks VR: it only reads, at `nice 19`.

```
pimax record tracking start [--scenario S] [--seconds N]   N = 0 or none: until stop
pimax record tracking stop      # ends the camera file cleanly (SIGINT), copies the logs
pimax record tracking status
```

Scenarios: `on-desk`, `worn-still`, `look-around`, `walk-room`, `fast-turns`, `controllers-in-out`, `play-session`,
`custom`. Takes go to `~/.local/share/pimax/tracking-recordings/YYYYmmdd-HHMMSS-<scenario>/`:

| File | What |
|---|---|
| `cameras.pxcam` | both cameras, every frame with its MCU timestamp (`pimax-cam record`, the `~/src/cam-data` format; replay with `run_slam.sh`/`pimax-cam broker --source`). From the camera broker when it runs; the free camera when no session runs; **not recorded** when a session holds the camera without a broker (`take.txt` says so). |
| `monado.log`, `ctrltrack.log`, `ctrl-imu-{left,right}.csv` | only what the session wrote during the take (csv keeps its header and whole lines only) |
| `head-imu.csv` | head IMU (1 kHz, as fed to the 6DoF tracker) + tracker pose times, written by a 6DoF session (`PIMAX_HEAD_IMU_LOG`, from 2026-10-16 builds). `pimax-slam take-euroc TAKE` turns cameras + this into a EuRoC dataset for `run_slam.sh replay` (Basalt offline); `pimax-slam take-head TAKE` works without it (head speed from the images). |
| `session.txt`, `start-argv.txt`, `perf-start.txt`, `perf-end.txt` | session mode and flags, performance stats at both ends |
| `take.txt` | scenario, times, what was and was not captured |

A timed take finishes itself: the app (or `status`) collects the logs once the camera recorder has ended. State is
`$XDG_RUNTIME_DIR/pimax-record-tracking.state`. Monado owns the headset's HID interface, so the head IMU comes from
the session itself: its 6DoF output logs every IMU sample to `head-imu.csv` in the session dir (a writer thread,
never blocks the IMU). The session's IMU logs (head and controllers) write for their first 10 minutes and then only
while `$XDG_RUNTIME_DIR/pimax-imu-log.on` exists, which the recorder creates at the start of a take and removes when it
collects the logs, so a take late in a session still has them. Takes recorded before 2026-10-16 have no head IMU. The
EuRoC SLAM recording (`PIMAX_SLAM_RECORD`) is set when Monado starts, so it cannot be switched on mid-session.

## Design: a tap inside the compositor, an encoder outside it

### Options considered

| Option | Verdict |
|---|---|
| The debug-GUI mirror (`comp_mirror_to_debug_gui.c`, `u_sink`) | It already blits the left scratch image, but it calls `vk_cmd_pool_end_submit_wait_and_free_cmd_buffer_locked`. That is a **blocking GPU wait on the compositor thread** every pushed frame. It also needs the debug GUI. Rejected as it is; its compute blit is reused. |
| `XRT_COMPOSITOR_DUMP_*` (`debug_dump_target`, used by `analysis/tools/run_hands_composite.sh`) | `vkQueueWaitIdle` twice per frame, a 7104x3840 image (109 MB) per frame, and it needs `TRANSFER_SRC` on the target images. Good for a few offline PNGs, unusable at 30 fps. |
| Export the final target image as a dmabuf | The direct-mode target images have no `SAMPLED` usage. Adding usage flags or export to the swapchain that drives DP-1 risks the display path that has already wedged the GPU (`analysis/nvidia-flip-hang.md`). The full panel image is also lens-distorted and huge. Rejected. |
| A separate OpenXR client (overlay) or Monado IPC | OpenXR has no API to read the composited result, and Monado's IPC carries client swapchains, not the compositor output. A client would only see its own layers. Rejected. |
| **A small tap in the compositor, with shared memory to a separate recorder process** | **Chosen.** It works on the pre-distortion squashed eye images (the "scratch" images, which already have `SAMPLED`), downscales them on the GPU, and hands them over without waiting. Everything heavy (copying, encoding, disk) runs in other threads or processes at low priority. |

### The tap (Monado, `src/xrt/compositor/main/comp_recorder.c`, branch `desk`)

- **Idle cost:** nothing. A worker thread polls once every 0.5 s for `$XDG_RUNTIME_DIR/pimax-record.shm` (or
  `$PIMAX_RECORD_SHM`). The compositor thread does one atomic load per frame. `PIMAX_RECORD=0` removes the tap
  entirely.
- **Per captured frame, on the compositor thread** (after the frame's own queue-idle wait, so the frame has already
  been presented):
  - one compute dispatch per eye, using the mirror's `blit.comp` on the scratch image's UNORM view, so the bytes stay
    sRGB;
  - a `vkCmdCopyImageToBuffer` into one of 3 host-visible buffers;
  - one `vkQueueSubmit` with a fence.

  There are no waits and no syscalls besides the submit. The frame is dropped and counted if it is not due yet (fps
  pacing), if all 3 readbacks are still in flight (`dropped_busy`), or if the eye was not squashed this frame
  (`skipped`).
- **The worker thread** runs at SCHED_OTHER, nice 10, never the compositor's RT priority. It:
  - creates and destroys every Vulkan object, and never does so while the compositor thread uses them (an atomic
    state plus a busy flag);
  - waits for the fences, copies each frame into a free ring slot (`dropped_full` when the ring is full), and keeps a
    heartbeat;
  - detaches when the recorder asks, when the recorder's heartbeat is 3 s old, when the file was removed, or after
    Vulkan errors;
  - never destroys a readback that did not signal (a stuck GPU): it leaks the readback and disables the tap for that
    process.
- **Fast path:** while a recorder is attached, the one-projection-layer fast path is off, because the fast path skips
  the scratch images. This costs one extra layer-squash pass per frame, only while recording. Desk mode (passthrough
  plus WayVR) never uses the fast path anyway.
- **Ring layout:** `comp_recorder_shm.h`, mirrored byte for byte by `rust/pimax-record/src/shm.rs` (tested offsets).
  - A 4096-byte header, then 4 slots of RGBA8 frames.
  - The recorder fills in the request and renames the file into place, so the tap never maps a half-written header.

### The recorder (`rust/pimax-record`, binary `pimax-record`, comm `pimax-record`)

- **Startup:**
  - It sets nice 10 and the lowest best-effort I/O priority.
  - It refuses to start when Monado is not running, when the GPU gate says reboot, when there is no ffmpeg, or when
    less than 2 GiB is free.
  - It creates the ring and waits up to 6 s for the tap. Without a tap it reports "this monado-service has no
    recorder tap" and exits 0.
- **Encoding:**
  - Frames are piped to `ffmpeg` (raw RGBA on stdin, `-pix_fmt yuv420p`, GOP of 2 s) into a **fragmented mp4**
    (`+frag_keyframe+empty_moov+default_base_moof`). The file is playable at any moment, even after a crash or a kill.
  - ffmpeg runs at nice 10 with the lowest I/O priority and `PR_SET_PDEATHSIG`, so it dies with the recorder.
- **Constant frame rate:**
  - Frames carry the compositor's predicted display time.
  - A frame the tap dropped is filled by writing the next frame twice (`duplicated`). A frame that arrives faster
    than the rate is skipped.
  - A gap longer than 3 frames is cut, not filled.
- **Backpressure:** when the encoder is behind, the recorder blocks on the pipe and the ring fills. The tap then drops
  frames (`dropped_ring`), and the compositor never notices. A separate thread keeps the heartbeat going while a write
  blocks. It kills a hung ffmpeg after 20 s with no progress, or after 5 s once a stop was requested.
- **Every way out finishes the file and exits 0:**
  - SIGINT or SIGTERM (`pimax record stop`, the launcher, pimaxd);
  - Monado gone (its heartbeat is 3 s old);
  - less than 1 GiB free;
  - an encoder error.

  On the way out, the recorder asks the tap to detach, closes ffmpeg's input (15 s grace, then SIGINT, then SIGKILL of
  ffmpeg only), removes the ring, and writes the status and an event.
- **Status and events:**
  - `<session dir>/record.status` (key=value) is read by `pimax record status`, the app and voice.
  - `<session dir>/record.events` holds JSON lines in the pimaxd event shape.
  - A desktop notification is shown for start, saved, and errors (`critical`).
  - Logs go to `<session dir>/record.log` and `record-ffmpeg.log`.

### Reliability (RESILIENCE-PLAN.md and PIMAXD.md)

- **Component `record`:** it has a row in `rust/pimax-components/src/spec.rs`, plus `components/record.rs`. It is a
  leaf with these properties:
  - a hard dependency on `monado`;
  - the leaf budget (5 restarts per 10 min) with backoff;
  - the pid file `record.pid` in the session dir;
  - `gpu: true`, because NVENC is a GPU client, so there is no automatic start while the gate is not ok;
  - the rule `on_demand`: it is never part of "everything on".

  pimaxd adopts it once it has been seen running, and shows it in its snapshot and on the app's System page:
  - a crash is restarted from the captured argv, which carries the settings, into a new file;
  - a user stop or a clean self-stop (exit 0) turns it off for the session.
- **Stop order:** `pimax desk stop` and `pimax vr stop` stop the recorder **first**, before any other client and while
  Monado still runs: SIGINT by pid, checked by comm, never by name, waiting up to 25 s. pimaxd's ordered stop puts it
  before Monado through the graph.
- **No effect on the session:** a crash or stop of the recorder never touches Monado, WayVR or passthrough. The tap
  only notices a stale heartbeat and detaches.

## Install (between sessions, never during one)

```
# 1. Monado with the tap (commit cfad375 on branch desk; build dir build-rec, the live build is untouched)
cd ~/src/monado-eye && nice ninja -C build-rec monado-service
ln -sfn build-rec ~/src/monado-eye/build          # was build-p9; build-rec has everything build-p9 has, plus the tap
sudo setcap cap_sys_nice+ep ~/src/monado-eye/build-rec/src/xrt/targets/service/monado-service   # every relink drops it
getcap ~/src/monado-eye/build-rec/src/xrt/targets/service/monado-service

# 2. The tools
cd ~/pimax-re/rust && cargo build --release -p pimax-record -p pimax-tools -p pimax-autod -p pimax-setup
install -m755 target/release/{pimax,pimax-record,pimax-autod,pimax-setup} ~/.local/bin/
systemctl --user restart pimaxd 2>/dev/null || true   # only if pimaxd is installed; restarting it never stops a session

# 3. Optional offline check (no headset, refuses while a session runs)
~/pimax-re/analysis/tools/record_offline_test.sh          # EYES=both ENCODER=nvenc to try those
```

To roll back, run `ln -sfn build-p9 ~/src/monado-eye/build`. That build still has its capability. Without the tap,
`pimax record start` reports "this monado-service has no recorder tap".

## Live checklist

1. Start the session as usual. `/tmp/pimax-vr-session/monado.log` shows `recorder: tap ready`.
2. `pimax record start`. Within about 1 s it prints `recording 0:00 to ~/Videos/pimax/pimax-....mp4 ...`, and
   `monado.log` shows `recorder: attached: 1 eye(s) 1168x1080 ...`.
3. Watch the perf panel (or `PIMAX_PERF_FILE`) for 1 minute. It should show **no new missed or late compositor
   frames** compared with before the start. `pimax record status` shows the dropped counts, which may be non-zero:
   frames dropped by the tap are fine, frames missed by the compositor are not.
4. `pimax record stop`. It prints `saved ... (N s, M MB ...)`. Play the file with `mpv`.
5. Repeat with `--eyes both --fps 60 --quality high`, then with `--encoder cpu`.
6. Start a recording and kill the recorder (`pkill -KILL -x pimax-record`). The session must not change, `monado.log`
   must show `recorder: detaching (the recorder stopped answering)` after about 3 s, and the partial file must still
   play.
7. Start a recording, then `pimax desk stop`. The recorder must stop first (its line comes before WayVR's), and the
   file must play.
8. Optional: the app's Record card (start, stop, the red REC indicator), and the `record` row on the System page.

## Voice

"Max, start recording", "Max, stop recording" and "Max, am I recording" (docs/VOICE.md, Recording). These are the
entries in `rust/pimax-voice/voice-intents.toml`, with audit cases in `tests/fixtures/audit.tsv`:

```toml
[[intent]]
id = "vr.record.start"
phrases = ["start recording", "start a recording", "record", "record video", "record this", "start the recording", "empieza a grabar", "graba", "grabar"]
gloss = "record a video of what the headset shows"
run = ["{pimax}", "record", "start"]
safety = "safe"
source = "vr"
output = true

[[intent]]
id = "vr.record.stop"
phrases = ["stop recording", "stop the recording", "end recording", "end the recording", "finish recording", "para de grabar", "deja de grabar", "termina la grabacion"]
gloss = "stop recording and save the video"
run = ["{pimax}", "record", "stop"]
safety = "safe"
source = "vr"
output = true

[[intent]]
id = "vr.record.status"
phrases = ["am i recording", "are we recording", "recording status", "is it recording", "estoy grabando"]
run = ["{pimax}", "record", "status"]
safety = "safe"
source = "vr"
output = true
```

## Limits of the basic version, and later

- **Downscaling** is one bilinear sample per output pixel, which can shimmer when a 3.5x reduction shows fine text. A
  mip or box-filter pass can be added later in the tap.
- **Audio** is in since 2026-10-16 (see Sound above; wall-clock timestamps, so a little drift on very long takes is possible). The old note: it needs real timestamps on the
  video path, either wallclock timestamps or NUT over the pipe, to stay in sync.
- **Right eye only:** not offered. **A fixed display-time warp:** the recording is the squashed image the compositor
  produced, so it does not include the lens distortion or the final scanout timewarp.
- **One tap per monado-service, one recorder at a time.** A second `pimax record start` is refused while one is
  running.
