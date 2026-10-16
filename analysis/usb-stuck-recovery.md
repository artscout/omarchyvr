# Dream Air stuck USB enumeration: recovery without rebooting

> **Correction 2026-10-15:** a dropped USB3 side (camera 34a4:0014 gone, HID 34a4:0044 still there) does NOT need a physical junction-box power unplug. `omarchyvr vr reset-headset` (exit mode) brought the camera back by itself ~90 s after the reset started (usb 2-4 disconnect 09:21:43; reset 09:24:~; camera back 09:26:31). `omarchyvr desk start` / `vr start --6dof` now run this automatically (`--no-auto-recover` skips it). The old tool gave up after 60 s; it now waits 150 s (`--wait`).


Scope: research only. Nothing was reset, unbound, power-cycled or written. Commands below are for the next incident. Do not run them while the headset is working.

## 1. Symptom summary

- Failure window in kernel log of previous boot (`journalctl -k -b -1`), starting 17:55 on Oct 13. The current boot (`-b 0`) has no matches.
- Signature:
  - `usb 1-5.2-port4: Cannot enable. Maybe the USB cable is bad?`
  - `usb 1-5.2-port4: cannot disable (err = -71)`
  - `usb 2-5: device not accepting address N, error -71` (about 15 distinct addresses)
  - `usb usb2-port5: attempt power cycle` (41 times) and `usb2-port4: attempt power cycle` (27 times)
- The failing hub was `1-5.2` at that time. Today the link box is `1-5.1` and the headset is `1-5.1.4.1`. The hub path changes with each replug, which is why the commands below use placeholders.
- Host state: xHCI `0000:00:14.0`, Intel, USB 3.2 host, `quirks 0x...9810`. Hubs have `power/control = auto`, and the headset's internal hub has `auto` too.
- Confirmed in previous boot: no cable-level evidence. Port-enable failure plus `-71` (EPROTO) on repeated address-setup attempts points at the host/hub state machine, not the cable alone.
- `uhubctl` is not installed. Not installed here, and per instructions not installed now.

## 2. Ranked recovery procedure (least to most invasive)

Set variables first. These find the failing hub and port from the kernel log, not from memory:

```bash
LINE=$(journalctl -k | grep -oE 'usb [0-9.-]+-port[0-9]+: Cannot enable' | tail -1)
HUB=$(echo "$LINE" | grep -oE '[0-9.-]+-port' | sed 's/-port$//')   # e.g. 1-5.2
PORT=$(echo "$LINE" | grep -oE 'port[0-9]+' | tr -dc 0-9)          # e.g. 4
echo "HUB=$HUB PORT=$PORT"
# Headset path (VID 34a4 PID 0044), found by sysfs scan, not assumed:
HEADSET=$(for d in /sys/bus/usb/devices/*; do [ "$(cat $d/idVendor 2>/dev/null)" = 34a4 ] && [ "$(cat $d/idProduct 2>/dev/null)" = 0044 ] && basename $d; done)
echo "HEADSET=$HEADSET"
```

If `$LINE` is empty, the failure is not in the current boot. Use `journalctl -k -b -1` instead.

**Step 0. Evidence first (read-only).** Do this before any write. See section 3.

**Step 1. Disable autosuspend on the hub chain (preventive, not a fix).** Harmless and reversible. Does not clear a stuck state.
```bash
sudo sh -c "echo on > /sys/bus/usb/devices/$HUB/power/control"
sudo sh -c "echo on > /sys/bus/usb/devices/$HUB.4/power/control"  # internal hub, if present
```
Effect: keeps hubs awake. Risk: negligible, more power draw.

**Step 2. Deauthorize and reauthorize the headset device only.** Forces a fresh enumeration of the headset. Does not touch the hub.
```bash
sudo sh -c "echo 0 > /sys/bus/usb/devices/$HEADSET/authorized; sleep 3; echo 1 > /sys/bus/usb/devices/$HEADSET/authorized"
```
Effect: headset re-enumerates. Risk: low, same as an unplug of the headset. Other devices unaffected.

**Step 3. Deauthorize and reauthorize the internal hub (`$HUB.4`).** Drops headset, dongle and Tobii together.
```bash
sudo sh -c "echo 0 > /sys/bus/usb/devices/$HUB.4/authorized; sleep 3; echo 1 > /sys/bus/usb/devices/$HUB.4/authorized"
```
Effect: re-enumerates the headset hub. Risk: low. Only the headset subtree is affected.

**Step 4. Deauthorize and reauthorize the link-box hub (`$HUB`).** Drops everything under the link box, including the headset.
```bash
sudo sh -c "echo 0 > /sys/bus/usb/devices/$HUB/authorized; sleep 3; echo 1 > /sys/bus/usb/devices/$HUB/authorized"
```
Effect: the whole Pimax chain re-enumerates. Risk: medium-low. Does not affect bus 2 (USB3 cameras).

**Step 5. USBDEVFS_RESET on the link-box hub.** Same subtree as step 4 but a driver-level reset (`usb_reset_device`). Use the python ioctl so nothing needs installing. USBDEVFS_RESET = `_IO('U', 20)` = `0x5514`.
```bash
BUS=$(cat /sys/bus/usb/devices/$HUB/busnum); DEV=$(cat /sys/bus/usb/devices/$HUB/devnum)
DEVNODE=$(printf '/dev/bus/usb/%03d/%03d' "$BUS" "$DEV")
sudo python3 -I -c 'import fcntl,os,sys; fd=os.open(sys.argv[1], os.O_WRONLY); fcntl.ioctl(fd, 0x5514, 0)' "$DEVNODE"
```
Effect: hub and children reset in place. Risk: medium. Same subtree as step 4.

**Step 6. Unbind and rebind the hub driver on the link-box interface.** Only the hub's interface, not the root hub.
```bash
sudo sh -c "echo $HUB:1.0 > /sys/bus/usb/drivers/hub/unbind; sleep 3; echo $HUB:1.0 > /sys/bus/usb/drivers/hub/bind"
```
Effect: hub driver reattaches and re-probes its ports. Risk: medium. Unbinding the root-port hub (`1-5:1.0`) would drop the whole chain. Never run that one.

**Step 7. uhubctl per-port power cycle (only if the hub supports it).** Support for Terminus FE1.1s and the Pimax internal hub is unknown; see section 4.
```bash
# Install uhubctl with your package manager, then read-only listing (no -a):
uhubctl
# Only if the link box shows a per-port power switch for port $PORT:
sudo uhubctl -l $HUB -p $PORT -a cycle
```
Effect: cuts VBUS to port `$PORT` on the link box, which powers the internal hub. Risk: medium. If the hub is ganged, `-a cycle` may drop more than one port. Power-cycling the headset's own hub port (`$HUB.4`) is untested and uncertain.

**Step 8. Unbind and rebind the xHCI controller (`0000:00:14.0`).** Last resort before PCI remove. **Warning:** drops every USB device on this controller, including keyboard, mouse, Bluetooth (`1-14`), the USB3 camera bus (bus 2), and the Pimax cameras. Run it detached with systemd-run so the rebind still happens if your input device goes away.
```bash
sudo systemd-run --unit=xhci-rebind sh -c 'echo 0000:00:14.0 > /sys/bus/pci/drivers/xhci_hcd/unbind; sleep 3; echo 0000:00:14.0 > /sys/bus/pci/drivers/xhci_hcd/bind'
```
Effect: controller state reset; enumeration starts clean. Risk: high. If the bind fails, recover with `echo 0000:00:14.0 | sudo tee /sys/bus/pci/drivers/xhci_hcd/bind`, then reboot if still dead.

**Step 9. PCI remove and rescan.** Same blast radius as step 8, plus a fragile rescan. Use only if step 8 fails.
```bash
sudo systemd-run --unit=xhci-pci-rescan sh -c 'echo 1 > /sys/bus/pci/devices/0000:00:14.0/remove; sleep 3; echo 1 > /sys/bus/pci/rescan'
```
Effect: controller removed and rediscovered. Risk: high. If rescan fails, the controller is gone until reboot.

**Step 10. usbcore parameters (for next boot, not a live fix).** These are read at boot if `usbcore` is built in. Check first: `ls -l /sys/module/usbcore/parameters/` and `cat /proc/cmdline`.
- `usbcore.old_scheme_first=1`: try the older init scheme first.
- `usbcore.initial_descriptor_timeout=10000`: give slow firmware more than the 5000 ms default for the first descriptor.
- `usbcore.autosuspend=-1`: disable autosuspend globally. Does not help enumeration failures, only suspend-related ones.
- `usbcore.quirks=34a4:0044:<flags>`: per-device quirks. Flag letters not verified in this session; check `Documentation/admin-guide/kernel-parameters.txt` in your kernel tree.
Effect: changes init behavior after reboot only. Risk: low, but requires reboot.

## 3. Evidence to collect next time (before any write)

```bash
# 1. Kernel log since the first symptom (adjust --since)
journalctl -k --since "-2h" | grep -E 'Cannot|cannot disable|not accepting|power cycle|xhci|usb ' > ~/omarchyvr/analysis/usb-incident-$(date +%H%M).log
# 2. Topology and power state right now
lsusb -t
for d in /sys/bus/usb/devices/[0-9]*; do printf '%s %s %s %s\n' "$(basename $d)" "$(cat $d/idVendor 2>/dev/null):$(cat $d/idProduct 2>/dev/null)" "$(cat $d/power/control 2>/dev/null)" "$(cat $d/power/runtime_status 2>/dev/null)"; done
# 3. Module state and boot params
cat /proc/cmdline; grep -H . /sys/module/usbcore/parameters/* 2>/dev/null
# 4. xHCI debugfs (if mounted): port and slot state
sudo mount -t debugfs none /sys/kernel/debug 2>/dev/null; ls /sys/kernel/debug/usb/xhci/ 2>/dev/null
# 5. Optional, heavier: usbmon capture (read-only)
sudo modprobe usbmon && sudo cat /sys/kernel/debug/usb/usbmon/0u | head -n 500 > ~/omarchyvr/analysis/usbmon-$(date +%H%M).txt
```
Also note the time of the physical unplug/replug and the hub path from the log at that moment.

## 4. What is known and unknown

- **Known:** `Cannot enable` with `-71` (EPROTO) on repeated address setup is reported on xHCI after repeated enumeration. Linux bug reports 64311 and the LKML threads below show the same family of errors.
- **Known:** `usbcore.old_scheme_first`, `use_both_schemes`, `initial_descriptor_timeout` exist and defaults are `N`, `Y`, `5000` (current values on this machine).
- **Known:** `usbreset`-style `USBDEVFS_RESET` is the standard driver-level reset.
- **Unknown:** Whether the Terminus FE1.1s (`1a40:0101`) link box or the headset's internal hub supports per-port power switching. No source found. Test with the read-only `uhubctl` listing.
- **Unknown:** Whether a PCI unbind/rebind of `0000:00:14.0` clears device firmware state. Xhci reset clears host state; firmware stuck state is not clearly covered by any source found.
- **Not established:** Terminus-specific link to `Cannot enable`. Searches found the message in many unrelated devices.
- **Verified in this session:** the failure signature is in the previous boot's kernel log. Current topology and power state were read-only checked.

## 5. Sources

- Kernel USB power management (autosuspend, `power/control`): https://docs.kernel.org/driver-api/usb/power-management.html
- usbcore `hub.c` with `old_scheme_first`, `use_both_schemes` definitions (v6.19 mirror): https://tangled.org/tjh.dev/kernel/blob/v6.19/drivers/usb/core/hub.c
- Commit adding `initial_descriptor_timeout`: https://googlers.googlesource.com/maze/linux/+/fd7c519dd40a0d561280bb797386143fb2026949%5E%21
- LKML, xHCI setup context error, Nov 2014: https://lkml.iu.edu/hypermail/linux/kernel/1411.3/04946.html
- LKML, xHCI follow-up, Dec 2014: https://lkml.iu.edu/hypermail/linux/kernel/1412.0/02820.html
- Kernel bugzilla 64311, "device not accepting address 35, error -71": https://bugzilla.kernel.org/show_bug.cgi?id=64311
- Kernel bugzilla 6266, "Cannot enable port. Maybe the USB cable is bad?": https://bugzilla.kernel.org/show_bug.cgi?id=6266
- Arch forum, device not accepting address with -71: https://bbs.archlinux.org/viewtopic.php?id=313864
- Manjaro forum, same signature: https://forum.manjaro.org/t/device-not-accepting-address-xhci-hcd-device-descriptor-read-error/176013
- Billauer, USB device stuck reset: https://billauer.co.il/01signal/other/usb-device-stuck-reset/
- uhubctl project (hub support, per-port switching caveats): https://github.com/mvp/uhubctl/
- uhubctl issue 14 (controller grouping limits): https://github.com/mvp/uhubctl/issues/14
- Arch forum, autosuspend and whitelist caveats: https://bbs.archlinux.org/viewtopic.php?id=202775

Note: sysfs `authorized` behavior is from kernel ABI knowledge, not from a fetched source in this session. The kernel docs fetch did not cover it.
