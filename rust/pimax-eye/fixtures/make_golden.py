#!/usr/bin/env python3
"""Independent Python decoder for the gaze fixtures; writes <name>.golden.gz next to each <name>.usbpcap.gz.

This deliberately shares no code with the Rust decoder (`src/gaze.rs`): it walks the TLV items with `struct` and writes
every decoded field as raw integers (S15.16 / S22.42 are NOT converted), one line per gaze notification:
    <field>=<raw>[,<raw>...] <field>=... (fields in wire order, only the complete ones)
tests/fixtures.rs compares the Rust decode against these files. Run with: python3 -I make_golden.py
"""
import glob, gzip, os, struct

HERE = os.path.dirname(os.path.abspath(__file__))

def packets(path):
    d = gzip.open(path, "rb").read()
    assert d[:4] == b"\xd4\xc3\xb2\xa1", "little endian microsecond pcap expected"
    off = 24
    while off + 16 <= len(d):
        _s, _u, incl, _orig = struct.unpack("<IIII", d[off:off + 16])
        off += 16
        yield d[off:off + incl]
        off += incl

def items(buf):
    o = 0
    while o + 5 <= len(buf):
        tag = buf[o]
        ln = struct.unpack(">I", buf[o + 1:o + 5])[0]
        if o + 5 + ln > len(buf):
            return
        yield tag, buf[o + 5:o + 5 + ln]
        o += 5 + ln

def raw(tag, v):
    if tag in (2, 3):
        return struct.unpack(">i", v)[0]
    if tag in (4, 6):
        return struct.unpack(">q", v)[0]
    raise ValueError("tag %d" % tag)

def decode(payload):
    it = list(items(payload[2:]))
    if not it or it[0][0] != 5:
        return None
    hdr = struct.unpack(">I", it[0][1])[0]
    assert hdr & 0xFFFF == 0x0BB8
    count = hdr >> 16
    i = 1
    out = []
    for _ in range(count):
        if i + 2 >= len(it):
            break
        assert it[i][0] == 5 and struct.unpack(">I", it[i][1])[0] == 0x20BB9
        fno = struct.unpack(">i", it[i + 1][1])[0]
        tag, v = it[i + 2]
        if tag == 5:
            k = struct.unpack(">I", v)[0] >> 16
            if i + 3 + k > len(it):
                break
            vals = [raw(*it[i + 3 + j]) for j in range(k)]
            i += 3 + k
        else:
            vals = [raw(tag, v)]
            i += 3
        out.append((fno, vals))
    return out

for path in sorted(glob.glob(os.path.join(HERE, "eye_gaze_*.usbpcap.gz"))):
    lines = []
    for p in packets(path):
        hl = struct.unpack("<H", p[:2])[0]
        ep, tt = p[21], p[22]
        data = p[hl:]
        if tt != 3 or ep != 0x83 or not (p[16] & 1) or len(data) < 34:
            continue
        wdir = struct.unpack("<I", data[:4])[0]
        magic = struct.unpack(">I", data[8:12])[0]
        if wdir != 1 or magic != 0x53:
            continue
        fields = decode(data[32:])
        lines.append(" ".join("%d=%s" % (f, ",".join(map(str, v))) for f, v in fields))
    out = path.replace(".usbpcap.gz", ".golden.gz")
    with gzip.open(out, "wb", 9) as f:
        f.write(("\n".join(lines) + "\n").encode())
    print(os.path.basename(out), len(lines), "samples")
