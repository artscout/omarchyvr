#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Make a synthetic (nominal) headset calibration XML from a real one, for public test fixtures.

A factory calibration is per-unit data and stays private. This keeps the file's structure and the rough geometry
of the model (so the parsers, the camera models and the synthetic tests behave like on a real headset) but none of the
unit's own values: intrinsics rounded to 0.5 px (both focal lengths equal), distortion to 3 decimals, extrinsic
rotations rounded and re-orthonormalised, translations to 1 mm, IMU terms to 2-3 decimals, the serial and the date
replaced. Usage: make-synthetic-calib.py REAL.xml OUT.xml
"""
import re
import sys

FAKE_UID = "P511X0X000000000001"


def fmt(v, nd):
    return f"{round(v, nd):.{nd}f}"


def nums(s):
    return [float(x) for x in s.split()]


def mat3_inv_t(m):
    a, b, c, d, e, f, g, h, i = m
    det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g)
    inv = [(e * i - f * h), -(b * i - c * h), (b * f - c * e),
           -(d * i - f * g), (a * i - c * g), -(a * f - c * d),
           (d * h - e * g), -(a * h - b * g), (a * e - b * d)]
    inv = [x / det for x in inv]
    # transpose
    return [inv[0], inv[3], inv[6], inv[1], inv[4], inv[7], inv[2], inv[5], inv[8]]


def orthonormal(m):
    for _ in range(30):
        it = mat3_inv_t(m)
        m = [0.5 * (x + y) for x, y in zip(m, it)]
    return m


def attr(name, fn):
    def sub(mo):
        return f'{name}="{fn(mo.group(1))}"'
    return re.compile(rf'\b{name}="([^"]*)"'), sub


def main():
    src, out = sys.argv[1], sys.argv[2]
    t = open(src, encoding="utf-8").read()
    rules = [
        attr("deviceUID", lambda s: FAKE_UID),
        attr("calibration_date", lambda s: "2000_01_01_00_00_00"),
        attr("principal_point", lambda s: " ".join(fmt(round(v * 2) / 2, 10) for v in nums(s)) + " "),
        attr("focal_length", lambda s: " ".join([fmt(round(sum(nums(s)) / len(nums(s)) * 2) / 2, 10)] * len(nums(s))) + " "),
        attr("radial_distortion", lambda s: " ".join(fmt(v, 3) + "0" * 7 for v in nums(s)) + "  "),
        attr("translation", lambda s: " ".join(fmt(v, 3) + "0" * 7 for v in nums(s)) + " "),
        attr("rowMajorRotationMat", lambda s: " ".join(fmt(v, 10) for v in orthonormal([round(x * 50) / 50 for x in nums(s)])) + " "),
    ]
    for k, nd in [("ombc", 2), ("tbc", 3), ("aBias", 2), ("wBias", 2), ("ka", 3), ("kg", 3), ("na", 3), ("ng", 3),
                  ("ombg", 2), ("accelDelta", 3)]:
        rules.append(attr(k, lambda s, nd=nd: " ".join(fmt(v, nd) for v in nums(s))))
    for rx, fn in rules:
        t = rx.sub(fn, t)
    open(out, "w", encoding="utf-8").write(t)


if __name__ == "__main__":
    main()
