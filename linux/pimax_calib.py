#!/usr/bin/env python3
"""Read the headset factory calibration XML over IF0 (84x ReadDocument) and diff with the Windows capture.
Sends ONLY `58 07 0a 00 01 AL AH 10`. Usage: pimax_calib.py [out.xml]"""
import glob, os, select, struct, sys, time, zlib, difflib

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = sys.argv[1] if len(sys.argv) > 1 else os.path.join(HERE, "..", "analysis", "headset-calibration-linux.xml")
REF = os.path.join(HERE, "..", "analysis", "headset-calibration.xml")


def find_if(num, vid="34A4", pid="0044"):
    for node in sorted(glob.glob("/sys/class/hidraw/hidraw*")):
        dev = os.path.realpath(os.path.join(node, "device"))
        try:
            if f"HID_ID=0003:0000{vid}:0000{pid}" not in open(dev + "/uevent").read():
                continue
            if int(open(os.path.join(os.path.dirname(dev), "bInterfaceNumber")).read(), 16) == num:
                return "/dev/" + os.path.basename(node)
        except OSError:
            pass


def read_block(fd, addr, timeout=1.0, tries=3):
    req = bytes([0x00, 0x58, 0x07, 0x0A, 0x00, 0x01, addr & 255, addr >> 8, 0x10]) + bytes(55)
    for _ in range(tries):
        os.write(fd, req)
        end = time.time() + timeout
        while time.time() < end:
            r, _, _ = select.select([fd], [], [], 0.1)
            if not r:
                continue
            d = os.read(fd, 64)
            if d[0] == 0x4C and d[2] == 0x0A and d[4] == 0x01 and d[5] | d[6] << 8 == addr:
                return d[8:24]
    raise SystemExit(f"no reply for addr 0x{addr:04x}")


def main():
    path = find_if(0)
    if not path:
        sys.exit("IF0 not found")
    fd = os.open(path, os.O_RDWR | os.O_NONBLOCK)
    blob = b"".join(read_block(fd, a) for a in range(0, 0x540, 0x10))
    n = struct.unpack_from("<I", blob)[0]
    xml = zlib.decompress(blob[16:16 + n])
    open(OUT, "wb").write(xml)
    print(f"read {len(blob)} B, zlib {n} B -> xml {len(xml)} B -> {os.path.normpath(OUT)}")
    if os.path.exists(REF):
        ref = open(REF, "rb").read()
        if ref == xml:
            print("IDENTICAL to analysis/headset-calibration.xml")
        else:
            print("DIFFERS from Windows capture:")
            for l in difflib.unified_diff(ref.decode().splitlines(), xml.decode().splitlines(), "windows", "linux", lineterm="", n=0):
                print(l[:200])


if __name__ == "__main__":
    main()
