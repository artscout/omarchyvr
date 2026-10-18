# Pimax Crystal Super on Linux (experimental)

Status 2026-10-18, one headset (Crystal Super Micro-OLED, Lighthouse faceplate), RTX 5090, Ubuntu 26.04 (KDE Wayland),
NVIDIA 595.99.02 open modules. Findings and captures: [../analysis/crystal-super-capture-20261017.md](../analysis/crystal-super-capture-20261017.md).

The Crystal Super speaks the Dream Air's USB protocol (`58 ..` frames on IF0, the same IMU and controller reports,
the same cameras `34a4:0014`, the same Tobii eye tracker), so the omarchyvr stack drives it: keepalive, video on, 3DoF,
the Pimax controllers (optical tracking works), the lens (a LUT from the headset's own Pimax distortion dump).
Inside-out head tracking (SLAM) is not stable yet. With the Lighthouse faceplate the light setup below works today.

## The light setup: SteamVR Lighthouse + CustomHeadsetOpenVR + omarchyvr

| Part | Who |
|---|---|
| Head and Index controller tracking | SteamVR's own lighthouse driver |
| The headset display in SteamVR (EDID vendor, render size, distortion, IPD) | [CustomHeadsetOpenVR](https://github.com/sboys3/CustomHeadsetOpenVR) (sboys3) with `linux/crystal-super/chovr-linux-direct-usb.patch` |
| Wake, keepalive, panel/video on | `omarchyvr keepalived --video 90` (user service) |
| 7104x3840 at 90 Hz over DSC | the NVIDIA patch `patches/nvidia/blackwell/` and the EDID override |
| Sound | USB audio of the headset (WirePlumber rule) |

Monado is not used in this mode. Start SteamVR from Steam as usual.

### Install (one time)

1. Access to the headset: the udev rule `linux/70-pimax.rules` (its first three lines are enough without the hub
   power helper), in `/etc/udev/rules.d/`.
2. The keepalive service: `~/.local/bin/omarchyvr` (from `rust/`), `linux/omarchyvr-keepalived.service` into
   `~/.config/systemd/user/`, plus the drop-in `linux/crystal-super/omarchyvr-keepalived-crystal-lighthouse.conf` as
   `~/.config/systemd/user/omarchyvr-keepalived.service.d/crystal-lighthouse.conf`; `systemctl --user enable --now
   omarchyvr-keepalived`.
3. The NVIDIA modules (`patches/nvidia/blackwell/README.md`): build the open modules of your driver version with the
   patch, create a MOK key and enroll it (`mokutil --import`, use a digits-only password: MokManager runs with a US
   keyboard layout), sign the five modules with `scripts/sign-file` of the kernel headers.
4. The EDID: `linux/crystal-super/make_crystal_edid.py /sys/class/drm/card*-DP-N/edid crystal-edid.bin` with the
   headset awake (the DRM connector whose EDID is `PVR`).
5. `sudo linux/crystal-super/install-nvidia.sh WORKDIR` (WORKDIR = `modules/`, `mok/MOK.der`, `edid/crystal-edid.bin`),
   reboot. Undo: `sudo linux/crystal-super/uninstall-nvidia.sh` (from a rescue boot: give the kernel version).
6. CustomHeadsetOpenVR: build it on Linux (its README), apply the patch, register it with
   `SteamVR/bin/vrpathreg.sh adddriver <build>/CustomHeadsetOpenVR`, and enable the headset in
   `~/.config/CustomHeadset/settings.json`: `{"crystalSuperMicroOLED": {"enable": true, "distortionProfile":
   "Crystal Super micro-OLED like Pimax"}}` (or with its GUI, `CustomHeadsetGUI`, a Tauri app that builds on Linux).
   The "like Pimax" profile (fitted to the Pimax compositor's mesh, optical centre off the panel centre) matched the
   lens best in the test; the driver's default profile is "Crystal Super Micro-OLED Default".
7. Sound: `linux/crystal-super/51-pimax-crystal-super.conf` into `~/.config/wireplumber/wireplumber.conf.d/`, restart
   WirePlumber. The speakers are ALSA device 1 of the card; the stock analog profiles only look at device 0.
8. Switch the Lighthouse faceplate on (its switch). The Valve devices (`28de:2300` LHR, `28de:2101` dongles) appear
   on their own.

### Known issues

- A small screen-fixed shimmer ("water running down glass") near the centre of both eyes, seen only in SteamVR Home
  (not in other apps' menus): an effect of that scene, not of the stack. Checked and ruled out: DSC (a slice height of
  240 instead of 1920 did not change it) and CustomHeadsetOpenVR's RadialBezier profile (smooth there when probed
  offline: slope ripple below 0.1 %, no fold; SteamVR samples it on a 127 mesh).
- About 4 % dropped frames when the head moves fast; SteamVR's asynchronous reprojection is off on this setup.
- Monitors blink once when SteamVR takes or gives back the headset's output.
- Inside-out 6DoF (Basalt on the headset cameras) diverges; work in progress.

## Without the faceplate: the omarchyvr stack

`omarchyvr keepalived`, Monado (`forks/monado`, the Pimax driver knows the Crystal Super's USB id) with
`PIMAX_LENS_CONF` pointing at a `lens.conf` with `lut = <pimax-lens make-lut of the headset's PVR dump>`, the camera
broker and `pimax-ctrltrack live --factory` with `PIMAX_CAM_CALIB` = the headset's factory calibration
(`slam/device_calibration_<serial>.xml` from a Pimax Play install). 3DoF, the picture and the Pimax controllers work;
6DoF of the head does not yet.
