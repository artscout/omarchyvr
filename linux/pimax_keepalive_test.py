#!/usr/bin/env python3
"""Does wake + keepalive stop the ~5.4 s USB reconnect cycle?

For each (re)connect: open IF0 at once, send wake (0x1a), then keepalive (0x39) every 213 ms
from the main loop, read IMU (IF2) + fw log (IF3), and report how long the connection lived.
    sudo python3 pimax_keepalive_test.py [seconds] [variant]
variant: "plain" = 58 04 39 00 00 00 (default), "rate" = 58 04 39 00 48 00, "none" = no keepalive
"""
import glob
import os
import select
import sys
import time

from pimax_probe import cmd, find_interfaces

SECONDS = float(sys.argv[1]) if len(sys.argv) > 1 else 40.0
VARIANT = sys.argv[2] if len(sys.argv) > 2 else "plain"
KEEPALIVE = {"plain": (0x58, 0x04, 0x39, 0x00, 0x00, 0x00),
             "rate": (0x58, 0x04, 0x39, 0x00, 0x48, 0x00),
             "none": None}[VARIANT]
READ_ALL = "all" in sys.argv[3:]
TAIL = 60 if "full" in sys.argv[3:] else 6  # also drain IF1 (controller endpoint)


def session(deadline):
    ifs = {}
    while time.time() < deadline and not {0, 2, 3} <= ifs.keys():
        ifs = find_interfaces()
        time.sleep(0.02)
    if not ifs:
        return None
    t_open = time.time()
    try:
        ctl = os.open(ifs[0], os.O_RDWR | os.O_NONBLOCK)
        imu = os.open(ifs[2], os.O_RDONLY | os.O_NONBLOCK)
        log = os.open(ifs[3], os.O_RDONLY | os.O_NONBLOCK)
        extra = [os.open(ifs[1], os.O_RDONLY | os.O_NONBLOCK)] if (1 in ifs and READ_ALL) else []
    except OSError as e:
        print(f"  open failed: {e}")
        time.sleep(0.2)
        return 0
    writes = errs = imu_n = 0
    tail = []
    try:
        if KEEPALIVE:
            os.write(ctl, cmd(0x58, 0x06, 0x1A, 0x05, 0x01, 0x00, 0x00))
    except OSError as e:
        errs += 1
    next_ka = time.time()
    buf = b""
    while time.time() < deadline:
        if KEEPALIVE and time.time() >= next_ka:
            next_ka += 0.213
            try:
                os.write(ctl, cmd(*KEEPALIVE))
                writes += 1
            except BlockingIOError:
                errs += 1
            except OSError:
                break
        r, _, _ = select.select([ctl, imu, log] + extra, [], [], 0.05)
        try:
            for fd in r:
                data = os.read(fd, 64)
                if fd == imu:
                    imu_n += 1
                elif fd == log:
                    buf += data.rstrip(b"\x00")
                    while b"\n" in buf:
                        line, buf = buf.split(b"\n", 1)
                        tail = (tail + [line.decode("ascii", "replace").strip()])[-TAIL:]
        except BlockingIOError:
            pass
        except OSError:
            break
    life = time.time() - t_open
    print(f"connection lived {life:5.2f} s | keepalive writes {writes} errors {errs} | imu reports {imu_n}")
    for line in tail:
        print("    fwlog", line)
    for fd in [ctl, imu, log] + extra:
        os.close(fd)
    return life


def main():
    deadline = time.time() + SECONDS
    print(f"variant={VARIANT}")
    while time.time() < deadline:
        if session(deadline) is None:
            break
        time.sleep(0.1)


if __name__ == "__main__":
    main()
