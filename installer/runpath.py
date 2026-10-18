#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""runpath.py: make the RUNPATHs of an installed prebuilt release ABSOLUTE.

The release tarball is relocatable: every RUNPATH is made of $ORIGIN-relative directories
(packaging/release/bundle-libs.py). But the realtime permission (setcap cap_sys_nice on monado-service) makes the kernel
set AT_SECURE for that process, and glibc then ignores $ORIGIN entries (and LD_LIBRARY_PATH) for the whole process,
including every library it loads later (libbasalt.so, libopenxr_monado.so, ...). An absolute DT_RUNPATH is honoured in
secure mode. So install.sh rewrites the RUNPATH of every installed ELF to the absolute, normalised path inside its prefix.

  runpath.py plan   RELEASE PREFIX PATCHELF OVERLAY
      For every regular ELF under RELEASE/bin and RELEASE/lib/omarchyvr whose RUNPATH has $ORIGIN: copy it to
      OVERLAY/<same relative path> and set the absolute RUNPATH there (the release tree itself is never changed: its
      MANIFEST.txt stays true). Prints "relative path TAB old TAB new" per file. install.sh installs the OVERLAY copy in
      place of the release file, and compares it with the installed one (same bytes: the installed file, and its file
      capability, stay).
  runpath.py check  RELEASE PREFIX PATCHELF
      Every ELF of RELEASE that is installed under PREFIX: no $ORIGIN in its RUNPATH and every entry is an existing
      directory. Exit 1 otherwise.
  runpath.py absolute DIR RUNPATH PREFIX   print the rewritten RUNPATH of a file in DIR (tests)
"""
import os, shutil, subprocess, sys

ROOTS = ("bin", "lib/omarchyvr")
ORIGINS = ("$ORIGIN", "${ORIGIN}")


def is_elf(p):
    try:
        with open(p, "rb") as f:
            return f.read(4) == b"\x7fELF" and not os.path.islink(p)
    except OSError:
        return False


def elfs(root, sub):
    base = os.path.join(root, sub)
    for d, _, fs in os.walk(base):
        for f in sorted(fs):
            p = os.path.join(d, f)
            if is_elf(p):
                yield p


def get_rpath(patchelf, p):
    r = subprocess.run([patchelf, "--print-rpath", p], capture_output=True, text=True)
    if r.returncode != 0:
        return ""
    return r.stdout.strip()


def has_origin(rp):
    return any(o in rp for o in ORIGINS)


def absolute(rel_dir, rpath, prefix):
    """rel_dir: directory of the file relative to the release/prefix root; rpath: the $ORIGIN-relative RUNPATH."""
    here = os.path.join(prefix, rel_dir)
    out = []
    for e in rpath.split(":"):
        if not e:
            continue
        for o in ORIGINS:
            if e == o or e.startswith(o + "/"):
                e = here + e[len(o):]
                break
        e = os.path.normpath(e)
        if e not in out:
            out.append(e)
    return ":".join(out)


def plan(release, prefix, patchelf, overlay):
    release, prefix = os.path.realpath(release), os.path.normpath(prefix)
    n = 0
    for sub in ROOTS:
        for p in elfs(release, sub):
            old = get_rpath(patchelf, p)
            if not has_origin(old):
                continue
            rel = os.path.relpath(p, release)
            new = absolute(os.path.dirname(rel), old, prefix)
            dst = os.path.join(overlay, rel)
            os.makedirs(os.path.dirname(dst), exist_ok=True)
            shutil.copy2(p, dst)
            r = subprocess.run([patchelf, "--set-rpath", new, dst], capture_output=True, text=True)
            if r.returncode != 0:
                print(f"ERROR patchelf failed on {rel}: {r.stderr.strip()}", file=sys.stderr)
                return 1
            print(f"{rel}\t{old}\t{new}")
            n += 1
    print(f"runpath: {n} ELF files rewritten to absolute RUNPATHs", file=sys.stderr)
    return 0


def check(release, prefix, patchelf):
    release, prefix = os.path.realpath(release), os.path.normpath(prefix)
    bad = n = 0
    for sub in ROOTS:
        for p in elfs(release, sub):
            rel = os.path.relpath(p, release)
            q = os.path.join(prefix, rel)
            if not is_elf(q):
                print(f"FAIL {rel}: not installed under {prefix}")
                bad += 1
                continue
            n += 1
            rp = get_rpath(patchelf, q)
            if has_origin(rp):
                print(f"FAIL {rel}: installed RUNPATH still has $ORIGIN ({rp})")
                bad += 1
            for e in rp.split(":"):
                if e and not os.path.isdir(e):
                    print(f"FAIL {rel}: RUNPATH entry {e} does not exist")
                    bad += 1
    print(f"runpath check: {n} installed ELF files, " + ("FAILED" if bad else "all RUNPATHs absolute and existing"), file=sys.stderr)
    return 1 if bad else 0


def main(a):
    if len(a) == 5 and a[0] == "plan":
        return plan(a[1], a[2], a[3], a[4])
    if len(a) == 4 and a[0] == "check":
        return check(a[1], a[2], a[3])
    if len(a) == 4 and a[0] == "absolute":
        print(absolute(a[1], a[2], a[3]))
        return 0
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
