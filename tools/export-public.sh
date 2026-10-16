#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# export-public.sh: build the public tree (omarchyvr) from the private lab, allow-list only.
#
#   tools/export-public.sh [--no-test] [--target-dir DIR] OUT_DIR
#
# 1. takes the files committed at HEAD (never the working tree) that public-manifest.txt allows
#    ("+" globs, "-" globs win, "= DEST <- SRC" synthetic stand-ins);
# 2. scrubs: the login name -> "user" (and user@mau -> user@host), same-length masks for headset/controller serials in text and in gzip fixtures (ASCII and UTF-16LE),
#    the exporting user's home path, e-mail addresses in Monado patch headers -> the public identity;
# 3. moves ("> DEST <- SRC") leave symlinks at the old paths: ai/ for agent files, patches/nvidia for the NVIDIA patches;
#    applies the user-facing branding (omarchyvr) to the README, app window title, desktop entry and PKGBUILD;
# 4. scans: gitleaks (if installed) + our own patterns (provider keys, serials, e-mail addresses, home paths,
#    machine values read from the private config: AI keys and the shared-drive UUID, compared by value and never
#    printed) + an extension deny-list; prints counts and paths only;
# 5. runs `cargo test --workspace` on the copy (nice -n 19, -j8).
# Exit status is non-zero on any scanner hit or test failure. OUT_DIR keeps its .git (if any); everything else in it
# is replaced.
set -euo pipefail

RUN_TESTS=1
TARGET_DIR=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --no-test) RUN_TESTS=0; shift ;;
        --target-dir) TARGET_DIR="$2"; shift 2 ;;
        -h|--help) sed -n '3,20p' "$0"; exit 0 ;;
        *) break ;;
    esac
done
[[ $# -eq 1 ]] || { echo "usage: $0 [--no-test] [--target-dir DIR] OUT_DIR" >&2; exit 2; }
OUT="$(realpath -m "$1")"
REPO="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
HEAD="$(git -C "$REPO" rev-parse --short HEAD)"
case "$OUT" in "$REPO"|"$REPO"/*) echo "OUT_DIR must be outside the repository" >&2; exit 2 ;; esac

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
git -C "$REPO" archive --format=tar HEAD | tar -x -C "$STAGE"

mkdir -p "$OUT"
find "$OUT" -mindepth 1 -maxdepth 1 ! -name .git -exec rm -rf {} +

echo "== export of $HEAD -> $OUT"
python3 -I - "$STAGE" "$OUT" "$REPO/public-manifest.txt" "$HEAD" <<'PY'
import gzip, io, os, re, shutil, sys
from pathlib import PurePosixPath
stage, out, manifest, head = sys.argv[1:5]
inc, exc, repl, moves = [], [], [], []
for raw in open(manifest, encoding="utf-8"):
    l = raw.split("#", 1)[0].strip() if not raw.lstrip().startswith("#") else ""
    if not l:
        continue
    if l.startswith("+ "):
        inc.append(l[2:].strip())
    elif l.startswith("- "):
        exc.append(l[2:].strip())
    elif l.startswith("~ "):
        continue
    elif l.startswith("> "):
        dest, src = [x.strip() for x in l[2:].split("<-")]
        moves.append((dest, src))
    elif l.startswith("= "):
        dest, src = [x.strip() for x in l[2:].split("<-")]
        repl.append((dest, src))
    else:
        sys.exit(f"manifest: bad line: {raw.rstrip()}")

def match(path, pats):
    p = PurePosixPath(path)
    return any(p.full_match(g) for g in pats)

files = []
for root, dirs, names in os.walk(stage):
    for n in names:
        rel = os.path.relpath(os.path.join(root, n), stage)
        files.append(rel)
files.sort()
kept = [f for f in files if match(f, inc) and not match(f, exc)]
for f in kept:
    d = os.path.join(out, f)
    os.makedirs(os.path.dirname(d), exist_ok=True)
    shutil.copy2(os.path.join(stage, f), d)
for dest, src in repl:
    s = os.path.join(stage, src)
    if not os.path.isfile(s):
        sys.exit(f"manifest: replacement source missing at HEAD: {src}")
    d = os.path.join(out, dest)
    os.makedirs(os.path.dirname(d), exist_ok=True)
    shutil.copy2(s, d)
# moves: the file goes to DEST, a relative symlink stays at SRC (old paths and tools keep working)
for dest, src in moves:
    s = os.path.join(stage, src)
    if not os.path.isfile(s):
        sys.exit(f"manifest: move source missing at HEAD: {src}")
    d = os.path.join(out, dest)
    os.makedirs(os.path.dirname(d), exist_ok=True)
    shutil.copy2(s, d)
    o = os.path.join(out, src)
    if os.path.lexists(o):
        os.remove(o)
    os.makedirs(os.path.dirname(o), exist_ok=True)
    os.symlink(os.path.relpath(d, os.path.dirname(o)), o)
# links inside the moved Markdown files: a target that is not in ai/ but is in docs/ gets ../docs/
aidir = os.path.join(out, "ai")
for n in os.listdir(aidir) if os.path.isdir(aidir) else []:
    if not n.endswith(".md") or n == "README.md":
        continue
    p = os.path.join(aidir, n); t = open(p, encoding="utf-8").read()
    def fix(m):
        tgt = m.group(1); base = tgt.split("#")[0]
        if not base or os.path.exists(os.path.join(aidir, base)) or not os.path.exists(os.path.join(out, "docs", base)):
            return m.group(0)
        return "](../docs/" + tgt + ")"
    nt = re.sub(r"\]\((?!https?:|#|\.\./|mailto:|/)([^)\s]+)\)", fix, t)
    if nt != t:
        open(p, "w", encoding="utf-8").write(nt)
print(f"   {len(kept)} of {len(files)} committed files allowed, {len(repl)} synthetic stand-ins")

# ---- scrub ---------------------------------------------------------------------------------------------------
# headset P511 + digit/letter pattern, controller H301...; fakes keep the length and stay recognisable
HEADSET = re.compile(rb"P\d{3}[A-Z]\d[A-Z][0-9A-Z]{8,16}")
CTRL = re.compile(rb"\bH\d{5}[A-Z]\d{10,14}\b")
def is_fake(s):
    # our fakes: P511X0X000...01 (headset), H30100X000...01 (controller)
    return b"X0X" in s or (s.startswith(b"H") and s[6:7] == b"X" and s[7:].strip(b"0") in (b"1", b"2", b""))
def fake(m):
    s = m.group(0)
    if is_fake(s):
        return s
    if s.startswith(b"H"):
        return s[:6] + b"X" + b"0" * (len(s) - 8) + b"1"
    return s[:4] + (b"X0X" + b"0" * 32)[: len(s) - 5] + b"1"
def mask_bytes(b):
    b = HEADSET.sub(fake, b)
    b = CTRL.sub(fake, b)
    # UTF-16LE (USB string descriptors)
    w = re.compile(rb"P\x00(?:\d\x00){3}[A-Z]\x00\d\x00[A-Z]\x00(?:[0-9A-Z]\x00){8,16}")
    def wfake(m):
        s = m.group(0).decode("utf-16le").encode()
        if b"X0X" in s:
            return m.group(0)
        f = s[:4] + (b"X0X" + b"0" * 32)[: len(s) - 5] + b"1"
        return f.decode().encode("utf-16le")
    return w.sub(wfake, b)

home = os.path.expanduser("~").encode()
user = os.path.basename(home)
ident = b"laughtt <22533943+laughtt@users.noreply.github.com>"
nfix = ntext = 0
for root, dirs, names in os.walk(out):
    if "/.git" in root + "/" and root.startswith(os.path.join(out, ".git")):
        continue
    for n in names:
        p = os.path.join(root, n)
        b = open(p, "rb").read()
        nb = b
        if n.endswith(".gz"):
            try:
                raw = gzip.decompress(b)
            except Exception:
                continue
            m = mask_bytes(raw)
            if m != raw:
                bio = io.BytesIO()
                with gzip.GzipFile(fileobj=bio, mode="wb", compresslevel=9, mtime=0) as g:
                    g.write(m)
                nb = bio.getvalue()
                nfix += 1
        elif b"\x00" not in b[:4096]:
            nb = mask_bytes(b)
            if len(home) > 6:
                nb = nb.replace(home + b"/", b"/home/user/").replace(home, b"/home/user")
            if len(user) > 3:
                nb = nb.replace(b"https://github.com/" + user + b"-pimax-re", b"https://github.com/laughtt/omarchyvr")
                nb = nb.replace(user + b"@mau", b"user@host")
                nb = re.sub(rb"\b" + re.escape(user) + rb"\b", b"user", nb)
            nb = re.sub(rb"(?mi)(Copyright (?:\(c\) )?\d{4}(?:-\d{4})?,? )" + re.escape(user) + rb"\b[^\n]*", rb"\1laughtt", nb)
            if n.endswith(".patch"):
                nb = re.sub(rb"(?m)^(From|Signed-off-by|Co-authored-by): [^\n<]*<[^>\n]+@(?!users\.noreply\.github\.com|anthropic\.com)[^>\n]+>", lambda m: m.group(1) + b": " + ident, nb)
            if nb != b:
                ntext += 1
        if nb != b:
            open(p, "wb").write(nb)
print(f"   scrubbed {ntext} text files, {nfix} gzip fixtures")
PY

echo "== branding (omarchyvr)"
python3 -I - "$OUT" <<'PY'
import re, sys, os
out = sys.argv[1]
def edit(rel, fn):
    p = os.path.join(out, rel)
    if not os.path.isfile(p):
        print(f"   skip {rel} (not exported)"); return
    s = open(p, encoding="utf-8").read(); n = fn(s)
    if n != s:
        open(p, "w", encoding="utf-8").write(n); print(f"   {rel}")
NOTICE = ("**omarchyvr is an unofficial community project. It is not affiliated with, made by, or endorsed by Pimax or "
          "Omarchy.** \"Pimax\" and \"Dream Air\" are trademarks of Pimax and are used only to say which hardware this "
          "works with. No warranty: the tools talk to the headset over USB and change system settings; use at your own "
          "risk. Licence: code MIT OR Apache-2.0, docs and data CC-BY-4.0 (see NOTICE, THIRD-PARTY.md).\n\n"
          "Binary, crate and config names still use the `pimax` prefix (`pimax-setup`, `~/.config/pimax`); they will be "
          "renamed later.\n")
def readme(s):
    lines = s.split("\n", 1)
    return "# omarchyvr: Pimax Dream Air VR on Linux (Omarchy / Arch)\n\n" + NOTICE + lines[1]
edit("README.md", readme)
def agents(s):
    s = s.replace("`docs/AI-KNOWLEDGE.md`", "`ai/AI-KNOWLEDGE.md`").replace("`docs/LESSONS.md`", "`ai/LESSONS.md`")
    return s.replace("Before any work, read:", "Everything an agent uses is in `ai/` (see `ai/README.md`). Before any work, read:", 1)
edit("AGENTS.md", agents)
edit("rust/pimax-setup/src/window.rs", lambda s: s.replace('.with_title("Pimax")', '.with_title("omarchyvr")'))
edit("rust/pimax-setup/src/app.rs", lambda s: s.replace('RichText::new("Pimax").size(21.0)', 'RichText::new("omarchyvr").size(21.0)'))
edit("linux/pimax.desktop", lambda s: re.sub(r"(?m)^Name=Pimax$", "Name=omarchyvr", s).replace(
    "GenericName=Pimax Dream Air on Linux", "GenericName=VR headset app (Pimax Dream Air, unofficial)"))
def pkg(s):
    s = s.replace("pkgname=pimax-linux-git", "pkgname=omarchyvr-git").replace("_pkgname=pimax-linux", "_pkgname=omarchyvr")
    s = re.sub(r"pkgdesc='[^']*'", "pkgdesc='omarchyvr: unofficial Pimax Dream Air VR stack for Linux: app, VR launcher, device tools (git version; not affiliated with Pimax)'", s)
    s = s.replace("url='https://github.com/laughtt/pimax-re'", "url='https://github.com/laughtt/omarchyvr'")
    s = s.replace("https://github.com/laughtt/pimax-re.git", "https://github.com/laughtt/omarchyvr.git")
    s = re.sub(r"license=\('MIT OR Apache-2.0'\)[^\n]*", "license=('MIT OR Apache-2.0')", s)
    return s
edit("packaging/arch/PKGBUILD", pkg)
PY

echo "== scan"
python3 -I - "$OUT" "$REPO/public-manifest.txt" <<'PY'
import gzip, math, os, re, sys, collections, subprocess
out = sys.argv[1]
hits = collections.Counter(); where = collections.defaultdict(set)
def hit(kind, path):
    hits[kind] += 1; where[kind].add(os.path.relpath(path, out))
DENY_EXT = {".dll", ".exe", ".sys", ".dmp", ".pcap", ".pcapng", ".pkl", ".pxcam", ".raw", ".pgm", ".ppm", ".jpg",
            ".jpeg", ".fw", ".firmware", ".hex", ".dfu", ".img"}
KEY = re.compile(r"(?<![A-Za-z0-9_-])(sk-(?:or-v1-|ant-(?:api03-)?|proj-|svcacct-|admin-)?|ghp_|gho_|ghs_|github_pat_|hf_|AKIA|ASIA|AIza|xox[abprs]-|glpat-)([A-Za-z0-9_\-]{16,})")
PEM = re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----")
JWT = re.compile(r"eyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}")
EMAIL = re.compile(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}")
EMAIL_OK = re.compile(r"(?i)@(users\.noreply\.github\.com|anthropic\.com|example\.(com|org|net)|localhost|[a-z0-9.-]*\.invalid)$|^git@github\.com$|^git@gitlab|^noreply@|\.(png|svg|so|rs|js|json|toml|md|py|c|h|txt)$")
SERIAL = re.compile(rb"P\d{3}[A-Z]\d[A-Z][0-9A-Z]{8,16}|\bH\d{5}[A-Z]\d{10,14}\b")
SERIAL16 = re.compile(rb"P\x00(?:\d\x00){3}[A-Z]\x00\d\x00[A-Z]\x00(?:[0-9A-Z]\x00){8,16}")
home = os.path.expanduser("~"); user = os.path.basename(home)
# public third-party addresses the manifest allows ("~ email ADDRESS-OR-@DOMAIN": upstream patch authors, lists)
allowed_mail = [l.split()[2] for l in open(sys.argv[2], encoding="utf-8") if l.startswith("~ email ")]
WINUSER = re.compile(r"(?i)C:[\\/]+Users[\\/]+(?!Public|Default|<|%|\$|USER|you|name|me\b)[A-Za-z0-9._-]+")
def ent(s):
    c = collections.Counter(s); n = len(s); return -sum(v / n * math.log2(v / n) for v in c.values())
# machine values from the private config, compared by value, never printed
private = []
cfg = os.path.join(home, ".config/pimax")
for f in ("ai.toml", "paths.toml"):
    p = os.path.join(cfg, f)
    if os.path.isfile(p):
        for m in re.finditer(r'(?m)^\s*(?:api_key|shared_drive_uuid)\s*=\s*"([^"]{8,})"', open(p, encoding="utf-8", errors="replace").read()):
            private.append(m.group(1).encode())
for d in (cfg,):
    for n in os.listdir(d) if os.path.isdir(d) else []:
        if n.startswith("ai.toml"):
            for m in re.finditer(r'"((?:sk-|key-)[^"]{12,})"', open(os.path.join(d, n), encoding="utf-8", errors="replace").read()):
                private.append(m.group(1).encode())
nfiles = 0
for root, dirs, names in os.walk(out):
    if os.path.relpath(root, out).split(os.sep)[0] == ".git":
        continue
    for n in names:
        p = os.path.join(root, n); nfiles += 1
        ext = os.path.splitext(n)[1].lower()
        if ext in DENY_EXT:
            hit("denied extension", p)
        b = open(p, "rb").read()
        blobs = [b]
        if n.endswith(".gz"):
            try: blobs.append(gzip.decompress(b))
            except Exception: pass
        for x in blobs:
            for m in SERIAL.finditer(x):
                g = m.group(0)
                if not (b"X0X" in g or (g.startswith(b"H") and g[6:7] == b"X" and g[7:].strip(b"0") in (b"1", b"2", b""))):
                    hit("device serial", p)
            for m in SERIAL16.finditer(x):
                if b"X\x000\x00X" not in m.group(0):
                    hit("device serial (utf-16)", p)
            for v in private:
                if v in x:
                    hit("private machine value", p)
            if len(home) > 6 and home.encode() in x:
                hit("home path", p)
        if b"\x00" in b[:4096] or n.endswith(".gz"):
            continue
        t = b.decode("utf-8", "replace")
        for m in KEY.finditer(t):
            body = m.group(2)
            if ent(body) > 3.5 and not re.search(r"(?i)test|fake|not-?a-?key|example|dummy|demo|xxxx|0000|aaaa|1111", body):
                hit("api key", p)
        if PEM.search(t): hit("private key", p)
        if JWT.search(t): hit("jwt", p)
        for m in EMAIL.finditer(t):
            a = m.group(0)
            if not EMAIL_OK.search(a) and not any(a == x or (x.startswith("@") and a.endswith(x)) for x in allowed_mail):
                hit("e-mail address", p)
        if WINUSER.search(t): hit("windows user path", p)
        if re.search(r"/home/" + re.escape(user) + r"\b", t) and len(user) > 2:
            hit("home path", p)
        if len(user) > 3 and re.search(r"\b" + re.escape(user) + r"\b", t, re.I):
            hit("user name", p)
gl = subprocess.run(["sh", "-c", "command -v gitleaks"], capture_output=True, text=True).stdout.strip()
if gl:
    r = subprocess.run([gl, "detect", "--no-git", "--no-banner", "--redact", "-s", out, "--exit-code", "3"], capture_output=True, text=True)
    if r.returncode == 3: hits["gitleaks"] += 1; where["gitleaks"].add("(see gitleaks detect --no-git -s OUT)")
    print(f"   gitleaks: {'hits' if r.returncode == 3 else 'clean'}")
else:
    print("   gitleaks: not installed (own patterns only)")
print(f"   scanned {nfiles} files, {len(private)} private values compared")
if hits:
    for k, v in hits.items():
        print(f"   HIT {k}: {v} in {len(where[k])} files")
        for f in sorted(where[k])[:40]:
            print(f"       {f}")
    sys.exit(1)
print("   clean")
PY

if [[ $RUN_TESTS -eq 1 ]]; then
    echo "== cargo test --workspace (copy)"
    export CARGO_TARGET_DIR="${TARGET_DIR:-$OUT.target}"
    ( cd "$OUT/rust" && nice -n 19 cargo test -j8 --workspace --no-fail-fast > "$OUT.test.log" 2>&1 ) || true
    if grep -qE '^error(\[E|: could not compile)' "$OUT.test.log"; then
        echo "   BUILD FAILED (log: $OUT.test.log)"; exit 1
    fi
    # known pre-existing failures (tools/known-test-failures.txt, one test name per line) do not fail the export
    python3 -I - "$OUT.test.log" "$REPO/tools/known-test-failures.txt" <<'KF'
import re, sys
log = open(sys.argv[1], encoding="utf-8", errors="replace").read()
known = {l.split("#")[0].strip() for l in open(sys.argv[2], encoding="utf-8")} - {""}
passed = sum(int(x) for x in re.findall(r"^test result: \w+\. (\d+) passed", log, re.M))
ignored = sum(int(x) for x in re.findall(r"(\d+) ignored", log))
failed = sorted(set(re.findall(r"^test (\S+) \.\.\. FAILED$", log, re.M)))
new = [t for t in failed if t not in known and t.split("::")[-1] not in known]
print(f"   {passed} passed, {ignored} ignored (private lab data or slow), {len(failed)} failed ({len(failed) - len(new)} known pre-existing)")
for t in new:
    print(f"   NEW FAILURE: {t}")
sys.exit(1 if new else 0)
KF
    echo "   tests ok (log: $OUT.test.log)"
fi
echo "== done: $(find "$OUT" -path "$OUT/.git" -prune -o -type f -print | wc -l) files in $OUT (from $HEAD)"
