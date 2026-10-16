# DSC research: Pimax Dream Air on Linux/NVIDIA

Date: 2026-10-13. Web research only, no system changes.
Confidence: H = confirmed in a primary source I read; M = secondary or search snippet; L = inference or unverified.
Note: LT7911UXE datasheet was NOT found. Lontium claims below are from search snippets and sibling-part docs.

## 1. VESA DSC 1.2a in plain terms

- DSC is a visually lossless, low-latency intra-frame codec. It compresses each line in small groups of pixels, so the link carries fewer bits per pixel than raw RGB. Source of field definitions: Linux DRM `drm_dsc.h` header (cites DSC v1.1/v1.2 Table 4.1) [https://android.googlesource.com/kernel/common/+/4b05a506bda0/include/drm/drm_dsc.h] (H for field layout, M for version attribution).
- Target bpp: compressed bits-per-pixel is set in PPS (bits_per_pixel is 10-bit, in 1/16 bpp units; the high 2 bits sit in PPS4[1:0], low 8 in PPS5) (H, drm_dsc.h). Typical targets 8, 10, 12 bpp; exact choice depends on link budget (L for specific values per mode).
- Slices: the picture is split into vertical slices that must share one width. Width is rounded up, e.g. 5120 px / 12 slices gives 427 px per slice (H arithmetic, source: Synopsys article [https://www.synopsys.com/articles/overcoming-design-challengse-vesa-dsc-ip.html]).
- PPS: a 128-byte Picture Parameter Set sent from encoder to decoder describing the whole stream (bpp, bits per component in PPS3[7:4], slice size, line buffer depth). Multi-byte fields are big-endian (H, drm_dsc.h).
- Sink capability discovery (DPCD, DP 1.4a), verified from Linux `include/drm/display/drm_dp.h` [https://raw.githubusercontent.com/torvalds/linux/master/include/drm/display/drm_dp.h] (H, fetched):
  - DP_DSC_SUPPORT 0x060 (bit 0 = decompression supported; bit 1 = DSC pass-through supported, DP_DSC_PASSTHROUGH_IS_SUPPORTED)
  - DP_DSC_REV 0x061; DP_DSC_SLICE_CAP_1 0x064; DP_DSC_MAX_BITS_PER_PIXEL 0x067/0x068; DP_DSC_MAX_SLICE_WIDTH 0x06C; DP_DSC_SLICE_CAP_2 0x06D; DP_DSC_BITS_PER_PIXEL_INC 0x06F
  - DP_FEC_CAPABILITY 0x090 / 0x091; DP_FEC_CONFIGURATION 0x120; DP_FEC_STATUS 0x280 (H)
  - Source-side enable: DP_DSC_ENABLE 0x160 (bit 0 decompress enable, bit 1 pass-through enable), DP_DSC_CONFIGURATION 0x161 (H)
  - Branch caps: DP_DSC_BRANCH_OVERALL_THROUGHPUT_0/1 0x0A0/0x0A1, DP_DSC_BRANCH_MAX_LINE_WIDTH 0x0A2 (DP 1.4a SCR) (H)
- A source enables DSC only if the sink advertises DSC in 0x060 and, for DSC over DP, FEC is usually required on the link (DP 1.4 DSC needs FEC; L on exact rule, from general DP 1.4 knowledge, not re-verified here).
- Pass-through vs decode-in-bridge: the DPCD bits above show a distinction exists in the protocol: a branch/repeater can advertise pass-through (0x060 bit 1), meaning it forwards the compressed stream untouched to a downstream DSC decoder. Decode-in-bridge means the bridge decompresses and outputs uncompressed pixels to the panel path. Which mode the Lontium part uses is not confirmed (see section 2). (H for protocol bits, L for how this applies to the bridge.)

## 2. Lontium LT7911UXE and similar headsets

- LT7911UXE datasheet: not found. Search only returned sibling parts [https://damodev.csdn.net/6a437b4010ee7a33f284da27.html] (M, Chinese product brief, LT7911UX: Type-C/DP1.4a to four-channel MIPI/LVDS bridge, DP 1/2/4 lanes, HDCP 1.3/2.3).
- LT7911UXC: LCSC-hosted datasheet snippet lists DSC pass-through up to 8K@60Hz and four MIPI DSI/CSI ports, D-PHY 1.2 / DSI 1.3 [https://wmsc.lcsc.com/wmsc/upload/file/pdf/v2/lcsc/2212091108_LONTIUM-SEMICONDUCTOR-LT7911UXC-U3B00AED_C5310989.pdf] (M, snippet only, not opened).
- LT7911 base R1.3 datasheet: DP receiver at DP 1.2 speeds, quad MIPI DSI/CSI (M, search snippet, older than UXE).
- Conclusion: the snippet suggests pass-through, not decode, for the UXC. Whether the UXE behaves the same is L until the datasheet is read. Action: obtain the UXE datasheet from Lontium or a distributor and check the DSC section.
- Pimax Crystal / Crystal Light / Dream Air DSC from GPU: NOT CONFIRMED. Dream Air listed as DisplayPort 1.4 with split DP-to-USB-C cable, 90 Hz, 53 PPD [https://www.scan.co.uk/products/pimax-dream-air-lighthouse-vr-headset-2x-3840-x-3552-micro-oled-lenses-53-ppd-90hz-displayport-14] (M). No source found stating whether it requires DSC. Pimax Crystal Pro wired mode uses DP 1.4a [https://www.avsim.com/forums/topic/702443-pimax-announces-crystal-pro-wireless-option-displayport/] (M).
- Bigscreen Beyond: its Linux failures are described as DSC-related in an NVIDIA forum thread (section 3), so DSC from the GPU is implied (M). Bigscreen's own spec sheet was not checked (L).
- Shiftall MeganeX, Apple-style micro-OLED: not researched further (L). MeganeX search returned only Monado generic docs [https://monado.freedesktop.org/direct-mode.html].

## 3. NVIDIA Linux DSC status

- Staff statement, 2022-08-25 (prikkonen): "DSC is functional today in the Linux drivers, but only for single connector displays." Dual-connector support said to be in progress [https://forums.developer.nvidia.com/t/display-stream-compression-on-linux-displayport-1-4/209998] (H for quote, date).
- Staff statement, 2023 (quoted by a forum user): "It's being worked on, but it's not a simple feature so I don't have an ETA." And DSC "already enabled and working at lower pixel clocks"; higher clocks need more complex hardware config [https://forums.developer.nvidia.com/t/display-stream-compression-on-linux/190266] (M, quoted second-hand).
- Driver 535: 240 Hz 5120x1440 over DP reported working by users (2023-06) (M). 575.64: dual-monitor DSC problems on proprietary driver reported 2025-07 (M).
- Bigscreen Beyond on NVIDIA Linux, the thread closest to our case: opened 2024-05-25, "unable to be initialized... possibly due to DSC BPP issue"; failures on drivers 535 through 555 and 560 beta; NVIDIA staff (aritger, 2024-07-18) said it looked like a 550 regression and filed internal bug 4834531; no fix by 2024-11 [https://forums.developer.nvidia.com/t/bigscreen-beyond-hmd-unable-to-be-initialized-on-nvidia-possibly-due-to-dsc-bpp-issue/294108/1] (H for thread facts; M for the suspected cause, which is an inference).
- Vron Linux: Bigscreen DSC fix for NVIDIA requires driver 580+ open kernel modules, is a patch to the open modules (nvidia-bsb-dsc-fix.patch), and is not in 580 and below; some distros ship pre-patched kernels/modules (Arch linux-bsb, CachyOS, Bazzite) [https://vronlinux.org/docs/hardware/bigscreen-beyond/] (H, fetched; whether the patch is still needed on 610.x is unknown, L).
- Pimax Crystal / MeganeX / Varjo on Linux NVIDIA DSC: no specific reports found (L).
- Workarounds seen: X.org `NoDisplayPortBandwidthCheck` ModeValidation option (a diagnostic, not a fix, if bandwidth is truly short) [https://forums.developer.nvidia.com/t/bigscreen-beyond-hmd-unable-to-be-initialized-on-nvidia-possibly-due-to-dsc-bpp-issue/294108/1] (M). Monado `XRT_COMPOSITOR_FORCE_NVIDIA_DISPLAY` allowlist override, and a note that proprietary NVIDIA direct mode does not need an allowlisted HMD [https://monado.freedesktop.org/direct-mode.html] (M).
- NVIDIA forum "force DSC" NVAPI setting thread (2025-07-02) exists in the related-topic list; not opened (L).

## 4. Headsets whose DSC depends on a DisplayID/EDID value or vendor-specific setup

- Patch series "DisplayID DSC passthrough timing support" (v4, Yaroslav Bolyukin, Oct 2025): the DisplayID VESA spec lets the display force its DSC bits-per-pixel. Patches parse it and make amdgpu use it. Tested on Vive Pro 2 and Bigscreen Beyond. Author says the value is unused in both amdgpu and NVIDIA open modules [https://patchew.org/linux/20251016001038.13611-2-iam@lach.pw] (H for content, M for claims about headset behavior). Merge status: not confirmed; v5 exists; author hoped for 6.19 (H).
- The same DisplayID bpp mechanism is the suspected cause for the Bigscreen NVIDIA failure (M, inference from the thread).
- Vendor USB commands before DPCD DSC is visible: NOT FOUND in this research (L). No evidence gathered either way.
- FEC/link training rate dependence: DSC over DP depends on FEC (DP 1.4), and link rate affects which modes are possible; the NVIDIA forum shows DSC limited to single connectors and 8-bit on some setups (M). No headset-specific source found (L).
- Action: dump the Dream Air EDID (analysis/dp-edid-*.bin already in repo) and check DisplayID type VII blocks for a DSC bpp value.

## 5. AMD alternative

- amdgpu has DSC for DP MST since Linux 5.6 [https://www.phoronix.com/news/AMD-Linux-5.6-DP-MST-DSC] (M, from search snippet title).
- HDMI 2.1 DSC: patches reported May 2026, expected in Linux 7.2 [https://phoronix.com/news/HDMI-2.1-DSC-AMDGPU-FRL] (M, search snippet).
- VR direct mode with amdgpu: Monado tested on RADV; AMDVLK cannot lease displays; HDMI HMDs unsupported in direct mode (per a VR guide) [https://vronlinux.org/docs/hardware/graphics-cards/] (M).
- The DisplayID DSC bpp fix (section 4) exists specifically for amdgpu and Vive Pro 2 / Bigscreen; not confirmed merged (H for existence, L for merge).
- Practical: AMD has the more-documented path for VR DSC bpp handling on Linux; NVIDIA has an open issue and a driver-580+ patch for Bigscreen (M).

## Gaps and next steps

- Obtain LT7911UXE datasheet (Lontium site or distributor) and read DSC section (decode vs pass-through, max input).
- Check whether the NVIDIA 610.x open modules still need the bsb DSC patch; test with a patched build only under a separate, approved plan (no system changes in this research).
- Check Dream Air EDID DisplayID for a forced DSC bpp value (analysis/dp-edid-linux.bin or dp-edid-tall72.bin).
- Search Reddit r/virtualreality and LVRA wiki for Pimax Linux DSC (not done; search returned none).
