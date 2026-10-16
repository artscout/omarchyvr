# NVIDIA flip hang on the Dream Air (2Heads1OR + DSC)

Status 2026-10-14 ~11:00. Source analysis done, kernel patches and the Monado mitigation written and built.
**Nothing is tested on hardware yet.** Sources cited: ~/src/open-gpu-kernel-modules at tag 610.57.04
(NM = src/nvidia-modeset/src). Status page: analysis/nvidia-flip-hang.html.

## 0. Summary

- **Freeze.** Both merged window channels (c67e:0 and :2) block on a semaphore acquire that never matches. NVKMS
  executes the client's flip sync objects (acquire semaphore, release semaphore, completion notifier) on BOTH merged
  hardware heads. The userspace protocol assumes one acquire and one release per flip. When the two heads do not
  latch a flip on the same frame, the release from the head that is behind overwrites the acquire value of a later
  flip, and nothing can ever satisfy it. Confidence: the semaphore block about 90%, the duplicated sync objects as the
  cause about 75%.
- **Stuck after the kill.** When a process is killed with SIGKILL, nvidia-drm can skip the revoke of the leased
  connector (it takes the modeset locks interruptibly while the signal is pending). The connector stays granted to a
  dead file and every later `vkAcquireDrmDisplayEXT` fails. Our EDID override blocks the driver's own cleanup.
  Confidence about 85%.
- **Patches** (built in a private copy, not installed): `linux/nvidia-2h1or-flip-fix.patch` (fix + idle fix + revoke
  fix + diagnostics, runtime switch for A/B) and `linux/nvidia-evo-makeroom-bounded-wait.patch` (PR author's stop-gap).
- **Monado** `~/src/monado-flipsafe` branch `flipsafe` (02889c9 on hangfix e64d487): on a stuck flip it revokes the
  NVIDIA permission itself, skips the Vulkan teardown and exits with `_exit()`, so no SIGKILL is needed. ctest 27/27.

## 1. What the symptoms decode to

1. `Idling display engine timed out: 0x0000c67e:{0,2}:0:1174` = hwclass:instance:subdevice:errorToken
   (NM/nvkms-rm.c:3400, `SyncOneEvoChannel`, 2 s per channel). errorToken is `__LINE__` of the caller:
   NM/nvkms-flip.c:1174 is `nvEvoClearSurfaceUsage()`, which syncs EVERY layer channel of EVERY head whenever ANY
   client frees a display surface (NM/nvkms-surface.c:1184-1186; the C6 HAL has no ClearSurfaceUsage hook,
   NM/nvkms-evo3.c:7869). 0xc67e = NVC67E window channel (Ada uses the nvEvoC6 HAL, NM/nvkms-hal.c:150); instances 0
   and 2 = `head * 2` = the main-layer windows of hardware heads 0 and 1. So BOTH merged heads' base windows had
   methods pending. The message comes from any client freeing a surface while the windows are stuck (for example
   Hyprland dropping a framebuffer when a window closes, 4 s under the nvkms lock each time), not from Monado.
2. The PR #1262 author (comment 2026-07-28, Vive Pro 2 4896x2448@120, DSC DUAL, RTX 4090, 610.43.03, Monado direct mode
   on a Hyprland lease) has the same stall: `Error while waiting for GPU progress: 0x0000c67e:0 2:0:56:48`
   (count:free:get:put): the ring of window channel 0 is full and the hardware GET pointer stopped. In his commit
   message: "the Vulkan queue wedges first, the headset freezes with nothing in the kernel log, and roughly three
   minutes later the EVO channels stop retiring". Simple scenes run for a long time, extra load freezes it. His fix
   (rvdende/fix-evo-makeroom-unbounded-wait, 3271055) only bounds the wait. So: not Pimax specific, not the DSC
   bridge; it is the merged-head flip path. H3 (headset) is out for this hang (the 07:55 freeze where Monado kept
   presenting is a separate, headset-side failure).
3. NVIDIA's own comment describes this kind of stuck state (NM/nvkms-surface.c:1017-1039): "the hardware is stuck on a
   semaphore acquire that will never complete with several frames pending in the pushbuffer".
4. Kernel logs of all boots since Oct 13 21:31 (`journalctl -k -b -11 .. -1`): boot -11 (21:31-23:25) already has six
   idle-timeout bursts (22:25, 22:51, 22:55, 22:59, 23:16, 23:17), interleaved with the DoE.exe Xid 31s. So the stall
   is older than the 00:41 WayVR Xid and the "long working session" of boot -10 was the exception. There is no state
   left over from the Xid to chase.

## 2. How a flip reaches the two window channels (task A)

- NVKMS_IOCTL_FLIP -> `nvFlipEvo()` (NM/nvkms-flip.c:499). For each hardware head of the api head the SAME request
  params are turned into a per-head `NVFlipEvoHwState` (:572-581, `nvAssignNVFlipEvoHwState`, NM/nvkms-hw-flip.c:3077):
  same surface, same completion notifier, **same sync objects** (`UpdateLayerFlipEvoHwStateCommon`,
  NM/nvkms-hw-flip.c:715-758 semaphores, :923-932 notifier). Only the viewport point-in and the cursor x get the
  `mergeHeadSection` offset (:1247-1251, :1152).
- `FlipEvo2Head1OrOneDisp()` (NM/nvkms-flip.c:440) runs for every ACTIVE 2H1OR api head on every flip on that GPU (also
  for Hyprland's flips on the desktop heads; then the update state is empty and `hal->Update` is a no-op). It calls
  `FlipEvoOneApiHead(..., allowFlipLock=TRUE)` -> `nvFlipEvoOneHead()` per hw head (NM/nvkms-hw-flip.c:2058) ->
  `hal->Flip` = `nvEvoFlipC6` (NM/nvkms-evo3.c:4440) -> `EvoFlipC5Common` -> `EvoFlipC3Common` (:3658, notifier
  :3674-3699, surface, sizes, present control) + `EvoProgramSemaphore6` (:3556). Then ONE `hal->Update`
  (`nvEvoUpdateC3`, :2991) for both windows.
- Merge mode setup: `nvEvoEnableMergeModePreModeset/PostModeset` (NM/nvkms-evo.c:9656, :9720): primary head =
  raster-lock server, secondary = client (lockout window 2 lines, :9708), and BOTH heads get `flipLock = TRUE` on pin
  INTERNAL(primaryHead) (:9748).
- `ProcessFlipLockUpdates()` (NM/nvkms-evo3.c:2883): both windows qualify (`flipLockQualifyingMask`,
  NM/nvkms-hw-flip.c:2172-2175) and both are in the update, so `flipLockUpdateMask == flipLockAllMask` (:2953) and the
  normal loop kicks them off: window 0 then window 2 (:3030-3054), each with SET_WINDOW_INTERLOCK_FLAGS = {win0, win2}
  (+ core when the flip touched the core channel) and UPDATE with FLIP_LOCK_PIN = INTERNAL_FLIP_LOCK_n, RELEASE_ELV
  (`UpdateWindowC3`, :2758). The per-flip programming is symmetric; only the side effects on client memory are
  duplicated.
- `EvoProgramSemaphore6` programs, on EACH window channel: an acquire semaphore (ACQ_MODE_EQ on `acquireValue`,
  :3574-3589) and a release semaphore (SKIP_ACQ, REL_MODE_WRITE of `releaseValue`, :3603-3619). The completion
  notifier is programmed on both as well (same surface and offset).
- The flip-occurred EVENT, by contrast, is registered on the primary head only (`nvEvoSetFlipOccurredEvent(...,
  proposedPrimaryHead, ...)`, NM/nvkms-modeset.c:3193-3198), as are the RG line 1 callbacks (:4647-4657). For syncpts
  each channel has its own post syncpt and the client gets the primary's (`FillPostSyncptReplyOneApiHead`,
  "XXX[2Heads1OR] Return per hardware-head post syncpt", NM/nvkms-flip.c:123). Only client semaphores and notifiers
  are written by both heads.
- What a window UPDATE can block on is visible in the accelerator list (NM/nvkms-evo3.c:6589-6595, :6833-6837;
  ctrlc370chnc.h:198-208): present interval, semaphore acquire, interlock, flip lock, timestamp.
- `nvPreFlip`/`nvPostFlip` (NM/nvkms-hw-flip.c:2885, :2927) only use accelerators (trash pending / unblock) when the
  client sets `skipPendingFlips` (:2828), which may not be combined with semaphores (:742-747).
- Semaphore semantics, from NVKMS's own software emulation of them in headSurface (NM/nvkms-headsurface.c:520-618,
  :653-679): a flip is ready when the semaphore EQUALS the acquire value; the release value is written when the
  channel flips AWAY from the buffer; "we must not release semaphores out of order, otherwise we could confuse client
  semaphore interlocking". kapi (nvidia-drm) uses per-flip slots with constant values (READY/DONE, 256 slots,
  kapi/src/nvkms-kapi-notifiers.c:249-318) and notes that "most users of semaphores use a ready value that varies
  from frame to frame".

## 3. The stall is a semaphore acquire (new evidence)

- When a process with stuck flips closes its NVKMS fd, `nvEvoFreeClientSurfaces()` flips to NULL and
  `nvIdleLayerChannels()` force-idles ALL its busy window channels together with accelerators IGNORE_PI | SKIP_SEMA
  only (NM/nvkms-flip.c:1036-1137, NM/nvkms-evo3.c:6814-6827). If that had failed, freeing the surfaces would log 2 s
  "Idling display engine timed out" lines per window per surface. After the SIGKILLs in boots -9 .. -1 the kernel log
  has none. So skipping the semaphores is enough to release both windows: they were not waiting on interlock or flip
  lock, and a present interval cannot block forever.
- Mechanism (H1a). Per image the userspace driver renders, releases "ready" to the image's flip semaphore, flips with
  acquire(== ready) and release(:= renderable when flipped away), and waits for "renderable" before rendering into that
  image again. With 2Heads1OR both windows run that protocol. If window X latches flip k+1 one frame before window Y,
  X flips away from image B and writes "renderable"; the GPU starts rendering frame k+2 into B and writes the new
  "ready" a few ms later; one frame later Y flips away from B and writes "renderable" AGAIN, over the new "ready". Flip
  k+2 (to B) can never be acquired by either window, the GPU waits for the release of the other image, which needs
  flip k+2. Everything waits forever, scanout continues, no Xid, vkQueueWaitIdle blocks.
- What makes X and Y latch in different frames is not proven. Candidate: the ready write arriving within the last
  lines before the latch point, where the secondary (raster-lock client, lockout window 2 lines,
  NM/nvkms-evo.c:9708) defers and the primary does not. That needs GPU completion right at vblank, which is where the
  Pimax compositor finishes (see 6).

## 4. Why the output stays stuck after the kill (task A) — two independent mechanisms

W1, nvidia-drm leaves the grant behind (explains `vkAcquireDrmDisplayEXT = VK_ERROR_INITIALIZATION_FAILED`):
- vkAcquireDrmDisplayEXT = DRM_IOCTL_NVIDIA_GRANT_PERMISSIONS on the lease fd: `nv_drm_grant_modeset_permission()`
  (kernel-open/nvidia-drm/nvidia-drm-drv.c:1257) records `modeset_permission_filep` on the connector and the crtc
  (:1354-1356) and refuses (-EINVAL) when the connector is already marked (:1300).
- The marks are only cleared by `nv_drm_revoke_modeset_permission()` (:1467), called from the revoke ioctl and from
  `nv_drm_postclose()` (:1588) when the lease file is released. It takes the modeset locks with
  `DRM_MODESET_LOCK_ALL_BEGIN(dev, ctx, DRM_MODESET_ACQUIRE_INTERRUPTIBLE, ret)` (:1478). On this kernel that macro
  (include/drm/drm_modeset_lock.h:175-213) jumps to `modeset_lock_fail` on any error and the function returns WITHOUT
  revoking. A process killed with `kill -9` runs its file releases with SIGKILL still pending (the shared pending
  SIGKILL is never dequeued on the fatal path), so the interruptible ww_mutex slow path returns -EINTR as soon as one
  modeset lock is contended (Hyprland is in an atomic commit, often blocked on the nvkms lock that the dying process's
  own NVKMS close held for seconds). Result: connector + crtc stay "granted" to a freed drm_file, NVKMS is never asked
  to shut the head down, every later grant fails. Probabilistic (needs the contention): "after ~5 SIGKILLs".
- The driver's own cleanup for a dead grant is in the detect path (nvidia-drm-connector.c:259-262, "disconnected and
  still granted -> revoke"), but our EDID override makes the dpy permanently "connected": `nvDpyGetDynamicData()`
  turns an override with connected=TRUE into forceConnected (NM/nvkms-dpy.c:3070-3094, :3130). So a headset DP power
  cycle with real HPD events can never trigger it (the 08:34-08:50 experiments). Only a forced-off detect can:
  `DRM_FORCE_OFF` sets forceDisconnected (nvidia-drm-connector.c:141-143), which wins over the override (:3071).
- A SIGINT stop where the service exits by itself does not have this problem: no signal is pending at exit.

W2, while the flip is stuck NVKMS cannot idle the two merged windows for a modeset:
- Every modeset starts with `IdleAllSatelliteChannels()` (NM/nvkms-modeset.c:4068, :4248) = `nvRMIdleBaseChannel()`
  for each hardware head, one at a time (NM/nvkms-rm.c:3445): wait 2 s, then `ForceIdleSatelliteChannel` for THAT
  window only (IGNORE_PI | SKIP_SEMA) with a flip to NULL interlocked only with itself (NM/nvkms-evo3.c:6764-6770).
  The stuck UPDATE of window 0 is interlocked and flip-locked with window 2, which is not accelerated and still
  blocked, so it cannot complete: 2 s more, FALSE, and `nvSetDispModeEvo()` fails (:4248-4251).
- That hits the head shutdown the revoke asks for (`nvShutDownApiHeads`, NM/nvkms.c:3710, NM/nvkms-modeset.c:4568;
  result ignored), a new client's SET_MODE, and Hyprland's own modesets on the desktop heads (each attempt stalls ~4 s
  under the nvkms lock and pushes one more NULL flip into the stuck ring; that is how the PR author's ring filled).
- The client-close path accelerates both windows together (section 3), so W2 ends when the stuck process is gone;
  W1 is what makes the wedge permanent.

## 5. Hypothesis ranking (task B)

| # | Hypothesis | Verdict | Why |
|---|---|---|---|
| H1 | race in the 2Heads1OR flip path, timing/load sensitive | **likely, ~75%**, as H1a: duplicated client sync objects -> lost acquire | 2H1OR-only (PR author too), semaphore block (3), protocol rule broken (2), load/timing dependent (6) |
| H4 | other NVKMS/RM paths (post-flip IMP timer 10 s, vblank callbacks, sem control) | unlikely, ~10% | none of them creates a semaphore wait; no mechanism found |
| H2 | viewport patch makes one head's flip not latch | unlikely, ~5-10% | it only changes the core channel's viewport point-in at modeset; flips never touch it; the KMS path programs the same offset on every flip; the block is a semaphore, not a latch condition. Not excluded by the PR author (he has the patch too) |
| H3 | headset | out for this hang, ~2% | stable 72 Hz into the bridge during the freeze; same stall on a Vive Pro 2; the stuck state is in the GPU's channels |

## 6. Why the Simulated HMD did not freeze

The trigger needs the GPU's "ready" write to land right before the latch point so that the two heads can latch in
different frames. The fake pacer wakes the compositor 3 ms before the predicted vblank (`U_PACING_COMP_MIN_TIME_MS=3`,
20 %). The Pimax path renders the 7104x3840 target with the distortion mesh and pre-rotation, 2.7-4.9 ms p99 on the GPU
(commit 2500694), so it finishes at or just after vblank in many frames: a split every few seconds, freeze in 5-30 s.
The Simulated HMD has no distortion (and its nominal rate is not the display's, so its phase to vblank is not locked
near the deadline), its work ends well before the latch point, both windows always acquire in the same frame. This is
a hypothesis; tests 3 and 4 below check it.

## 7. Tests, in order (for the main session with the user)

Each stuck run cost a reboot so far. Test 1 should remove that cost; run everything after it on the flipsafe build.

1. **Flipsafe build, normal use.** `PIMAX_MONADO_BUILD=~/src/monado-flipsafe/build pimax vr start`, worn, xr_room 60 s.
   On a freeze the log shows `giving up`, `GPU wedged (a presented image never finished flipping)` and
   `Released the NVIDIA modeset permission of connector 826 (DP-1, dpy 0x...) after a stuck flip (N s)`; the ioctl can
   take up to ~8 s on the stock kernel (W2). Then `pimax vr stop --grace 30`: expect `skipping the Vulkan teardown`
   and `Leaving with _exit` within a few seconds, no SIGKILL. Then `pimax vr start` again: if acquire works, W1 is
   confirmed and the reboot is avoided. If it still fails: `journalctl -k -b | tail -30`, then recovery 10.
2. **No flips at all:** `XRT_COMPOSITOR_PRESENT_MODE=shared_continuous_refresh` (offered by the driver; the shared
   image is scanned out continuously, tearing expected), worn 60 s. No freeze = it is the flip path. Freeze = not
   the flip path (look at H4).
3. **More margin:** `U_PACING_COMP_MIN_TIME_MS=8` (compositor wakes 8 ms before vblank), worn 2x60 s. Expect no
   freeze if timing at the latch point is the trigger; then `U_PACING_COMP_MIN_TIME_MS=1` should freeze faster.
   If 8 ms holds, it is also a workaround on the stock driver (costs ~5 ms of latency).
4. **Three images:** `XRT_COMPOSITOR_PREFERRED_IMAGE_COUNT=3`. Under H1a a clobber then needs a two-frame split, so
   freezes should become much rarer.
5. **Patched kernel (decisive).** Install section 8. Run worn 3x60 s with the default `merged_head_sync=1`: expect
   no freeze. Same boot: `echo 0 | sudo tee /sys/module/nvidia_modeset/parameters/merged_head_sync` (stock flip
   behavior), run again: expect a freeze within ~30 s, with `Stuck layer channel ... acquire ... (holds 0x...: NOT
   satisfied)` and the last 16 flips in the kernel log. That proves the mechanism. With the idle fix the head shutdown
   then works: set it back to 1, restart the service, no reboot.
   Optional, for the protocol layout: `echo 1 | sudo tee /sys/module/nvidia_modeset/parameters/debug` before a run
   logs the first 24 flips (acquire/release offsets, values, what the semaphores held).
   If mode 1 shows a torn or half-rendered right eye (the secondary flipping before rendering is done), use
   `merged_head_sync=2` (acquire on both heads, release and notifier on the primary).
6. Not needed with flipsafe, only if wedged with an old build: before rebooting, check whether the stuck windows still
   block modesets: `hyprctl monitors` for the desktop output name, then `hyprctl dispatch dpms off <name>; sleep 2;
   hyprctl dispatch dpms on <name>` (several seconds of stall = W2 while the stuck process lives).

## 8. Kernel patches (task C)

Files: `linux/nvidia-2h1or-flip-fix.patch` (applies on top of `nvidia-2h1or-viewport.patch`, and also without it) and
`linux/nvidia-evo-makeroom-bounded-wait.patch` (applies on top of the fix or alone). Built as user in the private copy
`~/src/nvidia-610.57.04-flipfix` (git: dc7a9f7 = /usr/src tree, 63495ac = fix, 4382611 = stop-gap), `make modules`
for 7.2.5-3-omarchy: rc 0, no warnings in the touched files. `linux/nvidia-patch-reapply.sh --check --patch ...`
reports both as "applies cleanly" on /usr/src/nvidia-610.57.04.

What the fix patch does:
1. `LimitMergedHeadSyncObjects()` in `nvUpdateFlipEvoHwState()` (NM/nvkms-hw-flip.c): for heads with
   `mergeHeadSection != 0` the client's semaphores and completion notifier are dropped from the dirty layers; the
   primary keeps them. The secondary follows through interlock + flip lock, like a core update interlocked with a
   window that waits on a semaphore (an exercised configuration). Module parameter `merged_head_sync` (0644):
   1 = primary only (default), 2 = acquire on both / release + notifier on the primary, 0 = stock.
2. `nvRMIdleBaseChannel()` (NM/nvkms-rm.c): if the head is merged, force-idle all merged main-layer windows together
   with `ForceIdleSatelliteChannelIgnoreLock`; `nvIdleLayerChannels()` retries with ignore-lock if the plain force idle
   failed. Fixes W2.
3. nvidia-drm: `nv_drm_revoke_modeset_permission()` takes the modeset locks non-interruptibly (fixes W1), always drops
   the grant even if the disable cannot be staged, handles ERR_PTR from `drm_atomic_get_*_state`; the grant ioctl
   first drops grants that belong to closed files (`nv_drm_drop_stale_modeset_permissions()`, checked against
   `dev->filelist`), so an already wedged connector recovers at the next acquire.
4. Diagnostics: `nvLogStuckLayerChannel()` at every layer-channel idle timeout (GET/PUT, last flip's semaphores and
   the values they hold), plus a 16-entry ring of merged-head flips printed with it; per-flip log of the first 24
   flips when `nvidia_modeset.debug=1`.

Install (root; the DISPLAY.md / nvidia-patch-reapply.sh flow):
```
sudo ~/pimax-re/linux/nvidia-patch-reapply.sh --patch ~/pimax-re/linux/nvidia-2h1or-flip-fix.patch
sudo ~/pimax-re/linux/nvidia-patch-reapply.sh --patch ~/pimax-re/linux/nvidia-evo-makeroom-bounded-wait.patch --rebuild
modinfo -k "$(uname -r)" nvidia-modeset | grep -c merged_head_sync     # expect 1
sudo reboot
cat /sys/module/nvidia_modeset/parameters/merged_head_sync              # expect 1
```
`--rebuild` runs `dkms build/install nvidia/610.57.04 -k $(uname -r) --force` and `limine-mkinitcpio` (the UKI
carries the modules). The `.pimax-orig` backups only keep the state before the last patch. The pacman hook
(`linux/nvidia-pimax-patch.hook`, not installed) re-applies only the viewport patch on a driver upgrade; the two new
patches would have to be re-applied by hand or the hook extended.
Rollback: `merged_head_sync=0` at runtime for the flip part; for everything,
`sudo patch -R -p1 -d /usr/src/nvidia-610.57.04 -i <patch>` (stop-gap first, then the fix), then the same dkms
build/install + limine-mkinitcpio + reboot.

## 9. Monado mitigation (task D)

Branch `flipsafe` in `~/src/monado-flipsafe` (worktree of ~/src/monado, based on hangfix e64d487), commit 02889c9.
Configured like ~/src/monado/build (RelWithDebInfo, Ninja, XRT_HAVE_SYSTEM_CJSON=OFF, XRT_BUILD_DRIVER_PIMAX=ON,
PIMAX_USE_RUST_LIBPIMAX=ON, PIMAX_RUST_TARGET_DIR=~/pimax-re/rust/target/release, PIMAX_CAM=OFF), built, ctest 27/27
(new tests_gpu_wedge). Not run on hardware.

D1, exit without a wedge. The teardown that blocked was `image_cleanup()`'s `vkDeviceWaitIdle()`
(util/comp_swapchain.c:287, the 07:11 capture), and every later step waits for the GPU too. The wedge needs a SIGKILL
while the lease is held (W1). So:
- util/u_gpu_wedge: process-wide flag. comp_renderer sets it when a bounded wait gives up. If the frame's GPU work is
  done (the flip is stuck) it calls the new `comp_target::release_display_wedged`.
- comp_window_direct_wayland implements it with DRM_IOCTL_NVIDIA_GET_DPY_ID_FOR_CONNECTOR_ID +
  DRM_IOCTL_NVIDIA_REVOKE_PERMISSIONS (type MODESET) on the lease fd, from the live process: the revoke W1 skips
  cannot be skipped here, and NVKMS is asked to shut the head down (works with patch fix 2, fails harmlessly
  without it). The renderer never renders or presents again after that.
- While wedged: destroyed swapchains stay queued, `comp_target_swapchain_cleanup()` leaks the swapchain,
  `compositor_destroy()` skips the Vulkan teardown, monado-service leaves with `_exit()`.
- Env: `XRT_COMPOSITOR_STUCK_FLIP_RELEASE=0` keeps the old behavior (no revoke), `XRT_COMPOSITOR_STUCK_FLIP_EXIT=1`
  also shuts the service down by itself (SIGTERM to itself) right after the release.

D2, KMS atomic commits on the leased fd instead of Vulkan direct display. Evaluation, no prototype (not small):
- What it buys: nvidia-drm flips go through kapi, which uses its own per-flip semaphore slots with constant values
  (256 slots, NOT_READY -> READY -> DONE, a slot is only reused after DONE). A duplicated DONE write from the second
  head cannot overwrite a newer value, so the H1a deadlock cannot happen on that path even on the stock driver. kapi
  also sets the per-head viewport offset on every flip (the reason kms_lease_test showed the full frame before the
  viewport patch). Real vblank/page-flip events would replace the fake pacer.
- Effort: a new comp_target (no KMS target exists in Monado): exportable VkImages with
  VK_EXT_image_drm_format_modifier (NVIDIA block-linear modifiers), dma-buf export, drmModeAddFB2WithModifiers,
  atomic commits on the lease's CRTC/primary plane with IN_FENCE_FD from a sync_fd semaphore, one commit per vblank
  with page-flip events, our own FIFO queueing and timing. About 1000-1500 lines plus hardware testing.
- Risks: NVIDIA scanout of imported dma-bufs at 7104x3840 in 10 bpc, modifier negotiation, explicit sync behavior
  of nvidia-drm 610 (in-fences become NVKMS semaphores, `__nv_drm_convert_in_fences`), commit latency under
  Hyprland's load, and losing NVIDIA's own direct-display timing. Worth doing only if the kernel fix does not hold.

## 10. Recovery from a W1 wedge without a reboot (untested, needs root)

```
echo off    | sudo tee /sys/class/drm/card1-DP-1/status
echo detect | sudo tee /sys/class/drm/card1-DP-1/status
```
The forced-off detect passes forceDisconnected, which beats the EDID override, reads the connector as disconnected
and makes nvidia-drm revoke the dead grant and shut the head down (nvidia-drm-connector.c:141-143, :259-262). Risk:
Hyprland sees DP-1 go away and come back; an earlier forced hotplug desynced Hyprland, but DP-1 was a desktop monitor
then. With the patched nvidia-drm a wedged connector instead recovers at the next acquire by itself.

## 11. Open questions

- The exact semaphore layout of the closed Vulkan driver (one slot per image or per swapchain, values). The debug
  flip log of test 5 answers it.
- What makes the merged heads latch in different frames. Not needed for the fix; the stuck dump will show whether
  the semaphore holds a release value of an older flip.
