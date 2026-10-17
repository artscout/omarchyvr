# Week 1: safety and correctness fixes (test-first)

Status: Planned, not started. Written 2026-10-17 at commit 7660e74.

## Do the current tests catch these bugs?

No. None of the existing tests fails for any of the seven problems below. Some existing tests protect behaviour
that must stay, and a few will break on purpose because they pin the old behaviour. So every step is red, then green:

1. Write a test that fails today and proves the bug.
2. Keep the listed guard tests green.
3. Fix until the new test is green.

| # | Problem | Existing test that catches it | Guards that must stay green | Tests that change on purpose |
|---|---|---|---|---|
| 1 | The kernel-log reader dies on non-UTF-8 input | none | `klog_follower_streams_lines` (omarchyvr-components/src/events.rs) | none |
| 2 | IPD NaN becomes 0 mm; bounds are checked only in callers | none | the capture-byte tests in pimax-eye/src/ipd.rs; `clamps_to_58_72` in pimax-ipd/src/manual.rs | none |
| 3 | A panic in an FFI function aborts Monado; NaN reaches `partial_cmp().unwrap()` | none (no test feeds NaN to the FFI; `fuse_ffi.rs` has no tests) | pimax-hands `tests/c_abi.rs`, `tests/properties.rs` and the unit tests in `fuse.rs`; pimax-capi `tests/abi.rs` and `tests/hands_export.rs`; pimax-cam `tests/c_abi.rs` | none |
| 4 | "Atomic" writes can collide, are not durable, and are created 0644 | none (`omarchyvr-components/src/util.rs` has no tests) | the config store tests, the `steam_setup` tests, the anchor lease tests, `missing_file_means_defaults_and_save_is_atomic` (record), `save_and_load_keep_both_hands` (fuse) | none |
| 5 | The HID allowlist is never enforced on writes | none (no writer depends on pimax-device) | pimax-device `tests/allowlist.rs`; the fake-session tests in `pimax/tests/session.rs`; pimax-proto `tests/framing.rs` and `tests/props.rs`; pimax-capi `tests/abi.rs` | the 0x2c request in `pimax/tests/session.rs` (around line 227, expects a timeout); `pimax-capi/tests/c/session.c:143` |
| 6 | Voice: "careful" commands never ask by default, typing is not gated, logs are 0644 | none for the default path (tests build `Actor::new`, which defaults to `Dangerous`; the daemon uses the config default `None`, see `omarchyvr-voice/src/main.rs:725`) | `lock_needs_a_spoken_yes_in_time` and `closing_needs_a_spoken_yes` (act.rs); `careful_tool_waits_for_yes`, `claude_cli_careful_calls_still_ask_for_yes`, `logs_rotate_and_redact` and `tools_come_from_the_catalogue_with_safety` (agent.rs); the state tests that force `None` | the default-is-`None` assertion at `omarchyvr-config/src/ai.rs:1907` |
| 7 | Root runs a script the user can write to; there is no polkit policy | none | gui-core `install.rs` tests (`the_setup_fix_runs_exactly_the_root_job_and_passes_the_safety_checks`, `the_safety_rules_refuse_other_root_scripts_and_modes`); `every_fix_command_passes_the_safety_rules` in `checklist.rs`; the tests in `caps.rs` and `safety.rs`; the `installer/test_*.sh` scripts (no runner calls them today) | `omarchyvr-gui-core/src/autosetup_tests.rs` around line 320 |

## Ground rules

- `.cargo/config.toml` routes every cargo build, test and run in this repo through `tools/vr-guard.sh` and
  `tools/vr-busy.sh`. During a VR session (`monado-service`, `vrserver` or `vrcompositor` running) cargo refuses
  with exit code 75. Otherwise it runs at nice 19. Run all builds and tests between sessions, with `-j8`.
- One commit per step: the test and the fix together. The order goes from smallest risk to largest.
- Steps 3 and 5 change `libpimax_capi.a`. After them, relink Monado, check the feature markers and `cap_sys_nice`.
- Install only at the end, as a whole set, from a clean build of committed HEAD.

## Step 0: baseline and one test runner (small)

Add `tools/test-all.sh`. It runs `cargo test --workspace --no-fail-fast`, every `installer/test_*.sh` (exit code 77
means skipped) and `tools/leak-scan-test.sh`. It prints PASS, FAIL or SKIP per test and compares the result with
`tools/known-test-failures.txt`. Run it on HEAD and keep the result as the baseline. After every later step: the
new tests are green, the baseline failures are unchanged, and there are no new failures.

## Step 1: kernel-log follower (small)

File: `rust/omarchyvr-components/src/events.rs`.

Red tests:
- `klog_survives_a_non_utf8_line`: feeding `a\n\377\376\nb Xid 13\n` must give 3 events. Today only `a` arrives.
- `klog_is_not_alive_when_the_reader_ended`: `alive()` stays true today.
- `klog_respawn_backs_off_and_retries_after_a_failed_spawn`, in `rust/omarchyvr-autod/src/daemon.rs` (the klog
  block near line 426). Today a failed spawn leaves klog empty forever, with no backoff.

Fix: read with `read_until` and `from_utf8_lossy`. Add a reader-done flag that `alive()` checks. Log when the reader
ends. Add a backoff retry (5 s up to 60 s). Kill the child if the reader thread fails to start.

## Step 2: IPD motor bounds in the sender (small)

File: `rust/pimax-eye/src/ipd.rs`.

Red tests:
- `motor_report_refuses_nan_and_out_of_range`.
- A proptest, `any_f32_bits_never_leave_58_72`.
- `send_motor_refuses_while_a_session_runs` (fake `/proc` root, injected writer).

Fix: `motor_report` returns a `Result` and accepts only finite values from 58 to 72 mm. `send_motor` checks for a
running session on every send. The callers keep their own user-facing clamp: `pimax-eye/src/cli.rs` (near lines 825
and 963), `pimax-eye/src/ipd_watch.rs` (near 372), `pimax-ipd/src/devices.rs` (near 85) and
`pimax-ipd/src/manual.rs` (near 395).

## Step 3: FFI panic guard, NaN, saves outside the lock (small to medium)

Add a new zero-dependency crate `pimax-ffi`. It holds the `guard()` that lives today in
`rust/pimax-cam/src/capi.rs` (near line 22) and a `finite()` helper.

Red tests:
- `pimax-hands/tests/ffi_nan.rs`, in its own test binary: NaN and infinite poses go into
  `pimax_ctrlfuse_optical`, then `get`. Today `fuse.rs:424` panics inside an `extern "C"` function, which aborts.
- A proptest over the exported float inputs. Reuse `any_f32` from `pimax-hands/tests/properties.rs`.
- `every_extern_c_fn_is_guarded`, a source-scan test.
- `save_trigger_fires_once_per_2000_samples`.

Fix: wrap every export in the guard. Reject non-finite inputs. Replace `partial_cmp().unwrap()` with `total_cmp`
outside tests. Do the file save outside the `Fuser` lock (`rust/pimax-hands/src/fuse_ffi.rs`, near line 109).

## Step 4: one atomic-write helper (medium)

Add a new zero-dependency crate `omarchyvr-fsutil` with:
- `write_atomic(path, bytes, Mode)`: a unique temp name `.<file>.tmp-<pid>-<n>`, `create_new`, the mode set at
  creation, fsync of the file and its directory, cleanup on error.
- `private_dir()`: creates a 0700 directory.

Red tests:
- `sibling_files_do_not_share_a_temp`: 8 threads write `x.pid` and `x.log`. Run it first against
  `omarchyvr_components::util::write_atomic` to show the collision.
- `private_files_are_0600_under_umask_022`.
- `failed_write_keeps_the_old_file_and_leaves_no_tmp`.

Migrate the hand-rolled helpers (about 12): `omarchyvr-components/src/util.rs:69`,
`omarchyvr-config/src/store.rs:593`, `omarchyvr-config/src/devicedata.rs:114`,
`omarchyvr-tools/src/vr/steam_setup.rs`, the room map, floor and setup stores in the room crates, the anchor config
and world files, the record settings, and `pimax-hands/src/fuse.rs` (`save_calib`, near line 589).
These files become private (0600, directories 0700): room maps, recordings, passthrough PNGs and voice logs.
The other rename sites (about 45) move to week 2.

## Step 5: the HID allowlist becomes the only way to write (medium to large)

Move `pimax-device/src/hid.rs` and `toml_lite.rs` (they depend only on each other) into a new zero-dependency crate
`pimax-hidgate`. pimax-device re-exports it, so `tests/allowlist.rs` runs unchanged. This avoids pulling
omarchyvr-doctor into `libpimax_capi.a`.

Red tests:
- `pimax/tests/allowlist_census.rs`: every `Session` flow on `pimax::fake` must pass the Dream Air allowlist.
- Census tests for tools, autod, eye and cam. They are red today: the backlight packets in
  `omarchyvr-tools/src/vr/display.rs` (`backlight_packets`) are not listed, and the camera auto-exposure dim-room stage (on by default in
  `pimax-cam/src/ae.rs`, `LightParams::default`) raises exposure and gain to 10410 / 449, above the allowed 4108 / 215.
- `session_refuses_unlisted_commands`.
- `pimax_send_returns_eperm_for_denied` (a C test).
- `no_raw_hidraw_writes_outside_the_gate`, a source scan. Today it fails for pimax-eye `ipd.rs` and `power.rs`, the
  autod pairing, unpair and visor code, and the tools buzz command.

Fix: `Session` checks the allowlist before `write_ctl`. `Error::Refused` maps to `-EPERM`. `write_report` becomes
`pub(crate)`. One checked `send_to_node` replaces the raw writers. The visor probe is refused. The command table in
docs/ARCHITECTURE.md is generated from the descriptor in a test. In Monado, confirm that `pimax_hmd_send_cmd` only
logs a refused auto-exposure push (auto-exposure is opt-in with `PIMAX_CAM_AE=1`).

## Step 6: voice defaults, typing gate, private logs (medium)

Red tests:
- `daemon_confirm_defaults_to_dangerous` (extract `confirm_from(cfg)` from `omarchyvr-voice/src/main.rs`).
- `careful_intent_asks_with_the_daemon_default`.
- `typing_into_a_terminal_asks_by_default`.
- `injected_title_cannot_type` (scripted model, using the agent test harness).
- `cloud_requests_strip_titles_by_default`.
- `http_cloud_url_is_refused`.
- `voice_logs_are_private_and_typed_log_rotates_and_redacts`.

Fix: default to `Dangerous` in the config, the daemon, the docs and the app help. Add a rule that text for
`type_text` and `key_press` coming from the agent must appear in the transcript. Strip window titles for cloud
providers. Rotate and redact `typed.log`. Allow only HTTPS for non-local URLs. Write logs 0600.

## Step 7: root helper owned by root, polkit policy (medium to large)

Design: the first setup (and an explicit "Update system parts" action) is the only time the user's copy runs as
root. It copies itself and its data to a root-owned directory under `/usr/local/lib/omarchyvr` and re-executes
from there. Every later prompt runs only the root-owned helper with a fixed verb. A polkit policy
(`/usr/share/polkit-1/actions/org.omarchyvr.policy`) names that helper.

Red tests (shell, in `installer/`):
- `test_root_plan_validation.sh`: `OVR_DATA=/tmp/x`, a bad NVIDIA version, an unknown package and `OVR_USER=root`
  must all be refused. Today the charset filter accepts them.
- `test_root_refuses_user_copy.sh`.
- `test_root_setcap_nofollow.sh`.

Red tests (Rust): `setup_fix_uses_the_root_owned_helper_once_installed`,
`safety_refuses_user_writable_root_helper_after_bootstrap`, `setcap_goes_through_the_helper_verb`,
`hooks_fix_copies_from_the_root_owned_dir`, `polkit_policy_names_the_installed_helper`.

Fix: `installer/omarchyvr-root.sh` installs itself and re-executes, and gains the verbs `validate`,
`setcap-service` and `hooks`. `OVR_USER` and the home directory come from `$PKEXEC_UID`. The code in
`omarchyvr-gui-core` (`install.rs`, `caps.rs`, `checklist.rs`, `autosetup.rs`) uses the verbs. `uninstall.sh` drops
its user-copy fallback. The package build installs the helper and the policy. The remaining
`pkexec /bin/sh -c` fixes become verbs in week 2.

## Verification (between sessions)

1. `tools/test-all.sh`: new tests green, baseline failures unchanged.
2. A clean build of committed HEAD. Monado relinked, all feature markers present, `cap_sys_nice` restored.
3. With a system snapshot taken first: run the system setup once (the dialog names omarchyvr), then Play into
   SteamVR. Check picture, brightness, IPD, controllers with haptics, that a "careful" voice command asks, and Room
   setup. Check that `omarchyvrd` reports the kernel log as on.
4. Look at the app screenshots of the AI and Setup pages.
5. Update the docs.

## Open decisions

- HID data: add the backlight commands (from the Windows brightness capture, see
  `analysis/windows-linux-command-diff.md`), and either clamp camera auto-exposure to the captured ranges or widen
  the ranges.
- `voice_confirm`: existing `ai.toml` files already say `none` (the app writes the key). Keep that, or migrate to
  `dangerous`.
- Root bootstrap model: trust the clone at first setup and use only root-owned files after that (recommended), or
  convert every `sh -c` fix to a verb now.
- The Monado and WayVR forks are private for now, so the public export cannot include `.gitmodules` until they are
  public.

## Docs and cleanup for week 2

- Rewrite the agent rule files (`CLAUDE.md`, `AGENTS.md`, `ai/`) for a public audience.
- Curate `analysis/`.
- Keep the lab-only docs (`OPEN-SOURCE-PLAN`, `RETRO`, `PARITY`, `PLAY-WORK-AUDIT`) out of the export.
- Split very long docs.
- Add a `CODE_OF_CONDUCT`.
- Add CI workflows: fmt, clippy, tests without lab data, cargo-deny and the leak scan.
- Turn the remaining `pkexec /bin/sh -c` fixes into fixed verbs.
- Add `.gitmodules` and the forks once the forks are public.
