"""Crystal Super eye tracking takes of 2026-10-18 (G:\\omarchyvr-windows\\EYE-ALL.cmd): gaze, eye positions and
headset rotation per marked step, in one table, and how much the gaze moves when the headset moves on the face.

    python3 crystal_eye_takes.py TAKE.pcap TAKE-markers.txt GAZE.csv [TAKE2.pcap TAKE2-markers.txt GAZE2.csv ...]

GAZE.csv: `pimax-eye pcap TAKE.pcap --csv GAZE.csv` (per notification: record = packet number in the pcap, per eye the
state, origin in mm and unit direction, both in the eye chip's frame). The pcap also gives the headset IMU (IF2 reports
53 31, gyro deg/s) and what the host sent to the headset; both are put on the gaze's time line by packet time.

Per step it prints the median eye origins and gaze angles of the still part of the step (the first 2 s after a marker
are left out: the user is still moving), and the headset rotation since the first step from the integrated gyro (bias
from the first 2 s of the take, when the user looks straight ahead and holds still). The gyro axis and sign that go
with gaze yaw and pitch are found from the data (correlation over the take), not assumed.

The user looked at one point that is still in the world (the Pimax home) the whole take, so the gaze in the headset's
frame should move by minus the headset's rotation; what is left (gaze change + rotation) is the error the eye tracker
does not compensate. Last, a least-squares fit over the still steps of all takes: that error per millimetre of eye
origin change (x, y, z), per eye.
"""
import csv
import math
import statistics as st
import sys

import usbp


def markers(path):
    out = []
    for line in open(path, encoding='utf-8', errors='replace'):
        p = line.split()
        if len(p) >= 3 and p[1] == 's':
            out.append((float(p[0].replace(',', '.')), p[2]))
    return out


def ang(x, y, z):
    # the chip's frame: +z forward, y up; x is mirrored against the head frame (PIMAX_EYE_FLIP_X=1), so the yaw
    # printed here is "right" in the head frame
    return (math.degrees(math.atan2(-x, z)), math.degrees(math.atan2(y, z)))


def load(pcap, gaze_csv):
    pk = usbp.read(pcap)
    t_of = [p['t'] for p in pk]
    imu_dev = None
    for p in pk:
        d = p['data']
        if p['tt'] == 1 and (p['info'] & 1) and len(d) >= 46 and d[0] == 0x53 and d[1] == 0x31:
            imu_dev = p['dev']
            break
    gyro = []  # (t, [dps x3])
    host = []  # (t, hex) host frames to the headset other than keepalive and camera exposure
    for p in pk:
        d = p['data']
        if p['dev'] != imu_dev or p['tt'] != 1:
            continue
        if (p['info'] & 1) and len(d) >= 46 and d[0] == 0x53 and d[1] == 0x31:
            for i in range(2):
                s = d[12 + 16 * i + 2:]
                g = [int.from_bytes(s[6 + 2 * k:8 + 2 * k], 'big', signed=True) / 16.4 for k in range(3)]
                gyro.append((p['t'] + i * 0.001, g))
        elif not (p['info'] & 1) and len(d) >= 4 and d[0] == 0x58 and d[:3] not in (b'\x58\x04\x39', b'\x58\x19\x51'):
            host.append((p['t'], d[:8].hex(' ')))
    gaze = []
    for r in csv.DictReader(open(gaze_csv)):
        t = t_of[int(r['record']) - 1] if int(r['record']) - 1 < len(t_of) else float(r['time_s'])
        eyes = []
        for e in 'AB':
            if r[e + '_state'] == '2':
                o = [float(r[e + '_origin_' + k]) for k in 'xyz']
                eyes.append((o, ang(float(r[e + '_dir_x']), float(r[e + '_dir_y']), float(r[e + '_dir_z']))))
            else:
                eyes.append(None)
        gaze.append((t, eyes))
    t0 = pk[0]['t']
    return t0, gyro, host, gaze


def integrate(gyro, t_bias_end):
    # the bias: the median of the still samples of the whole take (all axes under 3 deg/s); the first seconds alone
    # are not still enough (the user just pressed Enter)
    still = [g for t, g in gyro if max(abs(x) for x in g) < 3.0]
    bias = [st.median(g[k] for g in still) for k in range(3)] if len(still) > 1000 else \
        [st.mean(g[k] for t, g in gyro if t < t_bias_end) for k in range(3)]
    ang3 = [0.0, 0.0, 0.0]
    out = []
    prev = None
    for t, g in gyro:
        if prev is not None:
            dt = min(0.01, t - prev)
            for k in range(3):
                ang3[k] += (g[k] - bias[k]) * dt
        prev = t
        out.append((t, tuple(ang3)))
    return out


def at(series, t):
    # series sorted by time: the value at or before t
    lo, hi = 0, len(series) - 1
    if t <= series[0][0]:
        return series[0][1]
    while lo < hi:
        mid = (lo + hi + 1) // 2
        if series[mid][0] <= t:
            lo = mid
        else:
            hi = mid - 1
    return series[lo][1]


def corr(a, b):
    ma, mb = st.mean(a), st.mean(b)
    sa = math.sqrt(sum((x - ma) ** 2 for x in a))
    sb = math.sqrt(sum((y - mb) ** 2 for y in b))
    return sum((x - ma) * (y - mb) for x, y in zip(a, b)) / (sa * sb) if sa and sb else 0.0


def analyse(pcap, mpath, gcsv, pooled):
    name = pcap.split('/')[-1].replace('-hub2.pcap', '')
    t0, gyro, host, gaze = load(pcap, gcsv)
    mk = markers(mpath)
    rot = integrate(gyro, t0 + 2.0)
    # gaze yaw/pitch (mean of the tracked eyes) and the three gyro angles at 10 Hz, for the axis choice
    samples = []
    for t, eyes in gaze[::12]:
        v = [e[1] for e in eyes if e]
        if v:
            samples.append((st.mean(x for x, _ in v), st.mean(y for _, y in v), at(rot, t)))
    # The axes are the headset's, the same in every take: found from the strongest correlations of the 2026-10-18
    # takes (yaw: gyro axis 1, r 0.88 in slip, where the headset turned 8 degrees; pitch: gyro axis 0, r 0.80 in
    # fit), signs so that the gaze moves against the rotation. The correlations of this take are printed as a check.
    pick = {}
    for j, label, k, sign in ((0, 'yaw', 1, -1.0), (1, 'pitch', 0, -1.0)):
        cs = [corr([s[j] for s in samples], [s[2][k] for s in samples]) for k in range(3)]
        pick[label] = (k, sign, cs)
    print(f"\n### {name}")
    print("gyro axes: " + ", ".join(
        f"{lab}: axis {k} sign {s:+.0f} (r here {cs[k]:+.2f}; all axes {cs[0]:+.2f} {cs[1]:+.2f} {cs[2]:+.2f})"
        for lab, (k, s, cs) in pick.items()))
    print("host frames to the headset (no keepalive, no camera exposure):",
          "none" if not host else "")
    for t, h in host[:30]:
        print(f"  {t - t0:7.2f} s  {h}")
    print(f"{'step':12} {'from':>6} {'n':>5} | {'A origin mm':>20} {'A gaze':>13} | {'B origin mm':>20} {'B gaze':>13} | {'rot yaw/pitch':>13}")
    steps = []
    r3_all = []
    for i, (ts, m) in enumerate(mk):
        if m in ('stop', 'end'):
            continue
        te = mk[i + 1][0] if i + 1 < len(mk) else ts + 10
        a, b = t0 + ts + 2.0, t0 + te
        rows = [g for g in gaze if a <= g[0] < b]
        if len(rows) < 30:
            continue
        cells = []
        for e in range(2):
            v = [g[1][e] for g in rows if g[1][e]]
            if len(v) < 30:
                cells.append(None)
                continue
            o = [st.median(x[0][k] for x in v) for k in range(3)]
            cells.append((o, st.median(x[1][0] for x in v), st.median(x[1][1] for x in v), len(v)))
        r3 = [st.median(at(rot, g[0])[k] for g in rows) for k in range(3)]
        r3_all.append((m, r3))
        ry = pick['yaw'][1] * r3[pick['yaw'][0]]
        rp = pick['pitch'][1] * r3[pick['pitch'][0]]
        line = f"{m:12} {ts:6.1f} {len(rows):5} |"
        for c in cells:
            line += (" " * 36 + "|") if c is None else f" {c[0][0]:+6.1f} {c[0][1]:+6.1f} {c[0][2]:+6.1f} {c[1]:+6.2f} {c[2]:+6.2f} |"
        line += f" {ry:+6.2f} {rp:+6.2f}   gyro angles {r3[0]:+6.2f} {r3[1]:+6.2f} {r3[2]:+6.2f}"
        print(line)
        if all(cells) and m not in ('slide-up', 'slide-back', 'slide-left', 'in-hands', 'off-desk', 'close-play'):
            steps.append(((a + b) / 2 - t0, m, cells, ry, rp))
    # The error of every still step against the first one; then the gyro's slow drift (what is left of its bias) is
    # taken out with a straight line through the steps where the headset sat in its place (fixate, *good, worn):
    # those must have no error.
    if len(steps) >= 2:
        base = steps[0]
        errs = []
        for t, m, cells, ry, rp in steps:
            for e in range(2):
                do = [cells[e][0][k] - base[2][e][0][k] for k in range(3)]
                ex = (cells[e][1] - base[2][e][1]) + (ry - base[3])
                ey = (cells[e][2] - base[2][e][2]) + (rp - base[4])
                errs.append((e, t, m, do, ex, ey))
        good = [x for x in errs if x[2] in ('fixate', 'fit-good', 'hold-good', 'worn')]
        def line(vals):
            if len(vals) < 2:
                return 0.0, 0.0
            mt = st.mean(t for t, _ in vals)
            mv = st.mean(v for _, v in vals)
            den = sum((t - mt) ** 2 for t, _ in vals)
            k = sum((t - mt) * (v - mv) for t, v in vals) / den if den else 0.0
            return k, mv - k * mt
        ky, cy0 = line([(t, ex) for _, t, _, _, ex, _ in good])
        kp, cp0 = line([(t, ey) for _, t, _, _, _, ey in good])
        print(f"gyro drift taken out: yaw {ky * 60:+.2f} deg/min, pitch {kp * 60:+.2f} deg/min")
        for e, t, m, do, ex, ey in errs:
            if t == base[0]:
                continue
            pooled[e].append((do, ex - (ky * t + cy0), ey - (kp * t + cp0), f"{name}:{m}"))


def lsq(rows, idx):
    # error = c . d_origin (no constant: no shift, no error), normal equations 3x3
    A = [[0.0] * 3 for _ in range(3)]
    y = [0.0] * 3
    for d, ex, ey, _ in rows:
        e = (ex, ey)[idx]
        for i in range(3):
            y[i] += d[i] * e
            for j in range(3):
                A[i][j] += d[i] * d[j]
    for i in range(3):
        A[i][i] += 1e-6
    # Gauss
    M = [A[i] + [y[i]] for i in range(3)]
    for c in range(3):
        p = max(range(c, 3), key=lambda r: abs(M[r][c]))
        M[c], M[p] = M[p], M[c]
        for r in range(3):
            if r != c:
                f = M[r][c] / M[c][c]
                M[r] = [M[r][k] - f * M[c][k] for k in range(4)]
    return [M[i][3] / M[i][i] for i in range(3)]


def main(args):
    pooled = ([], [])
    for i in range(0, len(args), 3):
        analyse(args[i], args[i + 1], args[i + 2], pooled)
    print("\n### Gaze error the tracker leaves when the eyes move against the headset (still steps, all takes)")
    for e, label in ((0, 'eye A'), (1, 'eye B')):
        rows = pooled[e]
        if len(rows) < 3:
            print(f"{label}: too few steps")
            continue
        for d, ex, ey, where in rows:
            print(f"  {label} {where:28} d origin {d[0]:+6.1f} {d[1]:+6.1f} {d[2]:+6.1f} mm -> error yaw {ex:+6.2f} pitch {ey:+6.2f} deg")
        cy, cp = lsq(rows, 0), lsq(rows, 1)
        res = [math.hypot(ex - sum(cy[k] * d[k] for k in range(3)), ey - sum(cp[k] * d[k] for k in range(3)))
               for d, ex, ey, _ in rows]
        raw = [math.hypot(ex, ey) for _, ex, ey, _ in rows]
        print(f"{label}: yaw error = {cy[0]:+.3f} dx {cy[1]:+.3f} dy {cy[2]:+.3f} dz deg/mm; "
              f"pitch error = {cp[0]:+.3f} dx {cp[1]:+.3f} dy {cp[2]:+.3f} dz deg/mm; "
              f"error mean {st.mean(raw):.2f} deg, after this correction {st.mean(res):.2f} deg")


if __name__ == '__main__':
    main(sys.argv[1:])
