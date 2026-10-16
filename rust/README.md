# pimax (Rust)

Rust port of the Pimax Dream Air (34a4:0044) userspace stack, built **side by side** with the C library (`../libpimax`) and the
python tools (`../linux/*.py`). Nothing installed or running is touched by building or testing this workspace.
Design notes, results and the switch-over plan: [`../analysis/rust-port.md`](../analysis/rust-port.md).

| crate | what |
|---|---|
| `pimax-proto` | `no_std` + `forbid(unsafe_code)`, no I/O. Command framing (`58 LL CMD OPT ..` / `4c` replies), IMU, controller (battery = `byte46 & 0x7f`, bit 7 charging) and 0x37 status decoders (two messages per packet), calibration document reassembly + zlib inflate, firmware log line splitter. |
| `pimax` | hidraw I/O on `ppoll`, no async runtime: sysfs discovery (4 interfaces + bridge 34a4:0043), a session thread (wake, keepalive every 213 ms with rate byte or rate file, IF1-IF3 kept open and drained, auto-reconnect), request/reply matching with timeout, handler or channel events, controller attach + re-attach on 0x37, `video_prepare`. Includes an in-process fake headset for tests. |
| `pimax-capi` | `cdylib` + `staticlib` with exactly the C ABI of `libpimax/include/pimax.h` (soname `libpimax.so.0`). |
| `pimax-tools` | the `pimax` binary: `keepalived`, `probe`, `imu`, `controllers`, `calib`, `video-on`, `logtail`, `info`, `vr` (launcher: preflight, start/stop/status of monado-service, Steam options, compositor dump statistics). |
| `pimax-difftest` | test-only: compiles the original `libpimax/src/*.c` with `cc` (symbols renamed `c_*`) and compares it with the Rust code. |
| `pimax-lens` | Dream Air lens model for the Monado driver: per-channel radial polynomials, `lens.conf` parser, fit range, edge black-out / visibility mask, debug UV grid; `staticlib` with the C ABI `include/pimax_lens_rs.h` (the thin `pimax_lens.c` in Monado calls it) and the `pimax-lens` CLI (`show`, `grid`, `fit`). |
| `pimax-lens-difftest` | test-only: the original C lens model (Monado commit 1da3626, symbols renamed) and the real C glue against the Rust crate, bit for bit; links the staticlib from a C program. |
| `pimax-lenstest` | OpenXR (Vulkan, `openxr` + `ash`) test pattern client for the lens: grid, angle rings, per-channel grids, FOV edge, moving square 100 % / 25 %, corner frames; controllers, auto mode, capture of the compositor output as PNG, `simulate` / `compare` / `dumpdiff`. Driven by `analysis/tools/run_lens_test.sh`. |

## Build and test
    cd rust
    cargo build --release                       # target/release/pimax, libpimax_capi.{so,a}
    cargo test --workspace                      # 62 tests, no hardware needed (about 30 s once built)
    cargo clippy --workspace --all-targets      # clean
    # real headset, read-only, next to the running pimax-keepalived service:
    cargo test -p pimax       --test live -- --ignored --test-threads=1 --nocapture
    cargo test -p pimax-tools --test live -- --ignored --test-threads=1 --nocapture
    # benchmarks
    cargo bench -p pimax-difftest               # decode throughput Rust vs C
    cargo bench -p pimax --bench latency        # IMU report -> callback latency (fake device)
    python3 bench/cpu_compare.py 60             # CPU/RSS/wakeups of `pimax keepalived` vs the python service

Needs gcc (difftest, C-ABI tests), cmake (optional link test), tshark only to regenerate fixtures
(`python3 fixtures/make_fixtures.py`; the generated fixtures are committed in `fixtures/`).

## Tool cheat sheet
    pimax keepalived [--no-wake] [--rate-file P]   # == pimax_keepalived.py, rate file $XDG_RUNTIME_DIR/pimax-rate
    pimax probe [secs] [--passive]                 # == pimax_probe.py
    pimax imu [--json] [--rate HZ] [--auto-bias S] [--still N] [--axis-mode 0|1] ...   # == pimax_imu_orient.py (+ axis mode, default 1)
    pimax controllers [secs] [--raw]               # == pimax_controllers.py
    pimax calib [out.xml] [--blob F]               # == pimax_calib.py (offline decode with --blob)
    pimax video-on [fps] [secs]                    # == pimax_video_on.py  (the only command that changes headset state)
    pimax logtail [secs] [regex]                   # == pimax_logtail.py
    pimax info [--controllers]                     # serial, version, interfaces (== pimax-cli info)
    pimax vr start [--fps 72|90] [--dump DIR] [--wait]   # one-click VR: preflight + detached monado-service (see linux/DISPLAY.md)
    pimax vr stop | status | steam-options | dump-stats FILE.raw...

Environment: `PIMAX_SYSFS_ROOT` / `PIMAX_DEV_ROOT` redirect discovery (tests), `PIMAX_LOG=debug|warn|off`, `PIMAX_CALIB=file`.

## Layout of the tests
* `pimax-proto/tests`: framing, decoders on the real captures (`fixtures/*.rec.gz` from `analysis/p82.pkl`, `p83.pkl` and
  `captures2/*.pcap`), calibration round trip against `analysis/headset-calibration.xml`, port of `linux/test_controllers_replay.py`,
  proptest (7 properties x 20 000 cases).
* `pimax/tests`: fake sysfs discovery; session against the fake headset (wake, keepalive timing, rate byte, events, request matching,
  calibration with a lost reply, unplug/replug, attach and re-attach rules, prompt close); `live.rs` (ignored).
* `pimax-capi/tests`: struct layout vs the real header, symbol export, unmodified `pimax-cli.c` against the cdylib and the staticlib,
  a C program driving a whole session, decoder output identical to the C library, CMake consumer, Monado object imports.
* `pimax-tools/tests`: Mahony filter vs quaternions from the python implementation, CLI black-box tests, `live.rs` (ignored).
* `pimax-difftest/tests`: Rust == C on 166 658 captured reports, 49 152 single-byte mutations, 200 000 random reports, plus `find_interfaces`.
