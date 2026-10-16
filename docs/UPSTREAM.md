# Upstream contributions: plan and status

Status 2026-10-16: everything is **prepared locally, nothing submitted**. Each item waits for the user's review and
explicit go-ahead. Public identity for all submissions: `laughtt` (GitHub noreply address). No eye-tracking start
material, serials, captures, per-unit calibration, home paths or Mercury models go into any patch or text.

## NVIDIA open-gpu-kernel-modules
Upstream state: latest release 615.78.08 still has the 2Heads1OR viewport bug and the flip deadlock code paths.
Contributions need a GitHub PR plus the NVIDIA CLA (no DCO); NVIDIA re-applies changes internally.

| Item | Status |
|---|---|
| Issue: 2Heads1OR flips deadlock on a semaphore acquire (root cause, file:line) | drafted; A/B test with `merged_head_sync=0` should come first |
| PR: client flip sync objects on the primary merged head only (`merged_head_sync`) | branch on 615.78.08, builds; used daily, A/B not done: **needs hardware verification first** |
| PR: force-idle all merged window channels together | builds; path never exercised on hardware |
| PR: nvidia-drm revoke on file close (non-interruptible, stale grants) | builds; fixed path not reproduced on purpose |
| Viewport fix | is PR #1262 (rvdende): not resubmitted; "+1 with our reproduction" comment drafted |
| Bounded `nvEvoMakeRoom()` wait | rvdende's work: not resubmitted |
| Diagnostics part of our patch | kept local only |

Needs: a public fork of the NVIDIA repository, the CLA accepted by the user.

## WayVR (wlx-team/wayvr)
Rebased on upstream main; `cargo fmt`, build, clippy (no new warnings) and tests pass (the only failing test,
a missing German translation key, fails on upstream main too). Compile/unit tested only after the rebase.

| PR | Default |
|---|---|
| Ordered, bounded OpenXR shutdown and `_exit` after teardown (NVIDIA shutdown SIGSEGV, Xid 31, exit hang) | bug fix |
| Draw tracked hands (`show_hands: off/tracked_only/all`) | off |
| Screen number badges + `wayvrctl screen-numbers` | off |
| `wayvrctl screen-slots` / `screen-nudge` | no change unless used |
| `wayvrctl recenter` / `skybox` | no change unless used |

Not submitted: eye gaze (upstream added its own OpenXR eye gaze and handsfree pointer; follow-ups proposed:
One-Euro smoothing and dead zone, then dwell click), controller models, grab rework, camera-hand pointer (need
rework), app-specific watch buttons (excluded).
Needs: a **public** fork with a different name than the private `laughtt/wayvr`, e.g. `laughtt/wayvr-upstream`
(not created).

## Monado (freedesktop GitLab)
Policy: every commit needs a `Signed-off-by` that only a human can give (DCO 1.1), clang-format, no warnings,
a changelog fragment; large work starts with an RFC issue. Needs a gitlab.freedesktop.org account with fork rights.

Prepared (rebased on upstream main, build without warnings, ctest 27/27, clang-format clean, no sign-off yet):
- c/client: defer freeing destroyed Vulkan swapchain images until the GPU is done
- c/client: bounded wait for the last timeline semaphore value before destroying it
- t/slam: drop frames with a non-increasing timestamp
- c/main: debug dump of the final target image (env, off by default)
- st/steamvr_drv: input registration and uninitialised values

Outline (not prepared): an RFC issue for the headset driver, then (a) 3DoF HMD driver in plain C (HID protocol from
captures, lens model with parameters from a user config file, no per-unit data), (b) controllers, (c) 6DoF via
Monado's SLAM, (d) the stuck-flip handling as an RFC (NVIDIA-specific ioctls). Eye tracking is excluded: its start
needs replayed vendor material. The Rust static libraries would be replaced by C ports or an optional external
library. Legal: interoperability reverse engineering, protocol messages only (no firmware), BSL-1.0 headers.

## Order once approved
1. NVIDIA A/B hardware test, then the issue, then the three PRs and the #1262 comment.
2. WayVR: create the public fork, teardown PR first, then hands, badges, slots, wayvrctl.
3. Monado: account, sign-off, the small MRs one by one; RFC for the driver after they land.
