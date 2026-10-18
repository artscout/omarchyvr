#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# build-release.sh: build the prebuilt release tarball. Runs INSIDE the container of packaging/release/Dockerfile, as the
# unprivileged user "build" (HOME=/home/build). Maintainer side: docs/RELEASE.md.
#
#   build-release.sh --version 0.1.0 [--ref REF] [--repo /src] [--out /out] [--work /opt/ovr-build] [-j 8]
#
#   --repo   the mounted repository (read only is fine); it is cloned at REF with its submodules: nothing of the
#            working tree, no private lab data (.claude/) and no tools/lab-data.sh link ever reaches the build
#   --out    where omarchyvr-VERSION-x86_64.tar.zst and its .sha256 go
#
# Steps: clean clone -> installer/install.sh --stage-only (the SAME build steps and CMake option check as a user's
# from-source install, into a staging prefix) -> keep only runtime files -> strip -> bundle fragile libraries and make
# every RUNPATH $ORIGIN-relative (bundle-libs.py) -> MANIFEST.txt -> tar.zst -> verify-release.sh. A failed check
# deletes the tarball.
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
VERSION="" REF=HEAD REPO=/src OUT=/out WORK=/opt/ovr-build JOBS=8
while [[ $# -gt 0 ]]; do
	case $1 in
	--version) VERSION=${2:?}; shift ;;
	--ref) REF=${2:?}; shift ;;
	--repo) REPO=${2:?}; shift ;;
	--out) OUT=${2:?}; shift ;;
	--work) WORK=${2:?}; shift ;;
	-j) JOBS=${2:?}; shift ;;
	-h | --help) sed -n '3,17p' "$0"; exit 0 ;;
	*) echo "unknown option $1" >&2; exit 2 ;;
	esac
	shift
done
[[ -n $VERSION && $VERSION =~ ^[0-9][0-9A-Za-z._-]*$ ]] || { echo "--version X.Y.Z is required" >&2; exit 2; }
[[ $EUID -ne 0 ]] || { echo "run as the unprivileged build user, not root" >&2; exit 2; }
for t in git cmake ninja cargo patchelf zstd python3 strip readelf ldd; do
	command -v "$t" >/dev/null || { echo "missing tool in the container: $t" >&2; exit 2; }
done
say() { printf '==> %s\n' "$*"; }
die() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }

NAME=omarchyvr-$VERSION
CLONE=$WORK/src
STAGE=$WORK/stage
TREE=$WORK/tree/$NAME
mkdir -p "$OUT"
rm -rf "$WORK/src" "$WORK/stage" "$WORK/tree"
mkdir -p "$WORK/tree"

# ---- 1. a clean clone of the commit --------------------------------------------------------------------------------
say "1/8 Clean clone of $REF from $REPO"
git config --global --add safe.directory '*'
git clone --quiet --no-local "$REPO" "$CLONE"
git -C "$CLONE" -c advice.detachedHead=false checkout --quiet "$REF"
git -C "$CLONE" submodule update --init --recursive --quiet
COMMIT=$(git -C "$CLONE" rev-parse HEAD)
[[ -z $(git -C "$CLONE" status --porcelain) ]] || die "the clone is not clean"
[[ ! -e $CLONE/.claude && ! -e $CLONE/CLAUDE.local.md ]] || die "private files in the clone"
[[ -f $CLONE/packaging/release/bundle-libs.py ]] || die "$REF has no packaging/release"
# public stand-ins only: the eye frame list is the stand-in (no STEP lines) and the BASE list is the allowed one
! grep -q '^STEP ' "$CLONE/rust/pimax-eye/data/replay-frames.txt" || die "the eye frame list is not the public stand-in"
base_sha=$(sha256sum "$CLONE/rust/pimax-eye/data/base-frames.txt" | cut -d' ' -f1)
allowed=$(sed -n 's/^EYE_BASE_SHA256 = "\(.*\)"/\1/p' "$CLONE/tools/leak-scan.py")
[[ $base_sha == "$allowed" ]] || die "the eye BASE list differs from the one the leak scanner allows"
SOURCE_DATE_EPOCH=$(git -C "$CLONE" log -1 --format=%ct)
export SOURCE_DATE_EPOCH
MONADO_COMMIT=$(git -C "$CLONE/forks/monado" rev-parse HEAD)
WAYVR_COMMIT=$(git -C "$CLONE/forks/wayvr" rev-parse HEAD)
# shellcheck source=/dev/null
. "$CLONE/installer/components.lock"
# shellcheck source=/dev/null
. "$CLONE/installer/lib.sh"
# shellcheck source=manifest.sh
. "$CLONE/packaging/release/manifest.sh"

# ---- 2. build in a staging prefix with the installer's own steps --------------------------------------------------
say "2/8 Build (installer/install.sh --stage-only; Rust workspace, Basalt, Monado, WayVR, speech library)"
# no build path may reach a binary: Rust and C/C++ both remap the clone, the staging prefix and the home directory
export RUSTFLAGS="--remap-path-prefix=$WORK=/work --remap-path-prefix=$HOME=/build-home --remap-path-prefix=$CLONE=/src --remap-path-prefix=$STAGE=/stage"
MAP="-ffile-prefix-map=$WORK=/work -ffile-prefix-map=$CLONE=/src -ffile-prefix-map=$STAGE=/stage -ffile-prefix-map=$HOME=/build-home"
# --as-needed (Arch's own packaging default) keeps libraries nothing uses (OpenCV's GUI and video modules) out of
# NEEDED, so they are not bundled
export OVR_CMAKE_EXTRA="-DCMAKE_C_FLAGS=$MAP|-DCMAKE_CXX_FLAGS=$MAP|-DCMAKE_EXE_LINKER_FLAGS=-Wl,--as-needed|-DCMAKE_SHARED_LINKER_FLAGS=-Wl,--as-needed"
"$CLONE/installer/install.sh" --stage-only --prefix "$STAGE" --nvidia no -j "$JOBS" --no-ai 2>&1 | sed 's/^/    | /'
SERVICE=$STAGE/lib/omarchyvr/monado/src/xrt/targets/service/monado-service
[[ -x $SERVICE ]] || die "the stage has no monado-service"

# ---- 3. keep only what runs ----------------------------------------------------------------------------------------
say "3/8 Assemble $NAME (runtime files only)"
L=$STAGE/lib/omarchyvr
M=$L/monado
T=$TREE/lib/omarchyvr
mkdir -p "$TREE/bin" "$T/monado/src/xrt/targets/service" "$T/monado/src/xrt/targets/openxr" "$T/monado/src/xrt/targets/libmonado" \
	"$T/monado/src/xrt/drivers" "$T/basalt/lib" "$T/wayvr" "$T/sherpa-onnx/lib" "$TREE/share/omarchyvr"
for b in "${OVR_BINS[@]}"; do cp "$STAGE/bin/$b" "$TREE/bin/$b"; done
cp "$SERVICE" "$T/monado/src/xrt/targets/service/"
cp "$M/src/xrt/targets/openxr/libopenxr_monado.so" "$T/monado/src/xrt/targets/openxr/"
cp -a "$M"/src/xrt/targets/libmonado/libmonado.so* "$T/monado/src/xrt/targets/libmonado/"
[[ -x $M/src/xrt/drivers/pimax_ht_bench ]] && cp "$M/src/xrt/drivers/pimax_ht_bench" "$T/monado/src/xrt/drivers/" # the app's hand test
# the SteamVR driver tree: manifest, resources, driver_monado.so, pimax_rttime_preload.so
cp -a "$M/steamvr-monado" "$T/monado/steamvr-monado"
# the options the app's preflight reads (PIMAX_CAM, ...): a path-free copy of the cache
{
	echo "# omarchyvr release $VERSION: the Monado build options (filtered copy of CMakeCache.txt, no paths)"
	grep -E '^(PIMAX_[A-Z_]+|XRT_[A-Z_0-9]+|CMAKE_BUILD_TYPE|CMAKE_GENERATOR):[A-Z]+=[^/]*$' "$M/CMakeCache.txt" || true
} >"$T/monado/CMakeCache.txt"
grep -q '^PIMAX_CAM:BOOL=ON$' "$T/monado/CMakeCache.txt" || die "PIMAX_CAM is not ON in the Monado cache"
cp -a "$L"/basalt/lib/libbasalt.so* "$T/basalt/lib/"
[[ -f $L/basalt/COPYING ]] && cp "$L/basalt/COPYING" "$T/basalt/" || true
cp -a "$L/onnxruntime" "$T/onnxruntime"
rm -rf "$T/onnxruntime/include" "$T/onnxruntime/lib/cmake" "$T/onnxruntime/lib/pkgconfig"
cp -a "$L"/sherpa-onnx/lib/*.so "$T/sherpa-onnx/lib/"
# patchelf: install.sh sets the library paths of the installed files with it (installer/runpath.py), so the release
# needs no pacman dependency for it. A separate GPL-3.0 program (THIRD-PARTY.md): licence text and version travel along.
mkdir -p "$T/tools"
cp "$(command -v patchelf)" "$T/tools/patchelf"
{
	echo "patchelf $(patchelf --version | awk '{print $NF}') (https://github.com/NixOS/patchelf), GPL-3.0-or-later, Arch package binary, unmodified."
	echo "Shipped only so that install.sh can set the library paths (RUNPATH) of the installed files. Source: the URL above, the tag of this version."
	echo
	cat /usr/share/licenses/patchelf/COPYING 2>/dev/null || echo "(licence text: https://www.gnu.org/licenses/gpl-3.0.txt)"
} >"$T/tools/PATCHELF-LICENSE.txt"
cp "$L/wayvr/wayvr" "$L/wayvr/wayvrctl" "$T/wayvr/"
[[ -f $L/wayvr/libopenvr_api.so ]] && cp "$L/wayvr/libopenvr_api.so" "$T/wayvr/" || die "no libopenvr_api.so for WayVR"
cp -a "$STAGE/share/omarchyvr/data" "$TREE/share/omarchyvr/data"
# the installer and what it reads from the tree root (the same relative paths as in a clone)
cp "$CLONE/install.sh" "$CLONE/uninstall.sh" "$TREE/"
cp -a "$CLONE/installer" "$TREE/installer"
mkdir -p "$TREE/linux" "$TREE/packaging" "$TREE/tools" "$TREE/patches"
for f in omarchyvr.desktop omarchyvr.svg omarchyvr-keepalived.service; do cp "$CLONE/linux/$f" "$TREE/linux/"; done
cp -a "$CLONE/packaging/systemd" "$TREE/packaging/systemd"
cp -a "$CLONE/patches/nvidia" "$TREE/patches/nvidia"
cp "$CLONE/tools/vr-busy.sh" "$TREE/tools/"
cp "$CLONE"/LICENSE-* "$CLONE/NOTICE" "$CLONE/THIRD-PARTY.md" "$TREE/"
cp "$CLONE/packaging/release/README-INSTALL.txt" "$TREE/README-INSTALL.txt"
pacman -Q >"$TREE/BUILD-PACKAGES.txt"
# nothing of a build tree may be left (this tree is what the installer verifies)
find "$TREE" \( -name '*.a' -o -name '*.o' -o -name CMakeFiles -o -name '*.cmake' -o -name '*.ovr-new' \) -print -exec rm -rf {} + | sed 's/^/    removed /' || true

# ---- 4. strip (the Monado marker strings are data, they stay) ------------------------------------------------------
say "4/8 Strip"
while IFS= read -r -d '' f; do
	if [[ $(head -c4 "$f" | od -An -c | tr -d ' \n') == '177ELF' ]]; then strip --strip-unneeded "$f" || true; fi
done < <(find "$TREE" -type f -print0)
for m in "${OVR_MARKERS[@]}"; do
	[[ $(grep -ac -- "$m" "$T/monado/src/xrt/targets/service/monado-service" || true) -gt 0 ]] || die "marker '$m' lost by strip"
done

# ---- 5. bundle fragile libraries, relocatable RUNPATHs -------------------------------------------------------------
say "5/8 Bundle libraries and set RUNPATH to \$ORIGIN"
python3 -I "$CLONE/packaging/release/bundle-libs.py" bundle "$TREE" --report "$WORK/bundled-libs.tsv" || die "bundle-libs failed"
python3 -I "$CLONE/packaging/release/bundle-libs.py" check "$TREE" || die "the tree does not resolve its libraries"

# ---- 6. manifest ---------------------------------------------------------------------------------------------------
say "6/8 MANIFEST.txt"
{
	echo "version: $VERSION"
	echo "commit: $COMMIT"
	echo "built: $(date -u -d "@$SOURCE_DATE_EPOCH" +%FT%TZ) (commit time)"
	echo "monado-fork: $MONADO_COMMIT"
	echo "wayvr-fork: $WAYVR_COMMIT"
	echo "basalt: $BASALT_COMMIT"
	echo "onnxruntime: $ORT_VERSION"
	echo "sherpa-onnx: $SHERPA_PIP"
	echo "rust: $(rustc --version)"
	echo "builder: $(. /etc/os-release && echo "$PRETTY_NAME"), packages sha256 $(pacman -Q | sha256sum | cut -c1-16) (BUILD-PACKAGES.txt)"
	echo "arch: x86_64"
} >"$WORK/header.txt"
for m in "${OVR_MARKERS[@]}"; do echo "$m: $(grep -ac -- "$m" "$T/monado/src/xrt/targets/service/monado-service")"; done >"$WORK/markers.txt"
write_manifest "$TREE" "$WORK/header.txt" "$WORK/markers.txt" "$WORK/bundled-libs.tsv"
cp "$TREE/MANIFEST.txt" "$OUT/$NAME-MANIFEST.txt"

# ---- 7. tarball ----------------------------------------------------------------------------------------------------
say "7/8 $NAME-x86_64.tar.zst"
TARBALL=$OUT/$NAME-x86_64.tar.zst
rm -f "$TARBALL" "$TARBALL.sha256"
tar --sort=name --owner=0 --group=0 --numeric-owner --mtime="@$SOURCE_DATE_EPOCH" -C "$WORK/tree" -cf - "$NAME" |
	zstd -19 -T0 -q -o "$TARBALL"
(cd "$OUT" && sha256sum "$(basename "$TARBALL")" >"$(basename "$TARBALL").sha256")

# ---- 8. verify (a failure removes the tarball) ---------------------------------------------------------------------
say "8/8 Verify"
if ! OVR_FORBID="$HOME" OVR_WARN_PATHS="$WORK" "$HERE/verify-release.sh" "$TARBALL" "$CLONE"; then
	rm -f "$TARBALL" "$TARBALL.sha256"
	die "verification failed: no release was produced"
fi
ls -l "$OUT"
say "done: $TARBALL ($(du -h "$TARBALL" | cut -f1))"
