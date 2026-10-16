#!/usr/bin/env python3
"""First Linux test for the Pimax Dream Air: wake + keepalive, then read IMU, replies and firmware log.

Pure Python, hidraw only. Run as root (or with a udev rule for 34A4):
    sudo python3 pimax_probe.py [seconds]
Only sends the safe commands from analysis/init-sequence.md section 7 (steps 1-3).
"""
import glob
import os
import select
import struct
import sys
import threading
import time

VID, PID = 0x34A4, 0x0044
SECONDS = float(sys.argv[1]) if len(sys.argv) > 1 else 15.0


def find_interfaces():
    """Map USB interface number -> /dev/hidrawN for the headset."""
    found = {}
    for node in glob.glob("/sys/class/hidraw/hidraw*"):
        dev = os.path.realpath(os.path.join(node, "device"))
        try:
            uevent = open(os.path.join(dev, "uevent")).read()
        except OSError:
            continue
        if f"HID_ID=0003:{VID:08X}:{PID:08X}" not in uevent:
            continue
        intf = os.path.dirname(dev)  # .../1-1.2:1.0
        try:
            num = int(open(os.path.join(intf, "bInterfaceNumber")).read(), 16)
        except OSError:
            continue
        found[num] = "/dev/" + os.path.basename(node)
    return found


def cmd(*payload):
    """Build a hidraw write: report id 0 (unnumbered) + 64-byte report."""
    body = bytes(payload)
    return b"\x00" + body + bytes(64 - len(body))


def usb_devnum():
    for d in glob.glob("/sys/bus/usb/devices/*"):
        try:
            if open(d + "/idVendor").read().strip() == "34a4" and open(d + "/idProduct").read().strip() == "0044":
                return open(d + "/devnum").read().strip()
        except OSError:
            pass
    return "?"


def main():
    ifs = find_interfaces()
    print("headset hidraw interfaces:", ifs, "| usb devnum:", usb_devnum())
    if 0 not in ifs or 2 not in ifs:
        sys.exit("headset IF0/IF2 not found - is 34a4:0044 passed through?")

    ctl = os.open(ifs[0], os.O_RDWR | os.O_NONBLOCK)
    readers = {name: os.open(ifs[n], os.O_RDONLY | os.O_NONBLOCK)
               for name, n in (("imu", 2), ("log", 3), ("ctrl", 1)) if n in ifs}
    readers["reply"] = ctl

    stop = threading.Event()

    ka = {"ok": 0, "err": 0, "last": ""}

    def keepalive():
        while not stop.is_set():
            try:
                os.write(ctl, cmd(0x58, 0x04, 0x39, 0x00, 0x00, 0x00))
                ka["ok"] += 1
            except OSError as e:
                ka["err"] += 1
                ka["last"] = repr(e)
            stop.wait(0.213)

    # step 1: PC-alive / wake, step 2: keepalive loop, step 3: identity reads
    os.write(ctl, cmd(0x58, 0x06, 0x1A, 0x05, 0x01, 0x00, 0x00))
    threading.Thread(target=keepalive, daemon=True).start()
    for c in ((0x58, 0x04, 0x16, 0x00, 0x1E), (0x58, 0x03, 0x03, 0x00), (0x58, 0x03, 0x50, 0x00)):
        time.sleep(0.05)
        os.write(ctl, cmd(*c))

    imu_n, first_ts, last_ts, last = 0, None, None, None
    ctrl_n, log_buf = 0, b""
    t0 = time.time()
    next_print = t0 + 1
    by_fd = {fd: name for name, fd in readers.items()}
    while time.time() - t0 < SECONDS:
        r, _, _ = select.select(list(by_fd), [], [], 0.2)
        for fd in r:
            try:
                data = os.read(fd, 64)
            except BlockingIOError:
                continue
            except OSError as e:
                print(f"!! read error {e} after {time.time()-t0:.2f} s | keepalive ok={ka['ok']} err={ka['err']} {ka['last']}")
                stop.set()
                return
            name = by_fd[fd]
            if name == "imu" and data[:2] == b"\x53\x31":
                imu_n += 1
                ts = struct.unpack_from("<I", data, 4)[0]
                first_ts = first_ts if first_ts is not None else ts
                last_ts, last = ts, data
            elif name == "reply":
                print(f"  reply  {data[:12].hex(' ')}")
            elif name == "log":
                log_buf += data.rstrip(b"\x00")
                while b"\n" in log_buf:
                    line, log_buf = log_buf.split(b"\n", 1)
                    print("  fwlog ", line.decode("ascii", "replace").strip())
            elif name == "ctrl":
                ctrl_n += 1
        if time.time() >= next_print and last:
            next_print += 1
            ax, ay, az, gx, gy, gz = struct.unpack_from(">6h", last, 14)
            g = (ax * ax + ay * ay + az * az) ** 0.5 / 2048
            ipd = struct.unpack_from("<H", last, 45)[0] / 100
            rate = imu_n / max(time.time() - t0, 1e-3)
            print(f"IMU {rate:6.1f} rep/s | |a|={g:5.3f} g a=({ax:6d},{ay:6d},{az:6d}) "
                  f"gyro dps=({gx/16.4:7.2f},{gy/16.4:7.2f},{gz/16.4:7.2f}) | ipd {ipd:5.2f} mm | ctrl reports {ctrl_n} | keepalive ok={ka['ok']} err={ka['err']}")

    stop.set()
    span = (last_ts - first_ts) / 1e6 if first_ts is not None and last_ts else 0
    print(f"done: {imu_n} IMU reports, device clock span {span:.2f} s, usb devnum now {usb_devnum()}")


if __name__ == "__main__":
    main()
