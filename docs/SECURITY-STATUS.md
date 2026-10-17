# Security and robustness status

This page lists the known safety and robustness gaps in omarchyvr, in plain words. It is meant for people who want to
use, review or contribute to the project. A static review (no build, no test run, no hardware) was done on
2026-10-17 at commit `7660e74`. Each row below was checked again in the code at HEAD before it was written down, and
cites the file and line.

Nothing here is a secret. The gaps are open and the fixes are planned in
[plans/WEEK1-SAFETY.md](plans/WEEK1-SAFETY.md). To report a new problem, see [../SECURITY.md](../SECURITY.md).
The design these rows refer to is in [ARCHITECTURE.md](ARCHITECTURE.md).

## Known gaps

| Area | What is wrong | Impact | Status |
|---|---|---|---|
| HID allowlist | The allowlist library is not used on the write paths. `Gate::send` is called only from tests (`rust/pimax-device/tests/allowlist.rs:19,170,216`). The session writes through `write_ctl` (`rust/pimax/src/session.rs:278`). Raw writers do their own hidraw writes: `rust/pimax-eye/src/ipd.rs:48`, `rust/pimax-eye/src/power.rs:105`, `rust/omarchyvr-autod/src/pairing.rs:220`, `rust/omarchyvr-autod/src/unpair.rs:21`, `rust/omarchyvr-autod/src/visor.rs:30`, `rust/omarchyvr-tools/src/buzz.rs:63`. The C functions `pimax_send` and `pimax_request` accept any command (`rust/pimax-capi/src/lib.rs:358,372`). Backlight commands (`rust/omarchyvr-tools/src/vr/display.rs:94`) are not in `device.toml`. Camera auto-exposure can go above the descriptor ranges: the dim-room stage (on by default) and the overrides allow up to 10410 / 449 (`rust/pimax-cam/src/ae.rs:747-765,947-962,622-629`). | The safety rule "only send bytes seen in vendor captures" is a convention, not a check. A bug or a bad caller can send an unapproved command to the headset. | Open, planned: [week 1 step 5](plans/WEEK1-SAFETY.md) |
| IPD motor | The 58..72 mm range is checked only by callers. `motor_report` does not clamp (`rust/pimax-eye/src/ipd.rs:40`). `raw_from_mm` clamps to the `u16` range, and a NaN input becomes 0, which encodes as 0 mm (`ipd.rs:28-30`). | A bad value from a caller can reach the lens motor. | Open, planned: [week 1 step 2](plans/WEEK1-SAFETY.md) |
| C ABI | Only `pimax-cam` wraps calls in `catch_unwind` (`rust/pimax-cam/src/capi.rs:21-33`). The exported functions of `pimax-capi`, `pimax-hands` and `pimax-lens` have no guard. The hands code takes poses without a finiteness check (`rust/pimax-hands/src/fuse_ffi.rs:120-146`) and calls `partial_cmp(..).unwrap()` in the fuser (`rust/pimax-hands/src/fuse.rs:234,313,424`), which panics on NaN. A panic in an `extern "C"` function aborts the process. | One bad float can abort Monado, which ends the VR session and the compositor. | Open, planned: [week 1 step 3](plans/WEEK1-SAFETY.md) |
| Atomic writes and private data | `write_atomic` in `rust/omarchyvr-components/src/util.rs:69-75` builds the temp name with `with_extension`, which replaces the file's extension, so `x.pid` and `x.log` written by the same process share one temp file and can collide. It does not call `fsync`. Other writers do the same by hand, for example `rust/omarchyvr-roommap/src/floor.rs:489-494` and `rust/omarchyvr-room/src/map.rs:407`. Room maps and the typed-text log (`rust/omarchyvr-voice/src/desk.rs:91-100`) are created with the default file mode, not 0600. | A crash or power loss can leave a state file empty or old. Other local users may be able to read private room data if the umask is open. | Open, planned: [week 1 step 4](plans/WEEK1-SAFETY.md) |
| Kernel log follower | The reader thread returns on the first line that is not valid UTF-8 (`rust/omarchyvr-components/src/events.rs:235-239`). `alive()` only checks that the child process runs (`events.rs:245-247`), so it keeps saying "alive". | The GPU error (Xid) gate can go blind without any sign of it. | Open, planned: [week 1 step 1](plans/WEEK1-SAFETY.md) |
| Voice defaults | `voice_confirm` defaults to `none` (`rust/omarchyvr-config/src/ai.rs:9,857`), so careful commands run without asking. `strip_titles` defaults to false (test `ai.rs:1903`), so window titles are part of the state sent to a cloud model when one is used (`rust/omarchyvr-voice/src/claude.rs:12-15`). Typing into a window asks for confirmation only for terminals (`rust/omarchyvr-voice/src/desk.rs:131-141`). `typed.log` stores the typed text as it is and has no rotation (`desk.rs:85-100`). | A misheard command can act without a check. Window titles and typed text can contain private data. | Open, planned: [week 1 step 6](plans/WEEK1-SAFETY.md) |
| Root helper | System setup runs `omarchyvr-root.sh` from the user's data directory as root, through `pkexec /bin/sh -c` (`rust/omarchyvr-gui-core/src/cmd.rs:163`, `rust/omarchyvr-gui-core/src/install.rs:45,73`). The plan file the script reads can be written by the user. The script checks its values only by character set (`installer/omarchyvr-root.sh:86`). The root-script checker runs in the unprivileged app (`rust/omarchyvr-gui-core/src/safety.rs:343`). The repo has no polkit policy file. | Code running as the same user could change what runs at the next password prompt. Only the uninstall uses a root-owned copy (`install.rs:14,82`). | Open, planned: [week 1 step 7](plans/WEEK1-SAFETY.md) |

The rows are about the code, not about a known attack. No exploit is described here on purpose.

## Not a problem, by design

The review also looked for problems and found these parts sound.

- **The allowlist matcher and the embedded descriptors.** The matching rules (exact pattern, every field constrained,
  zero padding, denied prefixes) are in `rust/pimax-device/src/hid.rs`, with tests. Descriptors are compiled in with
  `include_str!` (`rust/pimax-device/src/module.rs:63`), so there is no file to swap at run time. The gap is that the
  write paths do not call it yet (first row above).
- **Bounded HID parsing.** `pimax-proto` forbids unsafe code (`rust/pimax-proto/src/lib.rs:10`) and works on
  fixed-size 64-byte reports with no I/O.
- **Downloads are pinned and hashed.** `installer/components.lock` pins every download and clone by commit or sha256.
  One exception: the sherpa-onnx speech library is installed from a pinned wheel version (`SHERPA_PIP=sherpa-onnx-core==1.13.8`),
  and the installer only warns, and keeps the file, when the library's checksum differs from the expected one
  (`installer/install.sh:399`).
- **The NVIDIA patcher.** It applies the patches to a scratch copy first and changes the real tree only if every patch
  applies (`patches/nvidia/nvidia-patch-reapply.sh:12-13,227`). It patches only versions listed in `tested-versions.txt`
  unless the user opts in, and it falls back to the stock module.
- **The udev rule.** `linux/70-pimax.rules` uses mode 0660 with the `uaccess` tag for the Pimax devices (vendor 34a4)
  and the eye tracker (2104:0561). It does not use 0666.
