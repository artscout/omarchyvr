# NVIDIA Vulkan direct-display + 2Head1OR: left half shown on both halves
 Researched 2026-10-13 (web only, no system changes).
Setup under study: RTX 4090, NVIDIA 610.57.04 open kernel modules, 7104x3840@72 DSC headset mode driven as 2Head1OR.
Symptom: plain DRM/KMS atomic modeset with dumb buffer shows full frame; Vulkan direct-to-display (VK_KHR_display swapchain, Monado/SteamVR direct mode) shows left half on both halves.

## Bottom line

1. **Strongest match found (H for mechanism, M for applicability):** NVIDIA/open-gpu-kernel-modules PR #1262 describes exactly this symptom on 2Heads1OR. Root cause claimed: `nvSetViewPortsEvo()` hardcodes viewport origin x=0 for every merged head, so both heads scan out the left half. Proposed fix adds `mergeHeadSection * width` offset. Status: open PR, maintainer said it will be merged internally (not yet public). Tested on driver 610.43.03, not 610.57.04.
2. **Possibly a related 8K tile bug (M):** NVIDIA forum thread on Dell UP3218K (two 3840-wide tiles) reports right tile black when source X offset (SRC_X=3840) is at/beyond mode width. Same family of "per-head source offset" bug. NVIDIA reproduced internally (bug 6265858, June 2026), no fix version.
3. **Open question:** if the nvkms viewport bug is in the generic modeset path, KMS should also be affected. The user says KMS shows the full frame, so the Vulkan WSI path probably sets up the per-head source/viewport differently (or KMS uses a different path). Needs a test, not a web answer.
4. **VK_EXT_acquire_drm_display on NVIDIA is unconfirmed (L).** No NVIDIA announcement found. Monado docs and NVIDIA-specific forum evidence point to VK_EXT_acquire_xlib_display (X11 leases) as NVIDIA's direct-mode route.
5. **No public report found** that matches "half screen duplicated" on Linux with 2Head1OR and Vulkan from Monado/SteamVR/ALVR/Pimax users. Searches returned nothing exact. Absence is weak evidence.
6. **Action:** check whether 610.57.04 contains the PR #1262 change (diff nvkms `nvSetViewPortsEvo` in the open modules tree against 610.43.03). If not, file an NVIDIA bug report with the PR reference; the tracker note says proprietary-driver bugs go to developer forums.

## Findings

### F1. PR #1262 (open-gpu-kernel-modules) describes the same bug (H for content, M for applicability)
- Title: "nvkms: fix duplicated half-image on 2Heads1OR displays (5K 120Hz VR headsets)". Opened 2026-07-27 by rvdende, still open.
- Bug: "any display requiring 2Heads1OR shows the left half of the framebuffer duplicated across the output." Reproduced on Vive Pro 2 at 4896x2448@120 over DP. "The headset showed the left half of the framebuffer on both eyes."
- Root cause: `nvSetViewPortsEvo() hardcoded the origin to 0 for every head`.
- Fix: origin `viewPort.in.width * mergeHeadSection`; head 0 x=0, head 1 x=2448. No-op for single head.
- Maintainer (Binary-Eater, 2026-07-28): "This fix makes sense to me for handling viewports with 2head1or. Will merge the change internally."
- Environment in PR: driver 610.43.03 open modules, kernel 7.0.0, RTX 4090 (same GPU as the user).
- Also reported in the PR: a compositor hang on the merged-head path under load (separate fix proposed in `nvEvoMakeRoom()`), and a question about `ViewportValidPointIn` / `ViewportValidSizeIn` overfetch for 2Heads1OR.
- Sources: https://github.com/NVIDIA/open-gpu-kernel-modules/pull/1262 (fetched directly). A mirror copy at download.plaud.ai was also seen in search; not used as a source.

### F2. Dell UP3218K 8K tile bug, black right tile (M)
- Dell UP3218K = two 3840x4320 tiles over two DP cables. RTX 4070 Ti, NixOS, sway (wlroots).
- Reporter: right tile black when scanning out block-linear buffers; right connector uses SRC_X=3840 at/beyond mode width. SRC_X=0 displayed left-tile content on right half. Reporter says it was found with AI help and may be inaccurate.
- NVIDIA staff (abchauhan): tracked internally as bug 6265858; reproduced internally 2026-06-05; "under Engineering investigation". No fix or workaround announced.
- Works on X11 and GNOME per reporter.
- Relevance: same kind of per-head source-offset handling failure, on a different path (wlroots/atomic).
- Source: https://forums.developer.nvidia.com/t/bug-right-half-right-tile-of-my-8k-monitor-is-black-on-wlroots-based-wayland-compositors/355579

### F3. DRM lease / direct-mode latency on NVIDIA (M, related but different symptom)
- NVIDIA forum thread (2025): visible swim/double-composite on Valve Index and Vive Pro with proprietary driver only; reportedly from driver 440/550 onward. NVIDIA bug 5372097 filed internally; asked for repro bug report.
- Workarounds reported: `VK_PRESENT_MODE_IMMEDIATE_KHR` (XRT_PRESENT_MODE=0, adds tearing); XRT_COMPOSITOR_USE_PRESENT_WAIT=1 (stutter under load); U_PACING_COMP_MIN_TIME_MS=10; U_PACING_COMP_TIME_FRACTION_PERCENT=90 (about 45 fps).
- Source: https://forums.developer.nvidia.com/t/substantial-drm-lease-presentation-latency-resulting-in-unusable-vr-hmd-experience/332386
### F4. NVIDIA direct mode uses X-lease extension in Monado (M)
- Monado docs: after finding a VkDisplayKHR via a driver-specific method, acquire it with VK_EXT_acquire_xlib_display; presentation via VK_KHR_display_swapchain; release via VK_EXT_direct_mode_display. On KMS/DRM, Monado uses VK_KHR_display; VK_EXT_acquire_xlib_display is X.Org only (per search snippet, not verified in full text).
- Monado source (comp_settings.h): "Since NVidia direct mode lets us 'acquire' any display, we need to be careful about which displays we attempt to acquire." Allowlist has e.g. "Sony SIE HMD *08" (PSVR), Index, Rift.
- Monado direct-mode page: if the desktop is extended onto the headset, direct mode init fails. Pimax P2 page: on Wayland + NVIDIA, EDIDs need patching to appear as non-desktop; reapply each boot.
- Sources: https://monado.pages.freedesktop.org/monado/comp__settings_8h_source.html ; https://gitlab.freedesktop.org/monado/webpage/-/raw/master/direct-mode.md (search snippet; fetch was blocked by Anubis on one URL) ; https://vronlinux.org/docs/hardware/pimax/

### F5. NVIDIA forum: xlib-display acquire failures and Monado crashes (M, older)
- NVIDIA forum: vkQueuePresentKHR segfault on VK_EXT_acquire_xlib_display swapchain (drivers 460–510 per Monado issue #122 reference).
- NVIDIA forum: vkAcquireXlibDisplayEXT returns VK_ERROR_UNKNOWN; BSB 3840x1920@90 selected.
- Proprietary-module DRM lease total failure thread (no hardware specifics).
- Sources: https://forums.developer.nvidia.com/t/linux-vkqueuepresentkhr-segmentation-fault-on-vk-ext-acquire-xlib-display-swapchain/191161 ; https://forums.developer.nvidia.com/t/nvidia-proprietary-non-open-modules-completely-unable-to-acquire-a-drm-lease-on-any-display-server-all-known-nvidia-drivers-any-hardware/341244


### F6. Bigscreen Beyond on NVIDIA, DSC (M)
- Early DSC bug: Linux drivers mangled DSC pass-through info -> garbled image; NVIDIA said fixed on Windows, slated for 580 series on Linux. Status on Linux unverified.
- Community patch for open modules targets DSC rainbow artifacts (triple-groove/nvidia-bsb-dsc-fix).
- Sources: https://forums.developer.nvidia.com/t/bigscreen-beyond-hmd-unable-to-be-initialized-on-nvidia-possibly-due-to-dsc-bpp-issue/294108 ; https://github.com/triple-groove/nvidia-bsb-dsc-fix/ ; https://www.pcgamer.com/hardware/vr-hardware/after-more-than-a-year-of-conflict-woes-between-bigscreen-beyond-vr-headsets-nvidia-gpus-and-linux-theres-good-news-at-long-last-as-a-fix-is-nearly-in-sight/

### F7. Driver 610.57.04 release (M for what it says)
- Released 2026-08-03. Phoronix: various Vulkan driver fixes, DKMS build fixes, GLX indirect crash fix. Ubuntuhandbook: a Vulkan display enumeration fix (with __VK_ENABLE_DEVICE_GROUPS=1) and a Runtime D3 suspend/resume fix. No mention of 2Heads1OR or merged viewport fix in summaries.
- Sources: https://www.phoronix.com/news/NVIDIA-610.57.04-Linux-Driver ; https://ubuntuhandbook.org/index.php/2026/08/nvidia-610-57-04-many-game-fixes/

## Questions answered

**(1) Reports matching the symptom:** F1 (PR #1262) is the only match for 2Heads1OR + left-half duplication. F2 is a related 8K tile bug. Nothing matched "half screen duplicated" with Monado, SteamVR, ALVR, Bigscreen, or Varjo reports. Confidence M overall.

**(2) Does NVIDIA VK_KHR_display / 2Head1OR work?** No NVIDIA statement found. The 2Heads1OR viewport bug (F1) suggests the merged-head path was not handled correctly in nvkms viewport setup, at least as of 610.43.03. Fixed or not in 610.57.04: unknown (L). VK_EXT_acquire_drm_display on NVIDIA: not confirmed (L).

**(3) Workarounds:** No direct workaround found for 2Head1OR. Options from the sources:
- Lower mode / avoid 2Heads1OR (not possible for 7104 wide without merge; one head of 3552x3840 would be a different mode, untested).
- Immediate present mode (latency workaround, not the half-screen bug).
- Patch nvkms locally with PR #1262 diff (requires building open modules; M).
- Present via DRM KMS with dmabuf import from Vulkan (not found in searches as a documented NVIDIA workaround; L, untested idea).
- No NVIDIA env var for Vulkan display or 2Head1OR found (L).

**(4) SteamVR on Linux direct mode on NVIDIA:** Monado docs and NVIDIA forum threads show the xlib acquire route (VK_EXT_acquire_xlib_display) used on NVIDIA. SteamVR's exact call on NVIDIA not verified (L). Do not assume vkAcquireDrmDisplayEXT is used on NVIDIA.

## Suggested next steps (for the user to run, not done here)
- Diff open-gpu-kernel-modules at tag for 610.57.04 against PR #1262 (`nvSetViewPortsEvo`, `mergeHeadSection`).
- Check `vulkaninfo` for VK_EXT_acquire_drm_display on the 4090 driver.
- Compare KMS vs Vulkan: which SRC_X / viewport values reach the nvkms head state (e.g. drm debug or nvidia-settings logs). The SRC_X=3840 finding (F2) suggests reading that value is the cheapest discriminator.
- If unfixed in 610.57.04: NVIDIA forum bug report referencing PR #1262 and the 7104x3840 repro; attach KMS-vs-Vulkan comparison.
