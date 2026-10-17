# Architecture

This page describes how omarchyvr is put together and where the design is ahead of the code. It mixes a design
(written 2026-10-14) with notes on what is built at HEAD (checked 2026-10-17). Where the two differ, the text says so.
The plan for opening the repo is in [OPEN-SOURCE-PLAN.md](OPEN-SOURCE-PLAN.md). Known safety gaps are listed in
[SECURITY-STATUS.md](SECURITY-STATUS.md).

## Built vs planned

| Part | State | What exists at HEAD |
|---|---|---|
| Device modules and descriptors | Built | `rust/pimax-device/devices/` has five descriptors (`dream-air`, plus the mocks `dream-air-se`, `crystal-light`, `crystal-super`, `8kx`). `module::all()` and `module::detect()` (`rust/pimax-device/src/module.rs:71,102`) are used by the app's model list (`rust/omarchyvr-gui-core/src/headsets.rs:10,28`). |
| HID allowlist (the library) | Built and tested | `Allowlist`, `Gate` and the `[[hid]]` parser in `rust/pimax-device/src/hid.rs`, tests in `rust/pimax-device/tests/allowlist.rs`. |
| HID allowlist (enforcement on write paths) | Not yet | Nothing outside the tests calls `Gate::send`. See [2.3](#23-the-hid-allowlist). Planned in [Step 5](plans/WEEK1-SAFETY.md). |
| System checks | Partly | `Check`, `Finding` and the fix catalogue exist (`rust/pimax-device/src/check.rs`). Four native checks run (`rust/pimax-device/src/checks/mod.rs:12`), plus `DoctorBridge`, which wraps the older `omarchyvr-doctor` checks. The other areas in 3.2 are planned. |
| Onboarding | Built | Progress file in `rust/pimax-device/src/onboarding.rs`; the app page is `rust/omarchyvr-app/src/pages/onboarding.rs`. |
| Autofix page | Built | Reply parser `rust/pimax-device/src/autofix.rs` (`MAX_FIXES = 5`), bundle and redaction in `rust/omarchyvr-gui-core/src/{bundle,redact,autofix}.rs`, page `rust/omarchyvr-app/src/pages/autofix.rs`. |
| "Test your setup" report | Partly | The app's Autofix page writes a redacted report and a local contribution branch (`report_toml`, `rust/omarchyvr-gui-core/src/autofix.rs:187`). The `pimax-device report` command from the design does not exist. |
| CI | Planned | There is no `.github/` directory and no CI config in the repo. See [6.3](#63-ci-planned-offline-only-no-device-no-gpu). |

Goals, from the user:
- Modular per headset: onboarding, controller pairing, drivers, GPU, CPU and audio.
- An LLM "Autofix" page.
- Open source, so people can test their own setup and contribute fixes.
- Small, minimalist Rust, plain files, no database.
- Scope is the Pimax family. The Dream Air SLAM is real; the Crystal Light, Crystal Super, 8KX and Dream Air SE are mocks.
- Nothing outside `devices/` assumes the vendor is Pimax. A module is matched only by its descriptor's USB IDs, so another vendor's headset would just be one more directory. We don't build generic multi-vendor layers now.

## 1. Layers

```
omarchyvr-app (GUI) ── pages: Home, Onboarding, Setup/Checks, Autofix, Tools, Logs, Profiles
     │ uses
omarchyvr-gui-core ──── facts, preflight, redact, bundle, profiles, job runner (runs only the user's approved commands)
     │ uses
pimax-device ────── HeadsetModule + descriptors (devices/<id>/device.toml), HID allowlist + Gate,
     │              Check trait + checks/, Fix catalogue, Autofix reply contract, onboarding progress file
     │ uses
omarchyvr-doctor ────── Host trait (real / fake), today's checks (bridged, then moved check by check)
                    ── tools that touch the device (pimax, pimax-eye, pimax-cam, omarchyvr-autod) build reports through
                       a module and send through Gate (after the migration)
```

`pimax-device` has no I/O of its own:
- Modules build reports.
- `Gate` writes them to a writer that the caller has opened.
- Checks read through `omarchyvr_doctor::Host`.
- Fixes are rendered as text for the user to run.

Its only dependency is `omarchyvr-doctor`.

### 1.1 Crate groups in `rust/`
`rust/Cargo.toml` builds everything with globs: `members = ["pimax", "pimax-*", "omarchyvr-*"]`. A new crate with one
of those name prefixes joins the workspace on its own. The crates fall into four groups.

- **Hardware crates (`pimax*`).** Talk to the headset and its parts.
  - `pimax-proto`: wire framing and decoders, `no_std`, no I/O.
  - `pimax`: hidraw I/O, the session thread, command and reply matching.
  - `pimax-eye` and `pimax-eyecal`: the eye-tracker chip, and eye calibration.
  - `pimax-cam`: the tracking cameras, including auto exposure.
  - `pimax-lens`: the lens model.
  - `pimax-hands` and `pimax-ctrltrack`: controller and hand tracking.
  - `pimax-ipd`, `pimax-selfcal`: the guided IPD test and camera self-calibration.
  - `pimax-device`: headset modules, the HID allowlist, checks, fixes, onboarding, the Autofix reply contract.
  - `pimax-capi`: the C ABI of libpimax.
- **Product crates (`omarchyvr-*`).** The app and the services around it.
  - `omarchyvr-app` (GUI) and `omarchyvr-gui-core` (everything the GUI does, without UI).
  - `omarchyvr-tools` (the `omarchyvr` command line), `omarchyvr-autod` (auto mode, pairing), `omarchyvr-doctor` (read-only health checks), `omarchyvr-components` (the component model of the daemon), `omarchyvr-config`.
  - Voice and AI: `omarchyvr-voice`, `omarchyvr-voicehud`, `omarchyvr-ai`.
  - Room and view: `omarchyvr-slam`, `omarchyvr-room`, `omarchyvr-roommap`, `omarchyvr-anchor`, `omarchyvr-passthrough`, `omarchyvr-record`, `omarchyvr-hud`, `omarchyvr-backgrounds`.
- **Development and test tools.** Not part of normal use. Their `Cargo.toml` descriptions say what they are:
  - `omarchyvr-xrtest`: OpenXR test clients;
  - `omarchyvr-desktest`: offline analysis of the VR-desktop tests;
  - `omarchyvr-perf`: the performance readout on a wrist panel;
  - `omarchyvr-voicebench`: a CPU latency harness for voice;
  - `pimax-lenstest`: lens test patterns and the eye accuracy test;
  - `pimax-difftest` and `pimax-lens-difftest`: differential tests against the original C code (`publish = false`).
- **C ABI static libraries that Monado links.** These crates build a `staticlib` so the C driver can call Rust:
  - `pimax-capi` (`crate-type = ["cdylib", "staticlib", "rlib"]`; it also exports the hands and eye functions);
  - `pimax-hands` (`["rlib", "staticlib"]`);
  - `pimax-lens` (`["staticlib", "rlib"]`);
  - `pimax-cam` (`["rlib", "staticlib"]`).

  The boundary rules are in [2.6](#26-c-abi-boundary).

## 2. Device modules

### 2.1 Shape: data plus a small trait
- **Data:** each headset gets `rust/pimax-device/devices/<id>/device.toml`.
- **Parser:** the TOML is read by a tiny parser for one-line values (`src/toml_lite.rs`), so no new dependencies.
- **Loading:** descriptors are compiled in with `include_str!` (`module::SOURCES`). There is no dynamic loading.
- **Trait:** `HeadsetModule` (`src/module.rs`) has:
  - `descriptor()`;
  - `ipd_report(mm)`, which clamps to the descriptor's range;
  - `pair_report(hand)`;
  - `quirks()`.
- **Code-only modules:**
  - A model whose behaviour can't be written as data gets a `src/devices/<id>.rs`. The Dream Air has one, for the hand-to-slot mapping, the 0x51 AE encoder and the keepalive rate.
  - Mocks are `DataModule`s with no code.
- **Detection:** `detect(vid, pid)` returns one of three results:
  - `Module` when a real module claims the IDs;
  - `Possibly` when the ID is an unverified candidate PID of a mock, so the GUI shows "possibly Crystal Light, not supported yet";
  - `None`.

### 2.2 Descriptor schema

| Table | Keys |
|---|---|
| `[device]` | `id`, `name`, `vendor`, `status` (`real` / `mock`), `usb_vid`, `usb_pids` (real only), `usb_pid_candidates` (mock, unverified), `usb_aux`, `firmware_seen`, `command_interface` |
| `[display]` | `status`, `mode` [w, h], `refresh_hz`, `edid_vendor`, `edid_product`, `needs` (host requirements such as `nvidia_2heads1or_patches`, `drm_lease`, `edid_override_non_desktop`; checks key off these) |
| `[tracking]` | `status`, `head` (`slam`, `lighthouse`, `inside_out`, `imu3dof`) |
| `[controllers]` | `status`, `slots`, `pairing` (`start_pair` / `unknown`) |
| `[eye_tracking]` | `status`, `usb_vid`, `usb_pid`, `calibration` |
| `[ipd]` | `status`, `motor`, `min_mm`, `max_mm`, `readback` |
| `[cameras]` | `status`, `count`, `tile`, `frame`, `passthrough`, `auto_exposure` |
| `[audio]` | `status`, `sink_match`, `source_match` (PipeWire node name substrings) |
| `[calibration]` | `factory`, `paths` (user files the module reads or writes) |
| `[onboarding]` | `steps`, the subset of the standard steps in order |
| `[[hid]]` | one approved command each (2.3) |
| `[[hid_denied]]` | `prefix` (with `??` wildcards), `why`: refused with a reason before the allowlist is consulted |

`status` is one of `works`, `experimental`, `none` (the model doesn't have it) or `unknown`. Validation is enforced at
load time, and the tests load every descriptor:
- A mock may not declare `[[hid]]`, may not claim `usb_pids`, and may not promise any capability.
- A real module needs `usb_pids`.
- `ipd.motor = true` requires an `ipd_motor` command.
- `pairing = "start_pair"` requires a `start_pair` command.
- Onboarding steps must be known.

### 2.3 The HID allowlist
This section first describes the design and the library. The state of enforcement at HEAD is under
[Current state](#current-state-as-of-2026-10-17) below. Short version: the library works, but the production write
paths do not use it yet.

Each `[[hid]]` entry has:
- `id`;
- `interface` (default 0);
- `bytes`: hex literals plus `{name:u8}` / `{name:u16le}` fields;
- `params`: `name=LO..HI` or `name=A|B`. Every field must have exactly one constraint;
- `report_len` (default 64);
- `policy`: `session`, `once_per_request` or `once_per_enumeration`;
- `min_interval_ms`;
- `source`: required. The capture and the `file:line` where the stack builds the command;
- `approved`: required for anything that is not `session`.

A report passes only if all of these hold:
- it matches a pattern exactly;
- every field is within its constraint;
- all remaining bytes are zero;
- it doesn't hit a denied prefix.

`Gate::send` (`rust/pimax-device/src/hid.rs:392`) runs this check before its single `write`, so a refused report writes
nothing. It also enforces `min_interval`. `Allowlist::build(id, params)` (`hid.rs:343`) runs the same check, so a
report is checked once when it is built and again when it is sent.

Dream Air commands (all on IF0, 64 bytes, zero padded). The descriptor is
`rust/pimax-device/devices/dream-air/device.toml`. Sources:

| id | bytes | constraint | built today in |
|---|---|---|---|
| wake | `58 06 1a 05 01 00 00` | | rust/pimax-proto/src/frame.rs:46 |
| keepalive | `58 04 39 00 RR 00` | RR ∈ {0, 0x48, 0x5a} | frame.rs:64; rust/omarchyvr-tools/src/vr/reset.rs:53-60 |
| app_unlock | `58 05 5b 05 02 02` | | frame.rs:71-80 (`video_prepare_commands`) |
| panel_on | `58 04 29 00 01` | | frame.rs:71-80 |
| announce_refresh | `58 0d 1a 05 02 FPS` | 72, 90 | frame.rs:71-80 |
| announce_gpu | `58 0d 1a 05 05` | | frame.rs:71-80 |
| stop | `58 04 1a 05 04` | | reset.rs:53-60 |
| read_serial / read_info / read_version | `58 04 16 SS 1e`, `58 03 50 SS`, `58 03 03 SS` | SS ∈ {0, 2, 3} | frame.rs:86-93 (`attach_commands`); rust/pimax/src/session.rs:481,487 |
| read_doc_controller | `58 07 0a SS 00 00 00 10` | SS ∈ {2, 3} | frame.rs:86-93 |
| read_calib_block | `58 07 0a 00 01 <addr u16le> 10` | any addr | rust/pimax-proto/src/calib.rs:38 |
| eye_power_on | `58 05 3c 05 00 01` | once per enumeration, 5 s | rust/pimax-eye/src/power.rs:17-22 |
| ipd_motor | `58 06 5c 05 01 <mm×100 u16le>` | 5800..7200 | rust/pimax-eye/src/ipd.rs:40-45 |
| start_pair | `58 04 04 00 SS` | SS ∈ {2, 3}, once per request, 30 s | rust/omarchyvr-autod/src/pairing.rs:88-93 |
| haptic | `58 09 07 SS AMP DUR(u16le) 00 00 00` | SS ∈ {2, 3}, AMP 1..100, DUR 1..65535; session policy | frame.rs:101-104; rust/omarchyvr-tools/src/buzz.rs:63-66 |
| unpair | `58 03 05 SS` | SS ∈ {2, 3}, once per request, 1 s | rust/omarchyvr-autod/src/unpair.rs:12-18; frame.rs:57 |
| sys_reset | `58 04 28 00 00` | once per request, 30 s | frame.rs:52; reset.rs:48 |
| camera_ae | `58 19 51 00 01 32` + 10 × u16le | exposure 100..4108, gain 13..215 | rust/pimax-cam/src/ae.rs:118-129 (limits in `AeParams::default`, ae.rs:560-575) |

Denied with a reason:
- `58 ?? 19` DeviceCheckResult;
- `58 ?? 3c 05 00 00` eye-module disable (never captured);
- `58 ?? 55` and `58 ?? 56` activation blobs.

Not listed, so the gate would refuse them:
- the visor probe `58 04 2c 00 00` (built in `rust/omarchyvr-autod/src/visor.rs:14`). It is not in `device.toml`
  and needs the user's approval before it can be added. Today the live probe in `visor.rs` still sends it with a raw
  write (see the current state below);
- any other 0x5b blob;
- anything unknown.

The eye-chip calibration ops (0x3f2/0x407/0x42e/0x3fc/0x44c/0x456) go over the Tobii bulk endpoint, not this HID
interface. pimax-eye keeps refusing them. A later `[[usb_bulk]]` allowlist could cover them the same way.

#### Current state (as of 2026-10-17)
The allowlist is a tested library. It is not yet on the production write paths. Checked at HEAD:

- **`Gate::send` is only called from tests.** `Gate::new` appears in `rust/pimax-device/tests/allowlist.rs:19,170,216`
  and nowhere else in `rust/`. The module report builders (`ipd_report`, `pair_report`, `ae_report`) are also not
  called from outside `pimax-device`.
- **The session writes without the gate.** `Shared::write_ctl` (`rust/pimax/src/session.rs:278`) calls
  `hid::write_report` directly. Keepalive, wake, the attach burst and `send`/`request` all go through it.
- **Raw writers open the hidraw node and write bytes they built themselves:**
  - `send_motor`, the IPD motor (`rust/pimax-eye/src/ipd.rs:48`);
  - `send_enable`, the eye power-on (`rust/pimax-eye/src/power.rs:105`);
  - `write_start_pair` (`rust/omarchyvr-autod/src/pairing.rs:220`);
  - `write_unpair` (`rust/omarchyvr-autod/src/unpair.rs:21`);
  - the visor probe loop (`rust/omarchyvr-autod/src/visor.rs:30`, via `pimax::hid::write_report`);
  - `write_direct`, the controller buzz (`rust/omarchyvr-tools/src/buzz.rs:63`).
  - The headset restart in `rust/omarchyvr-tools/src/vr/reset.rs` also writes with `pimax::hid::write_report` (line 820).
- **The C ABI accepts any command.** `pimax_send` and `pimax_request` (`rust/pimax-capi/src/lib.rs:358,372`) pass the
  caller's `cmd`, `opt` and data to the session unchanged. Monado links this library, so the C driver can send any
  command.
- **The IPD range is only checked by callers.** `motor_report` (`rust/pimax-eye/src/ipd.rs:40`) does not clamp to
  58..72 mm. `raw_from_mm` (`ipd.rs:28`) clamps to the `u16` range only, and a NaN input becomes 0 (a Rust float to
  integer cast turns NaN into 0), which encodes as 0 mm.
- **Backlight packets are not in `device.toml`.** `backlight_packets` (`rust/omarchyvr-tools/src/vr/display.rs:94`)
  builds commands 0x26, 0x2a and 0x27. No `[[hid]]` entry covers them.
- **Camera exposure limits can be exceeded.** The normal limits in `AeParams::default` (`rust/pimax-cam/src/ae.rs:560-575`)
  stay inside the `camera_ae` ranges (exposure up to 4108, gain 13..215). The dim-room extended stage does not: it is
  on by default (`LightParams::default`, `ae.rs:747-765`) and raises the caps to the power-on values 10410 / 449
  (`ae.rs:947-962`; the test at `ae.rs:1442-1447` expects exposures above 4108). The environment overrides
  `PIMAX_CAM_AE_EXP_MAX` / `PIMAX_CAM_AE_GAIN_MAX` also allow up to 10410 / 449 (`ae.rs:622-629`). All of these are
  above the ranges in `device.toml`. Sending is off unless `PIMAX_CAM_AE=1`.

The fix is planned as [Step 5 of the week 1 safety plan](plans/WEEK1-SAFETY.md): send through the gate on the
write paths, and add the missing commands to the descriptor with their captures. The wider list of gaps is in
[SECURITY-STATUS.md](SECURITY-STATUS.md).

### 2.4 Mocks
The Dream Air SE, Crystal Light, Crystal Super and 8KX have descriptors with:
- every capability set to `unknown`, or `none` where the model is known to lack it;
- no `usb_pids`, only unverified `usb_pid_candidates` (from CustomHeadsetOpenVR via the portability report, section 5);
- no HID commands, so their `Gate` refuses everything. A test checks this.

The mock ids match `omarchyvr-gui-core/src/headsets.rs`.

### 2.5 Adding a headset (contributor flow)
1. Copy `devices/dream-air-se/` to `devices/<id>/` and fill in what you can verify. Give every value a source.
2. Run `pimax-device report` on your machine (a planned CLI, see section 6) and attach the report.
3. Commands: only bytes you captured from the vendor's own software, with the capture name and frame numbers in `source`. A maintainer approves each state-changing command (`approved = "..."`). Read-only queries come first.
4. Switch `status` to `real` only when detection, display and tracking have been shown working in a test report.

### 2.6 C ABI boundary
Monado is written in C. It calls into Rust through `extern "C"` functions that come from four static libraries. A bug
on this boundary runs inside Monado's process, so it can take the compositor down.

| Crate | Source of the exports | Header |
|---|---|---|
| `pimax-capi` | `rust/pimax-capi/src/lib.rs` (session, send, request, haptic, decoders) | `libpimax/include/pimax.h` |
| `pimax-capi` | `rust/pimax-capi/src/eye.rs` (eye tracker) | `rust/pimax-eye/include/pimax_eye.h` |
| `pimax-hands` | `src/ffi.rs`, `src/fuse_ffi.rs`, `src/inhand_ffi.rs` | `rust/pimax-hands/include/pimax_hands.h`, `pimax_ctrlfuse.h`, `pimax_inhand.h` |
| `pimax-lens` | `rust/pimax-lens/src/capi.rs` | `rust/pimax-lens/include/pimax_lens_rs.h` |
| `pimax-cam` | `rust/pimax-cam/src/capi.rs` | `rust/pimax-cam/include/pimax_cam.h` |

What holds at HEAD:
- **Only `pimax-cam` guards its calls.** Its `guard` helper wraps each call in `catch_unwind`
  (`rust/pimax-cam/src/capi.rs:21-33`) and turns a panic into an error string. No other crate on this list uses
  `catch_unwind`.
- **A panic elsewhere ends the host.** Rust aborts the process when a panic would unwind out of an `extern "C"`
  function. For Monado that means the whole compositor exits.
- **Float input is not checked in the hands code.** The functions in `pimax-hands/src/fuse_ffi.rs` (for example
  `pimax_ctrlfuse_optical`, line 120) convert the caller's poses and pass them on. The fuser calls
  `partial_cmp(..).unwrap()` in `pimax-hands/src/fuse.rs:234,313,424`, which panics if a value is NaN.
- **ABI version functions.** Present: `pimax_eye_abi_version` (`pimax_eye.h:63`), `pimax_hands_abi_version`
  (`pimax_hands.h:69`) and `pimax_lens_rs_abi_version` (`pimax_lens_rs.h:27`). Missing: `pimax.h`, `pimax_cam.h`,
  `pimax_ctrlfuse.h` and `pimax_inhand.h` have none, so a C caller cannot check that it was built against the same
  layout.
- **`pimax_send` and `pimax_request` take any command** (see [2.3](#23-the-hid-allowlist)).

Planned in [Step 3 of the week 1 safety plan](plans/WEEK1-SAFETY.md): a panic guard on every exported function, NaN
and range checks at the boundary, and version functions for the headers that lack them.

## 3. System checks

### 3.1 Contract (`src/check.rs`)
- `trait Check { id(), area(), run(&Ctx) -> Vec<Finding> }`.
- `Ctx` holds the `Host` (real or fake), omarchyvr-doctor's `Opts` and the matched module.
- A check is pure and read-only. Through `Host` it can only read files, list directories, check access and run read-only commands.
- `Finding` has `check`, `area`, `severity` (Info < Ok < Warn < Fail, the same order as doctor's `Level`), `title`, `detail` and `fix: Option<FixRef>`.
- `FixRef` is a catalogue id plus arguments. `CATALOGUE` is a fixed list of `Fix` entries, each with:
  - `why`;
  - `root` and `reboot` flags;
  - command templates;
  - undo templates;
  - argument names.
- `FixRef::plan()` renders the commands. It refuses unknown ids, extra or missing arguments, and any argument that is not a plain path or word (`safe_arg`: no spaces, quotes, `$`, `;`, `|`, backticks or `..`).
- Root fixes are shown first. The GUI offers Copy and "Run with pkexec…", and nothing runs as root until the user confirms the password prompt (LESSONS row 21). How that prompt is built is in [3.3](#33-privilege-boundary).

### 3.2 Check modules (one file each under `src/checks/`)

Only part of this table is built. The files in `rust/pimax-device/src/checks/` are `cpu.rs`, `gpu.rs`, `runtime.rs`
and `doctor.rs`. The native checks that run today are `gpu.merged_head_sync`, `cpu.cap_sys_nice`, `cpu.governor` and
`runtime.steam_vr_games`. The rest of the table is the plan. Those areas are covered by `DoctorBridge` or not at all.

| Area | Checks | Source today |
|---|---|---|
| drivers | NVIDIA/AMD/Intel driver and version, open vs proprietary module, DKMS state, the three patches applied, kernel params (`nvidia-drm.modeset=1`, modprobe config), udev rule installed, hidraw/usb nodes writable | doctor `gpu`, `patches`, `udev` |
| gpu | model and VRAM (`/sys/class/drm/card*/device`), DRM lease protocol (`wp_drm_lease_device_v1`), connector with the module's EDID vendor/product and mode, `merged_head_sync` (**ported: `gpu.merged_head_sync`**), Xid / display-engine lines this boot | doctor `connector`, `lease`, `merged-head-sync`, `kernel-log` |
| cpu | governor (**new: `cpu.governor`**), core count, `cap_sys_nice` on every monado-service build (**ported: `cpu.cap_sys_nice`**) | doctor `setcap` |
| audio | PipeWire running (`pw-cli info 0`), a sink/source matching the module's `sink_match`/`source_match`, whether it is the default, and a fix that switches the default (`wpctl set-default`, undo to the previous id) | new |
| usb | headset on a SuperSpeed port (`/sys/bus/usb/devices/*/speed` ≥ 5000), on the same hub as the camera, keepalive service active, no other process on `/dev/video*` | doctor `headset`, `keepalive`; LESSONS 2 |
| runtime | active OpenXR runtime (`~/.config/openxr/1/active_runtime.json` → Monado), `openvrpaths.vrpath` → xrizer and read-only, stale monado-service | doctor `lease`, `stale-service`; LESSONS 20 |
| device | the module's own checks: calibration files present, lens.conf serial, firmware versions against `firmware_seen` | doctor `lens-conf`, `tobii` |

`checks::doctor::DoctorBridge` runs all of the omarchyvr-doctor checks (`run_checks`, `rust/omarchyvr-doctor/src/checks.rs:204`) and maps each one to a `Finding`. The doctor's
free-text fixes become catalogue `FixRef`s where an entry exists: setcap, udev, keepalive, patches, kernel-log,
stale-service. Other fixes stay as text in `detail`. This lets the GUI move to `Finding` before the checks themselves
move.

### 3.3 Privilege boundary
Some fixes and the system setup need root. The app asks for it through polkit's `pkexec`. This is how it works at
HEAD, and where it is meant to go.

**Today**
- **App fixes.** `pkexec_cmd` (`rust/omarchyvr-gui-core/src/cmd.rs:163`) builds `/usr/bin/pkexec /bin/sh -c <script>`
  from the lines the app showed to the user. The script text is checked by a text checker, `check_root_script`
  (`rust/omarchyvr-gui-core/src/safety.rs:343`). That checker runs inside the unprivileged app. Its own header calls it
  a guard against mistakes in the app's command builders, not a sandbox (`safety.rs:18-19`).
- **setcap.** The one-click fix runs `/usr/bin/pkexec /usr/bin/setcap cap_sys_nice+ep <service>`
  (`rust/omarchyvr-gui-core/src/caps.rs:38-43`). The service path is resolved and checked first.
- **System setup.** The root job is `omarchyvr-root.sh apply <plan>` through one pkexec prompt. The app runs the copy in
  the user's data directory (`root_script`, `rust/omarchyvr-gui-core/src/install.rs:45`). The root-owned copy in
  `/usr/local/lib/omarchyvr` (`ROOT_SCRIPT_INSTALLED`, `install.rs:14`) is used only for the uninstall.
- **The plan file.** `installer/omarchyvr-root.sh` reads the plan file (`PLAN`, lines 78-90) as root. The user can write
  that file. The script accepts only known keys, and each value must match a character set
  (`^[][A-Za-z0-9_./:@+\ -]*$`, line 86). It does not check what the values mean.
- **Policy.** The repo has no polkit policy file, so pkexec shows its generic prompt for `/bin/sh`.

**Target**
- A root-owned helper installed once, with a short list of fixed verbs instead of a shell script.
- Checks of what a request means, done by the helper as root, not by the app.
- A polkit action for the helper, so the prompt names it and the policy can be audited.

This is [Step 7 of the week 1 safety plan](plans/WEEK1-SAFETY.md). See also [SECURITY-STATUS.md](SECURITY-STATUS.md).

## 4. Onboarding flow

> Since 2026-10-17 the first four steps (detect, checks, fixes, udev and the rest of the root job) are one automatic run in the
> app (`omarchyvr_gui_core::autosetup`, see [GUI.md](GUI.md#setting-up-your-pc-the-automatic-setup-was-onboarding)). The app filters
> them out of the module's step list: only IPD, pairing, eye calibration and the room map stay as optional steps after Ready.

```
detect → match module → checks → fixes → udev → IPD → controller pairing → eye calibration* → room map → done
                                                                          (* if eye_tracking works)
```

- **Steps:** each module lists the steps it supports (`[onboarding] steps`). The mocks only list detect, checks, fixes, udev and done.
- **Progress file:** progress is stored in `~/.local/state/omarchyvr/onboarding.state` (`$XDG_STATE_HOME`), as plain `step = done|skipped|failed` lines plus `module = <id>`. The code is `src/onboarding.rs` (`Progress::{load, save, next}`).
  - The file is written atomically.
  - Unknown lines are ignored.
  - Deleting the file starts over.
- **Resume and skip:** every step can be resumed and skipped. `next()` returns the first step that is neither done nor skipped, so a failed step comes back. Each step's page has Skip.
- **What each step does** (all through existing tools; the GUI never sends HID itself):

| Step | How |
|---|---|
| detect | `facts.rs` USB list → `pimax_device::detect` |
| checks | `default_checks()` plus `DoctorBridge` |
| fixes | each Warn/Fail finding with a FixRef. User fixes run through the job runner; root fixes show their lines, Copy and pkexec |
| udev | the `udev.install_rules` fix, then re-check hidraw access |
| ipd | `pimax-eye ipd` measure, then apply once (`ipd_motor`) |
| pair | `omarchyvr-autod pair left/right` (`start_pair`), at most once per 30 s per hand |
| eye_cal | `pimax-eyecal` |
| room | the Room row: `omarchyvr-roommap`, or skip |

## 5. Autofix (LLM) page

1. **Collect:** gui-core `bundle.rs` collects:
   - the findings, as JSON from `Finding`;
   - the last 200 lines of the session logs;
   - versions: kernel, driver, Monado commit, module id, firmware seen.

   `redact.rs` (`Redactor::from_system(env, serials)`) then scrubs serials (headset and controllers, from `read_serial`), the user name, the home path, the hostname, the machine id and any API key.
2. **Show:** the page shows exactly the bytes that would be sent: the bundle text, the prompt and `autofix::catalogue_prompt()`. It sends nothing until the user presses Send.
3. **Backends:**
   - a local LLM if one is found. llama.cpp `llama-server` on `127.0.0.1:8080` or ollama on `127.0.0.1:11434` is probed with an HTTP GET to localhost only. Nothing is installed.
   - Claude, with the user's own API key. The key comes from `ANTHROPIC_API_KEY`, or is pasted into the session and kept in memory, or is read from a file of the user's choice outside the repo with mode 0600. It is never written into the repo, the bundle or the logs, and the redactor gets it as a secret.

   Default model: `claude-opus-5-5`, with `claude-sonnet-5-5` offered as the cheaper option. Check these IDs against Anthropic's models list when this is built. Model IDs live in one constant so they can be updated.
4. **Reply contract** (`src/autofix.rs`, tested): the model may answer only with `EXPLAIN <text>` and `FIX <catalogue id> arg=value…` lines.
   - `parse_reply` drops anything else as `Rejected`, and the rejected lines are shown greyed.
   - A FIX is accepted only if `FixRef::plan()` succeeds.
   - At most 5 fixes are accepted per answer.

   There is no way to express free-form shell.
5. **Approve:** each accepted fix is shown with its commands, undo, root and reboot flags. The user approves fixes one at a time; user-level fixes run through the job runner, root fixes go through Copy or pkexec. Each one is appended to `~/.local/state/omarchyvr/fixes.log` (`autofix::log_line`, tab-separated: time, id, outcome, commands, undo), and the page lists "Undo" for every logged fix.
6. **Contribute** (nothing is uploaded automatically):
   - A working fix or a new module becomes a local git branch `contrib/<date>-<id>` in the user's clone. It holds the descriptor or check change plus `reports/<date>-<module>.toml` (the test report, 6.2) and a `PATCH.md` with what changed, the evidence and the fixes.log lines.
   - The page prints `git format-patch` and `gh pr create` lines for the user to run.
   - A missing catalogue fix becomes a proposal: a new `Fix` entry in a patch, never something to run now.

## 6. Open-source contribution model

### 6.1 Repo layout (target)
```
rust/pimax-device/devices/<id>/device.toml   descriptor (+ QUIRKS.md, fixtures/*.rec.gz replay captures, scrubbed)
rust/pimax-device/src/devices/<id>.rs        only if the module needs code
rust/pimax-device/src/checks/<area>.rs       checks
reports/                                     contributors' setup reports (TOML), one per PR that claims "works"
AGENTS.md, CONTRIBUTING.md                   rules (6.4)
```

### 6.2 "Test your setup" report (`pimax-device report`, planned; plain TOML)
```toml
[report]
version = 1
date = "2026-10-14"
module = "dream-air"
omarchyvr = "<git sha>"
monado = "<git sha>"
[host]
distro = "Arch"
kernel = "7.2.5"
compositor = "Hyprland 0.56.2"
gpu = "RTX 4090"
driver = "nvidia-open 610.57.04"
patches = ["viewport", "flip-fix", "bounded-wait"]
[findings]                       # one line per check, redacted
"cpu.cap_sys_nice" = "ok: monado-service has cap_sys_nice"
[[result]]                       # what the contributor tried and what happened
feature = "6dof"
status = "works"
note = "600/605 poses, desk start --6dof"
```
No serials, user names or paths. The redactor runs on it, and CI rejects any report that matches a serial pattern
(`P5[0-9]{2}[A-Z0-9]{8,}`, `H30[0-9]{3}P[0-9]{9,}`).

### 6.3 CI (planned; offline only, no device, no GPU)
There is no CI yet. The repo has no `.github/` directory and no other CI configuration. Tests run by hand on a
developer machine. This is what CI should run once it exists:
- `cargo test --workspace` and `cargo clippy -D warnings`.
- Descriptor lint: every `devices/*/device.toml` loads and validates, every `[[hid]]` has a `source`, and every state-changing command has `approved`. Loading every descriptor is already exercised by a test (`registry_has_one_real_module_and_four_mocks`, `rust/pimax-device/tests/allowlist.rs:29`); CI would run it.
- Replay fixtures: the `*.rec.gz` HID captures are run through the decoders. A new `allowlist_replay` test (not written yet) checks that every host→device report in the fixtures passes the module's allowlist, so the allowlist and reality can't drift apart.
- Doctor `FakeHost` scenarios for every check.
- A secret and serial scan over the diff (the patterns above, plus `sk-ant-`, `ghp_` and `/home/<user>`). The repo already has local git hooks for this (`tools/git-hooks/`); CI would run the same scanners.

### 6.4 AGENTS.md / CONTRIBUTING (rules for humans and LLM agents)
- Only send bytes seen in vendor captures. Every `[[hid]]` cites its capture, and state-changing commands need a maintainer's approval line.
- **Never flash firmware.** Never send device-side calibration writes (eye blob 0x456 and friends) or activation blobs.
- **Never force a DRM hotplug** (`/sys/class/drm/*/status`). Never SIGKILL Monado; stop it with `omarchyvr vr stop --grace 30`, and kill by PID only, never by name pattern.
- No silent root: root actions are catalogue fixes that the user runs.
- New fixes are reviewed code (an entry in `CATALOGUE` with undo), never shell in prose.
- Don't commit vendor binaries, firmware, DLLs, crash dumps, raw USB captures with serials, or factory calibration files. Fixtures must be scrubbed, with serials replaced by `P511XXXXXXXX`-style masks of the same length.
- Agents work offline against `FakeHost` and replay fixtures. A live headset or GPU test happens only when the human asks for it, and is written up in a test report.
- Don't touch other people's uncommitted work. Keep one change per PR.

### 6.5 License and risks
- **Code:** `MIT OR Apache-2.0` (already the workspace license). The Monado patches stay under Monado's BSL-1.0, the NVIDIA kernel patches under their upstream licence (MIT/GPL dual, as the open module).
- **Protocol facts reverse engineered for interoperability:**
  - This is generally defensible (EU Software Directive art. 6; US DMCA §1201(f) interoperability).
  - Publish findings and our own code only, never Pimax code, decompiled sources or binaries.
  - The eye-tracker challenge/response analysis touches an authentication mechanism. Keep it private or reduce it to "not needed"; that is the highest legal risk.
- **Vendor material:**
  - Pimax Play DLLs, `pi_server` crash dumps, SteamVR driver resources, firmware updater logs and raw captures all stay out of the repo (they are in `pimax-data/` and `captures*/`, which are git-ignored).
  - Factory calibration files and any capture-derived file with serials stay out of the repo, or are scrubbed first. There is no `calib/` directory at HEAD.
  - Rewriting history is necessary, because removed files stay in history. The simplest route is a **fresh public repo** made from a scrubbed tree; the private repo keeps its history.
- **Trademarks:** "Pimax", "Dream Air" and "Crystal" are used only nominatively ("for Pimax headsets"), with no logos and a "not affiliated" line in the README.
- **Safety:** a user's headset can be bricked by bad commands. The allowlist and the "no flashing" rule are the mitigation; add a disclaimer and no-warranty notice.

## 7. GUI integration

Status at HEAD: items 1 to 4 are built (the model list in `rust/omarchyvr-gui-core/src/headsets.rs`, the onboarding, checks and Autofix pages in `rust/omarchyvr-app/src/pages/`). The text below is the original design.

1. **Model list:** `omarchyvr-gui-core/src/headsets.rs` replaces `MODELS` with `pimax_device::all()`.
   - Capability labels come from `Support::label()`.
   - `detect` uses `pimax_device::detect(vid, pid)` on `facts.rs`'s USB list.
   - `Possibly(m)` shows "possibly <name> (not supported yet)".
2. **Onboarding page:** replaces the first-run wizard. It is a stepper over `module.descriptor().onboarding`:
   - state comes from `onboarding::Progress::load(Progress::path(env))`;
   - each step's page has Run, Skip and Back;
   - "Start over" deletes the file.
   - The IPD, pair, eye_cal and room steps call the existing tools (pimax-eye ipd, omarchyvr-autod pair, pimax-eyecal, omarchyvr-roommap) through the job runner.
3. **Setup and Checks page:** shows `Vec<Finding>` grouped by `Area`, from `default_checks()` plus `DoctorBridge`.
   - The Fix button calls `FixRef::plan()`.
   - User fixes run in the job runner. Root fixes show the lines with Copy and pkexec, and must never run silently.
   - Every applied fix is appended to `fixes.log` and gets an Undo button.
4. **Autofix page**, with four sections:
   - (a) backend picker: local LLM found / API key from env or a pasted key; the key is never saved in the repo, and saved elsewhere only with an explicit choice;
   - (b) "What will be sent": the redacted bundle, read-only, with its byte count;
   - (c) Send, then the answer rendered from `autofix::parse_reply`: EXPLAIN lines as text, FIX cards with approve and decline, rejected lines greyed;
   - (d) Contribute: make the branch and report locally and show the `git` / `gh` lines.
   - No network call happens before Send. The page has its own log line per request: size, backend and model, never the content.
5. **Tests to add in gui-core:**
   - a demo onboarding run over a temp state dir;
   - an Autofix page that sends nothing without Send (the mock backend counts calls);
   - root fixes never run without pkexec confirmation.

## 8. Migration plan

Status at HEAD: steps 1 and 2 are done. Steps 3 and 4 are partly done (four checks ported, `Host` still lives in `omarchyvr-doctor`). Steps 5 to 9, which move the write paths onto the gate, are not started. They are the same work as [Step 5 of the week 1 safety plan](plans/WEEK1-SAFETY.md).

| Step | Change | Risk |
|---|---|---|
| 1 | GUI uses `pimax_device::all()` / `detect` for the model list (headsets.rs) | none, read only |
| 2 | GUI Setup page shows `DoctorBridge` + `default_checks()` findings; doctor CLI unchanged | none |
| 3 | Move doctor checks one at a time into `pimax-device/src/checks/<area>.rs` (order: setcap → merged-head-sync (done as ports), kernel-log, stale-service, lease, udev, keepalive, connector, patches, gpu, headset/tobii, lens-conf). Doctor's `run_checks` then calls the new checks and converts `Finding` → `Check` so its CLI/JSON stay identical; doctor's tests keep passing | low; one check per commit |
| 4 | Move `Host`/`FakeHost` from omarchyvr-doctor into pimax-device (re-export from doctor), flip the dependency | low |
| 5 | `pimax-eye ipd --apply` builds via `module.ipd_report()` and sends via `Gate` (replaces `motor_report` + raw write); same bytes, pinned by the existing test vectors | medium: touches a device path; test with the user present |
| 6 | `omarchyvr-autod` pairing: `pair_report` + `Gate`; keep the IF3 log watch | medium; user present |
| 7 | `pimax-cam` AE: `DreamAir::ae_report` + `Gate` | medium |
| 8 | `omarchyvr` session (keepalive, wake, video prepare, attach burst, calib reads) sends through a `Gate` held by the session writer thread | higher: the keepalive is timing critical; `Gate::send` is a linear scan over ~16 specs (sub-µs), measure first |
| 9 | Monado's C driver sends through the Rust C ABI (`pimax-capi`) → add `pimax_gate_send` and make the C side use it | highest; last |

## 9. Hard-coded Dream Air specifics found (to move into the descriptor over time)

The file and line numbers in this table come from 2026-10-14 and may have drifted. Search for the constant before relying on a line.

| What | Where |
|---|---|
| USB IDs 34a4:0044 (and 0043 bridge, 0014 cameras) | pimax-proto/src/lib.rs:35-41; omarchyvr-doctor lib.rs consts; pimax-eye/src/ipd.rs:81 (`HID_ID=0003:000034A4:00000044`); pimax-cam/src/v4l2.rs:37-39; omarchyvr-tools/src/vr/reset.rs:67; pimax/src/discovery.rs:54,93,112 |
| Eye tracker 2104:0561 | pimax-eye/src/replay.rs:21-23; pimax-eye/src/power.rs:64-69 |
| EDID PVR / 0x1019 | omarchyvr-tools/src/vr/edid.rs:5-7; doctor `Connector::is_pimax` |
| HID commands | section 2.3 |
| Display mode 7104x3840, 72/90 Hz, panel 3552x3840 | pimax-lens/src/pvrfit.rs:1716,1731; omarchyvr-tools/src/vr/config.rs:30-58; aspect 3840/3552 in pimax-lens (config.rs:516, lib.rs:306, model.rs:302, capi.rs:207,274, mask.rs:171) and the lenstest crates |
| Camera layout 4×640x480 tiles in 2560x832 | pimax-cam/src/frame.rs:13-28, v4l2.rs:389-411 |
| IPD 58-72 mm | pimax-eye/src/ipd.rs:16-17, ipd_watch.rs:194 |
| Lens defaults, `lens.conf` | pimax-lens/src/config.rs:481-485, bin/pimax-lens.rs:10 |
| Controller slots 2/3 | pimax-proto/src/lib.rs:49-51 |
| Calibration and user paths | ~/.config/omarchyvr/{lens.conf, ipd.env, eye-calibration.toml, auto.toml}, ~/.cache/omarchyvr/gyro-bias, ~/.local/share/omarchyvr/{roommap, eye-calibration, slam-recordings}; pimax-ctrltrack/src/model.rs:103,120 falls back to a Windows dump under `<repo>/pimax-data/` (git-ignored, never shipped) |
