# Architecture for the open-source release

Status 2026-10-14: a design, plus the scaffold crate `rust/pimax-device`, which is built and tested. Nothing else is
wired to it yet (see [Migration](#8-migration-plan)). The audit of what must leave the repo before it goes public is in
[../analysis/oss-readiness.md](../analysis/oss-readiness.md).

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

### 2.3 The HID allowlist (enforced at the send layer)

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

`Gate::send` (`src/hid.rs`) runs this check before its single `write`, so a refused report writes nothing. It also
enforces `min_interval`. `Allowlist::build(id, params)` runs the same check, so a report is checked once when it is
built and again when it is sent.

Dream Air commands (all on IF0, 64 bytes, zero padded). Sources:

| id | bytes | constraint | built today in |
|---|---|---|---|
| wake | `58 06 1a 05 01 00 00` | | pimax-proto/src/frame.rs:46 |
| keepalive | `58 04 39 00 RR 00` | RR ∈ {0, 0x48, 0x5a} | frame.rs:53; omarchyvr-tools/src/vr/reset.rs:26-30 |
| app_unlock | `58 05 5b 05 02 02` | | frame.rs:66 |
| panel_on | `58 04 29 00 01` | | frame.rs:67 |
| announce_refresh | `58 0d 1a 05 02 FPS` | 72, 90 | frame.rs:69 |
| announce_gpu | `58 0d 1a 05 05` | | frame.rs:70 |
| stop | `58 04 1a 05 04` | | reset.rs:27-28 |
| read_serial / read_info / read_version | `58 04 16 SS 1e`, `58 03 50 SS`, `58 03 03 SS` | SS ∈ {0, 2, 3} | frame.rs:77-80; pimax/src/session.rs:479,485 |
| read_doc_controller | `58 07 0a SS 00 00 00 10` | SS ∈ {2, 3} | frame.rs:79 |
| read_calib_block | `58 07 0a 00 01 <addr u16le> 10` | any addr | pimax-proto/src/calib.rs:38 |
| eye_power_on | `58 05 3c 05 00 01` | once per enumeration, 5 s | pimax-eye/src/power.rs:17-22 |
| ipd_motor | `58 06 5c 05 01 <mm×100 u16le>` | 5800..7200 | pimax-eye/src/ipd.rs:39-45 |
| start_pair | `58 04 04 00 SS` | SS ∈ {2, 3}, once per request, 30 s | omarchyvr-autod/src/pairing.rs:87-92 |
| camera_ae | `58 19 51 00 01 32` + 10 × u16le | exposure 100..4108, gain 13..215 | pimax-cam/src/ae.rs:30-36,118-129,386-389 |

Denied with a reason:
- `58 ?? 19` DeviceCheckResult;
- `58 ?? 3c 05 00 00` eye-module disable (never captured);
- `58 ?? 55` and `58 ?? 56` activation blobs.

Never listed, so refused:
- the visor probe `58 04 2c 00 00` (omarchyvr-autod/src/visor.rs:14; it needs the user's approval before it can be added);
- any other 0x5b blob;
- anything unknown.

The eye-chip calibration ops (0x3f2/0x407/0x42e/0x3fc/0x44c/0x456) go over the Tobii bulk endpoint, not this HID
interface. pimax-eye keeps refusing them. A later `[[usb_bulk]]` allowlist could cover them the same way.

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
- Root fixes are only ever shown: the GUI offers Copy and "Run with pkexec…", and the user runs them (LESSONS row 21).

### 3.2 Check modules (one file each under `src/checks/`)

| Area | Checks | Source today |
|---|---|---|
| drivers | NVIDIA/AMD/Intel driver and version, open vs proprietary module, DKMS state, the three patches applied, kernel params (`nvidia-drm.modeset=1`, modprobe config), udev rule installed, hidraw/usb nodes writable | doctor `gpu`, `patches`, `udev` |
| gpu | model and VRAM (`/sys/class/drm/card*/device`), DRM lease protocol (`wp_drm_lease_device_v1`), connector with the module's EDID vendor/product and mode, `merged_head_sync` (**ported: `gpu.merged_head_sync`**), Xid / display-engine lines this boot | doctor `connector`, `lease`, `merged-head-sync`, `kernel-log` |
| cpu | governor (**new: `cpu.governor`**), core count, `cap_sys_nice` on every monado-service build (**ported: `cpu.cap_sys_nice`**) | doctor `setcap` |
| audio | PipeWire running (`pw-cli info 0`), a sink/source matching the module's `sink_match`/`source_match`, whether it is the default, and a fix that switches the default (`wpctl set-default`, undo to the previous id) | new |
| usb | headset on a SuperSpeed port (`/sys/bus/usb/devices/*/speed` ≥ 5000), on the same hub as the camera, keepalive service active, no other process on `/dev/video*` | doctor `headset`, `keepalive`; LESSONS 2 |
| runtime | active OpenXR runtime (`~/.config/openxr/1/active_runtime.json` → Monado), `openvrpaths.vrpath` → xrizer and read-only, stale monado-service | doctor `lease`, `stale-service`; LESSONS 20 |
| device | the module's own checks: calibration files present, lens.conf serial, firmware versions against `firmware_seen` | doctor `lens-conf`, `tobii` |

`checks::doctor::DoctorBridge` runs all 13 omarchyvr-doctor checks now and maps each one to a `Finding`. The doctor's
free-text fixes become catalogue `FixRef`s where an entry exists: setcap, udev, keepalive, patches, kernel-log,
stale-service. Other fixes stay as text in `detail`. This lets the GUI move to `Finding` before the checks themselves
move.

## 4. Onboarding flow

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
pimax_re = "<git sha>"
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

### 6.3 CI (offline only; no device, no GPU)
- `cargo test --workspace` and `cargo clippy -D warnings`.
- Descriptor lint: every `devices/*/device.toml` loads and validates, every `[[hid]]` has a `source`, and every state-changing command has `approved`.
- Replay fixtures: the `*.rec.gz` HID captures are run through `omarchyvr` decoders. A new `allowlist_replay` test checks that every host→device report in the fixtures passes the module's allowlist, so the allowlist and reality can't drift apart.
- Doctor `FakeHost` scenarios for every check.
- A secret and serial scan over the diff (the patterns above, plus `sk-ant-`, `ghp_` and `/home/<user>`).

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
  - The eye-tracker challenge/response analysis (`analysis/eye-auth-feasibility.md`) touches an authentication mechanism. Keep it private or reduce it to "not needed"; that is the highest legal risk.
- **Vendor material:**
  - Pimax Play DLLs, `pi_server` crash dumps, SteamVR driver resources, firmware updater logs and raw captures all stay out of the repo (they are in `pimax-data/` and `captures*/`, which are git-ignored).
  - The tracked `calib/` factory files, and any capture-derived file with serials, must be removed or scrubbed (see the audit).
  - Rewriting history is necessary, because removed files stay in history. The simplest route is a **fresh public repo** made from a scrubbed tree; the private repo keeps its history.
- **Trademarks:** "Pimax", "Dream Air" and "Crystal" are used only nominatively ("for Pimax headsets"), with no logos and a "not affiliated" line in the README.
- **Safety:** a user's headset can be bricked by bad commands. The allowlist and the "no flashing" rule are the mitigation; add a disclaimer and no-warranty notice.

## 7. GUI integration (for the GUI agent; omarchyvr-app and omarchyvr-gui-core are not edited here)

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
| Calibration and user paths | ~/.config/omarchyvr/{lens.conf, ipd.env, eye-calibration.toml, auto.toml}, ~/.cache/omarchyvr/gyro-bias, ~/.local/share/omarchyvr/{roommap, eye-calibration, slam-recordings}; pimax-ctrltrack/src/model.rs:77-80 hard-codes a path with controller serials under pimax-data/ (must change before release) |
