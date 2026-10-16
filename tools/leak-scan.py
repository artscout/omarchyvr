#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""leak-scan.py: refuse commits and pushes that would publish private data.

  tools/leak-scan.py --staged          scan the staged diff (pre-commit)
  tools/leak-scan.py --range A..B      scan the commits A..B (pre-push; A may be empty for a new branch)

Same patterns as tools/export-public.sh: device serials (ASCII and UTF-16LE), provider API keys, private keys, JWTs,
e-mail addresses (except noreply and the public ones in public-manifest.txt "~ email"), the home path, the login name,
Windows user paths, machine values from ~/.config/omarchyvr (compared by value, never printed), eye-chip cold-start bytes
(compared with the private frame list when it is present), denied file types, and any path under .claude/ or a
private lab path. Only added lines of text files are scanned; binary and .gz files are scanned whole.
Prints the kind of hit and the path, never the value. Exit 1 on any hit.
"""
import collections, gzip, math, os, re, subprocess, sys
from pathlib import PurePosixPath

REPO = subprocess.run(["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True, check=True).stdout.strip()
HOME = os.path.expanduser("~")
USER = os.path.basename(HOME)


def git(*a, text=False):
    return subprocess.run(["git", "-C", REPO, *a], capture_output=True, check=True, text=text).stdout


PRIVATE_PATHS = [".claude/**", "CLAUDE.local.md", "**/*.local.md", "analysis/headset-calibration*.xml",
                 "rust/pimax-cam/tests/fixtures/evening2.pxcam", "rust/omarchyvr-passthrough/tests/fixtures/*.png",
                 "rust/fixtures/if0_replies_coldstart.rec.gz", "rust/pimax-eye/fixtures/*.usbpcap.gz",
                 "rust/pimax-eye/fixtures/*.golden.gz", "calib/**", "pimax-data/**", "captures*/**",
                 "**/ai.toml", "**/.env", "**/*.key", "**/*.pem"]
DENY_EXT = {".dll", ".exe", ".sys", ".dmp", ".pcap", ".pcapng", ".pkl", ".pxcam", ".raw", ".pgm", ".ppm", ".jpg",
            ".jpeg", ".fw", ".firmware", ".hex", ".dfu", ".img"}
KEY = re.compile(r"(?<![A-Za-z0-9_-])(sk-(?:or-v1-|ant-(?:api03-|oat01-)?|proj-|svcacct-|admin-)?|ghp_|gho_|ghs_|ghu_|github_pat_|hf_|AKIA|ASIA|AIza|xox[abprs]-|glpat-)([A-Za-z0-9_\-]{16,})")
PEM = re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----")
JWT = re.compile(r"eyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}")
EMAIL = re.compile(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}")
EMAIL_OK = re.compile(r"(?i)@(users\.noreply\.github\.com|anthropic\.com|example\.(com|org|net)|localhost|[a-z0-9.-]*\.invalid)$|^git@github\.com$|^git@gitlab|^noreply@|\.(png|svg|so|rs|js|json|toml|md|py|c|h|txt)$")
SERIAL = re.compile(rb"P\d{3}[A-Z]\d[A-Z][0-9A-Z]{8,16}|\bH\d{5}[A-Z]\d{10,14}\b")
SERIAL16 = re.compile(rb"P\x00(?:\d\x00){3}[A-Z]\x00\d\x00[A-Z]\x00(?:[0-9A-Z]\x00){8,16}")
WINUSER = re.compile(r"(?i)C:[\\/]+Users[\\/]+(?!Public|Default|<|%|\$|USER|you|name|me\b)[A-Za-z0-9._-]+")


def fake_serial(g):
    return b"X0X" in g or (g.startswith(b"H") and g[6:7] == b"X" and g[7:].strip(b"0") in (b"1", b"2", b""))


def entropy(s):
    c = collections.Counter(s)
    return -sum(v / len(s) * math.log2(v / len(s)) for v in c.values())


def allowed_mail():
    p = os.path.join(REPO, "public-manifest.txt")
    if not os.path.isfile(p):
        return []
    return [l.split()[2] for l in open(p, encoding="utf-8") if l.startswith("~ email ")]


def private_values():
    vals = []
    # the new dir and the one used before the rename (a machine may still have both)
    for cfg in (os.path.join(HOME, ".config/omarchyvr"), os.path.join(HOME, ".config/pimax")):  # names:keep
        for n in os.listdir(cfg) if os.path.isdir(cfg) else []:
            if n.startswith("ai.toml") or n == "paths.toml":
                t = open(os.path.join(cfg, n), encoding="utf-8", errors="replace").read()
                vals += [m.group(1).encode() for m in re.finditer(r'(?m)^\s*(?:api_key|shared_drive_uuid|token)\s*=\s*"([^"]{8,})"', t)]
                vals += [m.group(1).encode() for m in re.finditer(r'"((?:sk-|key-)[^"]{12,})"', t)]
    return vals


def eye_bytes():
    """hex payloads (>= 12 bytes) of the private eye-chip cold-start list, lower case, never printed"""
    out = set()
    for p in (os.path.join(REPO, ".claude/lab/repo/rust/pimax-eye/data/replay-frames.txt"),
              os.path.join(REPO, "rust/pimax-eye/data/replay-frames.txt")):
        if os.path.isfile(p):
            for h in re.findall(r"(?i)\b(?:data=)?([0-9a-f]{24,})\b", open(p, encoding="utf-8", errors="replace").read()):
                h = h.lower()
                if len(set(h)) > 3:
                    out.add(h[:48])
    return out


class Scan:
    def __init__(self):
        self.hits = collections.defaultdict(set)
        self.mail_ok = allowed_mail()
        self.priv = private_values()
        self.eye = eye_bytes()

    def hit(self, kind, path):
        self.hits[kind].add(path)

    def path(self, p, mode=None, blob=b""):
        if any(PurePosixPath(p).full_match(g) for g in PRIVATE_PATHS):
            self.hit("private path", p)
        if os.path.splitext(p)[1].lower() in DENY_EXT:
            self.hit("denied file type", p)
        if mode == "120000" and (b".claude" in blob or blob.startswith(b"/home/") or blob.startswith(HOME.encode())):
            self.hit("symlink into private or home path", p)

    def blob(self, p, b):
        blobs = [b]
        if p.endswith(".gz"):
            try:
                blobs.append(gzip.decompress(b))
            except Exception:
                pass
        for x in blobs:
            if any(not fake_serial(m.group(0)) for m in SERIAL.finditer(x)):
                self.hit("device serial", p)
            if any(b"X\x000\x00X" not in m.group(0) for m in SERIAL16.finditer(x)):
                self.hit("device serial (utf-16)", p)
            if any(v in x for v in self.priv):
                self.hit("private machine value", p)
            if len(HOME) > 6 and HOME.encode() in x:
                self.hit("home path", p)
            if self.eye:
                low = re.sub(rb"[^0-9a-f]", b"", x.lower()) if len(x) < 4_000_000 else b""
                if any(e.encode() in low for e in self.eye):
                    self.hit("eye-chip cold-start bytes", p)

    def text(self, p, t):
        self.blob(p, t.encode())
        for m in KEY.finditer(t):
            body = m.group(2)
            if entropy(body) > 3.5 and not re.search(r"(?i)test|fake|not-?a-?key|example|dummy|demo|xxxx|0000|aaaa|1111", body):
                self.hit("api key", p)
        if PEM.search(t):
            self.hit("private key", p)
        if JWT.search(t):
            self.hit("jwt", p)
        for m in EMAIL.finditer(t):
            a = m.group(0)
            if not EMAIL_OK.search(a) and not any(a == x or (x.startswith("@") and a.endswith(x)) for x in self.mail_ok):
                self.hit("e-mail address", p)
        if WINUSER.search(t):
            self.hit("windows user path", p)
        if len(USER) > 2 and re.search(r"/home/" + re.escape(USER) + r"\b", t):
            self.hit("home path", p)
        if len(USER) > 3 and re.search(r"\b" + re.escape(USER) + r"\b", t, re.I):
            self.hit("login name", p)


def added_lines(diff):
    """{path: added text} from a unified diff (-U0)"""
    out = collections.defaultdict(list)
    cur = None
    for line in diff.split("\n"):
        if line.startswith("+++ "):
            cur = line[6:] if line.startswith("+++ b/") else None
        elif cur and line.startswith("+"):
            out[cur].append(line[1:])
    return {k: "\n".join(v) for k, v in out.items()}


def scan_change(s, names_cmd, diff_cmd, show):
    entries = [l for l in git(*names_cmd, text=True).split("\0") if l]
    # --raw -z: ":old new oldsha newsha status\0path\0"
    i = 0
    changed = []
    while i < len(entries):
        meta = entries[i].split()
        status = meta[4] if len(meta) > 4 else ""
        n = 2 if status[:1] in ("R", "C") else 1
        path = entries[i + n]
        i += n + 1
        if status.startswith("D"):
            continue
        changed.append((meta[1], path))
    texts = added_lines(git(*diff_cmd, text=True))
    for mode, p in changed:
        if mode == "160000":  # submodule (gitlink): only the path is checked; the fork has its own scan
            s.path(p)
            continue
        b = show(p)
        s.path(p, mode, b if mode == "120000" else b"")
        if mode == "120000":
            continue
        if b"\x00" in b[:4096] or p.endswith(".gz"):
            s.blob(p, b)
        else:
            s.text(p, texts[p] if p in texts else b.decode("utf-8", "replace"))
    return len(changed)


def main():
    s = Scan()
    if sys.argv[1:2] == ["--staged"]:
        n = scan_change(s, ["diff", "--cached", "--raw", "-z", "--no-renames", "--diff-filter=ACMRT"],
                        ["diff", "--cached", "-U0", "--no-color", "--no-ext-diff", "--src-prefix=a/", "--dst-prefix=b/", "--diff-filter=ACMRT"],
                        lambda p: git("show", ":" + p))
        what = "staged changes"
    elif sys.argv[1:2] == ["--range"] and len(sys.argv) == 3:
        a, b = sys.argv[2].split("..")
        base = a or git("hash-object", "-t", "tree", "/dev/null", text=True).strip()
        n = scan_change(s, ["diff", "--raw", "-z", "--no-renames", "--diff-filter=ACMRT", base, b],
                        ["diff", "-U0", "--no-color", "--no-ext-diff", "--src-prefix=a/", "--dst-prefix=b/", "--diff-filter=ACMRT", base, b],
                        lambda p: git("show", f"{b}:{p}"))
        revs = git("rev-list", b if not a else f"{a}..{b}", text=True).split()
        for r in revs:
            meta = git("show", "-s", "--format=%ae%n%ce%n%B", r, text=True)
            ae, ce, body = meta.split("\n", 2)
            for e in (ae, ce):
                if not EMAIL_OK.search(e):
                    s.hit("commit identity is not a noreply address", r[:10])
            s.text(f"commit message {r[:10]}", body)
        what = f"changed files in {len(revs)} commit(s)"
    else:
        print(__doc__.strip().split("\n\n")[0])
        return 2
    if s.hits:
        print(f"leak-scan: REFUSED ({n} {what} scanned)")
        for k, v in sorted(s.hits.items()):
            print(f"   HIT {k}: {', '.join(sorted(v)[:20])}")
        print("   (values are never printed; fix the files, or keep private data under .claude/)")
        return 1
    print(f"leak-scan: clean ({n} {what}, {len(s.priv)} private values, {len(s.eye)} eye payloads compared)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
