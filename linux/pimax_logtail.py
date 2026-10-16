#!/usr/bin/env python3
"""Passive tail of the headset firmware log (IF3) and the display-bridge MCU log (34A4:0043).

Sends nothing, so it never resets the bridge state machine (pimax_video_on.py does).
    python3 pimax_logtail.py [seconds] [grep-regex]
"""
import os
import re
import select
import sys
import time

from pimax_probe import find_interfaces
from pimax_video_on import bridge_log_node

SECONDS = float(sys.argv[1]) if len(sys.argv) > 1 else 30.0
PATTERN = re.compile(sys.argv[2]) if len(sys.argv) > 2 else None


def main():
    srcs = {}
    hmd = find_interfaces().get(3)
    if hmd:
        srcs[os.open(hmd, os.O_RDONLY | os.O_NONBLOCK)] = "hmd"
    bridge = bridge_log_node()
    if bridge:
        srcs[os.open(bridge, os.O_RDONLY | os.O_NONBLOCK)] = "bridge"
    bufs = {}
    t0 = time.time()
    while time.time() - t0 < SECONDS:
        r, _, _ = select.select(list(srcs), [], [], 0.2)
        for fd in r:
            try:
                data = os.read(fd, 64)
            except BlockingIOError:
                continue
            except OSError:
                return
            name = srcs[fd]
            bufs[name] = bufs.get(name, b"") + data.rstrip(b"\x00")
            while b"\n" in bufs[name]:
                line, bufs[name] = bufs[name].split(b"\n", 1)
                text = line.decode("ascii", "replace").strip()
                if text and (not PATTERN or PATTERN.search(text)):
                    print(f"{time.time()-t0:6.2f}  {name:6} {text}", flush=True)


if __name__ == "__main__":
    main()
