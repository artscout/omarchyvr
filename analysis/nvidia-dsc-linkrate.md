# NVIDIA 610.57.04: DP link rate / DSC bandwidth check for the Pimax Dream Air
Source: open-gpu-kernel-modules 610.57.04. `DP`=src/common/displayport, `NT`=src/common/modeset/timing, `NM`=src/nvidia-modeset/src.
All citations file:line. Code research only; nothing was run on the system.

## Verdict
- Hypothesis A as stated (DP lib reads the base MAX_LINK_RATE) is NOT supported: the lib reads the extended caps and sees HBR3.
- The bandwidth numbers prove the rejection happens ONLY if the lib budgets HBR2 (highestAssessedLC or preferredLinkConfig = HBR2).
  At HBR3 both modes fit with DSC dual (2Head1OR). So the question is why the lib's budget is HBR2, not a cap-parsing bug.
- Hypothesis B (throughput/slices) is not a blocker: it merely forces 2Head1OR (8 slices), which nvkms enables on Ada.
- Hypothesis C: bpp exactly 8.0, 9-bit line buffer, slice width are all fine. One real extra limit found: non-dual DSC needs pic_width <= 5120.

## 1. Receiver caps and max link rate
- `DPCDHALImpl::parseAndReadCaps` reads DPCD 0x000E (dp_configcaps.cpp:165-169); if bit7 (EXTENDED_RX_CAP) is set it burst-reads
  16 bytes from NV_DPCD14_EXTENDED_REV (0x2200) INSTEAD of 0x0000 (dp_configcaps.cpp:173-180, buffer[16] at :146).
  Rev, MAX_LINK_RATE (buffer[1]), lanes etc. all come from that buffer. 0x2201=0x1e -> `dp2LinkRate_8_10Gbps` (dp_configcaps.cpp:263-265).
  So with 0x000E=0x81 the lib uses HBR3. No "bSkipExtendedCaps", no OUI/device-ID WAR is consulted in this function
  (the only revision WAR, :213-236, is the MSTAR DP1.1 downgrade keyed on DOWN_STREAM_PORT OUI_SUPPORT=NO and ESI sinkCount==0;
  it is inside the non-override branch and only changes revisionMajor/Minor, never maxLinkRate).
  If the 0x2200 read were NACKed the whole read fails and the sink is marked dpcdOffline (:182-188), not silently HBR2.
- eDP-only ILR table can only raise the rate (:277-290). `getMaxLinkRate()` = caps.maxLinkRate, min'd with LTTPR/DP-tunnel caps (:566-581).
- `ConnectorImpl::getMaxLinkConfig` (dp_connectorimpl.cpp:1144-1182) = min(sink max, `main->maxLinkRateSupported()`), lanes from sink.
  GPU side (dp_evoadapter.h:239-253): returns the DCB/DFP limit ONLY IF regkey APPLY_LINK_BW_OVERRIDE_WAR is set or RM reports
  `bOverrideLinkBw` (dp_evoadapter.cpp:298); otherwise the GPU max (your caps: maxLinkRate 4 = 8.1G). `bOverrideLinkBw` is filled by RM
  firmware, not visible in source; it is the one non-code way to get a silent HBR2 cap.
- `bAvoidHBR3` (RM flag, dp_evoadapter.cpp:301) only makes compoundQueryAttachSST try HBR2 first (dp_connectorimpl.cpp:2324-2347);
  if DSC at HBR2 fails it falls through to the HBR3 path, so it cannot cause this rejection. Regkey DP_DISABLE_AVOID_HBR3_WAR disables it.
- Where the budget comes from: `LinkConfiguration lc = highestAssessedLC` (dp_connectorimpl.cpp:2297), or `preferredLinkConfig` if valid (:2300-2303).
  `highestAssessedLC` starts HBR3x4 (:180, initMaxLinkConfig), is set to `_maxLinkConfig` when the link is quiesced/in firmware (:4911),
  otherwise `assessLink` trains at `_maxLinkConfig` and stores the RESULT of RM's fallback: `lConfig = activeLinkConfig` (:5118),
  `highestAssessedLC = lConfig` (:5173), warning at :5149 "assessLink(): Failed to reach max link configuration (%d x %rate)".
  => If HBR3 link training fails at hotplug (headset not awake, bridge not ready), RM falls back to HBR2 and the lib then budgets HBR2 for the
  whole session until the next assessLink (replug/HPD). The dpy attribute "link rate 20" is the ACTIVE link (event-sink.cpp:647), which for a
  modeset is the lowest config that fits, so it is not by itself proof; but your rejection of both modes is (see 2).
- preferredLinkConfig pins the budget to the then-active config if someone sets NVKMS attribute DISPLAYPORT_FORCE_FEC
  (nvkms-attributes.c:961 -> nvdp-connector.cpp:1220-1229 -> setPreferredLinkConfig with the ACTIVE config). nvidia-settings/nvkms clients only.

### Regkeys
- DP-lib regkeys (dp_evoadapter.cpp:78-117 table) are fetched by `EvoInterface::getRegkeyValue` -> `nvGetRegkeyValue` (nvdp-evo-interface.cpp:76-85,
  nvkms-utils.c:649-660) from `pDevEvo->registryKeys[]`, copied from the NVKMS ALLOC_DEVICE ioctl request (nvkms-evo.c:9008-9020).
  They are NOT read from NVreg_RegistryDwords and NOT from any module parameter of nvidia.ko/nvidia-modeset.ko.
  nvidia-drm's kapi allocates the device with a zeroed request and sets no keys (kapi/src/nvkms-kapi.c:462-491), so on the DRM/Wayland path
  these regkeys are unreachable without a source patch (only the X driver / NVKMS userspace clients can pass them).
- No regkey sets a max link rate for a DP (non-eDP) sink: APPLY_OVERRIDES_FOR_BUG_2489143 is honoured only `if (main->isEDP())` (:1150-1158).
  Relevant keys that exist: DISABLE_DSC, DP_DISABLE_AVOID_HBR3_WAR, APPLY_LINK_BW_OVERRIDE_WAR, DP_IGNORE_CAPS_AND_FORCE_HIGHEST_LC_WAR
  (only changes train(): force=true, skips validateLinkConfiguration, dp_connectorimpl.cpp:6425), OVERRIDE_DPCD_REV (rev only), DISABLE_DOWNSPREAD (no BW effect).
  None forces extended caps (they are already used) or raises the rate.

## 2. DSC bandwidth check, with the code's own formulas
Path: nvDPValidateModeEvo (nvkms-evo.c:7063) -> nvDPLibIsModePossible -> compoundQueryAttachSST (dp_connectorimpl.cpp:2268-2400).
Non-DSC check at 24 bpp fails first, then `compoundQueryAttachSSTDsc` (:2002) runs with lc.enableFEC(true) (:2024).
- Budget: `avail = convertMinRateToDataRate()*8*lanes` (:2046). minRate = rate - 3% (FEC+spread, dp_linkconfig.h:371-377), x8/10 (OVERHEAD_8B_10B).
- bpp: `bits_per_pixel(x16) = avail*16/pclk - 2` (NT/nvt_dsc_pps.c:2678), clamped to min(3*bpc*16, 32*16), then to sink max 128 (8.0 bpp, :2884);
  126/127 are bumped to 128 (:2876); dual mode additionally clamps to 256 (:2865); result must be >= 8*16 = 128 (:2929) else INVALID_BPP
  (maps to DP_IMP_ERROR_PPS_GENERIC_ERROR, dp_connectorimpl.cpp:1277-1304, which nvkms drops: nvdp-connector.cpp:598,702).
  Then watermark check `pclk*depth < 8*laneDataRate*lanes*16` (dp_watermark.cpp:459) which is the same bound.
- Results (4 lanes, FEC; python reproduction of the formulas):
  | link | avail | 697 MHz (7104x1200@72) | 2170.07 MHz (@72) | 2642.6 MHz (@90) | max pclk at 8.0 bpp |
  | HBR2 | 16.7616 Gb/s | 23.9 bpp ok (clamped 8) | 7.56 bpp FAIL | 6.19 bpp FAIL | 2095 MHz |
  | HBR3 | 25.1424 Gb/s | ok | 11.44 bpp ok (->8.0) | 9.38 bpp ok (->8.0) | 3143 MHz |
  So @72 misses HBR2 by 3.5% (2170 vs 2095 MHz); @90 can never fit HBR2 (7104x3840x90 = 2455 MHz active alone). Both fit HBR3 with margin.
- Slices (dual mode, which both modes get since pclk > head max): sink mask 0x2b = {1,2,4,8}; dual drops 1 and 6 (nvt_dsc_pps.c:2309);
  min slices = max(ceil(7104/2560)=3, ceil(pclk/340)) = 7 (@72) / 8 (@90) -> 8 (DSC_GetMinSliceCountForMode :1381-1424, throughput 340 Mpx/s/slice
  because DPCD 0x6B=0x11 -> MODE0=1 -> 340, :1306/dp_deviceimpl.cpp:2477). 2642/8 = 330 Mpx/slice <= 340 passes (:1670). Slice width 888 <= 2560.
  chunk check (:1695-1700) ok. Line buffer = min(sink 9 [DPCD 0x65=0 -> _9, dp_deviceimpl.cpp:2418], GPU 13) = 9, inside 8..13 (:992).
  maxBpp 8.0 vs min bpp 8.0 leaves zero slack but is legal.
- Why FORCE_ENABLE fails every mode: any 7104-wide mode that is NOT 2Head1OR (pclk <= head max, e.g. 7104x1200@72) is non-dual DSC, and
  `DSC_PpsCalcBase` hard-fails pic_width > 5120 for single head (nvt_dsc_pps.c:1013, RANGE_CHECK -> NVT_STATUS_ERR). Also single head
  limits slices to GPU maxHztSlices=4 (:2298) and 4*340 = 1360 Mpx/s. The big modes fail FORCE_ENABLE only through the HBR2 budget (or
  if they were NOT dual). That is consistent with all your observations.

## 3. 2Head1OR on Ada
- `nvEvoUse2Heads1OR` (nvkms-evo.c:9838-9877) -> TRUE when pclk > head `maxPClkKHz` (HW cap, nvkms-evo3.c:5183-5198), sink DSC possible, merge supported
  (C6 HAL supportsMergeMode=TRUE), viewport out == visible, and the even/odd raster parity (yours pass per nvkms-mode-validation.md).
  2170/2642 MHz are far above any Ada head clock, so b2Heads1Or=TRUE; nvkms then forces DSC_FORCE_ENABLE + DSC_DUAL (nvdp-connector.cpp:498-509,
  modepool.c:1724). DP lib dual rules: slices {2,4,8}, max 8 (nvt_dsc_pps.c:2298-2309), bpp <= 16, pic_width <= 8192 (:1009).
- Linux/nvidia-drm: nothing disables it; only the NVKMS override MAX_ONE_HARDWARE_HEAD (nvkms-evo.c:9846) does and nvidia-drm never passes it.
  So 2Head1OR is available; Windows uses the same mechanism. Note NO_DISPLAYPORT_BANDWIDTH_CHECK returns before DSC info is computed
  (nvkms-evo.c:7081), so a mode "validated" that way has no DSC/PPS and cannot light the panel at 2+ GHz.

## 4. Fix candidates (least to most invasive)
(a) Regkey/module param: NONE exists that raises the link rate (see regkeys above). `NVreg_RegistryDwords` is read by RM (nvidia.ko), not the DP lib.
    Only non-source lever: make assessLink train HBR3 successfully (replug / re-run assessment after the headset is awake, i.e. after
    pimax_video_on; check with `modprobe nvidia_modeset debug=1` and dmesg for "Failed to reach max link configuration"). Do not set
    nvidia-settings DisplayPort force-FEC attribute (it pins preferredLinkConfig).
(b) Source patches (rebuild open modules). Diagnostic first, because nvkms drops the reason:
```diff
--- a/src/common/displayport/src/dp_connectorimpl.cpp   (after DSC_GeneratePPSWithSliceCountMask in compoundQueryAttachSSTDsc, ~line 2121)
     if (ppsStatus != NVT_STATUS_SUCCESS)
     {
+        DP_PRINTF(DP_ERROR, "DSCDBG pps status=%d pclk=%llu w=%u dual=%d lc=%ux%llu avail=%llu pref=%d",
+                  (int)ppsStatus, (unsigned long long)modesetParams.modesetInfo.pixelClockHz,
+                  modesetParams.modesetInfo.surfaceWidth, (int)modesetInfoDSC.bDualMode, lc.lanes, (unsigned long long)lc.peakRate,
+                  (unsigned long long)availableBandwidthBitsPerSecond, (int)this->preferredLinkConfig.isValid());
         result = false;
```
    The log shows lc.peakRate (810=HBR3, 540=HBR2) and the PPS status (INVALID_BPP = bandwidth,
    ERR = pic_width>5120, MIN_SLICE_COUNT = non-dual large pclk.
    If lc is 540 because LT fell back, the lib is correct and the fix is outside the lib (HBR3 link training with the Lontium bridge).
    If lc is 810 and dual=0, parity/DSC-possible gate of nvEvoUse2Heads1OR failed instead (check head maxPClk, rasterSyncEnd etc.).
    Only if lc=540 while HBR3 trains fine on a later modeset: patch assessLink to never lower the budget below the sink max, e.g. in
    ConnectorImpl::compoundQueryAttachSST replace `LinkConfiguration lc = highestAssessedLC;` with `LinkConfiguration lc = getMaxLinkConfig();`
    (dp_connectorimpl.cpp:2297). The modeset path then trains highestAssessedLC with force (:6076-6078) and fails visibly instead of rejecting the mode.
    If RM reports bOverrideLinkBw with a DCB HBR2 limit: in dp_evoadapter.h:247 drop the `_useDfpMaxLinkRateCaps` term.
(c) Other: a custom EDID timing that fits HBR2 at 8.0 bpp: pclk <= 2095 MHz, e.g. 7104x3840@72 with htotal*vtotal <= 29.1M
    (hblank 312 -> htotal 7416, vtotal 3870 -> 2066 MHz; keep even htotal, odd sync/blank ends, even pclk kHz, dual-mode parity). 90 Hz cannot fit HBR2.
    Risk: the Lontium MIPI timing may need the stock blanking.

## Confidence
High: cap parsing, formulas, slice/width limits. Medium: root cause of HBR2 (LT fallback vs RM bOverrideLinkBw), because both live in RM/hardware state.
