#!/usr/bin/env python3
"""Offline replay of captured EP 0x82 controller reports through pimax_controllers.decode().

    python3 linux/test_controllers_replay.py

Data:
  captures2/20261013-130931-controller-buttons-hub2.pcap  (left controller = slot 2, marked timeline in captures2/notes.txt;
                                                           right controller = slot 3 idle) -- needs tshark
  analysis/p82.pkl  (round 1, right controller = slot 3: grip squeeze ~73 s, trigger ~75.5 s, touch bits)
Needs no USB device. Exit code 1 on failure.
"""
import os
import pickle
import re
import subprocess
import sys
from datetime import datetime

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)
from pimax_controllers import decode, parse_0x37  # noqa: E402

PCAP = os.path.join(ROOT, "captures2/20261013-130931-controller-buttons-hub2.pcap")
NOTES = os.path.join(ROOT, "captures2/notes.txt")
P82 = os.path.join(ROOT, "analysis/p82.pkl")
LEAD = 6.5   # the user typed markers up to ~5 s after the action (trigger) and ~0.5 s before it never
fails = []


def check(ok, msg):
    print(("ok   " if ok else "FAIL ") + msg)
    if not ok:
        fails.append(msg)


def load_pcap():
    out = subprocess.run(
        ["tshark", "-r", PCAP, "-Y", "usb.endpoint_address==0x82 && usb.data_len>0", "-T", "fields",
         "-e", "frame.time_epoch", "-e", "usb.capdata", "-e", "usbhid.data"],
        capture_output=True, text=True).stdout
    rows = []
    for line in out.splitlines():
        p = line.split("\t")
        h = p[1] if len(p) > 1 and p[1] else (p[2] if len(p) > 2 else "")
        if h:
            b = bytes.fromhex(h.replace(":", ""))
            if b[:3] == b"\x53\x32\x08":
                rows.append((float(p[0]), b))
    return rows


def load_markers(t_first_epoch):
    """notes.txt -> {name: seconds since first pcap packet}.  Notes use +08:00 local wall clock == pcap clock."""
    marks = {}
    for line in open(NOTES):
        m = re.match(r"(\S+) MARK controller-buttons: (.*)", line)
        if m:
            ep = datetime.fromisoformat(m.group(1)[:26] + m.group(1)[-6:]).timestamp()
            marks[m.group(2).strip()] = ep - t_first_epoch
    return marks


def first_active(frames, pred):
    return next((t for t, d in frames if pred(d)), None)


def test_left(rows):
    t0 = rows[0][0]
    left = [(t - t0, decode(b)) for t, b in rows if b[3] == 2]
    marks = load_markers(t0)
    check(len(marks) == 10, f"10 markers parsed from notes.txt ({len(marks)})")
    mk = {"trigger": marks["l trigger"], "grip": marks["l grip"], "A": marks["l a"], "B": marks["l b"],
          "stick": marks["l sick click"], "menu": marks["l menu"], "up": marks["l stick up"],
          "down": marks["l stick down"], "right": marks["l stck right"], "left": marks["l sticvk left"]}
    preds = {
        "trigger": lambda f: f["trig"] > 200,
        "grip": lambda f: f["grip"] > 100,
        "A": lambda f: "A" in f["buttons"],
        "B": lambda f: "B" in f["buttons"],
        "stick": lambda f: "stick" in f["buttons"],
        "menu": lambda f: "menu" in f["buttons"],
        "up": lambda f: f["sy"] > 100,
        "down": lambda f: f["sy"] < -100,
        "right": lambda f: f["sx"] > 100,
        "left": lambda f: f["sx"] < -100,
    }
    first = {}
    for name, pred in preds.items():
        t = first_active(left, pred)
        first[name] = t
        # grip: marker was typed ~1.5 s BEFORE the byte-44 squeeze (45.96 s vs marker 44.46 s; a stray trigger pull at
        # 43.5 s sits right before the marker), so give it +2 s of slack; every other control lags the marker.
        slack = 2.0 if name == "grip" else 0.5
        ok = t is not None and mk[name] - LEAD <= t <= mk[name] + slack
        check(ok, f"left {name}: first active at {t and round(t, 2)} s, marker {mk[name]:.2f} s (window -{LEAD}/+{slack})")
    order = sorted(first, key=lambda k: first[k] or 1e9)
    check(order == list(mk), f"left activation order matches marker order: {order}")
    # exclusivity: during each single-button window no other digital button is reported in the 0.2 s around the edge
    for name in ("A", "B", "menu"):
        t = first[name]
        near = {n for tt, f in left if t - 0.2 <= tt <= t + 0.1 for n in f["buttons"]}
        check(near == {name}, f"left {name} edge is exclusive: {sorted(near)}")
    # touch bits: A touched while A pressed, B touched while B pressed, stick touched while stick clicked/deflected
    def touch_at(t, tag, span=0.05):
        return any(tag in f["touch"] for tt, f in left if t - span <= tt <= t + span)
    check(touch_at(first["A"] + 0.05, "touch-A"), "left touch-A during A press")
    check(touch_at(first["B"] + 0.05, "touch-B"), "left touch-B during B press")
    check(touch_at(first["stick"] + 0.5, "touch-stick", 0.6), "left touch-stick during stick click")
    check(touch_at(first["up"] + 1.0, "touch-stick", 1.0), "left touch-stick during stick up")
    check(not any("touch-t4" in f["touch"] for _, f in left), "left bit4 of byte 49 constant (no touch-t4 deviation)")
    # analog sanity
    check(max(f["trig"] for _, f in left) == 255 and max(f["grip"] for _, f in left) == 255, "left trigger/grip reach 255")
    check(min(f["sx"] for _, f in left) <= -127 and max(f["sy"] for _, f in left) == 127, "left stick spans full range")
    check(all(abs(f["sx"]) < 4 and abs(f["sy"]) < 4 for tt, f in left if tt < 20), "left stick centred at start (idle)")
    mags = [sum(a * a for a in f["imu"][:3]) ** 0.5 / 1024 for tt, f in left[:200]]
    check(0.9 < sum(mags) / len(mags) < 1.1, f"left accel magnitude ~1 g at rest ({sum(mags)/len(mags):.3f})")
    ts = [f["ts"] for _, f in left]
    check(all(0 < b - a < 20000 for a, b in zip(ts, ts[1:])), "left device timestamps monotonic, deltas < 20 ms")
    check({f["batt"] for _, f in left} == {72}, "left battery 72%")


def test_right_idle(rows):
    right = [decode(b) for t, b in rows if b[3] == 3]
    check(len(right) > 40000, f"right controller streams in buttons capture ({len(right)})")
    check(all(not f["btn"] and f["trig"] == 0 and f["grip"] == 0 and f["sx"] == 0 and f["sy"] == 0 for f in right),
          "right idle in buttons capture: no buttons/touch/analog (byte 49 rests at 0 on slot 3, 0x10 on slot 2)")
    check({f["batt"] for f in right} == {99}, "right battery 99%")


def test_right_round1():
    P = pickle.load(open(P82, "rb"))
    right = [(t, decode(b)) for t, b in P if b[3] == 3]
    grip = [f["grip"] for t, f in right if 72.8 <= t <= 73.4]
    check(max(grip) > 150 and grip[0] < 20, f"right grip bell curve 72.9-73.3 s (max {max(grip)})")
    trig = [f["trig"] for t, f in right if 75.4 <= t <= 75.8]
    check(max(trig) == 255 and trig[0] < 30, f"right trigger 0->255->0 at ~75.5 s (max {max(trig)})")
    sy = [f["sy"] for t, f in right if 75.5 <= t <= 75.7]
    check(max(sy) >= 10, f"right stick Y blip 137/142 at 75.6 s (max {max(sy)})")
    check(all(not f["buttons"] for t, f in right),
          "right byte 47/48 never set in round 1 (A/B/stick/menu on slot 3 are UNVERIFIED, assumed = slot 2 layout)")
    touches = {n for t, f in right for n in f["touch"]}
    check({"touch-A", "touch-B", "touch-stick", "touch-t4"} <= touches,
          f"right controller shows touch bits {sorted(touches)} (physical meaning on the right unverified)")
    check(all("touch-t4" not in f["touch"] for t, f in right if t < 70), "right bit4 rests at 0")


def test_0x37():
    # two announces in one packet, from captures2 cold-start (left+right, battery 0x49 / 0x63)
    d = bytes.fromhex("4c0f370248e8b69102490002010008" "2d" "c70f37034846a791026300020100082e38") + bytes(31)
    check(parse_0x37(d) == [(2, 0x49), (3, 0x63)], f"parse_0x37 two messages per packet: {parse_0x37(d)}")
    d = bytes.fromhex("4c0f37034804889102630002010008" "2e") + bytes(47)
    check(parse_0x37(d) == [(3, 0x63)], "parse_0x37 single message")
    check(parse_0x37(bytes.fromhex("4c04390001") + bytes(59)) == [], "parse_0x37 ignores other replies")


def main():
    test_0x37()
    if subprocess.run(["which", "tshark"], capture_output=True).returncode == 0 and os.path.exists(PCAP):
        rows = load_pcap()
        check(len(rows) > 80000, f"loaded {len(rows)} EP 0x82 reports from pcap")
        test_left(rows)
        test_right_idle(rows)
    else:
        print("SKIP pcap tests (tshark or pcap missing)")
    if os.path.exists(P82):
        test_right_round1()
    print(f"\n{len(fails)} failure(s)")
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()
