# Headset EDID as seen on Linux (2026-10-13)

Raw: `dp-edid-linux.bin` (384 bytes, from /sys/class/drm/card1-DP-1/edid). NVIDIA 610.57.04 open, RTX 4090.
DP-1 is "connected" but nvidia-drm exposes **zero modes** (Hyprland: `0x0@60`, availableModes empty). No nvidia-modeset log line.

- Base block: PVR (0xD242) model 4121, no usable timings (two dummy descriptors).
  **Range limits: 55-65 Hz V, 50-70 kHz H, max dotclock 1000 MHz.**
- CTA block: audio only, no video timings.
- DisplayID 1.2 block, the only real modes:
  - 7104x3840 @ 72.0002 Hz, pixel clock 2170.07 MHz, H 280 kHz (+/+)
  - 7104x3840 @ 90.0001 Hz, pixel clock 2642.60 MHz, H 350 kHz (-/-)

Both modes break the base-block range limits (refresh, h-freq and dotclock). Main suspect for the
rejection: nvkms honours those limits. Windows (Pimax Play, NVAPI direct mode) runs 7104x3840 @ 90.

Bandwidth: 2170 MHz x 24 bpp = 52 Gbps > DP 1.4 HBR3 x4 (25.92 Gbps), so these modes **need DSC**
(about 8-10 bpp). The earlier note "no DSC" in the capture analysis can't be right for these timings.

Next: get nvkms to log mode validation (`nvidia-modeset debug=1`, needs module reload/reboot),
then override the EDID (fixed range limits) or relax validation.
