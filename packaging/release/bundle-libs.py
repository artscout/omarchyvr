#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""bundle-libs.py: make a release tree relocatable.

  bundle-libs.py bundle ROOT [--deps REL] [--report FILE] [--dry-run]
      Walk every ELF under ROOT. Each NEEDED library is one of:
        core    stable on every Arch system (glibc, libstdc++, Vulkan, Wayland, X11, SDL2, GStreamer, glib, dbus,
                systemd, libusb, hidapi, drm, zlib, bluetooth, jpeg, bsd, ...): left to the system;
        tree    shipped inside ROOT already (onnxruntime, basalt, sherpa-onnx, libopenvr_api): found by RUNPATH;
        bundle  anything else (OpenCV, tbb, fmt, dc1394, dav1d, epoxy, ...: rolling sonames): copied into ROOT/DEPS
                together with its own NEEDED closure (core libs are never followed).
      Then every ELF gets a RUNPATH made only of $ORIGIN-relative directories (patchelf), so the tree runs from any
      path. Build paths in an old RUNPATH are dropped.
  bundle-libs.py check ROOT
      ldd every ELF: no "not found", every NEEDED library is core or inside ROOT, no absolute RUNPATH. Exit 1 otherwise.
  bundle-libs.py origin FILE DIR...   print the RUNPATH string for FILE needing DIR (tests)

The decision list goes to --report (it is part of MANIFEST.txt).
"""
import os, re, shutil, subprocess, sys

# sonames left to the system (regexes on the soname). Rolling or version-suffixed ones are NOT here: they are bundled.
CORE = [
    r"ld-linux-x86-64\.so\.2", r"lib(c|m|dl|pthread|rt|util|resolv|nsl|anl)\.so\.\d+", r"libgcc_s\.so\.1", r"libstdc\+\+\.so\.6",
    r"libvulkan\.so\.1", r"libwayland-(client|cursor|egl|server)\.so\.0",
    r"libX11\.so\.6", r"libX11-xcb\.so\.1", r"libX(ext|randr|render|fixes|i|cursor|inerama|ss|xf86vm|composite|damage)\.so\.\d+",
    r"libSM\.so\.6", r"libICE\.so\.6", r"libxcb[a-z0-9-]*\.so\.\d+", r"libxkbcommon(-x11)?\.so\.0",
    r"libSDL2-2\.0\.so\.0", r"libgst[a-z0-9]+-1\.0\.so\.0", r"lib(glib|gobject|gio|gmodule|gthread)-2\.0\.so\.0",
    r"libdbus-1\.so\.3", r"libsystemd\.so\.0", r"libudev\.so\.1", r"libusb-1\.0\.so\.0", r"libhidapi-(libusb|hidraw)\.so\.0",
    r"libdrm\.so\.2", r"libz\.so\.1", r"libbluetooth\.so\.3", r"libjpeg\.so\.8", r"libbsd\.so\.0",
    r"libpipewire-0\.3\.so\.0", r"libasound\.so\.2", r"libinput\.so\.10", r"libopenxr_loader\.so\.1",
    r"lib(GL|GLX|GLdispatch|OpenGL|EGL|GLESv2|gbm)\.so\.\d+",
    r"libpng16\.so\.16", r"libzstd\.so\.1", r"liblz4\.so\.1", r"liblzma\.so\.5", r"libffi\.so\.8", r"libpcre2-8\.so\.0",
    r"libfontconfig\.so\.1", r"libfreetype\.so\.6", r"libexpat\.so\.1",
]
CORE_RE = re.compile("^(?:" + "|".join(CORE) + ")$")
ELF = b"\x7fELF"
RP_RE = r"\((?:RUNPATH|RPATH)\)\s+Library (?:runpath|rpath): \[([^\]]*)\]"


def is_elf(p):
    try:
        with open(p, "rb") as f:
            return f.read(4) == ELF and not os.path.islink(p)
    except OSError:
        return False


def elfs(root):
    for d, _, fs in os.walk(root):
        for f in sorted(fs):
            p = os.path.join(d, f)
            if is_elf(p):
                yield p


def readelf_d(p):
    return subprocess.run(["readelf", "-d", p], capture_output=True, text=True).stdout


def needed(p):
    return re.findall(r"\(NEEDED\)\s+Shared library: \[([^\]]+)\]", readelf_d(p))


def system_map():
    out = subprocess.run(["ldconfig", "-p"], capture_output=True, text=True).stdout
    m = {}
    for l in out.splitlines():
        mm = re.match(r"\s*(\S+) \(libc6,x86-64.*\) => (\S+)", l)
        if mm:
            m.setdefault(mm.group(1), mm.group(2))
    # private library directories that ldconfig does not list (pulseaudio's libpulsecommon, ...)
    for d in ("/usr/lib/pulseaudio", "/usr/lib/x86_64-linux-gnu"):
        if os.path.isdir(d):
            for f in os.listdir(d):
                if ".so" in f and os.path.isfile(os.path.join(d, f)):
                    m.setdefault(f, os.path.join(d, f))
    return m


def origin(file, dirs):
    base = os.path.dirname(file)
    parts = []
    for d in dirs:
        r = os.path.relpath(d, base)
        s = "$ORIGIN" if r == "." else "$ORIGIN/" + r
        if s not in parts:
            parts.append(s)
    return ":".join(parts)


def tree_index(root):
    """library file name (links too) -> directories of the tree that hold it"""
    idx = {}
    for d, _, fs in os.walk(root):
        for f in fs:
            if ".so" in f:
                idx.setdefault(f, []).append(d)
    return idx


def bundle(root, deps_rel, report, dry):
    root = os.path.realpath(root)
    deps = os.path.join(root, deps_rel)
    sysmap = system_map()
    idx = tree_index(root)
    todo = list(elfs(root))
    seen = set(todo)
    need_dirs = {}  # file -> [dirs]
    decisions = {}  # soname -> (kind, detail)
    errors = []
    while todo:
        p = todo.pop()
        dirs = []
        for so in needed(p):
            own = os.path.join(os.path.dirname(p), so)
            d = None
            if os.path.exists(own):
                d = os.path.dirname(p)
                decisions.setdefault(so, ("tree", os.path.relpath(d, root)))
            elif CORE_RE.match(so):
                decisions.setdefault(so, ("core", "system"))
                continue
            elif so in idx:
                d = idx[so][0]
                decisions.setdefault(so, ("tree", os.path.relpath(d, root)))
            elif so in sysmap:
                d = deps
                dest = os.path.join(deps, so)
                if so not in decisions:
                    if not dry:
                        os.makedirs(deps, exist_ok=True)
                        shutil.copy2(os.path.realpath(sysmap[so]), dest)
                        os.chmod(dest, 0o755)
                        seen.add(dest)
                        todo.append(dest)
                    idx.setdefault(so, []).append(deps)
                    decisions[so] = ("bundle", sysmap[so])
            else:
                errors.append(f"{os.path.relpath(p, root)}: needs {so}: not core, not in the tree, not on this system")
                continue
            if d not in dirs:
                dirs.append(d)
        need_dirs[p] = dirs
    patched = 0
    for p, dirs in sorted(need_dirs.items()):
        want = origin(p, dirs)
        cur = re.findall(RP_RE, readelf_d(p))
        if (cur[0] if cur else "") == want:
            continue
        patched += 1
        if dry:
            continue
        if want:
            subprocess.run(["patchelf", "--set-rpath", want, p], check=True)
        else:
            subprocess.run(["patchelf", "--remove-rpath", p], check=True)
    lines = ["# kind <TAB> soname <TAB> source"]
    for so in sorted(decisions):
        k, d = decisions[so]
        lines.append(f"{k}\t{so}\t{d}")
    if report:
        with open(report, "w") as f:
            f.write("\n".join(lines) + "\n")
    nb = sum(1 for k, _ in decisions.values() if k == "bundle")
    print(f"bundle-libs: {len(need_dirs)} ELF files, {nb} libraries bundled, {patched} RUNPATHs set")
    for e in errors:
        print("ERROR " + e, file=sys.stderr)
    return 1 if errors else 0


def check(root):
    root = os.path.realpath(root)
    bad = 0
    for p in elfs(root):
        out = subprocess.run(["ldd", p], capture_output=True, text=True)
        text = out.stdout + out.stderr
        rel = os.path.relpath(p, root)
        rp = readelf_d(p)
        for m in re.finditer(RP_RE, rp):
            for part in m.group(1).split(":"):
                if part and not part.startswith("$ORIGIN"):
                    print(f"FAIL {rel}: absolute RUNPATH entry {part}")
                    bad += 1
        if "not a dynamic executable" in text or "statically linked" in text:
            continue
        for l in text.splitlines():
            if "not found" in l:
                print(f"FAIL {rel}: {l.strip()}")
                bad += 1
        resolved = {}
        for l in text.splitlines():
            m = re.match(r"\s*(\S+) => (\S+)", l)
            if m:
                resolved[m.group(1)] = os.path.realpath(m.group(2)) if m.group(2).startswith("/") else m.group(2)
        for so in needed(p):
            path = resolved.get(so)
            if path is None:
                if not CORE_RE.match(so):
                    print(f"FAIL {rel}: {so} not resolved")
                    bad += 1
                continue
            if path.startswith(root + "/"):
                continue
            if not CORE_RE.match(so):
                print(f"FAIL {rel}: {so} comes from the system ({path}) but is not in the core list")
                bad += 1
    print("bundle-libs check: " + ("FAILED" if bad else "ok"))
    return 1 if bad else 0


def main(a):
    if len(a) >= 2 and a[0] == "bundle":
        deps, report, dry = "lib/omarchyvr/deps", None, False
        i = 2
        while i < len(a):
            if a[i] == "--deps":
                deps = a[i + 1]
                i += 1
            elif a[i] == "--report":
                report = a[i + 1]
                i += 1
            elif a[i] == "--dry-run":
                dry = True
            i += 1
        return bundle(a[1], deps, report, dry)
    if len(a) == 2 and a[0] == "check":
        return check(a[1])
    if len(a) >= 3 and a[0] == "origin":
        print(origin(a[1], a[2:]))
        return 0
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
