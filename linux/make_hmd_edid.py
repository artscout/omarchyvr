#!/usr/bin/env python3
"""Build non-desktop (HMD) EDIDs from pimax-dp-edid-native.bin: DisplayID 2.0 ext, primary use 7 (head-mounted VR)."""
import sys
src = open(sys.argv[1] if len(sys.argv) > 1 else 'pimax-dp-edid-native.bin', 'rb').read()
base, cta, did12 = src[:128], src[128:256], src[256:384]

def t7(clk_khz, hfp, hsync, hbp, hpol, vfp, vsync, vbp, vpol, pref):
    ha, va = 7104, 3840
    hb, vb = hfp + hsync + hbp, vfp + vsync + vbp
    d = (clk_khz - 1).to_bytes(3, 'little')
    d += bytes([(0x80 if pref else 0) | 0x07])        # aspect=undefined(7)? see note; progressive, no stereo
    for act, bl, fp, sw, pol in ((ha, hb, hfp, hsync, hpol), (va, vb, vfp, vsync, vpol)):
        d += (act - 1).to_bytes(2, 'little') + (bl - 1).to_bytes(2, 'little')
        d += ((fp - 1) | (0x8000 if pol else 0)).to_bytes(2, 'little') + (sw - 1).to_bytes(2, 'little')
    assert len(d) == 20
    return d

def did20(primary, ext_count):
    prod = b'\x00\x00\x00' + (0x1019).to_bytes(2, 'little') + b'\0\0\0\0' + bytes([0, 24, 5]) + b'Pimax'
    blocks = bytes([0x20, 0, len(prod)]) + prod
    # Display Parameters block (0x21): required by edid-decode; NVIDIA 'strong validation' also wants it
    prm = (1200).to_bytes(2,'little') + (700).to_bytes(2,'little') + (7104).to_bytes(2,'little') + (3840).to_bytes(2,'little')
    prm += bytes([0x00]) + b'\0' * 12 + b'\0' * 6 + bytes([0x01, 0x78])   # features, chromaticity(9)+white(3), luminance(6), 8bpc, gamma 2.2
    assert len(prm) == 29
    blocks += bytes([0x21, 0, len(prm)]) + prm
    tim = t7(2170070, 400, 48, 200, 1, 29, 1, 18, 1, True) + t7(2642600, 200, 48, 200, 0, 29, 1, 18, 0, False)
    blocks += bytes([0x22, 0, len(tim)]) + tim
    payload = blocks.ljust(121, b'\0')
    assert len(payload) == 121
    b = bytearray([0x70, 0x20, 121, primary, ext_count]) + payload
    b.append((-sum(b[1:])) & 0xff)          # DisplayID section checksum (bytes 1..126)
    b.append((-sum(b)) & 0xff)              # EDID block checksum
    assert len(b) == 128
    return bytes(b)

def fixbase(blk, next):
    b = bytearray(blk); b[126] = next; b[127] = (-sum(b[:127])) & 0xff; return bytes(b)

open('pimax-dp-edid-hmd.bin', 'wb').write(fixbase(base, 2) + cta + did20(7, 0))
# dual: DisplayID 2.0 FIRST (kernel only reads primary use from the first DisplayID ext), then the 1.2 block
open('pimax-dp-edid-hmd-dual.bin', 'wb').write(fixbase(base, 3) + cta + did20(7, 0) + did12)
