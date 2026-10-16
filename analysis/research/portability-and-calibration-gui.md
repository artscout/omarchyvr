# Pimax Dream Air on Linux for other people: portability report + calibration GUI design

Date 2026-10-14. Read-and-think work: no headset runs, no Monado runs, no sudo, no installs. Sources are local files
(path:line), the NVIDIA open kernel module source at tag 610.57.04 (`~/src/open-gpu-kernel-modules`, `NM` =
`src/nvidia-modeset/src`), the Monado fork (`~/src/monado`, branch `omarchyvr`, HEAD 4ec0348) and web pages (URLs).
Web research went through three helper agents. Several primary pages (gitlab.freedesktop.org, invent.kde.org, Phoronix)
blocked automated fetches, so some web facts come from search snippets only. Those are marked **(snippet)**. Anything
not checked is marked **UNVERIFIED**. No numbers were invented. Where a statement is my own reasoning, it says so.

Companion page: `analysis/portability-gui.html`.

---

## 0. Summary

**What works today (verified on this one machine):** RTX 4090 (Ada), nvidia-open 610.57.04 + our three patches, kernel
7.2.5, Arch/Omarchy, Hyprland 0.56.2, Monado fork (27 commits on top of upstream), Rust tools. 7104x3840 at 72 and 90 Hz
in direct mode through a Wayland DRM lease, Steam games through Proton + xrizer, 3DoF head tracking, controllers
(buttons, no optical tracking), controller-driven hands, DP audio, cameras, passthrough prototype.

**Support matrix (short):**

| GPU / driver | Can it drive 7104x3840 @72/90 on one DP? | Direct mode / lease | Status for us |
|---|---|---|---|
| NVIDIA Ada (RTX 40), open module 610.x + our patches | Yes (DSC + 2Heads1OR) | Yes (Hyprland lease) | **Works** (verified, this machine only) |
| NVIDIA Turing / Ampere (RTX 20/30), open module | Probably: same merge-mode code path (`supportsMergeMode = TRUE`) | Probably | **Untested**; same patches needed |
| NVIDIA Blackwell (RTX 50), open module | Unknown: merge mode is OFF in its display HAL; works only if one head is fast enough | Probably | **Untested, open question** |
| NVIDIA proprietary (closed) kernel module | Same display code, but we cannot patch it | Reported broken for leases | **Not supported** |
| AMD RDNA (amdgpu + RADV) | Plausible (DSC + ODM combine exist; no report for this mode) | Yes, the recommended Linux VR path | **Untested**; our NVIDIA patches not needed, EDID setup differs |
| Intel Arc (i915/xe + ANV) | Plausible on Battlemage/Lunar Lake (ultrajoiner); unknown on Alchemist | Reported: xe has no wired-HMD direct mode | **Unlikely for now** |
| Laptops (any) | Only if the DP/USB-C port is wired to the dGPU | – | **Untested, no reports found** |

**Top blockers for "someone else plugs in their Dream Air":**
1. NVIDIA needs patched kernel modules (viewport fix = PR #1262, still open; our flip-hang fix, not posted; bounded wait). Only the open module can be patched; every driver update needs a DKMS rebuild; our auto re-apply hook is Arch-only and covers only the viewport patch.
2. Display setup is hand-made: NVIDIA shows 0 modes with the stock EDID, so a boot-time EDID override (hard-coded PCI address `0000:01:00.0` and NVKMS name `DP-0`) is baked into the initramfs; non-desktop needs our hand-built Microsoft-VSDB EDID, because the kernel has no `PVR 0x1019` quirk.
3. Tracking is 3DoF only: the head has no position tracking (`pimax_hmd.c:575`), controllers have no optical tracking (fixed pose, `pimax_ctrl.c:8`). Most VR games need 6DoF. Basalt SLAM is only research so far.
4. Everything is built from source by hand: Monado fork not upstreamed, Rust static libs must be built first, `setcap cap_sys_nice` lost on each rebuild, launcher assumes `~/src/monado/build` and connector `DP-1`.
5. Per-unit tuning (lens centres, cant, distortion, colour) is done with terminal scripts while wearing the headset; `lens.conf` needs a service restart; there is no guided tool and no per-serial profile.

**GUI recommendation:** a Rust **egui/eframe desktop app** ("Pimax Setup") as the control centre (health checks,
profiles, backups, wizard, all file writes), which drives an **in-headset test client** (pimax-lenstest grown into
`pimax-calib-xr`) over a local Unix socket. The wearer adjusts with the controllers (stick = adjust, A = accept, B = back)
and sees the step name and values drawn into the pattern; the desktop window shows the same state for a helper.
A TUI/CLI (`omarchyvr doctor`) comes first as the shared health-check engine. No web UI.

**Build phases:** P0 per-serial paths + `omarchyvr info --json` (small) · P1 `omarchyvr doctor` + `omarchyvr-config` crate
(profiles, backups, atomic writes) + egui shell with health page · P2 in-headset control socket + alignment / distortion
/ CA steps writing `lens.conf` · P3 colour balance (per-eye gain in the compositor), refresh, hands, brightness (after a
flash-safety check) · P4 tracking: gyro bias, controller check, camera recording + selfcal, passthrough · P5 packaging
(install script, EDID generator, AUR, other distros).

---

# Part 1 — Portability report

## 1. NVIDIA

### 1.1 Why the Dream Air needs "2Heads1OR"
- The panel modes are 7104x3840 at 72 Hz (pixel clock 2170.07 MHz) and 90 Hz (2642.6 MHz) (`analysis/edid-non-desktop.md`
  section 3, edid-decode of `linux/pimax-dp-edid-msvsdb.bin`). They need DSC: the sink offers DSC 1.2, max 8.0 bpp, 1/2/4/8
  slices, plus FEC (`linux/DISPLAY.md` "Why it did not work"; `analysis/headset-dpcd.md`).
- NVKMS uses 2Heads1OR (two hardware heads feeding one output resource) only when the pixel clock is above one head's
  limit: `return (pTimings->pixelClock > pHeadCaps->maxPClkKHz);` (NM/nvkms-evo.c:9876). It also needs DSC or HW YUV420 and
  even timings (NM/nvkms-evo.c:9849-9873). The per-head limit comes from the resource manager at run time, not from this source.
  NVKMS reports "DualHead Mode: Yes" for 7104x3840 on the 4090 (`analysis/nvidia-pr1262-comment-draft.md`).

### 1.2 Generations (from the source, display HAL table NM/nvkms-hal.c:145-158)
| Generation | Display class / HAL | `supportsMergeMode` | Meaning for the Dream Air |
|---|---|---|---|
| Turing (RTX 20, GTX 16) | C5 / nvEvoC5 | TRUE (NM/nvkms-evo3.c:7812) | Same 2Heads1OR path as Ada, so same bugs and same patches. Per-head clock limit and DSC link rate: **UNVERIFIED** |
| Ampere (RTX 30) | C6 / nvEvoC6 | TRUE (NM/nvkms-evo3.c:7896) | Same as Ada (same HAL) |
| Ada (RTX 40) | C7 core, C6 windows / nvEvoC6 | TRUE | **Verified working** (with patches) |
| T239 (Tegra) | C8 / nvEvoC8 | TRUE (NM/nvkms-evo3.c:7980) | Not relevant |
| Blackwell (RTX 50: C9; GB20X: CA) | nvEvoC9 / nvEvoCA | **FALSE** (NM/nvkms-evo4.c:2878, :2962) | No 2Heads1OR at all. The mode works only if one Blackwell head's `maxPClkKHz` is above 2.64 GHz (or 2.17 GHz for 72 Hz). **UNVERIFIED** either way. Our viewport and flip-fix patches touch the merge path, so they would not matter there. Needs a tester. |

- Open vs proprietary: NVIDIA's README says the proprietary flavour supports Turing, Ampere, Ada and Hopper, and
  "Blackwell and later are only supported by the open kernel modules"
  (https://download.nvidia.com/XFree86/Linux-x86_64/595.84/README/kernel_open.html).
- Proprietary module: the closed package ships the NVKMS core as a prebuilt object, so our source patches cannot be
  applied. (From general knowledge of the driver packaging, **UNVERIFIED** here.) There are also reports that DRM leases
  fail entirely on the proprietary modules (NVIDIA forum thread 341244, cited in `analysis/nvidia-hmd-modes-research.md:23-24`,
  `:95`). **Conclusion: open module only.**
- Driver versions: we tested 610.57.04 only. The viewport patch "applies cleanly" to 610.57.04
  (`analysis/nvidia-pr1262-comment-draft.md`); the PR author tested 610.43.03 (`analysis/nvidia-flip-hang.md:34`).
  vronlinux says the open driver needs 565+ for VR and 580+ for Bigscreen Beyond
  (https://vronlinux.org/docs/hardware/graphics-cards/, https://vronlinux.org/docs/hardware/bigscreen-beyond/).
  We could not find release-note entries for DRM lease or direct-mode fixes between 560 and 610. The 570.86.16 beta added an
  opt-in low-latency display interrupt mode for VR stutter, `RMIntrLockingMode=1`
  (https://nvidia.com/de-de/drivers/details/240634). Older than 610: **UNVERIFIED**, and the patches may not apply.

### 1.3 The three patches and upstream status
| Patch | What | Upstream |
|---|---|---|
| `linux/nvidia-2h1or-viewport.patch` (27 lines) | `nvSetViewPortsEvo()` gives every merged head viewport x = 0, so in Vulkan direct mode both eyes show the left half | = NVIDIA/open-gpu-kernel-modules **PR #1262** (rvdende, opened 2026-07-27, Vive Pro 2 4896x2448@120). **Still open.** NVIDIA member Binary-Eater, 2026-07-28: "This fix makes sense to me for handling viewports with 2head1or" and said it would be merged internally. https://github.com/NVIDIA/open-gpu-kernel-modules/pull/1262. Our confirmation comment is drafted, not posted (`analysis/nvidia-pr1262-comment-draft.md`) |
| `linux/nvidia-2h1or-flip-fix.patch` (962 lines) | Flip semaphores/notifier programmed on both merged heads, so it freezes; idle fix; revoke-on-SIGKILL fix; diagnostics; param `nvidia_modeset.merged_head_sync` (1 = fix) | **Ours, not posted anywhere.** Tested worn: 60 s + 300 s, no freeze, full 72 Hz (memory log 2026-10-14 ~11:00). A/B with `merged_head_sync=0` not done yet. Root cause confidence ~75 % (`analysis/nvidia-flip-hang.md` section 0) |
| `linux/nvidia-evo-makeroom-bounded-wait.patch` (159 lines) | Bounds the `nvEvoMakeRoom` wait (stop-gap from the PR #1262 author, branch rvdende/fix-evo-makeroom-unbounded-wait) | Not merged. The PR author also saw the freeze with a Vive Pro 2 (`analysis/nvidia-flip-hang.md:34-40`), so the freeze is not Pimax-specific |

Related open items in that repo: #1366 (PR, selects 2Heads1OR for DP modes wider than the 5120 DSC limit, state
**UNVERIFIED**), #1405 (DP sink black after replug, open), #1348/#1352 (HDMI DSC slice parsing). All links:
https://github.com/NVIDIA/open-gpu-kernel-modules/issues/1405, /pull/1366, /issues/1348, /pull/1352.

**What a user must do until NVIDIA ships the fixes:** use `nvidia-open-dkms` (or the distro's open DKMS package), apply
both patches to `/usr/src/nvidia-<ver>`, `dkms build/install --force`, rebuild the initramfs, reboot, and check
`/sys/module/nvidia_modeset/parameters/merged_head_sync` = 1 (`linux/DISPLAY.md` lines 50-90). Gaps for other people:
- `linux/nvidia-patch-reapply.sh` and the pacman hook only handle the viewport patch (`PATCH_FILE` default, script line 26);
  the flip fix and bounded wait are not re-applied after an update (memory log 11:00 note). **Must be fixed before
  others use it.**
- The hook is pacman-only. Ubuntu (`nvidia-dkms-*-open`), Fedora (RPM Fusion `akmod-nvidia-open`, akmods not DKMS) and
  others need their own hook (apt `DPkg::Post-Invoke`, akmods patch dir). **UNVERIFIED** paths.
- Secure Boot: self-built modules must be signed (DKMS MOK). Not tested here.

**Ask upstream (cheap, high value):** post the PR #1262 confirmation; open a second PR with the flip fix split into
(a) primary-head-only sync objects, (b) idle fix, (c) revoke fix, each with a short repro (Vive Pro 2 + Dream Air both
hit it).

### 1.4 EDID and non-desktop on NVIDIA
- With the stock EDID, nvidia-drm gives **0 modes**: colour depth "undefined" makes NVIDIA send 18 bpp, and the HBR2 vs HBR3
  link assessment makes the 72 Hz mode miss the bandwidth check by ~3.5 % (`linux/DISPLAY.md` "Why it did not work",
  `analysis/nvidia-dsc-linkrate.md`). Fix: an EDID override at boot through `nvidia-modeset config_file` with
  `override.[0000:01:00.0].DP-0=/etc/pimax/dp-edid.bin`, put into the initramfs (`linux/DISPLAY.md` Setup step 2).
  **The PCI address and the NVKMS connector name are specific to this PC.** Other users need their own values (their PCI
  slot, and whichever DP port they use).
- Non-desktop: the kernel marks a connector non-desktop from DisplayID 2.0 primary use, a Microsoft HMD VSDB, or a quirk
  list. The list has SEN/SVR 0x1019 but **no PVR entry**, and the Dream Air reports `PVR` model 4121 = 0x1019
  (edid-decode; `analysis/edid-non-desktop.md` section 1). We use a hand-made EDID with an MS VSDB
  (`linux/pimax-dp-edid-msvsdb.bin`). That file is built from **our** unit's EDID (its EDID serial field is just 17, so it
  may be the same on every unit, but other firmware may differ: **UNVERIFIED**). Safer: ship the generator (`linux/make_hmd_edid.py`) and build from the user's own EDID
  (`/sys/class/drm/card*-DP-*/edid`).
- Upstream fix: a one-line kernel quirk `EDID_QUIRK('P','V','R', 0x1019, EDID_QUIRK_NON_DESKTOP)` in `drm_edid.c` would make
  the connector non-desktop on every GPU without an override (whether other Pimax models share 0x1019: **UNVERIFIED**).
  vronlinux says Pimax P2 headsets on Linux need "non-desktop patch and EDID checksum patch", and that NVIDIA + Wayland may
  need patched EDIDs (https://vronlinux.org/docs/fossvr/monado/pimax/), so this has hit others before.

### 1.5 Laptops / Optimus
No Linux reports found. My reasoning: direct mode needs the headset port wired to the NVIDIA GPU (a laptop with a MUX
switch or a dGPU-wired DP/USB-C port). A port wired to the iGPU makes it an AMD/Intel problem (section 2) at iGPU display
limits. The lease must also come from the compositor's DRM device for that GPU, and Monado would need
`XRT_COMPOSITOR_FORCE_GPU_INDEX` (https://monado.freedesktop.org/getting-started.html). **UNVERIFIED**.

### 1.6 Known failure modes users will hit on NVIDIA
- A stuck flip or SIGKILLed Monado wedges the display (`vkAcquireDrmDisplayEXT` fails); only a PC reboot clears it
  (memory log 08:5x; the flip-fix patch item 3 targets the revoke part). Xid 31 from a client also needs a reboot (memory log 23:40).
- `XRT_COMPOSITOR_FORCE_NVIDIA=0` is needed so Monado does not try the X11/xlib NVIDIA path on Wayland
  (`rust/omarchyvr-tools/src/vr/config.rs:208`; a 2026 Gaia Sky guide reports the same: https://gaiasky.space/news/2026/gaiasky-vr-linux-monado).

## 2. AMD and Intel

**Nothing is tested on AMD or Intel.** What changes vs NVIDIA (my reasoning, from the code we have):
- Our three NVIDIA patches are irrelevant there.
- The EDID override uses the generic kernel route (`drm.edid_firmware=DP-1:edid/pimax.bin` plus the file in the initramfs,
  or debugfs `edid_override`) instead of `nvidia-modeset config_file`. The kernel itself parses the DisplayID Type I timings,
  so the native modes may appear without an override. Whether amdgpu/i915 then pick 8 bpc + DSC at HBR3: **UNVERIFIED**.
- The kernel non-desktop logic is the same for all GPUs, so the MS-VSDB EDID or a PVR quirk is needed on AMD/Intel too.

### 2.1 AMD (amdgpu DC + RADV)
- ODM combine (2:1, 4:1) is the AMD equivalent of 2Heads1OR, and DC has carried fixes for ODM + DSC for years (e.g. 2:1
  ODM + DSC corruption fix in v5.7; "Disconnect DSC for unused pipes" v6.1; ODM update fix 2024):
  https://patchwork.kernel.org/comment/22837097,
  https://lkml.rescloud.iu.edu/hypermail/linux/kernel/2009.2/03660.html,
  https://patchew.org/linux/20240327121957.2835611-1-sashal@kernel.org/. Whether DC picks ODM automatically for a 2.17/2.64
  GHz DSC mode: **UNVERIFIED** (I expect it does, from the DML design, but have no source).
- Bigscreen Beyond on AMD needs kernel patches, including `amd-bsb-dsc-fix.patch` for DSC "rainbow static", and the Arch
  `linux-bsb` package (https://vronlinux.org/docs/hardware/bigscreen-beyond/). So high-res DSC headsets on AMD work but
  needed kernel fixes, and the Dream Air's mode is bigger than the Beyond's.
- RADV has `VK_EXT_acquire_drm_display` (Mesa MR 11014, https://gitlab.freedesktop.org/mesa/mesa/-/merge_requests/11014,
  **(snippet)**). vronlinux: AMD RDNA + RADV "Recommended for wired VR"; AMDVLK/AMDGPU-PRO "Unable to drive wired HMDs"
  (https://vronlinux.org/docs/hardware/graphics-cards/).
- DP 2.1 UHBR rates on RDNA3/4 under Linux: **UNVERIFIED** (only an old UHBR10 article, Phoronix "AMDGPU DP 2.0 Linux 5.16"
  **(snippet)**). The Dream Air link is HBR3 x4 (`linux/DISPLAY.md`), so UHBR is not needed.

### 2.2 Intel (i915/xe + ANV)
- i915 has "bigjoiner" (2 pipes); Xe2 "ultrajoiner" (4 pipes) landed for Battlemage and Lunar Lake in Linux 6.13; the
  cover letter says 8K@120 needs ultrajoiner with 12 DSC slices (Phoronix "Linux-6.13-Xe2-Ultra-Joiner-ARL" and
  "Intel-Battlmage-More-Res" **(snippet)**). Alchemist (DG2) joiner limits for this mode: **UNVERIFIED**.
- ANV has had `VK_EXT_direct_mode_display` since ~Mesa 18.2 and the acquire-DRM path. But vronlinux lists Intel **xe**:
  "No wired HMDs (Lacks direct mode implementation in driver)", and wired HMDs only with i915
  (https://vronlinux.org/docs/hardware/graphics-cards/). Arc A-series under xe would therefore not work today.
- No Pimax/Beyond-on-Arc reports found.

### 2.3 What we'd need for AMD/Intel
A tester with an RDNA2/3/4 card (and one with Arc). Test list: (1) `modetest -c` / `/sys/class/drm/*/modes` with the stock
EDID: are 7104x3840 modes listed? (2) with the MS-VSDB EDID via `drm.edid_firmware`: non-desktop = 1? (3) Monado direct mode
at 72 Hz (`XRT_COMPOSITOR_FORCE_WAYLAND_DIRECT=1`, `XRT_COMPOSITOR_DESIRED_MODE` from `XRT_COMPOSITOR_PRINT_MODES`), the
panel-rotation arrow test, the red/green eye test; (4) 90 Hz; (5) a 30-minute worn session. The launcher must stop forcing
NVIDIA-specific checks on non-NVIDIA systems (section 4).

## 3. Desktop environments / compositors

| Compositor | wp_drm_lease_device_v1 | Our evidence |
|---|---|---|
| Hyprland (aquamarine) | Yes | **Verified here** (0.56.2): Monado logs the lease, aquamarine logs "Non-desktop connector" (memory log, `linux/DISPLAY.md` non-desktop section). Hyprland offers a non-desktop output for lease only when no monitor rule claims it |
| Sway / wlroots | Yes, the first implementation (Drew DeVault, https://drewdevault.com/blog/DRM-leasing-and-VR-for-Wayland/) | Untested |
| KDE KWin | Groundwork MRs (https://invent.kde.org/plasma/kwin/-/merge_requests/542, /2619); shipping version **UNVERIFIED** | Untested |
| GNOME Mutter | DRM lease merged June 2024, aimed at GNOME 47 (https://www.gamingonlinux.com/2024/06/drm-lease-protocol-support-finally-merged-for-gnome-wayland-great-for-vr-fans/); shipped state **UNVERIFIED** | Untested |
| X11 (RandR 1.6 leases) | Monado X11 direct mode uses `VK_EXT_acquire_xlib_display` (https://monado.freedesktop.org/direct-mode.html) | Launcher does not support it (forces the Wayland path) |
| cosmic, niri, gamescope | not researched | – |

Protocol: `drm-lease-v1`, wayland-protocols 1.22 (2021) **(snippet)**.

What our launcher assumes (`rust/omarchyvr-tools/src/vr/config.rs`):
- `XRT_COMPOSITOR_FORCE_WAYLAND_DIRECT=1` (:206). It is a real Monado option (`~/src/monado/src/xrt/compositor/main/comp_settings.c:26`),
  although the getting-started page does not list it.
- `XRT_COMPOSITOR_WAYLAND_CONNECTOR=DP-1` (:207, default `DEFAULT_CONNECTOR = "DP-1"` :16; read in
  `comp_window_direct_wayland.c:33`). Other PCs may use DP-2/DP-3; the default should be "the connector whose EDID is PVR
  0x1019", found in sysfs.
- `XRT_COMPOSITOR_FORCE_NVIDIA=0` (:208), `XRT_COMPOSITOR_DESIRED_MODE` 2 = 72 Hz / 3 = 90 Hz (`linux/DISPLAY.md` "One-click
  VR"). **The mode index is the index in Monado's mode list for this EDID.** With another EDID or GPU the index changes.
  It should be found by matching width/height/refresh in the `XRT_COMPOSITOR_PRINT_MODES` output.
- Monado build in `$HOME/src/monado/build` unless `PIMAX_MONADO_BUILD` (config.rs:179-185).
- Preflight checks (`rust/omarchyvr-tools/src/vr/preflight.rs:102`): headset on USB, hidraw interfaces, keepalive service, connector
  connected, no Xid/"Idling" in this boot's kernel log, no Monado running. NVIDIA-specific wording for the kernel check.

## 4. Distros and packaging

| Piece | Today (this PC) | What a newcomer needs |
|---|---|---|
| Kernel | 7.2.5 (Omarchy) | Any kernel that the NVIDIA open module 610.x builds against; for AMD/Intel probably a recent one (6.13+ for Intel ultrajoiner). Lowest working version **UNVERIFIED** |
| GPU driver | nvidia-open-dkms 610.57.04 + 3 patches by hand | A patch package or DKMS hook per distro (section 1.3) |
| EDID override | `/etc/pimax/*`, `/etc/modprobe.d/nvidia-pimax.conf`, `/etc/mkinitcpio.conf.d/pimax.conf` | A generator from their EDID + PCI address + connector, and a distro-aware initramfs step (mkinitcpio / dracut / initramfs-tools; Omarchy uses `limine-mkinitcpio`, `linux/DISPLAY.md` re-apply section) |
| udev | `linux/70-pimax.rules` (uaccess for 34a4:* hidraw/usb and the Tobii 2104:0561) | Install the file; or upstream to `xr-hardware` (Monado recommends that package for udev rules: https://monado.freedesktop.org/getting-started.html) |
| Keepalive | user service `omarchyvr-keepalived` (Rust drop-in) | Packaged user unit. Without it the headset firmware detaches every ~5.4 s (memory log 18:20) and the display stays off |
| Realtime | `setcap cap_sys_nice=ep monado-service` (verified with getcap); GPU p99 4.9 -> 2.7 ms (memory log 08:2x) | Package install script runs setcap; launcher warns when missing (TODO noted in the memory log) |
| Monado | Fork, 27 commits on upstream (`git log origin/main..pimax`), patches 0001-0019 in `monado/` | A `monado-pimax-git` AUR/PKGBUILD or an Envision profile (Envision builds Monado from a repo/branch; custom-profile support **UNVERIFIED**, https://gitlab.com/gabmus/envision) |
| Rust | rustc 1.98.1; workspace MSRV 1.80 (`rust/Cargo.toml`) | cargo build of `omarchyvr-tools`, `pimax-capi`, `pimax-lens` before Monado (the Monado CMake stops if `libpimax_lens.a` is missing, `analysis/lens-distortion.md` "Rust model") |
| Steam | Proton + wineopenxr + xrizer, launch options from `omarchyvr vr steam-options` | xrizer via `VR_OVERRIDE` or `openvrpaths.vrpath` (https://github.com/Supreeeme/xrizer); xrizer calls itself "currently immature"; OpenComposite is the alternative |

**Newcomer install, as it would look for an alpha (Arch first):** install `nvidia-open-dkms` → install `pimax-linux`
package (udev rule, keepalive unit, `omarchyvr` binary, re-apply hook for all patches, `omarchyvr doctor`) → run
`omarchyvr doctor --fix-plan` (prints the exact sudo commands: patch + DKMS, EDID override from your EDID, initramfs) → reboot
→ install `monado-pimax-git` (setcap in the install script) → `omarchyvr vr start` → Pimax Setup wizard.

## 5. Headset variants and per-unit variation

| Model | USB IDs (CustomHeadsetOpenVR, `analysis/customheadsetopenvr.md` 1b) | Display | Our support |
|---|---|---|---|
| Dream Air (SLAM) | 34a4:0044 (+ bridge 0043) | 3840x3552 per eye, rotated, cant 2° | **This unit only** (prober matches 0044 only, `pimax_interface.h:30-31`) |
| Dream Air (Lighthouse) | same PIDs per CustomHeadsetOpenVR ("0x0043 is USB only") | same | Display/IMU path probably the same, tracking would come from lighthouse (Monado has `steamvr_lh`/vive drivers). **UNVERIFIED** |
| Dream Air SE | 34a4:0042 | 2544x2544 per eye, rotation 1, 90x90 FOV, cant 3° (`Config.h:327-348` in CustomHeadsetOpenVR) | Not supported: other PID, other panel geometry and lens profile; may not need 2Heads1OR (smaller mode). HID protocol likely similar: **UNVERIFIED** |
| Crystal / Crystal Light / Super, 5K/8K | 0x0012, 0x0018, 0x0040, 0x0101 | various | Different protocols; vronlinux: P2 (5K/8K) supported in a Monado `omarchyvr` branch; Crystals "not reverse engineered" (https://vronlinux.org/docs/fossvr/monado/pimax/) |

Per-unit data:
- **Factory calibration** (cameras, IMU bias/scale) lives on the headset and is read over USB (cmd 0x0a, 84 reads, zlib XML;
  `analysis/init-sequence.md:172`; `omarchyvr calib`). This works for any unit, no copy from Windows needed. The factory gyro
  bias was wrong for our unit; we use auto-bias plus a cache (`pimax_hmd.c:11`, `:211-213` `~/.cache/omarchyvr/gyro-bias`,
  **not per serial**).
- **No lens/distortion data** is in the factory XML or the Pimax profile (`analysis/lens-distortion.md` "Not found"); the
  `lens_rd` key (cmd 0x27) may be a per-eye offset, meaning unknown. So every unit starts from the shared default
  (`~/.config/omarchyvr/lens.conf`: cant 2.0, centres ±0.0271 0.009, k 0.60779 -0.72489 0.58858, ca_r 1.0045, ca_b 0.995) and
  may need tuning.
- What varied on ours: stereo fusion fixed by canting 2° (cant 2.0 and 2.95 both fused); a possible slight barrel / pupil
  swim; right eye looks redder; camera pair 3-8 px dy (selfcal); gyro bias. Which of these are unit-specific vs model-wide:
  **UNVERIFIED** until a second unit is measured.
- Firmware versions on ours: MCU 0.0.33/0.0.32, FPGA 0.1.1, bridge 1.2.6, SLAM 1.9.9.8, Tobii 7.32.0 (memory log). Other
  firmware may change the protocol; `omarchyvr doctor` should record versions and warn on unknown ones.
- Pimax's own software keeps the display alive over USB ("display shuts off unless Pimax software stays connected",
  CustomHeadsetOpenVR notes). Our keepalive does the same job. A Pimax firmware update could change that.

## 6. Ranked blockers and shortest path to a first public alpha

Ranked by "how likely it stops a stranger" × "how hard to remove":

1. **NVIDIA patches** (all NVIDIA users). Remove by: posting to PR #1262; upstream PR for the flip fix; meanwhile one
   re-apply script for all three patches + distro hooks.
2. **Display bring-up by hand** (all users): EDID override with this PC's PCI/connector, MS-VSDB EDID from our unit, NVIDIA
   0-mode problem. Remove by: EDID generator from the user's EDID, `omarchyvr doctor` printing exact commands, kernel quirk
   `PVR 0x1019` upstream.
3. **3DoF only** (all users, all content that needs 6DoF). Remove by: Basalt SLAM on the 4 cameras + IMU
   (`analysis/research/selfcal-roommap.md` recommends it), then controller tracking. Biggest work item; an alpha can ship
   as "seated 3DoF" if it says so clearly.
4. **Build from source** (Monado fork, Rust libs, setcap, hard-coded paths/connector/mode index). Remove by: PKGBUILDs,
   launcher config file (`~/.config/omarchyvr/setup.toml`), connector + mode auto-detection.
5. **Per-unit tuning only by terminal scripts** (some users). Remove by: the calibration GUI (Part 2).
6. **AMD/Intel unknown** (non-NVIDIA users). Remove by: one AMD tester with the test list in 2.3.
7. **Wedge = reboot** (all NVIDIA users, after a crash). Partly addressed by the flipsafe Monado branch and the revoke fix;
   document it in the alpha notes.

**Shortest path to alpha "Dream Air SLAM, NVIDIA RTX 20-40, open driver 610.x, Arch + Hyprland, seated 3DoF":**
1. Fix `nvidia-patch-reapply.sh` for all three patches; post the PR #1262 comment; open the flip-fix PR (days).
2. `omarchyvr doctor` (Phase 1 below) + EDID generator from the user's EDID + connector/mode auto-detect in the launcher.
3. PKGBUILDs: `pimax-linux` (tools, udev, keepalive, hooks) and `monado-pimax-git` (setcap in install).
4. Remove serials/factory data from anything public (the repo is private for that reason, memory log); publish a clean
   public repo with the code only.
5. Calibration GUI Phases 1-2 (health + lens steps) so testers can fix their own unit.
6. Recruit 3-5 testers (another Ada, one Ampere/Turing, one Blackwell, one AMD); collect `omarchyvr doctor --report` output.

---

# Part 2 — Calibration / adjustment GUI ("Pimax Setup")

## 7. What the user must be able to check and adjust

| Item | Why | Method (live preview in the headset) | Writes | Applies |
|---|---|---|---|---|
| IPD / lens centres | Eyes must sit on the optical axes | Read live IPD (IMU report bytes 45-46, `pimax-proto/src/imu.rs:17-29`); `lens_align` method: red line left eye, green line right, stick shifts them until they fuse (`analysis/tools/lens_align.sh`; 0.01 centre = 0.374°) | `[left]/[right] center` | Client-side preview live; lens.conf on accept → service restart |
| Canting | Our double far lines were canting (2°/eye) | Same align pattern far away + `swim` room; step cant 0.25° | `cant_deg` (global key, e09d81d; `PIMAX_CANT_DEG` override) | Restart |
| Distortion residual + pupil swim | Straight lines must stay straight while the head turns | World-locked room (`patterns/swim.rs`) + live dk1 / scale pre-warp (`src/tune.rs`: exact `scale' = scale*s`, k1..k5 refit, residual in degrees, warning above 0.1°) | `scale`, `k_*` | Pre-warp live; lens.conf on accept |
| Chromatic aberration | Colour fringes at the edges | Per-channel grids (`patterns/channels.rs`); keys for `ca_r`/`ca_b` in 0.002 steps (`analysis/lens-distortion.md` tuning step 4) | `ca_r`, `ca_b` | Needs a live pre-warp per channel (new) or restart per try |
| Per-eye colour balance | Right eye looks redder | Flat fields (`patterns/flat.rs`: white, 192, 128, 64, R, G, B) shown to one eye at a time / alternating; user nudges per-eye RGB gain until both match. Optional phone-photo method below | new `[left]/[right] gain = r g b` | Needs new per-eye gain in the compositor (patch 0011 has only a global `XRT_COMPOSITOR_OUTPUT_GAIN`) |
| Brightness / backlight | Comfort; 255 suspected of power dips (memory log H3) | Grey ramp pattern | headset command 0x26/0x27 key `pbrg` (`analysis/controllers-settings.md:48-49`) | **Only after** we confirm the value is not stored in flash (see safety) |
| Refresh 72/90 | Comfort vs GPU load | Moving-square pattern | `setup.toml fps` | Restart (`omarchyvr vr start --fps`) |
| Gyro bias | Drift / frozen pose | "Put the headset on the desk, don't touch" 3 s | `~/.cache/omarchyvr/<serial>/gyro-bias` | Next start (or a "recalibrate" control call, new) |
| Controllers | Pairing, buttons, battery | `omarchyvr controllers` decoded live; button tester screen | – | – |
| Hands on/off | Controller-driven hands (and the old hang suspicion) | Toggle + hand test (`xr-handtest`) | `setup.toml hands` → `PIMAX_CTRL_HANDS` | Restart |
| Camera calibration (selfcal) + recording | Passthrough/hand tracking quality | Guided 1-min lit recording (`analysis/research/selfcal-roommap.md` §7) then `pimax-selfcal` with its sanity gate | `cam-calib-overlay.json` (old kept as `.prev`, `pimax-selfcal/src/main.rs:9`) | Next passthrough start |
| Passthrough alignment | Real world at the right scale/depth | `omarchyvr-passthrough` keys p/+/-/[/]/g | new `passthrough.toml` | Live in the client |

**Phone-photo colour method (proposal, accuracy UNVERIFIED):** the GUI shows a flat white (then grey 128) field to the left
eye only, then the right eye only. The user takes a phone photo through each lens with fixed exposure and white balance
(pro mode or a locked AE/AWB), same distance, and drops both photos into the desktop app. The app takes the mean RGB of a
central patch of each, computes the per-channel ratio right/left, and proposes gains that pull the brighter channel down
(never above 1.0, so no clipping). Then the user confirms by eye with the alternating flat field. Phone sensor and lens
variation cancel out because both eyes go through the same phone settings.

**Health checks** (shared engine, CLI and GUI):
headset on USB (34a4:0044) + serial + firmware versions; udev access to hidraw (open succeeds without root); keepalive
service active; GPU vendor/driver/version (`/proc/driver/nvidia/version`); NVIDIA: open module, viewport patch present
(`nvidia-patch-reapply.sh --check` exit 0), flip fix loaded (`/sys/module/nvidia_modeset/parameters/merged_head_sync` exists and
= 1); connector found by EDID (PVR 0x1019), connected, 7104x3840 modes listed, non-desktop = 1 (DRM property read via ioctl);
compositor offers `wp_drm_lease_device_v1` (registry listing only, no lease taken); Monado build present and `getcap` shows
`cap_sys_nice`; Rust libs version matches the Monado build; kernel log for this boot: Xid, "Idling display engine timed
out", "Stuck layer channel" → "reboot before VR"; no stale monado-service; lens.conf parses (pimax-lens parser with range
checks) and belongs to this serial.

## 8. Architecture

### 8.1 Options compared

| Option | Good | Bad | Verdict |
|---|---|---|---|
| egui/eframe desktop app | Pure Rust, links our crates directly; immediate-mode fits "live values + sliders"; eframe runs on Wayland through winit (general knowledge; **UNVERIFIED** here on Hyprland/NVIDIA); easy for a helper sitting next to the wearer; works when VR is broken (health checks, fix plans) | The wearer cannot see it with the headset on | **Yes, as the control centre** |
| TUI (ratatui) / CLI | Tiny, scriptable, works over SSH, good for `omarchyvr doctor` and bug reports | Same "can't see it while wearing" problem; poor for sliders and photos | **Yes, for `omarchyvr doctor` only** (same engine) |
| In-headset OpenXR UI | The wearer sees it; controllers as input; we already have an OpenXR+Vulkan client that renders patterns and text (`pimax-lenstest`, `pimax-lens/src/font.rs`) | Useless when the display path is broken; text entry is painful; must run as the scene app (XR_EXTX_overlay is reported abandoned: https://community.khronos.org/t/overlay-app-with-openxr-and-cpp/110533) | **Yes, as the test-pattern + adjust surface**, driven by the desktop app |
| Web UI on localhost | Phone as remote control | A server, a browser, auth for a local port, JS + Rust split; more attack surface | **No** |

### 8.2 Recommended design
```
 omarchyvr-app (egui desktop app)  ──unix socket $XDG_RUNTIME_DIR/pimax/calib.sock (JSON lines)──  pimax-calib-xr (OpenXR client,
   │  wizard, health page, profiles,                                                              grown from pimax-lenstest:
   │  backups, photo analysis                                                                     patterns, tune pre-warp, controller input,
   │                                                                                               head-locked step text)
   ├── omarchyvr-config crate (profiles, lens.conf/setup.toml read/write, backups, atomic writes)
   ├── omarchyvr-doctor crate (health checks; also `omarchyvr doctor` CLI)
   └── omarchyvr-tools::vr (start/stop/status of monado-service; never SIGKILL a stuck flip)
```
- The XR client owns the live state while running: the GUI sends `set_step{pattern, params}`, `nudge{key, delta}`,
  `accept`, `cancel`; the client sends `state{values, residual_deg}` back after every change (controller or GUI). Both sides
  show the same numbers. Controller map: stick X/Y = adjust the current value (fine), trigger + stick = coarse, A = accept
  step, B = back / undo, menu = toggle the left-eye/right-eye/both view.
- Changes are previewed client-side (the client pre-warps / shifts its own pattern, exactly as `tune.rs` and the align
  pattern do now), so no service restart per nudge. On "accept" the GUI writes the new values into the profile, and at the
  end of a group of steps restarts monado-service once (`omarchyvr vr stop` with grace, then start) and re-shows the pattern
  with zero pre-warp to confirm.
- Later (optional): driver-side live reload of lens.conf (re-bake the 128x128 distortion table on a control message).
  Whether Monado's compositor can swap the distortion mesh at run time: **UNVERIFIED**; not needed for v1.

### 8.3 Files it writes
All under `~/.config/omarchyvr/`, per headset serial, so two headsets on one PC (or a swapped unit) never share tuning:
```
~/.config/omarchyvr/
  active -> profiles/P511X0X000000000001/default      (symlink to the active profile)
  lens.conf -> active/lens.conf                        (keeps today's path working for monado-service)
  profiles/<serial>/<name>/
    lens.conf             cant_deg, [common] fov/base/scale/k_*/ca_*, [left]/[right] center, + new [left]/[right] gain
    setup.toml            fps, hands, ctrl_pose, backlight (if enabled), connector override, monado_build
    passthrough.toml      offsets/scale from the passthrough step
    cam-calib-overlay.json   (selfcal output; .prev kept)
    meta.toml             serial, firmware versions, created, tool version, notes
  backups/<serial>/<timestamp>-<reason>/   full copy of the profile before every write (keep the last 30)
~/.cache/omarchyvr/<serial>/gyro-bias              (move from ~/.cache/omarchyvr/gyro-bias; driver change)
```
Format rules: keep `lens.conf` in today's INI format (parser in `pimax-lens/src/config.rs`); write with comments preserved
where possible; validate with the same parser and range checks before the rename; atomic write (temp file + fsync +
rename). The launcher reads `setup.toml` instead of hard-coded defaults; env vars still override it (for experiments).

### 8.4 Safety rules (hard rules in code, with tests)
1. **Never write headset flash.** No firmware update path. The command layer exposes an allowlist of commands; anything
   that may persist (key/value set 0x26, IPD 0x5c) is disabled until a capture proves Windows sends it every session (i.e.
   it is volatile). Read-only commands are always fine.
2. **Never force DRM hotplug** (`echo off/detect > /sys/class/drm/.../status`, debugfs EDID writes). They desynced
   Hyprland's CRTC state before (`linux/DISPLAY.md` "Do NOT") and the user forbade it.
3. **Never SIGKILL a monado-service with a stuck flip**; use `omarchyvr vr stop --grace`. If it reports "reboot needed", show that,
   don't retry.
4. **Never elevate.** The GUI never runs sudo/pkexec. System changes (patches, EDID, initramfs, setcap) are shown as a copyable
   command list with an explanation and an undo list. (This machine has passwordless sudo; an agent ran a root command by
   accident before, memory log 00:57.)
5. **Backup before every write**, one-click restore, "reset to defaults" = install the shipped default lens.conf into a new
   profile (the old one stays in backups).
6. **Serial check**: refuse to apply a profile whose serial differs from the connected headset without an explicit confirm.
7. **Bounded values**: every adjustable value has a min/max (reuse the pimax-lens range checks); colour gains ≤ 1.0.
8. **No display tests when the kernel log shows a stuck display** (Xid / Idling in this boot) — tell the user to reboot first.

## 9. First-run wizard
1. **Welcome / detect**: headset (USB 34a4:0044, serial, firmware), GPU + driver, compositor. Unknown model/firmware → warn.
2. **System check** (doctor): green/yellow/red list; each red item has "why", "fix commands", "undo". Stop here until all
   red items are fixed (reboot if needed; the wizard resumes from step 2).
3. **Profile**: create `profiles/<serial>/default` from the shipped defaults (or import a backup).
4. **First light**: start VR at 72 Hz; "put the headset on the desk, still" (gyro bias, 3 s); then "put it on: do you see the
   grid in both eyes?" (A = yes; B = no → troubleshooting page: panels dark, one eye only, both eyes same half = viewport
   patch missing).
5. **Fit**: adjust the physical IPD dial until the text is sharpest; the app shows live IPD mm.
6. **Stereo alignment**: align pattern; stick until the red and green lines fuse (centre), then far poles in the swim room
   (cant). Accept.
7. **Distortion & swim**: world-locked room; turn the head left/right; stick X = dk1 (lines bend), stick Y = scale (room too
   big/small); residual shown in degrees. Accept.
8. **Colour fringes**: channel grids at the edge; nudge ca_r/ca_b. Accept.
9. **Colour balance**: alternating flat fields; per-eye gains (or the phone-photo path on the desktop). Accept.
10. **Apply & verify**: write the profile (backup first), restart the service once, show all patterns with zero pre-warp;
    "does it still look right?" Yes → keep; No → restore the backup and repeat the failed step.
11. **Comfort**: refresh 72/90, brightness (if enabled), hands on/off.
12. **Tracking** (optional): controller button/battery test; camera recording + selfcal (advanced); passthrough check.
13. **Done**: summary of what changed, where the backup is, "Start Steam VR games" hint (`omarchyvr vr steam-options`).
Later runs open on a dashboard (health + "re-run step N").

## 10. Test plan

Offline (CI, no headset; follow the existing patterns: `Host` trait + `Mock` in `preflight.rs`, fake headset in `omarchyvr`,
`simulate`/`compare` in pimax-lenstest):
- omarchyvr-config: parse → write → parse round trip on every shipped and backed-up lens.conf; comment preservation; atomic write
  on a full disk / read-only dir; backup rotation; restore; serial mismatch refusal; symlink switching.
- omarchyvr-doctor: each check against a mock host (sysfs tree in a tempdir like `PIMAX_SYSFS_ROOT`, fake `/proc/driver/nvidia/version`,
  fake kernel log lines, getcap output); report rendering; "fix plan" text never contains a forbidden command (hotplug
  write, SIGKILL) — a test greps for them.
- Command allowlist: unit test that the HID command builder refuses 0x26 set / 0x5c / firmware opcodes unless enabled.
- Wizard state machine: pure transitions (next/back/skip/resume after reboot) with a fake clock.
- Socket protocol: JSON schema round trip; client state machine with a fake socket; nudge clamping.
- Tune math: existing `tune.rs` tests (exact scale, k refit residual); extend to ca_r/ca_b pre-warp.
- Photo analysis: fixture JPEGs (synthetic, with a known red cast) → expected gains within tolerance.
- Render: `pimax-calib-xr preview DIR` PNGs of each step compared with golden images (already done for patterns).
- Windowed integration (no headset): monado-service with `PIMAX_NO_DEVICE=1`, `XRT_COMPOSITOR_FORCE_XCB=1` + client capture
  (`analysis/lens-distortion.md` "Verification").

Needs a wearer (one checklist per wizard step, like `analysis/lens-test-checklist.md`):
first light both eyes; alignment fuses at near and far; swim residual subjectively gone while turning; fringes reduced;
colour match by eye; 72 and 90 Hz 10 min each without freeze; profile restore really restores; controller nudges map the
right way for each eye; a second person (different IPD) runs the wizard from scratch; one test on a second Dream Air unit.

## 11. Phased build plan (sizes are rough estimates in lines of Rust, not measurements)

| Phase | Scope | Size |
|---|---|---|
| P0 prerequisites | Per-serial gyro-bias path in the driver; `omarchyvr info --json` (serial, firmware, IPD); launcher reads `setup.toml`; connector auto-detect by EDID; mode index from the mode list; re-apply script for all 3 patches | ~600 + small C change |
| **P1 health + profiles** | `omarchyvr-config` crate; `omarchyvr-doctor` crate + `omarchyvr doctor [--json] [--fix-plan]`; egui app shell: dashboard, health page, profile list, backup/restore, start/stop VR | ~2,500 |
| P2 in-headset lens steps | Control socket in pimax-lenstest → `pimax-calib-xr`; step text overlay; controller mapping; wizard steps 4-8 and 10; write lens.conf + one restart | ~2,500 |
| P3 colour & comfort | Per-eye RGB gain in the Monado compositor (extend patch 0011 UBO per view) + lens.conf keys; flat-field step; phone-photo analysis; refresh/hands toggles; brightness only after the flash check | ~1,500 + Monado patch |
| P4 tracking | Gyro recalibration control; controller tester; camera recording + selfcal runner with progress; passthrough alignment step | ~1,500 |
| P5 packaging | PKGBUILDs (pimax-linux, monado-pimax-git), EDID generator wrapper, Ubuntu/Fedora notes, desktop entry, public repo without serials | ~800 + scripts |

### Tasks for Sonnet agents (one at a time, each with tests, offline unless noted)
1. P0-a: Monado pimax driver: gyro-bias path `~/.cache/omarchyvr/<serial>/gyro-bias` with fallback to the old file. C, ~50 lines, keep ctest green.
2. P0-b: `omarchyvr info --json` (serial, firmware versions, live IPD) in omarchyvr-tools, with fake-headset test.
3. P0-c: launcher: connector auto-detect (sysfs EDID PVR 0x1019) + mode-index lookup from `XRT_COMPOSITOR_PRINT_MODES` output (parser + tests on captured logs); keep `DP-1`/index as fallback.
4. P0-d: `nvidia-patch-reapply.sh` handles a list of patches (viewport, flip fix, bounded wait) with `--check` per patch; extend `test_nvidia_patch_reapply.sh`.
5. P1-a: crate `omarchyvr-config`: profile layout, lens.conf read/write via pimax-lens parser, `setup.toml`, backups, atomic write, serial check. Unit tests in tempdirs.
6. P1-b: crate `omarchyvr-doctor`: move preflight checks in, add the checks from section 7 behind a `Host` trait; `omarchyvr doctor` CLI with `--json`/`--fix-plan`; forbidden-command test.
7. P1-c: `omarchyvr-app` egui app shell: dashboard, health page (runs doctor), profiles + backups page, start/stop VR buttons (calls omarchyvr-tools::vr). Manual smoke test on Hyprland.
8. P2-a: control socket protocol crate (`pimax-calib-proto`): message types, JSON lines, tests.
9. P2-b: pimax-lenstest: socket server + head-locked step/value text + controller mapping per section 8.2; keep the CLI keys working.
10. P2-c: GUI wizard steps 4-8 + 10 (state machine crate with tests, then UI).
11. P2-d: CA pre-warp per channel in the client (extend tune.rs) + tests.
12. P3-a: Monado: per-eye RGB gain in the compute distortion pass, lens.conf `gain` key via pimax-lens; difftest with gain 1 = bit-identical.
13. P3-b: flat-field balance step + phone-photo analyser (image crate, fixtures).
14. P3-c: capture-based check whether the brightness key (0x26 `pbrg`) is sent every session on Windows (read-only analysis of existing pcaps); only then a `pimax brightness` command.
15. P4-a: controller tester page; gyro recalibration control path.
16. P4-b: selfcal runner page (recording guide, progress, sanity gate result, apply/rollback).
17. P5-a: PKGBUILDs + install hooks (setcap, udev, user unit) + public-repo scrub list.

## 12. Open questions / unverified list
- Blackwell single-head pixel clock limit (does 7104x3840@90 work without 2Heads1OR?).
- Turing/Ampere per-head limits and DSC at HBR3 for this sink.
- AMD ODM auto-selection and DSC behaviour for this mode; Intel Alchemist joiner limits; xe direct mode.
- KWin / Mutter shipping versions with working leases for a non-desktop NVIDIA connector.
- Whether other Pimax models share EDID PVR 0x1019; Dream Air SE protocol differences.
- Whether lens tuning differs much between units (needs a second unit).
- Whether the brightness and IPD commands write flash.
- Whether Monado can swap the distortion mesh at run time.
- egui/eframe behaviour on Hyprland + NVIDIA (expected fine, not tried).
