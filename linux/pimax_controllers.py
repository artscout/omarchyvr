#!/usr/bin/env python3
"""Minimal Pimax Dream Air controller reader (hidraw only, stdlib only).

    python3 pimax_controllers.py [seconds] [--raw]

Sends only the per-slot attach burst (0x16, 0x50, 0x0a, 0x03; init-sequence.md step 13),
then prints decoded EP 0x82 controller reports (`53 32 08 SS 43 ...`) ~10x/s per slot and
0x37 battery heartbeats. Runs happily next to pimax-keepalived (each open fd gets its own
copy of input reports). Field layout: analysis/controllers-settings.md R2.4.
"""
import os
import select
import struct
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from pimax_probe import cmd, find_interfaces  # noqa: E402  (import does not run main)

SLOTS = {2: "L", 3: "R"}
# Field layout verified on slot 2 (captures2 controller-buttons) by test_controllers_replay.py.
# Slot 3 (right) analog fields (trigger byte 7, grip byte 44, stick bytes 5/6) are confirmed from the round-1
# capture; its digital buttons (bytes 47/48) were never pressed in any capture and are ASSUMED identical.
BTN = [(47, 0x01, "B"), (47, 0x02, "A"), (47, 0x08, "stick"), (48, 0x01, "menu")]
# byte 49: capacitive touch. A press is 47:0x02 but A touch is 49:0x01 (and B press 47:0x01, B touch 49:0x02).
TOUCH = [(49, 0x01, "touch-A"), (49, 0x02, "touch-B"), (49, 0x08, "touch-stick")]
# bit4 of byte 49 rests at 1 on the left controller (slot 2) and at 0 on the right (slot 3); report only deviations
REST49_BIT4 = {2: 0x10, 3: 0x00}
# bytes printed explicitly as decoded fields; the diff covers everything else (50..63 + 4, 49 upper bits)
DECODED = set(range(0, 4)) | {5, 6, 7} | set(range(8, 12)) | set(range(16, 28)) | set(range(30, 42)) | {44, 46, 47, 48, 49}
DIFF_IGNORE = {28, 29, 45, 51}  # 45 blink phase, 28/29 aux ADC (0 or ~0x09c9), 51 checksum: all change constantly
ATTACH_CMDS = ((0x58, 0x04, 0x16, None, 0x1E), (0x58, 0x03, 0x50, None),
               (0x58, 0x07, 0x0A, None, 0, 0, 0, 0x10), (0x58, 0x03, 0x03, None))
RETRY_MIN = 2.5      # s between attach bursts for one slot (Windows needed exactly one per slot)
STALE_AFTER = 1.5    # s without EP 0x82 reports before a slot counts as detached


def open_all():
    ifs = find_interfaces()
    if 0 not in ifs:
        return None
    fds = {"cmd": os.open(ifs[0], os.O_RDWR | os.O_NONBLOCK)}
    # EP 0x82 reports: IF1 per docs; reports are recognised by magic 53 32 08 on whichever fd they arrive.
    if 1 in ifs:
        fds["if1"] = os.open(ifs[1], os.O_RDONLY | os.O_NONBLOCK)
    return fds, ifs


def attach(fd, slot):
    """The only host->controller traffic Windows sends in a normal start (init-sequence.md phase E)."""
    for c in ATTACH_CMDS:
        os.write(fd, cmd(*[slot if x is None else x for x in c]))
        time.sleep(0.01)


def parse_0x37(d):
    """Unsolicited controller-announce notifications on IF0 (`4c 0f 37 SS 48 <u32> <batt> 00 02 01 00 08 <fw>`).
    One 64 B packet can carry two of them (second one has no 0x4c magic, starts at byte 17).
    Returns [(slot, battery_pct)]."""
    out = []
    if d[0] != 0x4C:
        return out
    for off in (0, 16):
        if len(d) > off + 10 and d[off + 2] == 0x37 and d[off + 3] in SLOTS:
            out.append((d[off + 3], d[off + 9]))
    return out


def decode(d):
    """Decode one `53 32 08 SS 43 ...` report into a dict (pure function, used by the replay test)."""
    slot = d[3]
    s = struct.unpack_from(">6h", d, 16)
    s1 = struct.unpack_from(">6h", d, 30)
    touch = [n for b, m, n in TOUCH if d[b] & m]
    if (d[49] & 0x10) != REST49_BIT4.get(slot, 0):
        touch.append("touch-t4")
    return {
        "slot": slot,
        "ts": struct.unpack_from("<I", d, 8)[0],
        "sx": d[5] - 128, "sy": d[6] - 128,        # right/up positive
        "trig": d[7], "grip": d[44], "batt": d[46] & 0x7F, "charging": bool(d[46] & 0x80),
        "buttons": [n for b, m, n in BTN if d[b] & m],
        "touch": touch,
        "btn": [n for b, m, n in BTN if d[b] & m] + touch,
        "imu": s if any(s) else s1,
    }


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    raw = "--raw" in sys.argv
    duration = float(args[0]) if args else None
    st = open_all()
    if st is None:
        sys.exit("headset IF0 not found (34a4:0044 absent?)")
    fds, ifs = st
    print("interfaces:", ifs)
    by_fd = {fd: n for n, fd in fds.items()}

    t0 = time.time()
    last = {}                 # slot -> latest report
    last_rx = {}              # slot -> time of latest report
    prev = {}                 # slot -> report at last print
    count = {2: 0, 3: 0}
    batt37 = {}               # slot -> pct from 0x37
    seen37 = {2: 0.0, 3: 0.0}  # slot -> time of latest 0x37 announce
    tried = {2: 0.0, 3: 0.0}   # slot -> time of latest attach burst
    bursts = {2: 0, 3: 0}
    replied = {2: set(), 3: set()}  # slot -> cmds answered (0x16 / 0x0a / 0x03 only answer when the controller is awake)
    n37 = 0
    for slot in (2, 3):       # initial burst, as Windows does once the panel is on
        attach(fds["cmd"], slot)
        tried[slot] = time.time()
        bursts[slot] += 1
    next_print = t0 + 0.1
    next_status = t0 + 3.0
    seen_if = {}
    try:
        while duration is None or time.time() - t0 < duration:
            r, _, _ = select.select(list(by_fd), [], [], 0.05)
            for fd in r:
                try:
                    d = os.read(fd, 64)
                except BlockingIOError:
                    continue
                except OSError as e:
                    print(f"!! read error {e}: headset re-enumerated? reopening")
                    for f in by_fd:
                        try: os.close(f)
                        except OSError: pass
                    time.sleep(1)
                    st = open_all()
                    if st is None:
                        sys.exit("headset gone")
                    fds, ifs = st
                    by_fd = {f: n for n, f in fds.items()}
                    for slot in (2, 3):
                        attach(fds["cmd"], slot)
                        tried[slot] = time.time()
                    break
                name = by_fd[fd]
                now = time.time()
                if d[:3] == b"\x53\x32\x08" and d[3] in SLOTS:
                    seen_if[name] = seen_if.get(name, 0) + 1
                    if d[3] not in last:
                        print(f"[{now-t0:5.1f}s] {SLOTS[d[3]]} controller attached: EP 0x82 reports flowing")
                    last[d[3]] = d
                    last_rx[d[3]] = now
                    count[d[3]] += 1
                    if raw:
                        print(f"RAW {SLOTS[d[3]]} {d.hex(' ')}")
                elif d[0] == 0x4C:
                    ann = parse_0x37(d)
                    if ann:
                        for slot, pct in ann:
                            n37 += 1
                            batt37[slot] = pct
                            if now - last_rx.get(slot, 0) > STALE_AFTER:
                                seen37[slot] = now   # awake but not attached (attached ones stop announcing)
                        if raw or n37 <= 2:
                            print(f"0x37 {ann}: {d[:16].hex(' ')}")
                    elif d[2] in (0x16, 0x50, 0x0A, 0x03) and d[3] in SLOTS:
                        replied[d[3]].add(d[2])
                        if d[2] == 0x16:
                            print(f"[{now-t0:5.1f}s] {SLOTS[d[3]]} serial: {d[5:24].decode('ascii', 'replace')}")
                        elif raw:
                            print(f"reply 0x{d[2]:02x} slot {d[3]}: {d[:16].hex(' ')}")
                    elif d[2] not in (0x39, 0x19, 0x51):
                        print(f"reply [{name}] {d[:20].hex(' ')}")
            now = time.time()
            # Retry: the controller is awake (0x37 announce seen after our last burst) but has not attached.
            for slot in (2, 3):
                if (seen37[slot] > tried[slot] and now - last_rx.get(slot, 0) > STALE_AFTER
                        and now - tried[slot] >= RETRY_MIN):
                    print(f"[{now-t0:5.1f}s] {SLOTS[slot]} controller awake (0x37), re-sending attach burst")
                    attach(fds["cmd"], slot)
                    tried[slot] = time.time()
                    bursts[slot] += 1
            if now >= next_print:
                next_print = now + 0.1
                for slot, d in sorted(last.items()):
                    if now - last_rx[slot] > STALE_AFTER:
                        continue
                    f = decode(d)
                    p = prev.get(slot)
                    diff = ""
                    if p is not None:
                        ch = [f"{i}:{p[i]:02x}>{d[i]:02x}" for i in range(4, 64)
                              if p[i] != d[i] and i not in DECODED and i not in DIFF_IGNORE and not (8 <= i < 16)]
                        diff = " | chg " + " ".join(ch) if ch else ""
                    prev[slot] = d
                    b = batt37.get(slot)
                    bt = f"{f['batt']}%" + ("+chg" if f["charging"] else "") + (f"/0x37:{b}%" if b is not None else "")
                    ax, ay, az, gx, gy, gz = f["imu"]
                    print(f"{SLOTS[slot]}{slot} bat={bt} btn=[{','.join(f['btn']) or '-'}] trig={f['trig']:3d} "
                          f"grip={f['grip']:3d} stick=({f['sx']:+4d},{f['sy']:+4d}) "
                          f"acc=({ax/1024:+.2f},{ay/1024:+.2f},{az/1024:+.2f})g gyro=({gx},{gy},{gz}) "
                          f"n={count[slot]}{diff}")
            if now >= next_status:
                next_status = now + 3.0
                live = [s for s in (2, 3) if now - last_rx.get(s, 0) <= STALE_AFTER]
                if not live:
                    why = ("asleep: no 0x37 announce, 0x16/0x0a/0x03 unanswered" if not n37 else
                           "0x37 seen but not attached yet")
                    print(f"[{now-t0:5.1f}s] no controller reports ({why}). PRESS A BUTTON on each controller "
                          f"(or power it on) to wake it; the burst is re-sent automatically when it announces.")
    except KeyboardInterrupt:
        pass
    print(f"done: reports L={count[2]} R={count[3]}, 0x37 msgs={n37}, bursts L={bursts[2]} R={bursts[3]}, "
          f"cmd replies {{L:{sorted(hex(x) for x in replied[2])}, R:{sorted(hex(x) for x in replied[3])}}}, "
          f"report interface counts={seen_if}")


if __name__ == "__main__":
    main()
