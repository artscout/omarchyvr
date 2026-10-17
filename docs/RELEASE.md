# Making a release

Maintainer side (the command line is fine here). Users never build: they download `omarchyvr-<version>-x86_64.tar.zst`
from GitHub Releases, unpack it and run `./install.sh` ([INSTALL.md](INSTALL.md)).

## What a release is

One tarball with a finished build, made in a clean Arch Linux container from a committed commit and the public stand-ins
only (no `tools/lab-data.sh link`, no `.claude/`, the eye start list is the public BASE list).

    omarchyvr-0.1.0/
      install.sh uninstall.sh installer/        the same installer; MANIFEST.txt next to it selects the prebuilt mode
      MANIFEST.txt                              version, commit, Monado/WayVR fork commits, markers, bundled libraries, sha256 of every file
      bin/                                      the 30 omarchyvr binaries (OVR_BINS)
      lib/omarchyvr/monado/                     monado-service, libopenxr_monado.so, libmonado.so*, steamvr-monado/ (the SteamVR driver),
                                                CMakeCache.txt (path-free options copy for the app's preflight): the build-tree layout, fewer files
      lib/omarchyvr/{basalt,onnxruntime,sherpa-onnx,wayvr}/   libbasalt, ONNX Runtime, speech library, WayVR + libopenvr_api.so
      lib/omarchyvr/deps/                       bundled libraries (OpenCV, tbb, fmt, dc1394, ...)
      share/omarchyvr/data/                     what install.sh copies to $DATA (docs, udev rule, NVIDIA patches, device descriptors)
      README-INSTALL.txt BUILD-PACKAGES.txt     for the user; the builder's package list

Monado's `openxr_monado-dev.json` (absolute paths) is NOT in the tarball: `install.sh` writes it for the user's prefix.

## Steps

1. Commit everything; tag it (`git tag v0.1.0`). The submodules (`forks/monado`, `forks/wayvr`) must point at pushed commits
   of the public forks: the container clones them.
2. Build the builder image once (the build context is the repository root; the package lists come from `installer/lib.sh`):

       docker build -f packaging/release/Dockerfile -t omarchyvr-release-builder .
       # reproducible base: --build-arg ARCH_TAG=base-devel-YYYYMMDD.0.NNNNNN

3. Build the release (needs network: crates, Basalt, ONNX Runtime, the speech wheel; about 30-60 minutes, 40+ GB free in
   the container; nothing from your home directory is mounted except the repository, read only):

       mkdir -p dist
       docker run --rm -v "$PWD":/src:ro -v "$PWD/dist":/out omarchyvr-release-builder \
           /src/packaging/release/build-release.sh --version 0.1.0 --ref v0.1.0

   It clones the ref, runs `installer/install.sh --stage-only` (the same build steps and Monado CMake option check as a user's
   from-source install), keeps the runtime files, strips, bundles libraries, writes `MANIFEST.txt`, makes the tarball and
   runs `verify-release.sh`. If a check fails there is no tarball.
4. Verify again, on the tarball you will upload, in the same container (the leak scan needs the clean clone's `tools/`):

       docker run --rm -v "$PWD":/src:ro -v "$PWD/dist":/out omarchyvr-release-builder \
           /src/packaging/release/verify-release.sh /out/omarchyvr-0.1.0-x86_64.tar.zst /src

   Then, on an Omarchy test PC (between VR sessions): unpack in a path you like, `./install.sh`, check the app's Ready
   screen and Play. This is the only check that proves the bundled libraries work with the real GPU stack.
5. Upload:

       gh release create v0.1.0 dist/omarchyvr-0.1.0-x86_64.tar.zst dist/omarchyvr-0.1.0-x86_64.tar.zst.sha256 \
           dist/omarchyvr-0.1.0-MANIFEST.txt --title "omarchyvr 0.1.0" --notes-file docs/releases/v0.1.0.md

## What `verify-release.sh` checks (fail = no release)

| Check | How |
|---|---|
| Download integrity | the `.sha256` matches; `MANIFEST.txt` matches every file and link, and no file is missing from it |
| Monado features | every `OVR_MARKERS` string is in `monado-service` after strip (`grep -ac`) |
| Relocation | the tarball is unpacked to a path the build never saw; `bundle-libs.py check` runs `ldd` on every ELF: no "not found", every `NEEDED` library is in the core list or inside the tree, every RUNPATH entry is `$ORIGIN`-relative |
| No build paths | the builder's home directory is in no file (strings of all files, binaries too); build directories are listed as warnings |
| No private data | `scan-tree.py` runs the checks of `tools/leak-scan.py` over the whole tree: serials, keys, e-mail addresses, Windows paths, eye-chip bytes, per-device calibration (only the shipped Dream Air reference passes); no `*.onnx`/`*.gguf` or hand-model files |
| Layout | all 30 binaries, `libopenxr_monado.so`, `libmonado.so*`, the SteamVR driver, Basalt, WayVR, `PIMAX_CAM=ON` in the shipped cache copy |

## Which libraries are bundled

`bundle-libs.py` reads the `NEEDED` entries of every ELF file in the tree:

- **core** (left to the system, installed by `OVR_RUNTIME_PKGS` in `installer/lib.sh`): glibc, libstdc++, libgcc_s, Vulkan loader,
  Wayland, X11 and xcb, xkbcommon, SDL2, GStreamer, glib, dbus, systemd and udev, libusb, hidapi, drm, zlib, bluetooth,
  jpeg, bsd, PipeWire, ALSA, libinput, OpenXR loader, glvnd (GL/EGL), png, zstd, lz4, xz, ffi, pcre2, fontconfig, freetype, expat.
- **tree** (shipped as their own directories): ONNX Runtime, Basalt, the speech library, `libopenvr_api.so`.
- **bundled** (copied to `lib/omarchyvr/deps`, with their own closure): everything else the binaries need, for example the 13
  OpenCV libraries (soname `.500`, which changes), tbb, fmt, libdc1394, libepoxy, dav1d. The release build links Monado with
  `--as-needed` (Arch's default for packages), so OpenCV modules nothing uses (GUI, video) are not pulled in.

The list that was actually used (kind, soname, source) is in `MANIFEST.txt` under `[bundled-libs]`.

To move a library from bundled to core: add its soname pattern to `CORE` in `bundle-libs.py` and its package to
`OVR_RUNTIME_PKGS`. A core library whose soname changes needs a new release.

## How Monado finds Basalt

Monado loads Basalt through the VIT interface: `t_tracker_slam.cpp` reads the environment variable `VIT_SYSTEM_LIBRARY_PATH`
(default `libbasalt.so`). omarchyvr's launcher sets it to `~/.local/lib/omarchyvr/basalt/lib/libbasalt.so`
(`omarchyvr_config::paths::basalt_lib`, `omarchyvr vr sixdof`), so the prebuilt layout needs no fork change. Basalt's own
libraries are found by its RUNPATH (`$ORIGIN/../../deps`).

## Known limits

- Not signed. Check the `.sha256`; `MANIFEST.txt` has a checksum for every file.
- The container cannot test the GPU: bundled libraries are checked with `ldd`, not run against a graphics driver (step 4).
- x86_64 Arch/Omarchy only. The runtime packages are those of `OVR_RUNTIME_PKGS`.
