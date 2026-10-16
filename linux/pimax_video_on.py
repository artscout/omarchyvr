#!/usr/bin/env python3
"""Prepare the headset for video (init-sequence.md section 7 steps 5-8) and tail both firmware logs.

    python3 pimax_video_on.py [fps=72] [seconds=40]
Needs pimax-keepalived running (it sends wake + keepalive). After this, scan out the DP connector
at exactly <fps> Hz; the bridge log should show `fps calc <fps>` and after ~5.5 s `Panel ON`.
Writes <fps> to $XDG_RUNTIME_DIR/pimax-rate so pimax-keepalived switches to `58 04 39 00 <fps> 00`.
"""
import glob
import os
import select
import sys
import time

from pimax_probe import cmd, find_interfaces

FPS = int(sys.argv[1]) if len(sys.argv) > 1 else 72
SECONDS = float(sys.argv[2]) if len(sys.argv) > 2 else 40.0

STEPS = [
    ("ept app_unlock", (0x58, 0x05, 0x5B, 0x05, 0x02, 0x02)),
    ("panel-on request", (0x58, 0x04, 0x29, 0x00, 0x01)),
    (f"announce {FPS} Hz", (0x58, 0x0D, 0x1A, 0x05, 0x02, FPS)),
    ("GPU cfg nvidia", (0x58, 0x0D, 0x1A, 0x05, 0x05, 0x00)),
]


def bridge_log_node():
    """hidraw node of the display bridge MCU 34A4:0043 (EP 0x81 = ASCII log)."""
    for node in glob.glob("/sys/class/hidraw/hidraw*"):
        try:
            if "HID_ID=0003:000034A4:00000043" in open(node + "/device/uevent").read():
                return "/dev/" + os.path.basename(node)
        except OSError:
            pass
    return None


def main():
    ifs = find_interfaces()
    if 0 not in ifs:
        sys.exit("headset not found")
    ctl = os.open(ifs[0], os.O_RDWR | os.O_NONBLOCK)
    srcs = {ctl: "reply"}
    if 3 in ifs:
        srcs[os.open(ifs[3], os.O_RDONLY | os.O_NONBLOCK)] = "hmd"
    bridge = bridge_log_node()
    if bridge:
        srcs[os.open(bridge, os.O_RDONLY | os.O_NONBLOCK)] = "bridge"
    print("headset", ifs, "bridge", bridge)

    try:
        with open(os.path.join(os.environ.get("XDG_RUNTIME_DIR", "/tmp"), "pimax-rate"), "w") as f:
            f.write(str(FPS))
    except OSError as e:
        print("could not write rate file:", e)

    for name, payload in STEPS:
        os.write(ctl, cmd(*payload))
        print(f">> {name}: {bytes(payload).hex(' ')}")
        time.sleep(0.15)

    bufs = {}
    t0 = time.time()
    while time.time() - t0 < SECONDS:
        r, _, _ = select.select(list(srcs), [], [], 0.2)
        for fd in r:
            try:
                data = os.read(fd, 64)
            except BlockingIOError:
                continue
            except OSError as e:
                print("!! device gone:", e)
                return
            name = srcs[fd]
            if name == "reply":
                if data[2:3] != b"\x39":  # skip keepalive noise
                    print(f"{time.time()-t0:6.2f}  reply  {data[:12].hex(' ')}")
                continue
            bufs[name] = bufs.get(name, b"") + data.rstrip(b"\x00")
            while b"\n" in bufs[name]:
                line, bufs[name] = bufs[name].split(b"\n", 1)
                print(f"{time.time()-t0:6.2f}  {name:6} {line.decode('ascii', 'replace').strip()}")


if __name__ == "__main__":
    main()
