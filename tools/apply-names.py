#!/usr/bin/env python3
"""Apply tools/names.tsv (lab names -> omarchyvr names) to a tree: path renames + text substitutions.

usage: apply-names.py TREE [--names tools/names.tsv]
Used by tools/lab-sync.sh on lab exports, and it was used for the one-time rename. A line with `names:keep` (any
comment syntax) or a file with `names:keep-file` near its top is not touched. Idempotent on already
renamed text (new names are not matched again). Binary files are only renamed, never rewritten.
"""
import os, re, sys

skips = []


def load(path):
    prot, rx, words, idents, paths = [], [], [], [], []
    skips.clear()
    for line in open(path, encoding="utf-8"):
        line = line.rstrip("\n")
        if not line or line.startswith("#"):
            continue
        k, o, n = (line.split("\t") + ["", ""])[:3]
        if k == "protect": prot.append(re.compile(o))
        elif k == "regex": rx.append((re.compile(o), n))
        elif k == "word": words.append((o, n))
        elif k == "ident": idents.append((o, n))
        elif k == "path": paths.append((o, n))
        elif k == "skip": skips.append(o)
    words.sort(key=lambda t: -len(t[0]))
    idents.sort(key=lambda t: -len(t[0]))
    wr = [(re.compile(r"(?<![A-Za-z0-9_-])" + re.escape(o) + r"(?![A-Za-z0-9_])"), n) for o, n in words]
    ir = [(re.compile(r"(?<![A-Za-z0-9_])" + re.escape(o) + r"(?![A-Za-z0-9_]|\.[ch]\b)"), n) for o, n in idents]
    return prot, rx, wr, ir, dict(paths)

KEEP = "names:keep"


def transform(text, rules):
    """Lines containing `names:keep` are left as they are; a file with `names:keep-file` in its first lines is skipped
    (legacy-recognition code and the docs of the mapping must keep the old names)."""
    if KEEP + "-file" in text[:800]:
        return text
    if KEEP in text:
        return "".join(l if KEEP in l else transform(l, rules) for l in text.splitlines(keepends=True))
    prot, rx, wr, ir, _ = rules
    saved = []
    def hide(m):
        saved.append(m.group(0))
        return "\x00%d\x00" % (len(saved) - 1)
    for p in prot:
        text = p.sub(hide, text)
    for p, n in rx + wr + ir:
        text = p.sub(n, text)
    return re.sub("\x00(\\d+)\x00", lambda m: saved[int(m.group(1))], text)

def main():
    args = sys.argv[1:]
    names = os.path.join(os.path.dirname(os.path.abspath(__file__)), "names.tsv")
    if "--names" in args:
        i = args.index("--names"); names = args[i + 1]; del args[i:i + 2]
    tree = args[0]
    rules = load(names)
    skip = {".git"}
    for root, dirs, files in os.walk(tree):
        dirs[:] = [d for d in dirs if d not in skip]
        for f in files:
            p = os.path.join(root, f)
            rel = os.path.relpath(p, tree)
            if os.path.islink(p) or f in ("names.tsv", "apply-names.py") or any(rel.startswith(sk) for sk in skips):
                continue
            try:
                data = open(p, "rb").read()
                txt = data.decode("utf-8")
                if "\x00" in txt:
                    raise UnicodeDecodeError("x", b"", 0, 1, "nul")
            except (UnicodeDecodeError, OSError):
                continue
            new = transform(txt, rules)
            if new != txt:
                open(p, "w", encoding="utf-8", newline="").write(new)
    # path renames, deepest first
    allp = []
    for root, dirs, files in os.walk(tree, topdown=False):
        if ".git" in root.split(os.sep):
            continue
        for n in files + dirs:
            allp.append(os.path.join(root, n))
    for p in allp:
        rel = os.path.relpath(p, tree)
        exact = rules[4].get(rel)
        base = os.path.basename(rel)
        nb = os.path.basename(exact) if exact else transform(base, rules)
        if exact and os.path.dirname(exact) != os.path.dirname(rel):
            dst = os.path.join(tree, exact); os.makedirs(os.path.dirname(dst), exist_ok=True)
        else:
            dst = os.path.join(os.path.dirname(p), nb)
        if dst != p and os.path.exists(p) and not os.path.exists(dst):
            os.rename(p, dst)

if __name__ == "__main__":
    main()
