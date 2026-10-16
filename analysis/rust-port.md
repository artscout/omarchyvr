# Rust port of the userspace stack (2026-10-13)

Workspace: `rust/` (see `rust/README.md` for the crate list and commands). Everything was added next to the existing code.
Not touched: the running `omarchyvr-keepalived` service and its unit, `~/.config/systemd`, `~/src/monado`, the C library, the python tools,
the headset display. Nothing is committed.

## Architecture
```
pimax-proto   pure functions on 64-byte reports        no_std, forbid(unsafe_code), no I/O
   |
pimax         ppoll session thread + discovery         std, libc (only unsafe: ppoll/eventfd/socketpair/sigaction in sys.rs)
   |    \
   |     omarchyvr-tools  (bin `omarchyvr`)                    8 subcommands replacing linux/*.py
   |
pimax-capi    extern "C" pimax.h, cdylib + staticlib   drop-in for libpimax (soname libpimax.so.0)
pimax-difftest (test only)                             C decode.c/pimax.c via `cc` crate, symbols renamed c_*
```
Session thread (`pimax::Session`), same shape as `libpimax/src/pimax.c`: discover (IF0 + IF2 required) -> open IF0 rw, IF1-IF3 ro,
non-blocking -> wake `58 06 1a 05 01 00 00` -> loop { keepalive due? write `58 04 39 00 RR 00`; `ppoll` on the four fds + an eventfd
(wakes the thread for `close()`); one read per ready fd; dispatch by interface (IF0 replies / 0x37 / pending request, IF1+IF2
IMU and controller reports, IF3 log lines) } -> on hang-up, ENODEV/EIO write errors: callbacks `Disconnected`, retry in 100 ms.
Backends are pluggable (`Backend` trait): `HidrawBackend` for real hardware, `fake::pair()` (seqpacket socketpairs, which keep
message boundaries and poll like hidraw) for tests and benchmarks. Events go to a `Handler` (all callbacks on the session thread) or
to a bounded channel (`Session::open_channel`, drops instead of blocking).
Controller attach (`Config::auto_attach`): initial burst for slots 2 and 3, then a re-burst when a 0x37 heartbeat is newer than the last
burst and no report arrived for 1.5 s, at most once per 2.5 s (the rule of `pimax_controllers.py` and Monado's `pimax_ctrl.c`); bursts
are queued with 10 ms spacing on the session thread, so nothing blocks.

## Deliberate differences from the existing code
| | python / C | Rust | why / effect |
|---|---|---|---|
| keepalive wire form | python `58 04 39 00 RR 00` (LL=4, as Windows), C `58 05 39 00 RR 00` | `58 04 39 00 RR 00` everywhere | follows the Windows capture and the running service; firmware accepted both so far |
| `pimax_controller_report.battery` | C: raw byte 46 (bit 7 = charging, e.g. 218) | `byte46 & 0x7f` (percent); `raw[46]` still has the raw byte | per the controllers-linux.md finding. Monado's `pimax_ctrl.c` reads `raw[46]`, so it is unaffected |
| `on_controller_status` | C: first 0x37 message of a packet only | both messages of a packet | python `parse_0x37` already handled two |
| log lines | python keeps `\r` inside a line | `\r` and NUL dropped (as C) | cosmetic |
| `logtail` filter | python `re` | `regex-lite` (no look-around/backrefs) | smaller dependency |
| keepalived log | "headset gone (strerror)" | "headset gone after X s, N keepalives" | no errno text |
| `imu` | identity axes | `--axis-mode` 0/1, default 1 (X, Z negated) | what the Monado driver uses (imu-orientation.md, tilt test); mode 0 reproduces python exactly |
| discovery | fixed `/sys/class/hidraw`, `/dev` | same, overridable with `PIMAX_SYSFS_ROOT`/`PIMAX_DEV_ROOT` | hermetic tests |
| extra export | - | `pimax_open_with_fds()` (not in pimax.h) | C-ABI tests on a fake device |
| IF1/IF3 open failure | python: session error; C: ignored | ignored with a warning (IF0, IF2 required) | as C |

Everything else is wire-identical: same discovery rule, wake, 213 ms period, drain of IF1-IF3, request matching on `cmd`/`opt`,
reply data length `LL-3` clamped to 60, serial parsing, attach burst bytes, video-prepare bytes (`5b/05[02 02]`, `29/00[01]`,
`1a/05[02 FPS ..]`, `1a/05[05 00 ..]`), calibration reads (`58 07 0a 00 01 AL AH 10`, 84 blocks, `blob[16:16+u32le]` zlib).

## Test results (2026-10-13, this machine, Arch rust 1.98.1)
`cargo test --workspace`: **62 passed, 0 failed**, 11 live tests ignored by default (all 11 pass when run, see below). `cargo clippy
--workspace --all-targets`: 0 warnings.

| crate | tests | notes |
|---|---|---|
| pimax-proto | 25 | decoders on real data (IMU p83: 63 369 reports; controller p82 + captures2 buttons capture: 102 334 reports; python golden for all of them), 0x37 on the 28 captured packets vs python, calibration blob reassembled from the Windows cold-start capture inflates to `headset-calibration.xml` byte-for-byte, round trip/shuffle/duplicates/truncation/corruption, log lines from the real IF3 capture, framing bytes vs python/video_on/attach, port of `test_controllers_replay.py` (all 10 left-controller events in marker order, touch bits, analog ranges, right idle/round-1 checks), 7 proptest properties x 20 000 random/structured reports (no panics, magic acceptance, invariants) |
| pimax | 15 | discovery in a fake sysfs (4 IF + bridge, partial, empty); session on the fake headset: wake first, keepalive spacing 213 +- 40 ms, rate byte from handle and from rate file, passive mode silent, IMU/controller/status/log events and counters, request matching (a 0x37 and an unrelated reply do not satisfy it), timeout, calibration with a lost reply (retry), unplug/replug (wake again, states Connected/Disconnected/Connected, pending request aborted), auto-attach initial burst and rate-limited re-attach, no re-attach while reports flow, video-prepare order, close < 100 ms |
| pimax-capi | 7 | real `pimax.h` vs Rust: sizes/offsets of all structs, constants, 14/14 functions exported, soname; **unmodified `libpimax/tools/pimax-cli.c` builds and runs against the cdylib (installed as libpimax.so.0) and links against the staticlib**; a C program drives a full session (callbacks, requests, `-ETIMEDOUT`/`-EMSGSIZE`/`-ENOTCONN`, attach and video-prepare wire bytes, unplug); decoder output of C `decode.c` and Rust byte-identical (`%a` floats) on 366k reports; CMake consumer (static driver lib -> executable) with C sources and with `libpimax_capi.a`; Monado's built objects import 5 libpimax symbols, all exported |
| omarchyvr-tools | 9 | Mahony filter vs quaternions from `pimax_imu_orient.py` on the real p83 capture (126 checkpoints, max deviation < 1e-9), wrap handling, axis modes; CLI black box (usage, no-headset errors, logtail, keepalived idle CPU and SIGTERM, offline `calib --blob` == Windows XML) |
| pimax-difftest | 6 | **Rust == C**: 166 658 captured reports (63 369 IMU, 102 334 controller, 955 other), 49 152 single-byte mutations of three real reports, 200 000 proptest reports, slot/sample-selection corners, `find_interfaces` on this machine (mask 0b1111, same four paths); bit-exact floats. 0 differences. The only intended difference (battery masking) is checked as `c.battery == rust.battery | charging << 7` |

Live (headset connected, `omarchyvr-keepalived` running, all read-only, ran in parallel with the service, service never left `active`,
USB devnum unchanged): `cargo test -p pimax --test live -- --ignored`: 7/7 (interfaces, identity reads `0x16` -> `P511X0X000000000001`,
`0x03` -> `00 00 21 01`, `0x50`; calibration 84 reads in 250 ms == Windows XML; IMU 501 reports/s, device clock span 3.00 s, |a| 0.998 g;
controller attach: both slots answered 0x16/0x50/0x0a/0x03, L 1237 / R 1246 reports in 5 s, battery 90 % / 100 %; passive log; a 7 s
passive session while only the service keeps the headset alive). `cargo test -p omarchyvr-tools --test live -- --ignored`: 4/4.
Never sent: video-prepare, IPD, brightness, eye power, pairing. The sessions in the live tests send neither wake nor keepalive.

## Performance
| measurement | python / C | Rust |
|---|---|---|
| `keepalived` CPU over 60 s, same window, headset connected (`bench/cpu_compare.py`) | python service: 403.0 ms = 0.672 % | `omarchyvr keepalived`: 340.8 ms = 0.568 % (85 %). An earlier run before dropping the per-report EAGAIN read: 434 vs 398 ms |
| `keepalived` RSS / threads | 12.5 MB / 1 | 3.1 MB / 2 (main thread sits in `sigwait`) |
| `keepalived` wakeups | 778 /s | 789 /s (dominated by the 1000 reports/s that must be drained: syscall bound for both) |
| `decode_imu` (`cargo bench -p pimax-difftest`) | C 5.4 ns | 3.3 ns (0.61x) |
| `decode_controller` | C 7.0 ns | 3.6 ns (0.51x) |
| IMU report -> `on_imu`, fake device, 500 Hz paced | - | p50 7 us, p90 13 us, p99 94 us, max 125 us; session thread CPU 0.03 s per 10 s |
| same, 1 kHz | - | p50 3 us, p99 102 us; unpaced burst 1.2-1.9 M reports/s |
| live IMU inter-arrival (3 s, headset, passive session) | - | p50 2.00 ms, p99 2.23 ms, max 3.10 ms at 501 reports/s |
Conclusion: functionally identical and a little cheaper in CPU, 4x less memory. The daemon's cost is the kernel work of draining
~1000 reports/s, which both implementations pay; there was no hidden win to be had without changing what is drained.
Latency was measured against the in-process fake (the C library cannot be pointed at a fake device), so the numbers cover the
poll wake-up, read, decode and callback dispatch, not USB.

## Switch-over plan (EXECUTED 2026-10-13 22:47: keepalive drop-in rust.conf active; Monado built with PIMAX_USE_RUST_LIBPIMAX=ON)
### (a) systemd user service: python -> `omarchyvr keepalived`
1. Build and install the binary (no effect on the running service):
   `cd ~/pimax-re/rust && cargo build --release && install -Dm755 target/release/omarchyvr ~/.local/bin/omarchyvr`
2. Re-run `cargo test --workspace` and the two live suites.
3. Exact unit diff. `~/.config/systemd/user/omarchyvr-keepalived.service` is a symlink to `linux/omarchyvr-keepalived.service`, so either edit that file:
```diff
--- a/linux/omarchyvr-keepalived.service
+++ b/linux/omarchyvr-keepalived.service
@@
-Description=Pimax Dream Air USB keepalive (wake + 58 04 39 every 213 ms)
+Description=Pimax Dream Air USB keepalive, Rust (wake + 58 04 39 every 213 ms)
 
 [Service]
-ExecStart=/usr/bin/python3 %h/pimax-re/linux/pimax_keepalived.py
+ExecStart=%h/.local/bin/omarchyvr keepalived
 Restart=always
 RestartSec=1
```
   or, leaving the repo file alone, use a drop-in (`~/.config/systemd/user/omarchyvr-keepalived.service.d/rust.conf`):
```ini
[Service]
ExecStart=
ExecStart=%h/.local/bin/omarchyvr keepalived
```
   A ready, verified (`systemd-analyze --user verify`) stand-alone unit is in `rust/systemd/omarchyvr-keepalived-rust.service`.
4. Switch with no gap longer than the 5.4 s firmware window:
```
systemd-run --user --unit=pimax-ka-trial %h/.local/bin/omarchyvr keepalived --no-wake   # second sender keeps the headset alive
systemctl --user daemon-reload && systemctl --user restart omarchyvr-keepalived         # python -> rust (wake is re-sent, as on any restart)
systemctl --user status omarchyvr-keepalived; journalctl --user -u omarchyvr-keepalived -n 20
systemctl --user stop pimax-ka-trial
```
   (A plain `daemon-reload` + `restart` also works: the restart gap is about 1 s.) Rate file `$XDG_RUNTIME_DIR/pimax-rate` is shared in
   both directions, so `pimax_video_on.py` and `omarchyvr video-on` both work with either daemon.
5. Rollback: remove the drop-in (or `git checkout linux/omarchyvr-keepalived.service`), `systemctl --user daemon-reload && systemctl --user restart omarchyvr-keepalived`.
   The python script is untouched. `rm ~/.local/bin/omarchyvr` is optional.

### (b) Monado pimax driver on `pimax-capi` (opt-in, default stays the C sources)
Patch (checked with `git apply --check` against `~/src/monado`, not applied): `rust/monado-opt-in-rust-libpimax.patch`. It only edits
`src/xrt/drivers/CMakeLists.txt`: the pimax block gets `option(PIMAX_USE_RUST_LIBPIMAX OFF)` and `PIMAX_RUST_TARGET_DIR`; when ON, the driver
links `libpimax_capi.a` (+ `-ldl -lm -lrt -lutil -lpthread`, the native-static-libs rustc reports) instead of compiling
`libpimax/src/pimax.c` and `decode.c`. No driver source changes are needed: the driver imports `pimax_open/close/wait_connected/
video_prepare/controller_attach` and uses the pimax.h structs, all identical.
```
cd ~/pimax-re/rust && cargo build --release -p pimax-capi
cd ~/src/monado && git apply ~/pimax-re/rust/monado-opt-in-rust-libpimax.patch
cmake -S . -B build -DPIMAX_USE_RUST_LIBPIMAX=ON && ninja -C build        # a Monado restart picks it up
```
Rollback: `cmake -S . -B build -DPIMAX_USE_RUST_LIBPIMAX=OFF && ninja -C build` (or `git apply -R` the patch); the C library is untouched.
Verified without touching Monado: the patch applies; the same CMake structure builds and links in a scratch project against both the
C sources and `libpimax_capi.a`; the symbols imported by Monado's existing `pimax_hmd/ctrl/prober` objects are all exported.
NOT verified: a real Monado rebuild and a VR session on the Rust library (rules: no Monado runs, display untouched). Do the first run
with the headset on the desk and watch the log for `pimax_open`/`controller slot N` lines.
Note: when Monado links the Rust library the headset sees keepalives in the Windows form `58 04 39 ..` instead of `58 05 39 ..`.

## Not ported / open
* Not ported: `pimax_keepalive_test.py`, `wait_and_probe.sh`, `pimax_display_on.sh`, `make_hmd_edid.py` (display/EDID side, out of scope).
* Everything libpimax does not do either: cameras, eye tracking, IPD/brightness set, pairing.
* `video-on` and `Session::video_prepare` are implemented and unit-tested at byte level but were never run on the headset.
* The bridge log (34a4:0043) is only read by the tools (`logtail`, `video-on`), not by `Session`.
* No cbindgen: header compatibility is enforced by the hand-written layout/export/behaviour tests above.
* The measured CPU gain of the daemon is modest (15 %); the memory gain is larger. A coalescing drain mode (sleep 20 ms, then read
  everything, about -30 % syscalls) was considered and left out because the hidraw ring holds only 64 reports per client.
