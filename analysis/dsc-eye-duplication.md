# Both panels show the LEFT half: DSC / 2Head1OR analysis (NVIDIA open 610.57.04)
Source tree: /home/user/src/open-gpu-kernel-modules (tag 610.57.04). NT=src/common/modeset/timing, NM=src/nvidia-modeset/src.
Research only; nothing was run or changed. Confidence: H = read in code, M = inference, L = guess (no bridge datasheet).

## 1. What the driver sends for 7104x3840@72 (pclk 2170.07 MHz)
- Mode takes 2Head1OR + DSC (forced: NM/dp/nvdp-connector.cpp:498-509). Sink slice mask 0x2b = {1,2,4,8}; dual mode drops 1 and 6
  (NT/nvt_dsc_pps.c:2306-2310, :1561-1570).
- Min slices = max(ceil(7104/2560)=3, ceil(2170/340)=7) = 7 (DSC_GetMinSliceCountForMode, nvt_dsc_pps.c:1381-1424).
  7 is not in {2,4,8}, so the next valid count is 8 (:1411-1417). Max is 8 (:2299), so **8 is the only legal choice**
  (possibleSliceCountMask = {8}; RM IMP cannot pick anything else, NM/dp/nvdp-connector.cpp:666). Same result at 90 Hz (2642/340=7.8 -> 8). (H)
- Per head: 4 slices (dual mode, each head max 4; GetTileWidth/headDscSlices, NM/nvkms-evo4.c:2688-2735 is the Blackwell variant, Ada uses
  EvoSetDpDscParams MODE=DUAL on each head, nvkms-evo3.c:7227-7262). Total 8. (H for the 8, M for the 4+4 hardware split)
- Slice width = ceil(7104/8) = 888 (nvt_dsc_pps.c:1601). Slice height: auto, loop i=1..16 takes pic_height/i=3840 first if
  DSC_PpsCheckSliceHeight passes (:1790-1801), so most likely **slice_height = 3840 (one slice spans the whole frame height)**. (M: check
  only verified by reading, not by running the lib.)
- bpp: link allows 11.4 bpp but sink max is 8.0 (DPCD 0x67/68 -> 128/16) and dual min is 8.0, so **bits_per_pixel = 128 (8.0 bpp)**
  (nvt_dsc_pps.c:2865-2885, :2929). chunk_size = 888*128/(8*16) = 888 bytes (:1689). One line = 8 chunks = 7104 bytes.
- **PPS: one combined PPS** for the whole picture. `in->pic_width = pModesetInfo->activeWidth` = 7104 (nvt_dsc_pps.c:2603); pic_width range for
  dual is 64..8192 (:1009). pps[8..9]=7104, pps[12..13]=888, pps[10..11]=slice_height. The same 32-dword PPS is written to BOTH heads
  (NVC57D HEAD_SET_DSC_PPS_DATA0..31 per head, nvkms-evo3.c:7256-7286, SDP id 0x00 type 0x10) from pDscInfo->dp.pps; there is no per-head PPS. (H)
- Merge on the wire: each head gets half the raster (nvEvoGetSingleMergeHeadSectionHwModeTimings, nvkms-evo.c:9798-9835: x/2, pclk/2,
  viewPort.in.width/2). Secondary head's viewport point-in X is shifted by in.width*mergeHeadSection (nvkms-hw-flip.c:1247-1251, flip.c:821);
  RG_MERGE MASTER/SLAVE (nvkms-evo3.c:7422-7432). HW then emits one DSC stream; per DP/VESA DSC each line carries the slice chunks
  left to right (s0..s7), i.e. a normal 7104-wide, 8-slice stream with one PPS. The interleave itself is in silicon, not in source. (M)
- DPCD 0x060 = 0x01 on this sink: decompression only, the DPCD pass-through bit (bit1, dp_deviceimpl.cpp:1729) is NOT set, so the driver
  treats the bridge as a normal DSC decoder (0x160 decompress enable), not as a pass-through device.

## 2. Which layouts would give "both panels = left half"
Bridge model (L): 4 DSI ports, 2 per panel, panel = 3552 px = exactly 4 slices of 888. So 8x888 splits perfectly 4+4. Candidates:
1. Bridge/DDIC use a FIXED per-panel PPS (pic_width 3552, 4 slices; typical for DDIC init sequences) and the bridge fails to route
   slices 4..7 to the right panel. Both DDICs then decode slices 0..3 = left half. Cause would be bridge config (split/"copy" mode), selected
   from the incoming DP timing or by firmware/USB setup, not from the PPS.
2. The bridge forwards the whole 7104-wide line (8 chunks) to each panel; each DDIC (PPS pic_width 3552) consumes the first 4 chunks and
   drops the rest. Same visible result.
3. The bridge forwards our PPS (pic_width 7104, 8 slices) unmodified to both DDICs; a DDIC configured for 3552 would reject it, so this
   gives garbage rather than a clean duplicate. Less likely.
4. Slice count mismatch: if the bridge expected 4 slices/line (2 per DSI port x 2 ports for the pair) it would break, not duplicate. Unlikely.
5. NOT bridge-related: the secondary head fetches the same left half of the surface (viewport X offset not applied), so the 8-slice stream
   itself contains left|left. Flip-time offset is applied only when viewPortIn.specified (kapi sets it on modeset, nvkms-kapi.c:3056-3059), so
   the code looks right, but the windowed test only proved the compositor output, never the scanout of head 2. Cheap to separate: see section 5.
Windows typical layout: the same RM/DP-lib code builds PPS on Windows (shared nvt_dsc_pps), so at 7104x3840@90 it must also be 8 slices x 888,
8.0 bpp, one PPS. Pimax/Varjo-specific slice counts are not documented anywhere I could find. So the slice layout probably does NOT differ from
Windows; the likelier differences are timing (72 vs 90 Hz, our custom EDID/non-desktop VSDB), PPS slice_height/RC params, or bridge state. (M)

## 3. Knobs reachable on Linux
- NvKmsModeValidationParams (NM/../interface/nvkms-api.h:418-475): `dscMode` (DEFAULT/FORCE_ENABLE/FORCE_DISABLE) and
  `dscOverrideBitsPerPixelX16`. There is NO slice count / slice width / PPS field in validation params. Slice count is derived (section 1);
  forcedParams->sliceCount is only filled post-IMP from RM's choice (nvdp-connector.cpp:511-515).
- nvidia-drm never passes them: kapi InitNvKmsModeValidationParams sets only NO_RRX1K_CHECK (nvkms-kapi.c:2519-2538). No nvidia-drm property,
  no module param, no regkey for DSC bpp/slices (DP regkeys only: DISABLE_DSC etc., unreachable from nvidia-drm, nvidia-linkrate.md).
  => bpp/slices cannot be changed without rebuilding nvidia-modeset (and bpp is already pinned: 8.0 is both min and sink max).
- DisplayID 2.0 Type VII "DSC passthrough" IS implemented in 610.57: Type VII block header bit `dsc_passthrough` with block revision >= 1
  (DISPLAYID_2_0_TYPE7_DSC_PASSTHRU_REVISION=1, displayid20.h:218) sets NVT_FLAG_DISPLAYID_T7_DSC_PASSTHRU (nvt_displayid20.c:636) ->
  modepool.c:416 flags.dscPassThrough -> the status line "DSCPassThrough: Yes". Effect: DSC forced on and bpp forced to the value in the
  DisplayID VESA vendor-specific data block (OUI 3A-02-92): `pass_through_integer_dsc` (6 bits, data byte 2) * 16 + `pass_through_fraction_dsc`
  (4 bits, byte 3) (nvdp-connector.cpp:523-548, nvt_displayid20.c:1377-1380, nvtiming.h:2477-2478); integer==0 rejects the mode.
  It changes ONLY bpp (and forces RGB, nvkms-evo.c:6904-6915). It does NOT change slice count, slice width or the PPS layout, and not the
  DPCD 0x160 enable. Reachable without a rebuild: yes, via the EDID override (debugfs edid_override or the existing EDID path), as the
  repo already does for the non-desktop VSDB. Useful only if the bridge wants a bpp other than 8.0; the sink max is 8.0, so the value to
  test is 8 (integer=8, fraction=0). This is the same mechanism the Bigscreen/Vive Pro 2 DisplayID patches target (patchew 20251016001038).
- Max slice width / count are DPCD (0x64, 0x6C) and cannot be altered by EDID. A rebuild could mask them in dp_deviceimpl.cpp:2642-2745.

## 4. Known reports
- No report found of a Pimax or any headset duplicating one eye with DSC on NVIDIA Linux (web search, 2026-10-13). Closest: Bigscreen Beyond
  fails to initialise on NVIDIA 550+ (DSC bpp, forum.developer.nvidia.com thread 294108, NVIDIA bug 4834531); fix is the out-of-tree
  nvidia-bsb-dsc-fix.patch ("corrects DSC timings"/RC parameter tables, per vronlinux.org Bigscreen page; shipped in linux-bsb, CachyOS, Bazzite).
  Whether 610.57 still needs it is unknown. Fixes there are driver patches, none EDID-only. See also dsc-research.md.

## 5. Experiments, ordered by value/cost (all read-only or EDID-only; none run yet)
1. **Run 7104x3840@90 (2642.6 MHz, in the EDID, fits HBR3 at 8.0 bpp)** with the unchanged EDID otherwise. Same 8x888 layout as Windows and the
   exact timing Windows uses. If stereo is correct, the bridge keys its split on the input timing (or 72 Hz bridge setup), and 72 Hz needs
   Windows-like blanking/clock. If still left|left, the DSC/timing is not the key.
2. Head-2 scanout check without the bridge: put a 7104x3840 test pattern (left red / right blue, or an index ramp) straight through
   nvidia-drm (dumb buffer atomic commit) on any other 2Head1OR output, or on the headset, bypassing Monado/Vulkan. If a normal monitor in
   2Head1OR also shows left|left, the bug is NVIDIA's secondary-head viewport (hypothesis 5), not the bridge.
3. Edid-override with a DisplayID 2.0 Type VII timing (dsc_passthrough=1) plus VSDB bpp 8.0, same 7104x3840@72/90 timing. Gets
   "DSCPassThrough: Yes" in /proc/driver/nvidia-modeset (read-only). Low odds of fixing duplication (only bpp is affected), but it matches what the
   headset's real Windows EDID may advertise; compare with the Windows EDID dump if available.
4. Rebuild nvidia-modeset with a DSCDBG printk of PPS bytes (pic_width, slice_height, chunk, RC) at nvdp-connector.cpp:675 and compare with a
   Windows DSC capture (bridge-windows-logs.md) if the bridge exposes the received PPS over USB logs.

## Most promising concrete experiment
Test 7104x3840@90 first (exp. 1): it is free, matches Windows exactly (same 8 slices, 888 wide, 8.0 bpp), and tells us if the split is keyed on timing.
