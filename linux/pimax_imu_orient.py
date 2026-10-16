#!/usr/bin/env python3
"""Pimax Dream Air IMU orientation (Mahony filter) from hidraw IF2, read-only (sends nothing).

Report (64 B): 53 31 08 00 | u32 LE ts_us @4 | 2 samples @12 and @28 (s1 = ts+1000 us):
  +0 be16 temp, +2 acc x,y,z (be16, 2048 LSB/g), +8 gyro x,y,z (be16, 16.4 LSB/dps).
Sensor frame: +Y up (gravity reads +Y at rest). Euler printed assuming sensor frame == OpenXR
(X right, Y up, Z back; see analysis/imu-orientation.md, medium-low confidence for X/Z signs).
Usage: pimax_imu_orient.py [--json] [--rate HZ] [--no-calib] [--auto-bias S] [--still N] [--seconds T]
"""
import argparse, glob, json, math, os, re, select, struct, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
CAL = os.path.join(HERE, "..", "analysis", "headset-calibration.xml")
ACC_LSB, GYR_LSB, G = 2048.0, 16.4, 9.80665
D2R = math.pi / 180


def find_if2():
    for node in sorted(glob.glob("/sys/class/hidraw/hidraw*")):
        dev = os.path.realpath(os.path.join(node, "device"))
        try:
            if "HID_ID=0003:000034A4:00000044" in open(dev + "/uevent").read() and \
               int(open(os.path.join(os.path.dirname(dev), "bInterfaceNumber")).read(), 16) == 2:
                return "/dev/" + os.path.basename(node)
        except OSError:
            pass
    sys.exit("IF2 hidraw not found")


def load_cal():
    try:
        t = open(CAL).read()
        g = lambda k: [float(x) for x in re.search(k + r'="([^"]+)"', t).group(1).split()]
        return {k: g(k) for k in ("aBias", "wBias", "ka", "kg")}
    except Exception:
        return None


def samples(fd):
    """Yield (t_us_unwrapped, acc_g[3], gyro_dps[3], temp_raw) at 1 kHz from IF2 reports."""
    wraps, last = 0, None
    while True:
        r, _, _ = select.select([fd], [], [], 1.0)
        if not r:
            continue
        d = os.read(fd, 64)
        if len(d) < 28 or d[:2] != b"\x53\x31":
            continue
        ts = struct.unpack_from("<I", d, 4)[0]
        if last is not None and ts < last - (1 << 31):
            wraps += 1
        last = ts
        for k in (0, 1):
            o = 12 + 16 * k
            temp, ax, ay, az, gx, gy, gz = struct.unpack_from(">H6h", d, o)
            yield (ts + (wraps << 32) + 1000 * k, (ax / ACC_LSB, ay / ACC_LSB, az / ACC_LSB),
                   (gx / GYR_LSB, gy / GYR_LSB, gz / GYR_LSB), temp)


class Mahony:
    """Quaternion q (w,x,y,z) = sensor->world, world up = +Y. Accel-gated tilt correction."""
    def __init__(self, kp=1.0, ki=0.0):
        self.q = [1.0, 0.0, 0.0, 0.0]; self.kp, self.ki = kp, ki; self.ei = [0.0] * 3

    def update(self, g, a, dt):
        q0, q1, q2, q3 = self.q
        n = math.sqrt(sum(x * x for x in a))
        ex = ey = ez = 0.0
        if 0.7 < n < 1.3 and sum(abs(x) for x in g) < 150:   # skip accel when accelerating/fast rotation
            ax, ay, az = (x / n for x in a)
            vx, vy, vz = 2 * (q1 * q2 + q0 * q3), q0 * q0 - q1 * q1 + q2 * q2 - q3 * q3, 2 * (q2 * q3 - q0 * q1)
            ex, ey, ez = ay * vz - az * vy, az * vx - ax * vz, ax * vy - ay * vx
        if self.ki:
            for i, e in enumerate((ex, ey, ez)):
                self.ei[i] += self.ki * e * dt
        gx, gy, gz = (g[0] * D2R + self.kp * ex + self.ei[0], g[1] * D2R + self.kp * ey + self.ei[1],
                      g[2] * D2R + self.kp * ez + self.ei[2])
        h = 0.5 * dt
        q = [q0 + h * (-q1 * gx - q2 * gy - q3 * gz), q1 + h * (q0 * gx + q2 * gz - q3 * gy),
             q2 + h * (q0 * gy - q1 * gz + q3 * gx), q3 + h * (q0 * gz + q1 * gy - q2 * gx)]
        m = math.sqrt(sum(x * x for x in q)); self.q = [x / m for x in q]

    def euler(self):
        w, x, y, z = self.q
        col = lambda c: [(1 - 2 * (y * y + z * z), 2 * (x * y + w * z), 2 * (x * z - w * y)),
                         (2 * (x * y - w * z), 1 - 2 * (x * x + z * z), 2 * (y * z + w * x)),
                         (2 * (x * z + w * y), 2 * (y * z - w * x), 1 - 2 * (x * x + y * y))][c]
        r, u, b = col(0), col(1), col(2)           # right, up, back in world
        f = (-b[0], -b[1], -b[2])
        yaw = math.degrees(math.atan2(-f[0], -f[2]))
        pitch = math.degrees(math.asin(max(-1, min(1, f[1]))))
        roll = math.degrees(math.atan2(-r[1], u[1]))
        return yaw, pitch, roll


def correct(acc, gyr, cal):
    if not cal:
        return acc, gyr
    a = [(acc[i] * G - cal["aBias"][i]) / (1 + cal["ka"][i]) / G for i in range(3)]
    w = [(gyr[i] * D2R - cal["wBias"][i]) / (1 + cal["kg"][i]) / D2R for i in range(3)]
    return a, w


def still(fd, secs, cal):
    rec, t0 = [], None
    for t, a, g, _ in samples(fd):
        t0 = t if t0 is None else t0
        if t - t0 > secs * 1e6:
            break
        rec.append((a, g))
    n = len(rec); h = n // 2
    mean = lambda rows, i: sum(r[i] for r in rows) / len(rows)
    def std(rows, i):
        m = mean(rows, i)
        return math.sqrt(sum((r[i] - m) ** 2 for r in rows) / len(rows))
    gy = [r[1] for r in rec]; ga = [r[0] for r in rec]
    print(f"{n} samples over {secs} s")
    names = "xyz"
    m1 = [mean(gy[:h], i) for i in range(3)]; m2 = [mean(gy[h:], i) for i in range(3)]
    mall = [mean(gy, i) for i in range(3)]
    print("gyro raw mean (dps) :", " ".join(f"{names[i]}={mall[i]:+.4f}" for i in range(3)))
    print("gyro noise 1-sigma  :", " ".join(f"{names[i]}={std(gy, i):.4f}" for i in range(3)), "dps/sample (1 kHz)")
    print("ARW estimate        :", " ".join(f"{names[i]}={std(gy, i) / math.sqrt(1000):.4f}" for i in range(3)), "dps/sqrt(s) (std/sqrt(fs), i.e. deg/sqrt(s))")
    if cal:
        wb = [cal["wBias"][i] / D2R for i in range(3)]
        print("calib wBias (dps)   :", " ".join(f"{names[i]}={wb[i]:+.4f}" for i in range(3)))
        print("drift/min with calib bias removed (deg):", " ".join(f"{names[i]}={(mall[i] - wb[i]) * 60:+.2f}" for i in range(3)))
    print("drift/min no bias removal (deg)        :", " ".join(f"{names[i]}={mall[i] * 60:+.2f}" for i in range(3)))
    print("drift/min, bias from 1st half, applied to 2nd half (deg):",
          " ".join(f"{names[i]}={(m2[i] - m1[i]) * 60:+.3f}" for i in range(3)))
    am = [mean(ga, i) for i in range(3)]
    print("accel mean (g)      :", " ".join(f"{names[i]}={am[i]:+.4f}" for i in range(3)), f"|a|={math.sqrt(sum(x*x for x in am)):.4f}")
    ac = correct(am, mall, cal)[0] if cal else am
    for lab, v in (("raw", am), ("calib", ac)):
        pitch = math.degrees(math.atan2(v[2], math.hypot(v[0], v[1])))  # note: forward=-Z => nose-up = -az
        roll = math.degrees(math.atan2(v[0], v[1]))
        print(f"static tilt from accel ({lab}): pitch(nose-up, -Z fwd)={-pitch:+.2f} deg  roll(+X side up)={roll:+.2f} deg")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--json", action="store_true"); ap.add_argument("--rate", type=float, default=10)
    ap.add_argument("--no-calib", action="store_true"); ap.add_argument("--auto-bias", type=float, default=0,
                    help="measure gyro bias for S seconds first (headset must be still)")
    ap.add_argument("--still", type=float, default=0); ap.add_argument("--seconds", type=float, default=0)
    ap.add_argument("--kp", type=float, default=1.0); ap.add_argument("--ki", type=float, default=0.0)
    a = ap.parse_args()
    cal = None if a.no_calib else load_cal()
    fd = os.open(find_if2(), os.O_RDONLY | os.O_NONBLOCK)
    if a.still:
        return still(fd, a.still, cal)
    bias = [0.0] * 3
    if a.auto_bias:
        acc, t0 = [], None
        for t, ac, g, _ in samples(fd):
            t0 = t if t0 is None else t0
            if t - t0 > a.auto_bias * 1e6: break
            acc.append(correct(ac, g, cal)[1])
        bias = [sum(x[i] for x in acc) / len(acc) for i in range(3)]
        print("auto gyro bias (dps, after calib):", [round(b, 4) for b in bias], file=sys.stderr)
    f = Mahony(a.kp, a.ki); prev = None; nxt = 0; wall0 = time.time(); init = False
    for t, ac, g, _ in samples(fd):
        ac, g = correct(ac, g, cal); g = [g[i] - bias[i] for i in range(3)]
        if not init:   # seed tilt from accel so convergence is instant
            n = math.sqrt(sum(x * x for x in ac)) or 1
            ux, uy, uz = (x / n for x in ac)           # sensor-frame up
            # shortest rotation taking sensor-up to world +Y (sensor->world)
            w = 1 + uy; qx, qz = -uz, ux
            m = math.sqrt(w * w + qx * qx + qz * qz) or 1
            f.q = [w / m, qx / m, 0.0, qz / m]; init = True; prev = t; continue
        dt = (t - prev) * 1e-6; prev = t
        if not 0 < dt < 0.05:
            continue
        f.update(g, ac, dt)
        if t >= nxt:
            nxt = t + 1e6 / a.rate
            if a.json:
                print(json.dumps({"t_us": t, "q": [round(x, 6) for x in f.q]}), flush=True)
            else:
                y, p, r = f.euler()
                print(f"t={t*1e-6:9.3f}s yaw={y:+8.2f} pitch={p:+7.2f} roll={r:+7.2f} deg", flush=True)
        if a.seconds and time.time() - wall0 > a.seconds:
            break


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        pass
