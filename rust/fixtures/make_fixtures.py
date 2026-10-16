#!/usr/bin/env python3
"""Generate rust/fixtures/* from the real captures. Needs tshark and the pickles in analysis/.
Record format of *.rec.gz: repeated [f64 LE time][64 raw bytes]. Re-run: python3 rust/fixtures/make_fixtures.py"""
import gzip, json, os, pickle, re, struct, subprocess, sys
from datetime import datetime

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, os.path.join(ROOT, "linux"))
from pimax_controllers import decode, parse_0x37  # noqa: E402

def write_rec(name, rows):
    with gzip.open(os.path.join(HERE, name), "wb", 9) as f:
        for t, b in rows:
            b = bytes(b)[:64].ljust(64, b"\0")
            f.write(struct.pack("<d", t) + b)
    print(name, len(rows))

def tshark(pcap, ep):
    out = subprocess.run(["tshark", "-r", os.path.join(ROOT, "captures2", pcap), "-Y",
        f"usb.endpoint_address=={ep} && usb.data_len>0", "-T", "fields", "-e", "frame.time_epoch",
        "-e", "usb.capdata", "-e", "usbhid.data"], capture_output=True, text=True).stdout
    rows = []
    for line in out.splitlines():
        p = line.split("\t")
        h = p[1] if len(p) > 1 and p[1] else (p[2] if len(p) > 2 else "")
        if h:
            rows.append((float(p[0]), bytes.fromhex(h.replace(":", ""))))
    return rows

for n in ("p82", "p83"):
    write_rec(n + ".rec.gz", pickle.load(open(os.path.join(ROOT, "analysis", n + ".pkl"), "rb")))

rows = [(t, b) for t, b in tshark("20261013-130931-controller-buttons-hub2.pcap", "0x82") if b[:3] == b"\x53\x32\x08"]
write_rec("ctrl_buttons_hub2.rec.gz", rows)
marks = {}
for line in open(os.path.join(ROOT, "captures2/notes.txt")):
    m = re.match(r"(\S+) MARK controller-buttons: (.*)", line)
    if m:
        marks[m.group(2).strip()] = datetime.fromisoformat(m.group(1)[:26] + m.group(1)[-6:]).timestamp()
json.dump(marks, open(os.path.join(HERE, "ctrl_marks.json"), "w"), indent=1)

ep81 = [(t, b) for t, b in tshark("20261013-130457-cold-start-hub2.pcap", "0x81") if b[:1] == b"\x4c"]
write_rec("if0_replies_coldstart.rec.gz", ep81)
ep84 = tshark("20261013-130457-cold-start-hub2.pcap", "0x84")
write_rec("if3_log_coldstart.rec.gz", ep84)

# Python golden outputs (reference implementation = linux/pimax_controllers.py)
BTN = ["B", "A", "stick", "menu"]; TCH = ["touch-A", "touch-B", "touch-stick", "touch-t4"]
def golden_ctrl(rows):
    out = []
    for t, b in rows:
        if b[:3] != b"\x53\x32\x08" or b[3] not in (2, 3):
            continue
        f = decode(b)
        bm = sum(1 << BTN.index(n) for n in f["buttons"])
        tm = sum(1 << TCH.index(n) for n in f["touch"])
        out.append(f'{f["slot"]},{f["ts"]},{f["sx"]},{f["sy"]},{f["trig"]},{f["grip"]},{f["batt"]},{int(f["charging"])},{bm},{tm},{",".join(map(str, f["imu"]))}')
    return "\n".join(out) + "\n"
allrows = [(t, b) for t, b in pickle.load(open(os.path.join(ROOT, "analysis/p82.pkl"), "rb"))] + rows
with gzip.open(os.path.join(HERE, "ctrl_golden.csv.gz"), "wb", 9) as f:
    f.write(golden_ctrl(allrows).encode())
# 0x37 golden
g37 = []
for t, b in ep81:
    r = parse_0x37(b.ljust(64, b"\0"))
    if r:
        g37.append(f'{b.ljust(64, b"\0").hex()} {";".join(f"{s}:{p}" for s, p in r)}')
open(os.path.join(HERE, "status37_golden.txt"), "w").write("\n".join(g37) + "\n")
print("0x37 packets", len(g37))

# Mahony golden (reference = linux/pimax_imu_orient.py, identity axes, XML calibration, no auto-bias)
import pimax_imu_orient as po
cal = po.load_cal()
f = po.Mahony(1.0, 0.0); prev = None; init = False; wraps = 0; last = None; gold = []; idx = 0
for t, b in pickle.load(open(os.path.join(ROOT, "analysis/p83.pkl"), "rb")):
    if b[:2] != b"\x53\x31":
        continue
    ts = struct.unpack_from("<I", b, 4)[0]
    if last is not None and ts < last - (1 << 31):
        wraps += 1
    last = ts
    for k in (0, 1):
        o = 12 + 16 * k
        temp, ax, ay, az, gx, gy, gz = struct.unpack_from(">H6h", b, o)
        tt = ts + (wraps << 32) + 1000 * k
        ac, g = po.correct((ax / 2048.0, ay / 2048.0, az / 2048.0), (gx / 16.4, gy / 16.4, gz / 16.4), cal)
        if not init:
            n = (sum(x * x for x in ac) ** 0.5) or 1
            ux, uy, uz = (x / n for x in ac)
            w = 1 + uy; qx, qz = -uz, ux
            m = (w * w + qx * qx + qz * qz) ** 0.5 or 1
            f.q = [w / m, qx / m, 0.0, qz / m]; init = True; prev = tt; continue
        dt = (tt - prev) * 1e-6; prev = tt
        if not 0 < dt < 0.05:
            continue
        f.update(g, ac, dt)
        idx += 1
        if idx % 1000 == 0:
            gold.append(" ".join(repr(x) for x in [idx] + f.q + list(f.euler())))
open(os.path.join(HERE, "mahony_golden.txt"), "w").write("\n".join(gold) + "\n")
print("mahony golden rows", len(gold))
