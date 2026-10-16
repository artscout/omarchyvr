# NVKMS / nvidia-drm mode validation (open-gpu-kernel-modules tag 610.57.04)
Paths: `NDRM`=kernel-open/nvidia-drm, `NM`=src/nvidia-modeset/src, `NT`=src/common/modeset/timing,
`KAPI`=src/nvidia-modeset/kapi/src. Source tree: scratchpad `nvsrc/src`. Citations are file:line.

## 1. How the connector mode list is built
- DRM `.get_modes` = `nv_drm_connector_get_modes` (NDRM/nvidia-drm-connector.c:381-452). It loops
  `nvKms->getDisplayMode(modeIndex++)` until `ret==0`, and silently `continue`s on `!valid` (:410-414). No log, no reason.
- `getDisplayMode` = KAPI/nvkms-kapi.c:2540 `GetDisplayMode` -> ioctl `NVKMS_IOCTL_VALIDATE_MODE_INDEX` (:2565).
  `pInfoString` is never set (struct zero-init :2548), so every rejection text is discarded.
  Params are `InitNvKmsModeValidationParams` (:2519-2538): `overrides = NVKMS_MODE_VALIDATION_NO_RRX1K_CHECK` ONLY.
  No pclk/EDID/DP-bandwidth override is ever passed by nvidia-drm. dscMode=DEFAULT.
- override_edid: NDRM/nvidia-drm-connector.c:149-163 (`connector->override_edid` or `drm_edid_override_connector_update`)
  copies the blob into `pDetectParams->edid` + `overrideEdid=TRUE` (<= buffer size) -> `getDynamicDisplayInfo`
  -> NM/nvkms-dpy.c:195 `GetEdidOverride`. Then `nvDpyReadAndParseEdidEvo` (nvkms-dpy.c:224-290) pushes it to RM
  (`WriteEdidToResman`, :250-268) and reads it back (`ReadEdidFromResman`, :270).
- Mode source order: NM/nvkms-modepool.c:101-142 `nvValidateModeIndex` = `ValidateModeIndexEdid` (:341, every
  `parsedEdid.info.timing[i]`), then `ValidateModeIndexVesa` (:58, only reached for non-EDID/no-EDID dpys).
  ZERO modes therefore means: (a) `parsedEdid.valid` false / no timings, or (b) every timing hit `!valid`.
  `timing.etc.status==0` entries are skipped before validation (:368). `PatchAndParseEdid` zeroes status for
  timings without pixel-rep bit0 (nvkms-dpy.c:2326-2342).
- EDID acceptance gate BEFORE any mode: `ValidateEdid` (nvkms-dpy.c:1373-1487) -> `NvTiming_EDIDValidationMask`.
  Any failure other than ext-DTD / range-limit / (opt) checksum makes the whole EDID rejected (:1479-1487;
  :276 `goto fail`). DisplayID ext checks (NT/nvt_edid.c:2475-2560): `ext_count==0`, section length byte `==0x79`,
  block checksum, version 0x12 (0x13 flagged), every Type-I block must parse (`parseDisplayIdBlock`), non-zero
  trailing bytes after last block -> ERR. Live EDID in /proc/driver/nvidia-modeset/dpys passes these.
- `ValidateMode` (nvkms-modepool.c:1619-1790) order, each failure -> mode `valid=FALSE`:
  1. `ValidateModeTimings` (:1078-1485): interlaced (:1105-1109); non-EDID source rejected (:1113-1168, see 5);
     h/v total ordering (:1173); EDID max pclk (:1226-1285) - for EDID-sourced modes it only WARNS ("EDID contradicts
     itself", :1263) and continues; `maxRasterWidth/Height` (:1289-1312); HorizSync (:1317) / VertRefresh (:1373)
     ranges - EDID-sourced + EDID ranges = warn only; dscPassThrough vs force-disable (:1473-1478).
  2. `GetDpyOutputColor` (:1678). 3. `nvConstructHwModeTimingsEvo` (:1709). 4. `nvEvoUse2Heads1OR` (:1724).
  5. DP: `nvDPValidateModeEvo` (:1747 -> NM/nvkms-evo.c:7063): link BW/DSC via DP lib; downgrades color fmt/bpc; skipped
     only by NO_DISPLAYPORT_BANDWIDTH_CHECK (evo.c:7081). HDMI: FRL config (:1727-1744).
  6. `nvValidateHwModeTimingsViewPort` (:1760), 7. IMP/RM check `nvConstructHwModeTimingsImpCheckEvo` (:1773 ->
     nvkms-evo.c:5974-6050; "GPU extended capability check failed"; RM `nvValidateImpOneDispDowngrade`, bypass flag
     NO_EXTENDED_GPU_CAPABILITIES_CHECK :6024).
- TMDS-only: NO_MAX_PCLK_CHECK is consulted only for TMDS/HDMI (nvkms-evo.c:6822,6829; nvkms-hdmi.c:2129), NOT DP.
  DP has no fixed pclk gate other than head cap / DSC / link / IMP.

## 2. nvtiming and DisplayID-only timings
- `NvTiming_ParseEDIDInfo` (NT/nvt_edid.c:866): ext tag 0x70 -> if `(pExt[1]&0xF0)==0x20` DisplayID 2.x path
  (`getDisplayId20EDIDExtInfo`), else DisplayID 1.x path `getDisplayIdEDIDExtInfo` (nvt_edid.c:1100-1155;
  NT/nvt_edidext_displayid.c:76). Return value deliberately ignored (:1119 comment). Timings added before an error stay.
- `parseDisplayIdTiming1` (nvt_edidext_displayid.c:467) / descriptor (:499-590): 20-byte Type I, pclk 24-bit
  `(hi<<16|mid<<8|lo)+1` x10 kHz (:505-506), 16-bit h/v fields, `status=NVT_STATUS_DISPLAYID_1`. Only rejects if
  `data_bytes%20!=0` or `HTotal < HVisible+HFP+HSyncWidth` (:530). No v2-only/primary-use restriction. Preferred bit only
  sets a flag (:568). Section `product_type > NVT_DISPLAYID_PROD_MAX_NUMBER` aborts (:86) - ext product type 0 is OK.
- Limits are derived from the timings: `NvTiming_CalculateEDIDLimits` (nvt_edid.c:2995-3085): max_pclk_10khz = max over
  timings; a Display Range Limits LDD only raises it (:3075-3079). So a missing range descriptor cannot cause rejection.
- No overflow issue for 7104x3840: HTotal 7752 fits NvU16, pclk 2.17/2.64 GHz fits NvU32 (kapi pixelClockHz NvU32,
  kapi.h:92; drm clock kHz int). DTD (base/CTA) caps at 655.35 MHz, so >2 GHz MUST come from DisplayID Type I (or T7/CTA-VFD).
- Live check (read-only): /proc/driver/nvidia-modeset/dpys prints the EDID NVKMS actually holds (NM/nvkms.c:6234-6308);
  /proc/driver/nvidia-modeset/heads shows active mode. Connector naming: NVKMS `DP-0` == DRM `DP-1`.
  Your current live EDID shows Type I 7104x3840 (0x034fae => 2169.75 MHz, hTotal 7752, vTotal 3888) parsed fine.
## 3. Ada per-head limits, DSC, 2Head1OR
- Ada = display class C7 using the C6 HAL (NM/nvkms-hal.c:153-154, `nvEvoC6`, nvkms-evo3.c:7820); `supportsMergeMode=TRUE`
  (nvkms-evo3.c:7896). Raster size not a limit: C67D fields 14:0 (32767) (clc67d.h:827), nvkms-hal.c:92-93.
- Max pclk per head is read from HW: `NVC373_HEAD_CLK_CAP_PCLK_MAX * 10 MHz` (nvkms-evo3.c:5183-5198; reset default 0x85
  = 1330 MHz, real value is fused/queried at runtime, not printed anywhere).
- `nvEvoUse2Heads1OR` (nvkms-evo.c:9838-9877): returns TRUE (merge two heads into one OR) only if pclk > head max, AND
  merge supported, NOT `MAX_ONE_HARDWARE_HEAD`, AND (sink DSC possible OR HW YUV420), viewport out == visible width,
  and parity: rasterSize.x even, rasterSyncEnd/BlankEnd/BlankStart x odd, pclk(kHz) even, viewPort.in.width even.
  Your two timings satisfy parity (FP400/SW48/BP200 and FP200/SW48/BP200, pclk kHz even).
  If pclk > head max and 2Head1OR is refused, the single-head IMP check fails (that is the "no DSC" failure mode).
- 2Head1OR forces DSC on DP (NM/dp/nvdp-connector.cpp:498-509). DSC possible = sink DPCD DSC (+FEC) caps parsed in
  common/displayport/src/dp_deviceimpl.cpp:2642-2745 (`bDSCPossible`), via `nvDPDpyIsDscPossible` (nvdp-device.cpp:153).
  So the HMD's DPCD must advertise DSC; DSC slice/throughput/line-buffer errors surface as `DP_IMP_ERROR_PPS_*`
  (dp_connectorimpl.cpp:1277-1304) but nvkms drops the code (nvdp-connector.cpp:598,702).
## 4. Logging the reason / knobs
- nvidia_modeset `debug=1` (nvidia-modeset-linux.c:124-126, perm 0600; `nvkms_debug_logging` :225) is nearly useless:
  nvkms builds with `-UDEBUG` (nvidia-modeset.Kbuild:56) so `nvEvoLogDebug/DevDebug` are no-ops (NM/include/nvkms-utils.h:252),
  including the kapi "Failed to get validated mode" line. It only enables `nvDoDebugLogging()` users: DP-lib `dpPrintf`
  (NM/dp/nvdp-host.cpp:66) and HDMI (nvkms-hdmi.c:1877). DP-lib prints on hotplug/link-train/DPCD events, not on
  `echo detect`; replug (or DP re-attach) is needed to see them. Per-mode rejection text is NEVER printed by the DRM path.
- `config_file` (nvidia-modeset-linux.c:128-136 charp, perm 0400 = boot/modprobe only; read once at module load
  :1838-1860, built unless Android Kbuild:58-67) parses key=value via `nvKmsReadConf` (NM/nvkms-conf.c:323). The ONLY
  subparser is `override` (nvkms-conf.c:103, `SUBPARSE(override)` at :278; any other key =>
  "Parser not found"). Syntax: `override.[domain:bus:slot.fn].<dpyname>=<value>` or `override.tegra...`/`rm-soc-disp`;
  value = `/abs/path/edid.bin` | hex string | `disconnected`; `#` comments. Example:
  `override.[0000:01:00.0].DP-0=/etc/pimax.edid`. It is just another EDID override. There are NO ModeValidation keys
  (NoMaxPClkCheck / NoEdidMaxPClkCheck / NoDisplayPortBandwidthCheck / AllowNonEdidModes are X-driver option names that
  exist here only as API bits `NVKMS_MODE_VALIDATION_*`, interface/nvkms-api.h:341-359). grep finds no parser for them.
  nvidia_modeset other params (`modinfo`): output_rounding_fix, disable_hdmi_frl, force_frl_rate, hdmi_deepcolor,
  conceal_vrr_caps ... none affect DP validation. nvidia-drm has no mode-validation param either.
- Way to get the exact reason with NO reboot/reload: a tiny userspace NVKMS client. `/dev/nvidia-modeset` is 0666 here.
  Sequence: open; `NVKMS_IOCTL_ALLOC_DEVICE`(deviceId from nvidia gpu id; shares pDevEvo with nvidia-drm, no ownership
  grab) -> `QUERY_DISP` -> `QUERY_DPY_STATIC/DYNAMIC` (do NOT set overrideEdid; it would mutate shared dpy state)
  -> `NVKMS_IOCTL_VALIDATE_MODE_INDEX` (nvkms.c:1987, ENTRY_CUSTOM_USER :5098, no ownership needed) with
  `request.pInfoString/infoStringSize` (nvkms-api.h:1606-1706). Reply text = LogModeValidationBegin/End (modepool.c:908-940):
  "PixelClock ... too high", "DP Bandwidth check failed", "GPU extended capability check failed", "ViewPort ... exceeds
  hardware capabilities", etc. The same request carries `modeValidation.overrides` and `dscMode`, so you can A/B each
  override (NO_EXTENDED_GPU_CAPABILITIES_CHECK, NO_DISPLAYPORT_BANDWIDTH_CHECK, dscMode=FORCE_ENABLE) to find the
  blocking check. Header: src/nvidia-modeset/interface/nvkms-api.h (+ nvkms-ioctl.h). Validation only, no state change.
## 5. Custom modes in atomic commits
- Yes you can submit any `drm_mode_modeinfo` blob, but it is validated, not trusted:
  1. DRM core atomic check calls the connector helper `.mode_valid` (kernel drm_atomic_helper_check_modeset ->
     mode_valid_path) = `nv_drm_connector_mode_valid` (NDRM/nvidia-drm-connector.c:454-479) -> `validateDisplayMode`
     (KAPI/nvkms-kapi.c:2631-2672, same fixed overrides) -> `NVKMS_IOCTL_VALIDATE_MODE` -> `nvValidateModeEvo`
     (modepool.c:146-185).
  2. `nv_drm_atomic_check` (NDRM/nvidia-drm-modeset.c:531, final call :583) runs
     `nv_drm_atomic_apply_modeset_config(commit=false)` -> `applyModeSetConfig` test-only (:499) -> NVKMS SetMode
     test with the same ValidateMode pipeline. Failure => -EINVAL (:499-505).
  3. `ConstructModeTimingsMetaData` (modepool.c:1984-2021) calls `nvFindEdidNVT_TIMING` (modepool.c:1859,
     matches all porch/sync/pclk, ignores RRx1k and size_mm). Match => source=Edid (EDID-contradiction checks relax to
     warnings). No match => source=Unknown.
  4. Source!=Edid and no ALLOW_NON_EDID_MODES (never passed by kapi) => reject "Only EDID-provided modes are allowed"
     (modepool.c:1113-1168) whenever `parsedEdid.valid && isDigital`; continuous-freq EDID bit is irrelevant for digital.
     => A custom mode works only if its timing exactly equals a timing present in the EDID NVKMS holds, OR
     the dpy has no valid parsed EDID at all. Then steps 5-7 of ValidateMode (DP BW/DSC/IMP) still apply.
- Consequence: a custom mode cannot dodge pclk/DSC/IMP; it only avoids EDID listing. The practical lever is the
  EDID (debugfs edid_override, no reboot). Live observation: with a base-block DTD EDID NVKMS accepted
  1920x1080@72, 3840x2160@72 and 7104x1200@72 (/sys/class/drm/card1-DP-1/modes) - same DP/IMP path, so the
  7104x3840@72 rejection is specific to pclk 2170 MHz (head cap / 2Head1OR / DSC / IMP), not to EDID parsing.

## Summary of what needs a reboot
- No-reboot: debugfs EDID override (already used); userspace NVKMS validate client with infoString (section 4);
  DRM atomic custom mode only if timing equals an EDID timing.
- Needs module (re)load/boot: `config_file=` (0400), `debug=` is runtime (0600, root) but near-useless, no param exists
  to relax DP/pclk/IMP checks. Relaxing checks needs a patched nvidia-modeset (e.g. add overrides in
  `InitNvKmsModeValidationParams`, KAPI/nvkms-kapi.c:2537) = rebuild + reload.
