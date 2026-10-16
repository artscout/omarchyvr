# Dream Air display on Linux (NVIDIA, Hyprland) — what works (2026-10-13)

Result: DP-1 runs **7104x3840 @ 72 Hz with DSC**; both panels show the desktop (left half -> left eye,
right half -> right eye). Each eye image appears rotated 90 deg (panels are mounted rotated) — handled
later in the Monado driver, not in Hyprland.

## Why it did not work out of the box
- Stock EDID: only 7104x3840 @72/@90 in a DisplayID 1.2 block, colour depth "undefined" (NVIDIA then sends 18 bpp).
- These modes need DSC (2170 / 2643 MHz). The sink advertises DSC 1.2 + FEC (DPCD 0x060/0x090), max 8.0 bpp.
- Base DPCD says HBR2, extended caps (0x2200) say HBR3. At HBR2 the 72 Hz mode misses by ~3.5 %
  ("DP Bandwidth check failed"); the boot where it worked, the link was assessed at HBR3.
  See analysis/nvidia-dsc-linkrate.md, analysis/headset-dpcd.md.

## Setup (needs sudo)
1. Keepalive: `systemctl --user enable --now pimax-keepalived` (linux/pimax-keepalived.service, udev rule linux/70-pimax.rules).
2. EDID override at boot (8 bpc + native modes + 3840x2160/1080p fallbacks):
   ```
   sudo install -D -m644 linux/pimax-dp-edid-native.bin /etc/pimax/dp-edid.bin
   echo 'override.[0000:01:00.0].DP-0=/etc/pimax/dp-edid.bin' | sudo tee /etc/pimax/nvidia-modeset.conf
   echo 'options nvidia-modeset config_file=/etc/pimax/nvidia-modeset.conf' | sudo tee /etc/modprobe.d/nvidia-pimax.conf
   echo 'FILES+=(/etc/pimax/nvidia-modeset.conf /etc/pimax/dp-edid.bin)' | sudo tee /etc/mkinitcpio.conf.d/pimax.conf
   sudo mkinitcpio -P && reboot
   ```
   (nvidia modules load from the initramfs, so the EDID must be inside it.) Fallback EDID without native modes:
   linux/pimax-dp-edid.bin.
3. Hyprland (~/.config/hypr/monitors.lua), headset kept away from the main screen:
   `hl.monitor({ output = "DP-1", mode = "7104x3840@72", position = "8000x8000", scale = 2 })`
4. The headset firmware still needs the video-prepare commands (wake is sent by the keepalive service;
   linux/pimax_video_on.py sends ept unlock, panel-on request, required fps 72, GPU cfg).

## Do NOT
- Force connector hotplugs (`echo off > /sys/class/drm/card1-DP-1/status`): it desynced Hyprland's CRTC
  bookkeeping and every DP-1 commit failed with EINVAL until a restart.

## Tools
- linux/nvkms_why/: read-only NVKMS client. `nvkms_why --dpy Pimax --timing 72,7104,400,48,200,3840,29,1,18 --no-edid-max-pclk`
  validates a custom timing and prints the rejection reason. `sudo dpcd_read --rm-only` dumps DPCD (read-only AUX).

## Non-desktop (for Monado direct mode / Hyprland DRM lease) — 2026-10-13
Use `linux/pimax-dp-edid-msvsdb.bin`: the native EDID plus a Microsoft HMD vendor block (OUI CA-12-5C,
payload 21 bytes, version 2) in the CTA section. The kernel (drm_edid.c drm_parse_microsoft_vsdb) then sets
`non_desktop = 1`, while NVIDIA keeps reading the DisplayID 1.2 timings. Live test: non-desktop value 1 AND
7104x3840 @72/@90 modes listed.
Dead ends: a DisplayID 2.0 block with primary use "HMD VR" also sets non-desktop, but NVIDIA then drops the
7104x3840 modes (it only reads the first DisplayID block and ignores 2.0 Type VII timings here).
Install: copy to /etc/pimax/dp-edid.bin, `mkinitcpio -P`, remove the DP-1 line from monitors.lua (Hyprland only
offers non-desktop outputs for lease when no monitor rule disables them), reboot.
Verified after reboot (2026-10-13): /etc/pimax/dp-edid.bin = pimax-dp-edid-msvsdb.bin, non-desktop = 1, modes
7104x3840 @72/@90 + 3840x2160 + 1920x1080, Hyprland (aquamarine) logs "drm: Non-desktop connector" and keeps
DP-1 off the desktop; monitors.lua has no DP-1 rule. Desktop unaffected.

## Both eyes showed the left half in Vulkan direct mode -> NVIDIA driver bug, FIXED by patch (2026-10-13)
Symptom: Monado direct mode (Vulkan VK_EXT_acquire_drm_display) showed the LEFT half on both panels; plain atomic
KMS through the same lease was correct (analysis/tools/kms_lease_test.c). Cause: in 2Heads1OR (needed for
7104x3840) nvSetViewPortsEvo() sets viewport origin x=0 for every merged head, so head 2 rescans the left half
(NVIDIA/open-gpu-kernel-modules PR #1262, not in 610.57.04). Fix: linux/nvidia-2h1or-viewport.patch.
Apply (nvidia-open-dkms):
  cd /usr/src/nvidia-610.57.04 && sudo cp -a . /usr/src/nvidia-610.57.04.orig && sudo patch -p1 < ~/pimax-re/linux/nvidia-2h1or-viewport.patch
  sudo dkms build nvidia/610.57.04 -k $(uname -r) --force && sudo dkms install nvidia/610.57.04 -k $(uname -r) --force
  sudo mkinitcpio -P && reboot
Verified after reboot with analysis/tools/vk_display_test: left eye red, right eye green.
RE-APPLY after every nvidia-open-dkms update until NVIDIA merges the fix.

### Automatic re-apply after nvidia-open-dkms upgrades (pacman hook, 2026-10-14; NOT installed yet, needs sudo)
Every nvidia-open-dkms upgrade replaces /usr/src/nvidia-<ver> with the pristine source, and dkms then builds the
module from it: the left half would show in both eyes again, and flips would hang the display engine, after the next
`pacman -Syu` that updates the driver. `linux/nvidia-patch-reapply.sh` re-applies, in order, `nvidia-2h1or-viewport.patch`,
`nvidia-2h1or-flip-fix.patch` and `nvidia-evo-makeroom-bounded-wait.patch` idempotently (`patch -R --dry-run` decides what
is already in; all patches are tried on a scratch copy first, so a conflict never leaves a half-patched tree; it is
reported loudly, never forced; originals are kept as <file>.pimax-orig). `linux/nvidia-pimax-patch.hook` runs it from
pacman after the package is installed and BEFORE dkms builds; `linux/nvidia-pimax-verify.hook` runs after the dkms build
and checks `modinfo nvidia-modeset | grep merged_head_sync`
(hooks run sorted by file name: 65-nvidia-pimax-patch < 70-dkms-install < 75-nvidia-pimax-verify < 90-mkinitcpio-install).
Install (copies to a root-owned directory because pacman runs the hook as root; one command, see also the header of
nvidia-pimax-patch.hook):
  sudo env L="$HOME/pimax-re/linux" bash -c 'install -Dm755 $L/nvidia-patch-reapply.sh /usr/local/lib/pimax/nvidia-patch-reapply.sh && for p in nvidia-2h1or-viewport nvidia-2h1or-flip-fix nvidia-evo-makeroom-bounded-wait; do install -Dm644 $L/$p.patch /usr/local/lib/pimax/$p.patch; done && install -Dm644 $L/nvidia-pimax-patch.hook /etc/pacman.d/hooks/65-nvidia-pimax-patch.hook && install -Dm644 $L/nvidia-pimax-verify.hook /etc/pacman.d/hooks/75-nvidia-pimax-verify.hook'
Check without changing anything (also works as a normal user): `linux/nvidia-patch-reapply.sh --check`
(exit 0 = patched, 2 = needs a patch). Verify the built module: `linux/nvidia-patch-reapply.sh --verify`.
Manual one-shot incl. rebuild: `sudo linux/nvidia-patch-reapply.sh --rebuild`
(dkms build + install for the running kernel, then `limine-mkinitcpio` (this Omarchy/Limine host boots a UKI that
contains the nvidia modules and has no mkinitcpio presets) or `mkinitcpio -P` where limine-mkinitcpio does not exist;
then the merged_head_sync check; then reboot). The pacman path needs neither: `90-mkinitcpio-install.hook` rebuilds the
boot image after the dkms hook. Test of the script on scratch copies (no root): `linux/test_nvidia_patch_reapply.sh`.
If the hook cannot apply the patches it prints a banner and exits 1; pacman still continues with the dkms build, which
then makes an UNPATCHED module: look for the banner in the pacman output / `journalctl -t pimax-nvidia-patch`
(/var/log/pacman.log also records that the hook failed).
Remove it when NVIDIA merges the fixes into the driver you run:
  sudo rm /etc/pacman.d/hooks/65-nvidia-pimax-patch.hook /etc/pacman.d/hooks/75-nvidia-pimax-verify.hook && sudo rm -r /usr/local/lib/pimax

## One-click VR: `pimax vr` (2026-10-14)
Needs the new `pimax` binary (the installed one predates the `vr` subcommand; the keepalive service runs the
installed file, so replace it atomically and leave the service alone):
  cd ~/pimax-re/rust && cargo build --release -p pimax-tools
  install -m755 target/release/pimax ~/.local/bin/pimax.new && mv -f ~/.local/bin/pimax.new ~/.local/bin/pimax
Commands (details: `pimax vr help`):
  pimax vr start [--fps 72|90] [--dump DIR [--dump-frames 100,300]] [--wait] [--debug] [--no-notify]
      preflight (headset 34a4:0044 on USB, pimax-keepalived active, DP-1 connected, no Xid/"Idling" in this boot's
      kernel log, no monado-service running), then monado-service detached (own session, log
      /tmp/pimax-vr-session/monado.log, pid file monado.pid), returns after "gyro bias done" (keep the headset still
      ~3 s). Env as analysis/tools/run_vr_session.sh plus PIMAX_FPS (video prepare + keepalive rate + frame interval)
      and XRT_NO_STDIN=1. 72 Hz = display mode index 2, 90 Hz = index 3 (`--mode N` overrides).
  pimax vr stop [--grace S] [--all] SIGINT to the service this tool started (pid file), SIGKILL after 5 s, 15 s while a client is
                                    still connected (with a warning), then the kernel log check (exit 2 = reboot); --all also stops
                                    monado-service processes of yours that were not started by `pimax vr start`. One stop at a
                                    time (Monado's SIGINT handler is single-shot: a second SIGINT would kill it half way)
  pimax vr status [--notify]        service, rate, log summary (frame interval, missed frames, clients), headset/display/kernel
  pimax vr steam-options            prints the Steam launch options line (pipe it into wl-copy)
  pimax vr dump-stats FILE.raw...   mean/dominant colours, black %, probe pixels of start --dump images, --ppm writes a preview
  pimax vr dump-diff A.raw B.raw    pixel diff of two dumps, separating edge noise from flat-area differences
  pimax vr start --gamma G --gain K --target-format NAME   colour experiments (need analysis/overnight/srgb-target.patch)
  Under `flock LOCK pimax vr start` the lock is released when start returns (descriptors are not inherited by the
  service); use `--wait` to keep it for the whole session.
Desktop entry (template `linux/pimax-vr.desktop`, not installed; actions Start 90 Hz / Stop / Status):
  sed "s|@PIMAX@|$HOME/.local/bin/pimax|g" ~/pimax-re/linux/pimax-vr.desktop > ~/.local/share/applications/pimax-vr.desktop
  desktop-file-validate ~/.local/share/applications/pimax-vr.desktop && update-desktop-database ~/.local/share/applications
Test scripts (headset!, run under the lock when other agents are active): analysis/tools/run_vr_fps_test.sh (72/90 Hz
with dumps and firmware log) and analysis/tools/run_vr_color_test.sh (sRGB vs UNORM client).

## Desk mode: `pimax desk` (2026-10-14)
`pimax desk start` = Monado (as `pimax vr start`, honours `PIMAX_MONADO_BUILD`), then `pimax-passthrough run` (room camera view
as the MAIN OpenXR app), then WayVR (`--openxr --show --no-autostart`, `XDG_CONFIG_HOME=~/src/desk-out/wayvr-home-am/config`,
CPU `capture_method: screencopy`) on top. WayVR creates an `XR_EXTX_overlay` session (`create_overlay_session` in
wayvr/src/backend/openxr/helpers.rs; it refuses to start without the extension), so the passthrough app stays the main session.
Flags: `--no-passthrough`, `--eye` (PIMAX_EYE=1 + `~/src/monado-eye/build`). Logs in the session dir (`monado.log`,
`passthrough.log`, `wayvr.log`, `wayvr.stdout`). `pimax desk stop`: SIGINT WayVR, then passthrough (by pid, 10 s each), then Monado
(SIGINT, SIGKILL only after `--grace`, default 30 s), then the Xid / "Idling display engine" kernel log check.
Not yet tried on the headset: passthrough + WayVR overlay together (each was run separately, WayVR over xr_room in
analysis/overnight/desk.md). The camera must be free: no PIMAX_CAM / PIMAX_HANDS=mercury in the environment.
`pimax vr start` also finds the headset connector by EDID (PVR 0xD242 / 0x1019) instead of `DP-1`, and warns when
monado-service lacks `cap_sys_nice` (prints the setcap line). `pimax mount-shared` prints the mount command for the shared NTFS drive.
