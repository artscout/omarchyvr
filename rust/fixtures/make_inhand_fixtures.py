#!/usr/bin/env python3
"""Cut the in-hand detector fixtures (rust/pimax-hands/src/inhand.rs tests) out of a session's controller IMU log.

Source: ~/.local/state/omarchyvr/sessions/2026-10-16-1818/ctrl-imu-left.csv (pimax-ctrlfuse log v1, `I` rows: host_ns,
dev_us, gyro x y z, accel x y z, raw LSB). Output: inhand_<name>.csv.gz, one line per sample: `dt_us,gx,gy,gz,ax,ay,az`
(dt_us = microseconds since the previous sample, the first line since the segment start).

Segments (seconds from the first sample; what the controller did, read from the gyro/accel and the session):
  desk      0-20     lying on the desk, untouched
  desk_bump 51-102   lying on the desk (another orientation), a small knock at 70.7 s
  pickup    14-30    desk, picked up at 20.5 s, waved
  held      120-180  in the hand, mostly quiet (pointing, small moves) -> must never count as put down
  putdown   283-312  in the hand, put down at 290.5 s, picked up again at 305 s
"""
import gzip, os, sys

SRC = sys.argv[1] if len(sys.argv) > 1 else os.path.expanduser(
    "~/.local/state/omarchyvr/sessions/2026-10-16-1818/ctrl-imu-left.csv")
OUT = os.path.dirname(os.path.abspath(__file__))
SEGS = {"desk": (0, 20), "desk_bump": (51, 102), "pickup": (14, 30), "held": (120, 180), "putdown": (283, 312)}

rows = []
for line in open(SRC):
    if line.startswith("I,"):
        p = line.strip().split(",")
        if len(p) >= 9:
            rows.append((int(p[1]), [int(x) for x in p[3:9]]))
t0 = rows[0][0]
for name, (a, b) in SEGS.items():
    lo, hi = t0 + int(a * 1e9), t0 + int(b * 1e9)
    last = lo
    with gzip.open(os.path.join(OUT, f"inhand_{name}.csv.gz"), "wt") as f:
        for t, v in rows:
            if lo <= t < hi:
                f.write(f"{(t - last) // 1000},{','.join(map(str, v))}\n")
                last = t
