#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""scan-tree.py: the leak scan of tools/leak-scan.py, run over an unpacked release tree (not a git diff).

  scan-tree.py --repo CLONE --root TREE [--forbid TEXT]... [--data-prefix share/omarchyvr/data/]

Uses the same checks as the pre-commit hook (device serials, private machine values, eye-chip cold-start bytes, per-device
calibration outside the shipped reference files, API keys, private keys, JWTs, e-mail addresses in text files, Windows user
paths, denied file types, private paths), applied to every file of TREE. ELF files and other binaries are scanned whole.
--forbid TEXT: the bytes must appear in no file at all (the builder's home directory, the build directories).
Also refuses any model file (the Mercury hand models have no licence; no AI weights are shipped).
Prints the kind of hit and the path, never the value. Exit 1 on any hit.
"""
import hashlib, importlib.util, os, re, sys

MODEL_EXT = {".onnx", ".gguf", ".safetensors", ".ggml", ".tflite", ".pt", ".pth", ".ckpt"}
# upstream code and libraries we only repackage: their strings (author e-mails, test keys in OpenSSL-like libraries) are
# not ours to scrub, so only the checks for OUR private data run on them (serials, machine values, eye bytes, calibration,
# the builder's paths)
THIRD_PARTY = ("lib/omarchyvr/deps/", "lib/omarchyvr/onnxruntime/", "lib/omarchyvr/sherpa-onnx/", "lib/omarchyvr/basalt/")
MODEL_NAME = re.compile(r"(?i)hand-tracking-models|grayscale_detection|hand_regression")


def load_leakscan(repo):
    os.chdir(repo)  # the module asks git for the repository at import
    spec = importlib.util.spec_from_file_location("leakscan", os.path.join(repo, "tools", "leak-scan.py"))
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


def main(a):
    repo = root = None
    forbid = []
    prefix = "share/omarchyvr/data/"
    i = 0
    while i < len(a):
        if a[i] == "--repo":
            repo = os.path.realpath(a[i + 1]); i += 1
        elif a[i] == "--root":
            root = os.path.realpath(a[i + 1]); i += 1
        elif a[i] == "--forbid":
            forbid.append(a[i + 1].encode()); i += 1
        elif a[i] == "--data-prefix":
            prefix = a[i + 1]; i += 1
        else:
            print(__doc__); return 2
        i += 1
    if not repo or not root:
        print(__doc__); return 2
    ls = load_leakscan(repo)
    # a throw-away build account is not a person: its login name is a common word
    if ls.USER in ("build", "builder", "root", "user"):
        ls.USER = ""
    s = ls.Scan()
    # binaries embed the shipped reference calibration (include_bytes!, e.g. the factory XML with deviceUID "reference"
    # in pimax-cam and Monado): a byte-identical copy of an allowed file is cut out before the scan, anything else stays
    refs = []
    for r, sha in ls.CALIB_REFERENCE.items():
        rb = open(os.path.join(repo, r), "rb").read()
        if hashlib.sha256(rb).hexdigest() != sha:
            print(f"scan-tree: the reference file {r} in the clone differs from the allowed one"); return 1
        refs.append(rb)
    n = 0
    for d, _, fs in os.walk(root):
        for f in sorted(fs):
            p = os.path.join(d, f)
            rel = os.path.relpath(p, root)
            if os.path.islink(p):
                t = os.readlink(p)
                if t.startswith("/"):
                    s.hit("absolute symlink", rel)
                continue
            n += 1
            b = open(p, "rb").read()
            data_rel = rel[len(prefix):] if rel.startswith(prefix) else rel
            if ls.CALIB_REFERENCE.get(data_rel) == hashlib.sha256(b).hexdigest():
                s.calib_ok.add(rel)
            if data_rel == ls.EYE_BASE_PATH and hashlib.sha256(b).hexdigest() == ls.EYE_BASE_SHA256:
                s.eye_ok.add(rel)
            s.path(data_rel)
            if os.path.splitext(f)[1].lower() in MODEL_EXT or MODEL_NAME.search(rel):
                s.hit("model file (hand models and AI weights are never shipped)", rel)
            for x in forbid:
                if x in b:
                    s.hit("forbidden string " + x.decode(errors="replace")[:20], rel)
            if rel.startswith(THIRD_PARTY):
                s.blob(rel, b)
            elif b[:4] == b"\x7fELF" or b"\x00" in b[:4096] or rel.endswith(".gz"):
                for rb in refs:
                    b = b.replace(rb, b"")
                s.blob(rel, b)
                txt = "\n".join(m.decode("ascii") for m in re.findall(rb"[\x20-\x7e]{16,}", b))
                for m in ls.KEY.finditer(txt):
                    body = m.group(2)
                    if ls.entropy(body) > 3.5 and not re.search(r"(?i)test|fake|not-?a-?key|example|dummy|demo|xxxx|0000|aaaa|1111", body):
                        s.hit("api key", rel)
                if ls.PEM.search(txt):
                    s.hit("private key", rel)
                if ls.JWT.search(txt):
                    s.hit("jwt", rel)
            else:
                s.text(rel, b.decode("utf-8", "replace"))
    if s.hits:
        print(f"scan-tree: REFUSED ({n} files scanned)")
        for k, v in sorted(s.hits.items()):
            print(f"   HIT {k}: {', '.join(sorted(v)[:20])}")
        return 1
    print(f"scan-tree: clean ({n} files, {len(s.priv)} private values, {len(s.eye)} eye payloads compared)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
