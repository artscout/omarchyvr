#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# verify-release.sh: the checks a release tarball must pass (build-release.sh runs it last; a maintainer can run it again
# on a downloaded tarball). Exit 1 = do not publish.
#
#   verify-release.sh TARBALL [CLONE]
#
# CLONE (default: the repository this script is in) provides tools/leak-scan.py for the leak scan.
# Environment: OVR_FORBID="a:b"  strings that must appear in NO file (the builder's home directory);
#              OVR_WARN_PATHS="a:b"  build directories: hits are listed, not fatal.
#
# 1. the .sha256 next to the tarball matches; 2. the tarball is unpacked to a DIFFERENT, deeper path (relocation test);
# 3. MANIFEST.txt matches every file (the installer's own verify_release); 4. every Monado marker is in monado-service;
# 5. every ELF resolves its libraries: only the core system set from the system, the rest inside the tree, no absolute
# RUNPATH (bundle-libs.py check = ldd of every ELF); 6. no builder path, no private data, no secrets, no model files
# (scan-tree.py = tools/leak-scan.py over the tree); 7. layout: the 31 binaries, the runtime files the app expects.
set -uo pipefail
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
TARBALL=${1:?usage: verify-release.sh TARBALL [CLONE]}
CLONE=${2:-$(cd "$HERE/../.." && pwd)}
fails=0
: "${OVR_FORBID=$HOME}" # the builder's home directory must be in no file (set OVR_FORBID= to skip)
git config --global --add safe.directory '*' 2>/dev/null || true # a mounted clone belongs to another uid
ok() { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=$((fails + 1)); }

[[ -f $TARBALL ]] || { echo "no such file: $TARBALL" >&2; exit 2; }
if [[ -f $TARBALL.sha256 ]]; then
	if (cd "$(dirname "$TARBALL")" && sha256sum -c --quiet "$(basename "$TARBALL").sha256" >/dev/null 2>&1); then ok "sha256 file matches the tarball"; else bad "sha256 file does not match"; fi
else bad "no $TARBALL.sha256"; fi

W=$(mktemp -d "${TMPDIR:-/tmp}/ovr-verify.XXXXXX")
trap 'rm -rf "$W"' EXIT
D=$W/relocated/to/another/place/entirely
mkdir -p "$D"
zstd -dc "$TARBALL" | tar -xf - -C "$D" || { echo "cannot unpack $TARBALL" >&2; exit 1; }
R=$(find "$D" -mindepth 1 -maxdepth 1 -type d | head -1)
[[ -n $R && -f $R/MANIFEST.txt ]] || { echo "no MANIFEST.txt in the tarball" >&2; exit 1; }
ok "unpacked to $D (a path the build never saw)"

# shellcheck source=../../installer/lib.sh
. "$R/installer/lib.sh"
if verify_release "$R" 2>"$W/verify.err"; then ok "MANIFEST.txt matches every file and link"; else bad "MANIFEST.txt: $(head -3 "$W/verify.err" | tr '\n' ' ')"; fi

SERVICE=$R/lib/omarchyvr/monado/src/xrt/targets/service/monado-service
if [[ -x $SERVICE ]]; then
	miss=""
	for m in "${OVR_MARKERS[@]}"; do [[ $(grep -ac -- "$m" "$SERVICE" || true) -gt 0 ]] || miss+="$m, "; done
	[[ -z $miss ]] && ok "all ${#OVR_MARKERS[@]} Monado markers are in monado-service (after strip)" || bad "Monado markers missing: ${miss%, }"
else bad "no monado-service"; fi

if out=$(python3 -I "$HERE/bundle-libs.py" check "$R" 2>&1); then ok "every ELF resolves its libraries (core set from the system, the rest in the tree, RUNPATH relative)"; else bad "libraries: $(grep '^FAIL' <<<"$out" | head -5 | tr '\n' ';')"; fi

forbid=()
IFS=: read -ra fl <<<"${OVR_FORBID:-}"
for f in "${fl[@]}"; do [[ ${#f} -gt 5 ]] && forbid+=(--forbid "$f"); done
if out=$(python3 -I "$HERE/scan-tree.py" --repo "$CLONE" --root "$R" "${forbid[@]}" 2>&1); then ok "$(tail -1 <<<"$out")"; else bad "leak scan: $(tr '\n' ' ' <<<"$out" | cut -c1-600)"; fi
IFS=: read -ra wl <<<"${OVR_WARN_PATHS:-}"
for w in "${wl[@]}"; do
	[[ ${#w} -gt 5 ]] || continue
	hits=$(grep -rlaF -- "$w" "$R" 2>/dev/null | sed "s|^$R/||" | head -5 | tr '\n' ' ')
	[[ -z $hits ]] && ok "no build directory $w in any file" || echo "warn build directory $w appears in: $hits"
done

missing=""
for b in "${OVR_BINS[@]}"; do [[ -x $R/bin/$b ]] || missing+="bin/$b "; done
for f in lib/omarchyvr/monado/src/xrt/targets/openxr/libopenxr_monado.so lib/omarchyvr/monado/steamvr-monado/driver.vrdrivermanifest \
	lib/omarchyvr/monado/steamvr-monado/bin/linux64/driver_monado.so lib/omarchyvr/monado/steamvr-monado/bin/linux64/pimax_rttime_preload.so \
	lib/omarchyvr/monado/CMakeCache.txt lib/omarchyvr/basalt/lib/libbasalt.so lib/omarchyvr/sherpa-onnx/lib/libsherpa-onnx-c-api.so \
	lib/omarchyvr/wayvr/wayvr lib/omarchyvr/wayvr/wayvrctl lib/omarchyvr/wayvr/libopenvr_api.so lib/omarchyvr/tools/patchelf lib/omarchyvr/tools/PATCHELF-LICENSE.txt installer/runpath.py share/omarchyvr/data/COMMIT install.sh uninstall.sh; do
	[[ -e $R/$f ]] || missing+="$f "
done
ls "$R"/lib/omarchyvr/monado/src/xrt/targets/libmonado/libmonado.so.* >/dev/null 2>&1 || missing+="libmonado.so.* "
ls "$R"/lib/omarchyvr/onnxruntime/lib/libonnxruntime.so.* >/dev/null 2>&1 || missing+="libonnxruntime.so.* "
[[ -z $missing ]] && ok "layout: ${#OVR_BINS[@]} binaries and every runtime file" || bad "layout: missing $missing"
grep -q '^PIMAX_CAM:BOOL=ON$' "$R/lib/omarchyvr/monado/CMakeCache.txt" 2>/dev/null && ok "Monado options: PIMAX_CAM=ON in the shipped cache copy" || bad "PIMAX_CAM is not ON in the shipped cache copy"
if "$R/lib/omarchyvr/tools/patchelf" --version >/dev/null 2>&1; then ok "the bundled patchelf runs (the installer sets the library paths with it)"; else bad "lib/omarchyvr/tools/patchelf does not run"; fi
if "$R/bin/omarchyvr" --help >/dev/null 2>&1 || "$R/bin/omarchyvr" help >/dev/null 2>&1; then ok "bin/omarchyvr starts from the relocated tree"; else echo "warn bin/omarchyvr did not print help here"; fi

echo
if [[ $fails == 0 ]]; then echo "release verified"; else echo "$fails check(s) FAILED"; fi
[[ $fails == 0 ]]
