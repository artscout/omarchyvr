# Headset DPCD read attempt (read-only), 2026-10-13

Tool: linux/nvkms_why/dpcd_read.c (`make dpcd_read`; `./dpcd_read` = NVKMS stage, `./dpcd_read --rm-only` = RM stage).
Driver 610.57.04 open modules; source tag 610.57.04 re-downloaded to the session scratchpad (old copy was gone).

## Result: raw DPCD NOT read (needs root); NVKMS gives only link-rate/bpc/etc.

### Stage 1: NVKMS (works as user, read-only)
NVKMS has no ioctl exposing DPCD, sink DSC/FEC caps or lane count (DynamicData has only name, maxPclk,
EDID, DP GUID (invalid here)). `NV_KMS_DISP_ATTRIBUTE_QUERY_DP_AUX_LOG` is only a log of past AUX traffic
(printed to the kernel log, sets dpAuxLoggingEnabled) - not used. GET_DPY_ATTRIBUTE for the headset
(dpy 0x200, "Pimax Tech Co., Ltd Pimax (DP-0)", maxPclk 2670000 kHz, vrHMD=1) at the moment of the run (headset currently lit, head 1, hw head 2):

    DISPLAYPORT_LINK_RATE            = 20   (x270 MHz = HBR2, 5.4 Gbps/lane)
    DISPLAYPORT_LINK_RATE_10MHZ      = 540  (HBR2)
    DISPLAYPORT_FORCE_ENABLE_FEC     = 0
    DISPLAYPORT_CONNECTOR_TYPE       = 1 (DisplayPort)
    DISPLAYPORT_IS_MULTISTREAM       = 0
    DISPLAYPORT_SINK_IS_AUDIO_CAPABLE= 1
    DIGITAL_SIGNAL = 2 (DisplayPort), DIGITAL_LINK_TYPE = 3
    CURRENT_COLOR_BPC = 8, CURRENT_COLOR_FORMAT = 0 (RGB), CURRENT_DITHERING depth = 2 (8bpc)
    NUMBER_OF_HARDWARE_HEADS_USED = 1
    (BACKLIGHT/FRAMELOCK_DISPLAY_CONFIG/UPDATE_GLS_FRAMELOCK/DPMS: EPERM)

There is no DSC-active attribute. Note the active link is HBR2: if that is also the max the sink
advertises/GPU trained to, 4 lanes x 17.28 Gbps effective cannot carry 7104x3840@72 uncompressed
(needs ~ 2170 MHz x 24 = 52 Gbps), consistent with the "DP Bandwidth check failed".

### Stage 2: RM (/dev/nvidiactl) - works up to the privileged call
/dev/nvidiactl, /dev/nvidia0, /dev/nvidia-modeset are all 0666. Sequence that works as uid 1000:
open /dev/nvidia0 (REQUIRED in-process: nv_is_gpu_accessible() walks current->files, otherwise
NV01_DEVICE_0 gives 0x1b), ROOT -> GET_PROBED_IDS (gpuId 0x100) -> ATTACH_IDS -> DEVICE_0 -> SUBDEVICE_0
-> NV04_DISPLAY_COMMON -> SYSTEM_GET_SUPPORTED (displayMask 0x3f00) -> SYSTEM_GET_CONNECT_STATE
(cached; connected = 0x200, i.e. headset = displayId 0x200).

Then every DP-level control returns status 0x1b (NV_ERR_INSUFFICIENT_PERMISSIONS):
NV0073_CTRL_CMD_DP_AUXCH_CTRL (flags 0x844 incl. RMCTRL_FLAGS_PRIVILEGED),
NV0073_CTRL_CMD_DP_GET_CAPS (0x820046, also privileged), and SPECIFIC_GET_EDID_V2 returned 0 bytes.
So raw DPCD through RM requires a privileged RM client = root (CAP_SYS_ADMIN). Nothing was written anywhere.

### What is needed
Run as root (NOT done - not authorized): `sudo ./linux/nvkms_why/dpcd_read --rm-only`.
It only issues AUXCH_CTRL with cmd = AUX|REQ_TYPE_READ (no writes, no I2C, no modeset, no ownership) for
0x0000-0x000F, 0x0020, 0x0060-0x006F (DSC), 0x0090 (FEC), 0x2200-0x220F, 0x0400-0x040F, 0x0500-0x050F,
on displayId 0x200, plus GPU-side DP_GET_CAPS (sor 0-3, shows the 4090 DSC encoder caps).
Caveat: this AUX read is concurrent with NVKMS's DP library traffic; RM serialises AUX transactions, risk is low
but non-zero while the headset link is up. Decoding of the returned bytes is still TODO (tool prints hex only).

### DSC advertised?
Unknown from this run (no DPCD bytes obtained). Indirect evidence only: Windows runs 7104x3840@90 so sink DSC
is very likely; Linux nvkms rejects DSC FORCE_ENABLE for all modes, which points at the sink-cap / DSC
decision in the DP library (not at the GPU: 4090 supports DSC) - exactly what DPCD 0x060/0x2200 will confirm.

## Live DPCD read (2026-10-13, after fixing AUX size to 0-based; `sudo dpcd_read --rm-only`)
```
00000: 12 14 c4 81 01 00 01 01 02 0f 04 00 00 00 81 00   rev 1.2, max rate 0x14 HBR2, 4 lanes, 0x00E bit7 = extended caps present
02200: 14 1e c4 81 01 00 01 01 02 0f 04 00 00 00 81 00   rev 1.4, max rate 0x1e HBR3
00060: 01 21 03 7f 2b 00 00 80 00 01 06 11 08 00 00 00   DSC supported, v1.2, slices 1/2/4/8, max bpp 8.0, RGB, 8/10 bpc, max slice width 2560
00090: bf                                                FEC capable
00400/00500: all zero (no sink/branch OUI)
```
GPU encoder (DP_GET_CAPS): DSC supported, FEC, maxHztSlices 4. Live link trains at HBR2.
Theory: NVIDIA Linux uses base-cap HBR2; 2170 MHz x 8 bpp = 17.4 Gbps > HBR2 x4 ~17.3 => "DP Bandwidth check failed". At HBR3 both 72 and 90 Hz fit.
