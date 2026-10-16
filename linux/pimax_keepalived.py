#!/usr/bin/env python3
"""Keep the Pimax Dream Air attached: wake + keepalive on every (re)connect, forever.

Without a host sending 58 04 39 every ~213 ms the headset firmware drops off USB after ~5.4 s.
Runs as a user service (udev rule 70-pimax.rules grants hidraw access via uaccess):
    systemctl --user enable --now omarchyvr-keepalived
It must also read IF1-IF3: the kernel only polls a HID interrupt endpoint while its hidraw node is
open, and if nobody drains the 500 Hz IMU stream the firmware's tx queue fills and it detaches.
Other tools can open the headset hidraw nodes in parallel; each open fd gets its own copy of the reports.
"""
import os
import select
import signal
import sys
import time

from pimax_probe import cmd, find_interfaces

WAKE = cmd(0x58, 0x06, 0x1A, 0x05, 0x01, 0x00, 0x00)
PERIOD = 0.213
# pimax_video_on.py writes the video rate here; the keepalive then carries it (58 04 39 00 RR 00)
RATE_FILE = os.path.join(os.environ.get("XDG_RUNTIME_DIR", "/tmp"), "pimax-rate")


def keepalive():
    try:
        rate = int(open(RATE_FILE).read().strip() or 0)
    except (OSError, ValueError):
        rate = 0
    return cmd(0x58, 0x04, 0x39, 0x00, rate & 0xFF, 0x00)

running = True


def log(msg):
    print(time.strftime("%H:%M:%S"), msg, flush=True)


def session(ifs):
    """Drive one connection until the device goes away. Returns seconds it lived."""
    t0 = time.time()
    ctl = os.open(ifs[0], os.O_RDWR | os.O_NONBLOCK)
    fds = [ctl] + [os.open(ifs[n], os.O_RDONLY | os.O_NONBLOCK) for n in (1, 2, 3) if n in ifs]
    sent = 0
    try:
        os.write(ctl, WAKE)
        next_ka = time.time()
        while running:
            now = time.time()
            if now >= next_ka:
                next_ka = max(next_ka + PERIOD, now)
                try:
                    os.write(ctl, keepalive())
                    sent += 1
                except BlockingIOError:
                    pass
            r, _, _ = select.select(fds, [], [], max(0.0, next_ka - time.time()))
            for fd in r:
                try:
                    os.read(fd, 64)  # drain everything so no queue ever fills
                except BlockingIOError:
                    pass
    except OSError as e:
        log(f"headset gone ({e.strerror}) after {time.time() - t0:.1f} s, {sent} keepalives")
    finally:
        for fd in fds:
            os.close(fd)
    return time.time() - t0


def main():
    def stop(*_):
        global running
        running = False
    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)

    log("waiting for headset 34a4:0044")
    while running:
        ifs = find_interfaces()
        if not {0, 2} <= ifs.keys():
            time.sleep(0.05)
            continue
        node = ifs[0]
        try:
            log(f"headset on {node}, sending wake + keepalive")
            session(ifs)
        except PermissionError:
            log(f"no access to {node} (udev rule missing?), retrying in 2 s")
            time.sleep(2)
        except FileNotFoundError:
            pass
        time.sleep(0.1)
    log("stopped")


if __name__ == "__main__":
    sys.exit(main())
