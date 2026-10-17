#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# omarchyvr installer: the only command a user types after `git clone --recursive` (./install.sh at the repo root).
#
#   ./install.sh [options]
#
# 1. checks the machine (distro, compositor, GPU and driver, Secure Boot, disk, packages, headset), read-only;
# 2. builds everything at nice 19 with 8 jobs: the Rust workspace, Monado (forks/monado, every feature in one build),
#    WayVR (forks/wayvr), Basalt and ONNX Runtime (pinned), the speech library; nothing is fetched that is already on
#    disk with the right checksum (AI models, voice models, backgrounds);
# 3. installs the set together into ~/.local/{bin,lib/omarchyvr,share/omarchyvr} and writes the install manifest
#    (every file with its checksum, and the originals it replaced) that ./uninstall.sh reverses;
# 4. prepares the ONE root job (udev rule, NVIDIA patches on a tested driver, EDID override, the realtime permission)
#    and opens the app: the app finishes the setup by itself (checks, user fixes, the root job with ONE password prompt,
#    checks again, restart if needed). Nothing asks for a password in this terminal.
#
# Options:
#   --prefix DIR        install into DIR instead of ~/.local (a staging install: no user config, units, autostart,
#                       OpenXR/SteamVR registration or root job; they are printed as WOULD lines)
#   --no-root           never run the root job (the app's wizard runs it later)
#   --dry-run-root      print exactly what the root job would do, change nothing as root
#   --nvidia auto|yes|no  patch the NVIDIA driver: auto = only on a tested version with a supported headset plugged
#                       in (default); yes = also without the headset (opt-in); no = never
#   --nvidia-allow-untested  also patch an NVIDIA driver version that is not in patches/nvidia/tested-versions.txt
#                       (the app's switch "Allow the patch on untested NVIDIA drivers"; off by default)
#   --lab-data DIR      private lab data (the eye cold-start list) to link before the build (default: found
#                       automatically in .claude/lab/repo of this repo or ~/omarchyvr)
#   --gpu-speech        also build whisper.cpp with Vulkan (for the 'Speech on graphics card' switch; off by default)
#   --hand-models       fetch the Mercury hand-tracking models from Monado's public server (no licence: personal use only;
#                       the same download as the app's Tracking > Download hand models button)
#   --no-ai             skip the local AI runtime and models
#   --no-downloads      use only what is on disk (no network apart from git submodules already cloned)
#   --install-packages  install missing build packages with pacman right away (one extra password prompt, only on a
#                       machine that lacks them)
#   --no-app            do not open the app at the end
#   -h, --help
set -euo pipefail

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# shellcheck source=lib.sh
. "$REPO/installer/lib.sh"
# shellcheck source=components.lock
. "$REPO/installer/components.lock"
# the tested NVIDIA driver versions live in one place: patches/nvidia/tested-versions.txt (the patch script reads it too)
if [[ -r $REPO/patches/nvidia/tested-versions.txt ]]; then
	NVIDIA_TESTED_VERSIONS=$(sed 's/#.*//' "$REPO/patches/nvidia/tested-versions.txt" | tr -s ' \t\n' ' ' | sed 's/^ //; s/ $//')
fi

PREFIX="$HOME/.local" NO_ROOT=0 DRY_ROOT=0 NVIDIA_MODE=auto LAB_DATA="" GPU_SPEECH=0 HAND_MODELS=0 NO_AI=0 NV_ALLOW_UNTESTED=no
NO_DL=0 INSTALL_PKGS=0 OPEN_APP=1 JOBS=8 DRY=0
while [[ $# -gt 0 ]]; do
	case $1 in
	--prefix) PREFIX=$(realpath -m "${2:?}"); shift ;;
	--no-root) NO_ROOT=1 ;;
	--dry-run-root) DRY_ROOT=1 ;;
	--nvidia) NVIDIA_MODE=${2:?}; shift ;;
	--nvidia-allow-untested) NV_ALLOW_UNTESTED=yes ;;
	--lab-data) LAB_DATA=$(realpath -m "${2:?}"); shift ;;
	--gpu-speech) GPU_SPEECH=1 ;;
	--hand-models) HAND_MODELS=1 ;;
	--no-ai) NO_AI=1 ;;
	--no-downloads) NO_DL=1 ;;
	--install-packages) INSTALL_PKGS=1 ;;
	--no-app) OPEN_APP=0 ;;
	-j) JOBS=${2:?}; shift ;;
	-h | --help) sed -n '3,38p' "$0"; exit 0 ;;
	*) die "unknown option $1 (see --help)" ;;
	esac
	shift
done
[[ $NVIDIA_MODE =~ ^(auto|yes|no)$ ]] || die "--nvidia auto|yes|no"
[[ $EUID -ne 0 ]] || die "run install.sh as your user, not as root: the root steps run once through the app's password prompt"

STAGING=0
[[ "$PREFIX" == "$HOME/.local" ]] || STAGING=1
BIN="$PREFIX/bin"
LIBDIR="$PREFIX/lib/omarchyvr"
SHARE="$PREFIX/share/omarchyvr"
DATA="$SHARE/data"
CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/omarchyvr/build"
[[ $STAGING == 1 ]] && CACHE="$PREFIX/.build"
CONF="${XDG_CONFIG_HOME:-$HOME/.config}"
CARGO_TARGET="$CACHE/cargo"
MONADO_BUILD="$LIBDIR/monado"
SERVICE="$MONADO_BUILD/src/xrt/targets/service/monado-service"
LOG="$CACHE/install.log"
# an install from before the rename / the manifest (lab names: pimax-setup, pimax-keepalived) must be removed first:
# two keepalive services would fight over the headset
if [[ $STAGING == 0 ]]; then
	for l in "$CONF/systemd/user/pimax-keepalived.service" "$HOME/.local/bin/pimax-setup" "$HOME/.local/bin/pimaxd"; do
		[[ -e $l || -L $l ]] && die "an older install is present ($l): run ./uninstall.sh first (it keeps your downloads), then ./install.sh"
	done
	# downloads kept from an older install live under the old data dir: move them once (as the app's migration does)
	if [[ -d "$HOME/.local/share/pimax" && ! -L "$HOME/.local/share/pimax" && ! -e "$SHARE" ]]; then
		mv "$HOME/.local/share/pimax" "$SHARE"
		mkdir -p "$HOME/.local/state/omarchyvr/logs"
		echo "$(date -u +%FT%TZ) moved ~/.local/share/pimax -> ~/.local/share/omarchyvr (install.sh)" >>"$HOME/.local/state/omarchyvr/logs/migration.log"
		note "kept downloads moved: ~/.local/share/pimax -> $SHARE"
	fi
fi
mkdir -p "$CACHE" "$SHARE"
manifest_init

say "omarchyvr install from $REPO ($(git -C "$REPO" describe --always --dirty 2>/dev/null || echo '?'))"
note "prefix $PREFIX$([[ $STAGING == 1 ]] && echo ' (staging: no user config, no root job)'), build dir $CACHE"

# ==== 1. requirements (read only) ===================================================================================
say "1/7 Checking this computer"
TIER=tested
REQ_FAIL=()
if is_arch; then
	if is_omarchy; then note "distro: Omarchy (Arch) - tested"; else note "distro: Arch - experimental (tested on Omarchy)"; TIER=experimental; fi
else
	REQ_FAIL+=("distro $(os_id): unsupported (Arch Linux / Omarchy only; the packages and pacman hooks are Arch's)")
fi
COMP=$(compositor)
case $COMP in
Hyprland) note "compositor: Hyprland - tested" ;;
unknown) note "compositor: none detected (fine for a build; VR needs a Wayland compositor with DRM lease)" ;;
*) note "compositor: $COMP - experimental (needs wp_drm_lease_device_v1)"; TIER=experimental ;;
esac
GPUS=$(gpu_vendors)
NV_VER="" NV_PLAN=skip NV_WHY=""
if [[ " $GPUS " == *" nvidia "* ]]; then
	NV_VER=$(nvidia_pkg_version)
	if [[ -z $NV_VER ]]; then
		NV_WHY="NVIDIA GPU without the $NVIDIA_PACKAGE package (the proprietary or another flavour): no patches. The headset needs the open modules."
	elif ! nvidia_is_open; then
		NV_WHY="the loaded NVIDIA module is not the open flavour: no patches"
	elif [[ $NVIDIA_MODE == no ]]; then
		NV_WHY="NVIDIA patches turned off (--nvidia no)"
	elif [[ " $NVIDIA_TESTED_VERSIONS " != *" $NV_VER "* ]]; then
		# not tested: the root job installs the hooks and the choice, but patches only if the user allows it
		# (app: setup screen > NVIDIA driver patch > "Allow the patch on untested NVIDIA drivers")
		NV_PLAN=patch
		NV_WHY="NVIDIA $NV_VER is NOT a tested version ($NVIDIA_TESTED_VERSIONS): the patch is applied only if you allow it in the app (off by default). Please send a report from the app (Help > Report a problem) so support can be added."
	else
		NV_PLAN=patch
		NV_WHY="NVIDIA $NV_VER open modules: tested"
	fi
	note "GPU: NVIDIA, driver ${NV_VER:-?} (loaded $(nvidia_loaded_version || echo '?')): $NV_WHY"
elif [[ -n $GPUS ]]; then
	note "GPU: $GPUS- experimental (no driver patches; untested with the Dream Air)"
	TIER=experimental
else
	note "GPU: none found in /sys/class/drm"
fi
SB=$(secure_boot)
note "Secure Boot: $SB"
if [[ $SB == on && $NV_PLAN == patch ]]; then
	if command -v mokutil >/dev/null 2>&1 && mokutil --test-key /var/lib/dkms/mok.pub 2>/dev/null | grep -q 'already enrolled'; then
		note "  dkms signing key is enrolled: the patched module will be signed like the stock one"
	else
		NV_PLAN=skip
		NV_WHY="Secure Boot is on and the dkms signing key is not known to be enrolled (mokutil): no patch, the module could not load"
		warn "$NV_WHY"
	fi
fi
HS=$(headset_state)
case $HS in
tested) note "headset: Pimax Dream Air ($HEADSET_TESTED) - tested" ;;
untested) note "headset: a Pimax device that is not tested: $(pimax_usb | tr '\n' ';') - read-only probe only, label 'untested'" ;;
absent) note "headset: not plugged in (fine; the display steps that need it are skipped)" ;;
esac
if [[ $NV_PLAN == patch && $HS != tested && $NVIDIA_MODE != yes ]]; then
	NV_PLAN=skip
	NV_WHY="no supported headset plugged in: the NVIDIA patch is not needed (plug it in and run the setup again, or --nvidia yes)"
	note "  $NV_WHY"
fi
EDID_KEY=""
if [[ $NV_PLAN == patch ]] && CONNECTOR=$(headset_connector); then
	read -r PCI DRM_CONN NV_CONN <<<"$CONNECTOR"
	EDID_KEY="override.[$PCI].$NV_CONN"
	note "headset display: $DRM_CONN on $PCI -> EDID override key $EDID_KEY (detected on this PC)"
elif [[ $NV_PLAN == patch && $HS == tested ]]; then
	# chicken and egg (2026-10-17): the headset shows its display only after the keepalive wakes it, and the keepalive
	# can open the headset only once the root job has written the udev rule. So the root job looks for the connector
	# itself, right after the rule is in.
	EDID_KEY=auto
	note "headset display: not visible yet (the headset sleeps until the access rule is installed)"
	note "  the connector is found during the system setup, after the headset access rule wakes it (up to 30 s)"
elif [[ $NV_PLAN == patch ]]; then
	note "headset display: not found (no EDID with the Pimax id): the EDID override step is skipped"
fi
FREE_GB=$(df -BG --output=avail "$(dirname "$CACHE")" | tail -1 | tr -dc 0-9)
note "disk: ${FREE_GB} GB free for the build (needs about 40 GB)"
[[ ${FREE_GB:-0} -ge 40 ]] || REQ_FAIL+=("not enough disk space: ${FREE_GB} GB free, 40 GB needed")
[[ -f "$REPO/forks/monado/CMakeLists.txt" && -f "$REPO/forks/wayvr/Cargo.toml" ]] ||
	REQ_FAIL+=("the forks are missing: clone with --recursive (or: git submodule update --init --recursive)")

BUILD_PKGS=(git base-devel cmake ninja rust clang pkgconf python patch eigen opencv onetbb boost boost-libs fmt lz4
	vulkan-headers vulkan-icd-loader glslang shaderc hidapi libusb sdl2-compat gstreamer gst-plugins-base libjpeg-turbo
	libbsd bluez-libs wayland wayland-protocols libdrm libxkbcommon libx11 libxcb libxrandr openxr pipewire dbus
	systemd-libs alsa-lib fontconfig libepoxy qt6-base)
RUN_PKGS=(polkit)
[[ $HAND_MODELS == 1 ]] && BUILD_PKGS+=(git-lfs)
MISSING_BUILD=$(pacman -T "${BUILD_PKGS[@]}" 2>/dev/null | tr '\n' ' ' || true)
# rehearsals only (staging): OVR_ALLOW_MISSING="boost" builds without those packages and skips what needs them
SKIP_BASALT=0
if [[ $STAGING == 1 && -n ${OVR_ALLOW_MISSING:-} ]]; then
	for a in $OVR_ALLOW_MISSING; do
		if [[ " $MISSING_BUILD " == *" $a "* ]]; then
			MISSING_BUILD=${MISSING_BUILD//$a/}
			warn "rehearsal: package $a is missing and allowed (OVR_ALLOW_MISSING)"
			[[ $a == boost ]] && SKIP_BASALT=1
		fi
	done
fi
MISSING_RUN=$(pacman -T "${RUN_PKGS[@]}" 2>/dev/null | tr '\n' ' ' || true)
if [[ -n ${MISSING_BUILD// /} ]]; then
	if [[ $INSTALL_PKGS == 1 && $STAGING == 0 ]]; then
		say "Installing the missing build packages (one password prompt): $MISSING_BUILD"
		# shellcheck disable=SC2086
		pkexec /usr/bin/pacman -S --needed --noconfirm $MISSING_BUILD || die "pacman failed"
	else
		REQ_FAIL+=("missing build packages: $MISSING_BUILD (rerun with --install-packages, or install them with pacman)")
	fi
fi
pacman -Q steam >/dev/null 2>&1 || note "Steam is not installed: install it (multilib) and SteamVR in Steam to play games; the desk works without"
[[ -d "$HOME/.local/share/Steam/steamapps/common/SteamVR" ]] || note "SteamVR is not installed in Steam yet (Play needs it)"
if [[ ${#REQ_FAIL[@]} -gt 0 ]]; then
	for r in "${REQ_FAIL[@]}"; do warn "$r"; done
	die "requirements not met (nothing was changed)"
fi
note "result: $TIER"

# never replace a running set (ai/CLAUDE.md: install only between sessions)
RUNNING=""
if [[ $STAGING == 0 ]]; then
	for p in monado-service vrserver wayvr; do pgrep -x "$p" >/dev/null 2>&1 && RUNNING+="$p "; done
fi

# ==== 2. sources: lab data, pinned upstreams =========================================================================
say "2/7 Sources"
if [[ -z $LAB_DATA ]]; then
	for c in "${PIMAX_LAB_DATA:-}" "$REPO/.claude/lab/repo" "$HOME/omarchyvr/.claude/lab/repo"; do
		[[ -n $c && -d $c ]] && { LAB_DATA=$c; break; }
	done
fi
if [[ -n $LAB_DATA && -d $LAB_DATA ]]; then
	note "lab data: $LAB_DATA (the eye cold-start list; linked into the tree before the build)"
	"$REPO/tools/lab-data.sh" link "$LAB_DATA" | sed 's/^/    /'
else
	warn "no lab data: the eye tracker's cold-start list is the empty public stand-in (eye tracking will not start). Pass --lab-data DIR."
fi
mkdir -p "$CACHE/src"
fetch_git() { # fetch_git URL COMMIT DIR [--recursive]
	local url=$1 commit=$2 dir=$3
	if [[ -d $dir/.git ]] && [[ "$(git -C "$dir" rev-parse HEAD 2>/dev/null)" == "$commit" ]]; then
		note "$dir at ${commit:0:9} (reused)"
	else
		[[ $NO_DL == 1 ]] && die "$dir missing and --no-downloads given"
		rm -rf "$dir"
		git clone --quiet --filter=blob:none "$url" "$dir"
		git -C "$dir" -c advice.detachedHead=false checkout --quiet "$commit"
	fi
	[[ ${4:-} == --recursive ]] && git -C "$dir" submodule update --init --recursive --quiet
	return 0
}
fetch_file() { # fetch_file URL SHA DEST [REUSE_CANDIDATE...]
	local url=$1 sum=$2 dst=$3 c
	shift 3
	if file_ok "$dst" "$sum"; then note "$(basename "$dst"): present, checksum ok (reused)"; return 0; fi
	for c in "$@"; do
		if file_ok "$c" "$sum"; then mkdir -p "$(dirname "$dst")"; cp "$c" "$dst"; note "$(basename "$dst"): copied from $c (checksum ok)"; return 0; fi
	done
	[[ $NO_DL == 1 ]] && die "$dst missing and --no-downloads given"
	mkdir -p "$(dirname "$dst")"
	curl -fL --retry 3 -o "$dst.part" "$url"
	file_ok "$dst.part" "$sum" || { rm -f "$dst.part"; die "checksum mismatch for $url (refused)"; }
	mv -f "$dst.part" "$dst"
	note "$(basename "$dst"): downloaded, checksum ok"
}
fetch_file "$ORT_URL" "$ORT_SHA256" "$CACHE/dl/onnxruntime-$ORT_VERSION.tgz" "$HOME/$ORT_REUSE"
[[ $SKIP_BASALT == 1 ]] || fetch_git "$BASALT_URL" "$BASALT_COMMIT" "$CACHE/src/basalt" --recursive
[[ $GPU_SPEECH == 1 ]] && fetch_git "$WHISPER_URL" "$WHISPER_COMMIT" "$CACHE/src/whisper.cpp"

# ==== 3. build (nice 19, -j8) ========================================================================================
say "3/7 Building (nice 19, $JOBS jobs; log: $LOG)"
: >"$LOG"
build() { local what=$1; shift; note "$what"; if ! heavy "$@" >>"$LOG" 2>&1; then tail -30 "$LOG" >&2; die "$what failed (full log: $LOG)"; fi; }
export CARGO_TARGET_DIR="$CARGO_TARGET"
HASH=$(git -C "$REPO" rev-parse --short=9 HEAD)
build "Rust workspace (every binary, and the static libraries Monado links)" \
	env PIMAX_GIT_HASH="$HASH" cargo build --manifest-path "$REPO/rust/Cargo.toml" --release --locked --workspace -j "$JOBS"
REL="$CARGO_TARGET/release"
for b in "${OVR_BINS[@]}"; do [[ -x "$REL/$b" ]] || die "the Rust build did not produce $b"; done
for a in libpimax_capi.a libpimax_cam.a libpimax_hands.a libpimax_lens.a; do [[ -f "$REL/$a" ]] || die "missing $REL/$a"; done

ORT_DIR="$LIBDIR/onnxruntime"
if [[ ! -f "$ORT_DIR/lib/libonnxruntime.so.$ORT_VERSION" ]]; then
	mkdir -p "$ORT_DIR"
	tar -xzf "$CACHE/dl/onnxruntime-$ORT_VERSION.tgz" -C "$ORT_DIR" --strip-components=1
fi
record dir "$ORT_DIR" REBUILD "ONNX Runtime $ORT_VERSION"

BASALT_PREFIX="$LIBDIR/basalt"
if [[ $SKIP_BASALT == 1 ]]; then
	warn "Basalt: skipped in this rehearsal (boost headers missing on this PC)"
elif [[ ! -f "$BASALT_PREFIX/lib/libbasalt.so" ]] || [[ "$(cat "$BASALT_PREFIX/.commit" 2>/dev/null)" != "$BASALT_COMMIT" ]]; then
	build "Basalt ${BASALT_COMMIT:0:9} (6DoF; the longest step)" cmake -S "$CACHE/src/basalt" -B "$CACHE/basalt-build" -G Ninja \
		-DCMAKE_BUILD_TYPE=RelWithDebInfo -DCMAKE_INSTALL_PREFIX="$BASALT_PREFIX" -DBASALT_BUILD_SHARED_LIBRARY_ONLY=ON
	build "Basalt (compile)" ninja -C "$CACHE/basalt-build" -j "$JOBS"
	build "Basalt (install)" ninja -C "$CACHE/basalt-build" install
	echo "$BASALT_COMMIT" >"$BASALT_PREFIX/.commit"
fi
[[ $SKIP_BASALT == 1 ]] || record dir "$BASALT_PREFIX" REBUILD "Basalt ${BASALT_COMMIT:0:9}"

# Monado: built in its installed place, so the realtime permission set once stays (a rebuild without changes does
# not relink; a relink happens only on an update, whose root job sets it again)
build "Monado (forks/monado, configure)" cmake -S "$REPO/forks/monado" -B "$MONADO_BUILD" -G Ninja \
	-DCMAKE_BUILD_TYPE=RelWithDebInfo \
	-DXRT_BUILD_DRIVER_PIMAX=ON -DPIMAX_CAM=ON -DPIMAX_USE_RUST_LIBPIMAX=ON -DXRT_HAVE_SYSTEM_CJSON=OFF \
	-DPIMAX_LIBPIMAX_DIR="$REPO/libpimax" -DPIMAX_RUST_TARGET_DIR="$REL" \
	-DPIMAX_CAM_DIR="$REPO/rust/pimax-cam" -DPIMAX_EYE_DIR="$REPO/rust/pimax-eye" -DPIMAX_HANDS_DIR="$REPO/rust/pimax-hands" \
	-DPIMAX_LENS_DIR="$REPO/rust/pimax-lens" \
	-DONNXRuntime_ROOT_DIR="$ORT_DIR" -DONNXRuntime_INCLUDE_DIR="$ORT_DIR/include" -DONNXRuntime_LIBRARY="$ORT_DIR/lib/libonnxruntime.so"
# the fresh cache must match the tested build's options (installer/monado-cache.expect)
BAD=0
while IFS= read -r l; do
	[[ -z $l || $l == \#* ]] && continue
	must=0; [[ $l == !* ]] && { must=1; l=${l#!}; }
	k=${l%%=*} v=${l#*=}
	got=$(sed -n "s/^$k:[A-Z]*=//p" "$MONADO_BUILD/CMakeCache.txt")  # BOOL, or UNINITIALIZED for an option_with_deps whose deps are absent
	if [[ "$got" != "$v" ]]; then
		if [[ $must == 1 ]]; then warn "Monado option $k=$got (tested build: $v)"; BAD=1; else note "Monado option $k=$got (tested: $v)"; fi
	fi
done <"$REPO/installer/monado-cache.expect"
[[ $BAD == 0 ]] || die "the Monado configuration lacks a required feature (see above); missing package?"
build "Monado (compile)" ninja -C "$MONADO_BUILD" -j "$JOBS"
for m in "${OVR_MARKERS[@]}"; do
	[[ $(grep -ac -- "$m" "$SERVICE" || true) -gt 0 ]] || die "Monado marker '$m' missing from $SERVICE: refusing this build"
done
note "Monado markers: all ${#OVR_MARKERS[@]} present"
[[ -f "$MONADO_BUILD/steamvr-monado/driver.vrdrivermanifest" ]] || die "Monado built without the SteamVR driver"
record dir "$MONADO_BUILD" REBUILD "Monado (forks/monado $(git -C "$REPO/forks/monado" rev-parse --short HEAD))"

build "WayVR (forks/wayvr; features $WAYVR_FEATURES)" env CARGO_TARGET_DIR="$CACHE/wayvr-target" \
	cargo build --manifest-path "$REPO/forks/wayvr/Cargo.toml" --release --locked -j "$JOBS" $([[ $WAYVR_FEATURES == default ]] || echo --no-default-features --features "$WAYVR_FEATURES")

SHERPA_DIR="$LIBDIR/sherpa-onnx/lib"
if ! file_ok "$SHERPA_DIR/libsherpa-onnx-c-api.so" "$SHERPA_CAPI_SHA256"; then
	mkdir -p "$SHERPA_DIR"
	reused=0
	for c in $HOME/$SHERPA_REUSE; do
		if file_ok "$c/libsherpa-onnx-c-api.so" "$SHERPA_CAPI_SHA256"; then cp -a "$c"/*.so "$SHERPA_DIR/"; reused=1; note "speech library: copied from $c"; break; fi
	done
	if [[ $reused == 0 ]]; then
		[[ $NO_DL == 1 ]] && die "speech library missing and --no-downloads given"
		build "speech library ($SHERPA_PIP, in a private venv)" python3 -m venv "$CACHE/venv-sherpa"
		build "speech library (pip)" "$CACHE/venv-sherpa/bin/pip" install --quiet "$SHERPA_PIP"
		cp -a "$CACHE"/venv-sherpa/lib/python3*/site-packages/sherpa_onnx/lib/*.so "$SHERPA_DIR/"
		file_ok "$SHERPA_DIR/libsherpa-onnx-c-api.so" "$SHERPA_CAPI_SHA256" || warn "speech library checksum differs from the tested one (a newer wheel); kept"
	fi
fi
record dir "$LIBDIR/sherpa-onnx" REBUILD "sherpa-onnx C API"

if [[ $GPU_SPEECH == 1 ]]; then
	build "whisper.cpp (Vulkan, server only)" cmake -S "$CACHE/src/whisper.cpp" -B "$LIBDIR/whisper.cpp" -G Ninja -DCMAKE_BUILD_TYPE=Release \
		-DGGML_VULKAN=ON -DBUILD_SHARED_LIBS=ON -DWHISPER_BUILD_SERVER=ON -DWHISPER_BUILD_TESTS=OFF -DWHISPER_SDL2=OFF
	build "whisper.cpp (compile)" ninja -C "$LIBDIR/whisper.cpp" -j "$JOBS" whisper-server
	record dir "$LIBDIR/whisper.cpp" REBUILD "whisper.cpp ${WHISPER_COMMIT:0:9} (Vulkan)"
fi

# ==== 4. install the set (together) =================================================================================
say "4/7 Installing into $PREFIX"
if [[ -n $RUNNING ]]; then
	die "a VR session is running ($RUNNING): the set is built but not installed. Stop VR in the app and run ./install.sh again (it reuses the build)."
fi
mkdir -p "$BIN"
for b in "${OVR_BINS[@]}"; do put "$REL/$b" "$BIN/$b" 755 REBUILD "omarchyvr $HASH"; done
for b in wayvr wayvrctl; do put "$CACHE/wayvr-target/release/$b" "$LIBDIR/wayvr/$b" 755 REBUILD "WayVR $(git -C "$REPO/forks/wayvr" rev-parse --short HEAD)"; done
# the data the app and the root job read: docs, udev rule, EDID files, NVIDIA patches and hooks, installer
rm -rf "$DATA.new"
mkdir -p "$DATA.new"
(cd "$REPO" && git ls-files -z docs linux patches installer ai analysis/tools tools/lab-data.sh LICENSE-* NOTICE THIRD-PARTY.md README.md |
	xargs -0 cp --parents -a -t "$DATA.new")
git -C "$REPO" rev-parse HEAD >"$DATA.new/COMMIT"
rm -rf "$DATA"
mv "$DATA.new" "$DATA"
record dir "$DATA" REBUILD "repository data $HASH"

if [[ $STAGING == 1 ]]; then
	DRY=1
	say "staging install: the user-level registration below is printed, not done"
fi
# desktop entry, icon, autostart
APPS="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
ICONS="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/scalable/apps"
tmp=$(mktemp)
sed -e '/^#/d' -e "s|@PIMAX_SETUP@|$BIN/omarchyvr-app|g" -e "s|@OMARCHYVR_APP@|$BIN/omarchyvr-app|g" "$REPO/linux/omarchyvr.desktop" >"$tmp"
put "$tmp" "$APPS/omarchyvr.desktop" 644 RESET "app launcher"
put "$REPO/linux/omarchyvr.svg" "$ICONS/omarchyvr.svg" 644 RESET "app icon"
printf '[Desktop Entry]\nType=Application\nName=omarchyvr\nComment=Starts the omarchyvr app at login\nExec=sh -c "sleep 6; exec %s"\nX-GNOME-Autostart-enabled=true\n' "$BIN/omarchyvr-app" >"$tmp"
put "$tmp" "$CONF/autostart/omarchyvr-app.desktop" 644 RESET "app at login"
# user units (copies, never symlinks into the repo): keepalived and omarchyvrd enabled; omarchyvr-autod conflicts
# with omarchyvrd (same socket) and omarchyvr-ai is started by the app's AI switch, so both are installed, not enabled
for u in "${OVR_UNITS[@]}"; do
	src="$REPO/packaging/systemd/$u"
	[[ -f $src ]] || src="$REPO/linux/$u"
	[[ -f $src ]] || die "unit $u not found in the repo"
	sed -e "s|%h/.local/bin/|$BIN/|g" "$src" >"$tmp"
	put "$tmp" "$CONF/systemd/user/$u" 644 RESET "user unit"
done
rm -f "$tmp"
if [[ $DRY == 0 ]]; then
	systemctl --user daemon-reload
	for u in omarchyvr-keepalived.service omarchyvrd.service; do
		systemctl --user enable "$u" >/dev/null 2>&1 && record enabled "$u" RESET "enabled at login"
		systemctl --user start "$u" || warn "could not start $u now (it starts at the next login)"
	done
else
	note "WOULD: systemctl --user daemon-reload; enable --now omarchyvr-keepalived.service omarchyvrd.service"
fi
# OpenXR / SteamVR registration (Play = SteamVR with the Monado driver of this install)
if [[ $DRY == 0 ]]; then
	for f in "$HOME/.config/openxr/1/active_runtime.json" "$HOME/.config/openvr/openvrpaths.vrpath"; do
		record edit "$f" RESET "OpenXR/OpenVR registration" "$(backup_once "$f")"
	done
	"$BIN/omarchyvr" vr games steamvr --monado-build "$MONADO_BUILD" | sed 's/^/    /' || warn "SteamVR registration failed (the app's Play page shows why)"
	"$BIN/omarchyvr" vr steam-setup on | sed 's/^/    /' || warn "Steam VR setup failed (the app shows why)"
	record edit "$HOME/.local/share/applications/steam.desktop" RESET "Steam with the OpenXR import variable (steam-setup)"
	record file "$CONF/environment.d/90-omarchyvr-steam-vr.conf" RESET "session variable for Steam (steam-setup)"
else
	note "WOULD: omarchyvr vr games steamvr --monado-build $MONADO_BUILD; omarchyvr vr steam-setup on"
fi
DRY=0

# ==== 5. downloads (reused when present) ============================================================================
say "5/7 Downloads (reused when the checksum matches)"
if [[ $STAGING == 1 ]]; then
	note "staging: downloads are checked, not fetched"
	for f in "$PARAKEET_DIR/encoder.int8.onnx:$PARAKEET_ENCODER_SHA256" "$VAD_FILE:$VAD_SHA256"; do
		file_ok "$HOME/${f%%:*}" "${f##*:}" && note "~/${f%%:*}: present, checksum ok" || note "~/${f%%:*}: WOULD download"
	done
else
	if ! { file_ok "$HOME/$PARAKEET_DIR/encoder.int8.onnx" "$PARAKEET_ENCODER_SHA256" && file_ok "$HOME/$PARAKEET_DIR/decoder.int8.onnx" "$PARAKEET_DECODER_SHA256" && file_ok "$HOME/$PARAKEET_DIR/joiner.int8.onnx" "$PARAKEET_JOINER_SHA256"; }; then
		[[ $NO_DL == 1 ]] && die "speech model missing and --no-downloads given"
		note "speech model: downloading (source $PARAKEET_SOURCE: $PARAKEET_URL)"
		mkdir -p "$CACHE/dl" "$HOME/models"
		curl -fL --retry 3 -o "$CACHE/dl/parakeet.tar.bz2" "$PARAKEET_URL"
		tar -xjf "$CACHE/dl/parakeet.tar.bz2" -C "$CACHE/dl"
		d=$(find "$CACHE/dl" -maxdepth 1 -type d -name '*parakeet-tdt-0.6b-v3-int8*' | head -1)
		file_ok "$d/encoder.int8.onnx" "$PARAKEET_ENCODER_SHA256" || die "speech model checksum mismatch (refused)"
		rm -rf "${HOME:?}/$PARAKEET_DIR"
		mv "$d" "$HOME/$PARAKEET_DIR"
	else
		note "speech model ~/$PARAKEET_DIR: present, checksum ok"
	fi
	record data "$HOME/$PARAKEET_DIR" KEEP "speech model"
	fetch_file "$VAD_URL" "$VAD_SHA256" "$HOME/$VAD_FILE"
	record data "$HOME/$VAD_FILE" KEEP "voice activity model"
	if [[ $GPU_SPEECH == 1 ]]; then
		fetch_file "$WHISPER_MODEL_URL" "$WHISPER_MODEL_SHA256" "$HOME/$WHISPER_MODEL"
		record data "$HOME/$WHISPER_MODEL" KEEP "whisper model (GPU speech)"
	fi
	if [[ $NO_DL == 0 ]]; then
		heavy "$BIN/omarchyvr-backgrounds" download 2>&1 | tail -3 | sed 's/^/    /' || warn "backgrounds: download failed (retry from the app)"
	fi
	record data "${XDG_DATA_HOME:-$HOME/.local/share}/omarchyvr/backgrounds" KEEP "VR backgrounds"
	if [[ $NO_AI == 0 && $NO_DL == 0 ]]; then
		heavy "$BIN/omarchyvr-ai" install --two-stage --router lara 2>&1 | tail -5 | sed 's/^/    /' || warn "local AI: install failed (retry from the app's AI tab)"
	fi
	record data "${XDG_DATA_HOME:-$HOME/.local/share}/omarchyvr/ai" KEEP "local AI runtime and models"
	if [[ $HAND_MODELS == 1 ]]; then
		# the one implementation (also behind the app's "Download hand models" button); --hand-models is the consent
		note "Mercury hand models: NO licence, personal use only; fetched from Monado's public server, never redistributed"
		heavy "$BIN/omarchyvr" vr hand-models download --yes 2>&1 | sed 's/^/    /' || warn "hand models: download failed (retry from the app: Tracking > Download hand models)"
		record data "$HOME/$HANDMODELS_DIR" KEEP "Mercury hand models (no licence)"
	fi
fi

# ==== 6. the root job (one password, in the app's wizard) ============================================================
say "6/7 System setup (needs your password once)"
STEAMVR_LAUNCHER="$HOME/.local/share/Steam/steamapps/common/SteamVR/bin/linux64/vrcompositor-launcher"
[[ -f $STEAMVR_LAUNCHER ]] || STEAMVR_LAUNCHER=""
PLAN="$SHARE/root-plan.env"
{
	echo "# written by install.sh $(date -u +%FT%TZ)"
	echo "OVR_USER=$(id -un)"
	echo "OVR_HOME=$HOME"
	echo "OVR_DATA=$DATA"
	echo "OVR_SERVICE=$SERVICE"
	echo "OVR_PACKAGES=${MISSING_RUN% }"
	echo "OVR_NVIDIA=$NV_PLAN"
	echo "OVR_NVIDIA_VERSION=$NV_VER"
	echo "OVR_NVIDIA_TESTED=$NVIDIA_TESTED_VERSIONS"
	echo "OVR_NVIDIA_ALLOW_UNTESTED=$NV_ALLOW_UNTESTED"
	echo "OVR_EDID=$EDID_KEY"
	echo "OVR_STEAMVR_LAUNCHER=$STEAMVR_LAUNCHER"
	echo "OVR_SNAPSHOT=yes"
} >"$PLAN"
record file "$PLAN" RESET "root job plan"
"$DATA/installer/omarchyvr-root.sh" plan "$PLAN" | tee "$SHARE/root-plan.txt" | sed 's/^/    /'
record file "$SHARE/root-plan.txt" RESET "root job plan (text the wizard shows)"
ROOT_CMD="/usr/bin/pkexec $DATA/installer/omarchyvr-root.sh apply $PLAN"
if [[ $STAGING == 1 || $DRY_ROOT == 1 ]]; then
	note "the root job is only printed ($([[ $STAGING == 1 ]] && echo staging || echo --dry-run-root)): $ROOT_CMD"
elif [[ $NO_ROOT == 1 || $OPEN_APP == 1 ]]; then
	note "The app finishes the setup (one password): it shows this list while it runs it. No password is asked in this terminal."
fi

# ==== 7. done ======================================================================================================
say "7/7 Done: $(grep -vc '^#' "$SHARE/manifest.tsv") artefacts in $SHARE/manifest.tsv"
if [[ $STAGING == 0 && $OPEN_APP == 1 && -n ${WAYLAND_DISPLAY:-}${DISPLAY:-} ]]; then
	note "opening the app: it checks this PC, asks for the password once and finishes the setup by itself"
	setsid -f "$BIN/omarchyvr-app" >/dev/null 2>&1 </dev/null || true
fi
