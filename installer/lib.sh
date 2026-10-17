# SPDX-License-Identifier: MIT OR Apache-2.0
# shellcheck shell=bash
# Shared helpers of install.sh / uninstall.sh (sourced, never run). User level only: nothing here elevates.

OVR_NAME=omarchyvr
# the set of user binaries one install puts into $BIN (the whole Rust workspace's binaries; installed together)
OVR_BINS=(
	omarchyvr omarchyvr-app omarchyvrd omarchyvr-autod omarchyvr-ai omarchyvr-anchor omarchyvr-backgrounds
	omarchyvr-desktest omarchyvr-doctor omarchyvr-health omarchyvr-passthrough omarchyvr-perf omarchyvr-record
	omarchyvr-room omarchyvr-roommap omarchyvr-slam omarchyvr-voice omarchyvr-voicebench omarchyvr-voice-hud
	pimax-cam pimax-ctrltrack pimax-eye pimax-eyecal pimax-ipd pimax-lens pimax-lenstest pimax-selfcal
	xr-gaze xr-handtest xr-pinchtest
)
# Monado feature markers the build must carry (ai/CLAUDE.md section 3; the app's preflight refuses builds without them)
OVR_MARKERS=(PIMAX_SLAM PIMAX_ANCHOR PIMAX_EYE PIMAX_CTRL_TRACK "held from the tracker" PIMAX_TOUCH_GRIP
	PIMAX_ANCHOR_OWNER PIMAX_CTRL_INHAND PIMAX_HANDS PIMAX_AE_EXTENDED PIMAX_CTRL_ANGLE PIMAX_ANCHOR_GLIDE)
# user units (installed as copies; see enable policy in install.sh)
OVR_UNITS=(omarchyvr-keepalived.service omarchyvrd.service omarchyvr-autod.service omarchyvr-ai.service)

say() { printf '==> %s\n' "$*"; }
note() { printf '    %s\n' "$*"; }
warn() { printf '!!  %s\n' "$*" >&2; }
die() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }

# heavy work: lowest CPU and I/O priority, 8 jobs (ai/CLAUDE.md section 4)
heavy() { nice -n 19 ionice -c3 "$@"; }

sha256() { sha256sum "$1" | cut -d' ' -f1; }

# file_ok PATH SHA256: the file exists and has this checksum
file_ok() { [[ -f "$1" ]] && [[ "$(sha256 "$1")" == "$2" ]]; }

# ---- manifest ------------------------------------------------------------------------------------------------------
# One line per artefact: kind <TAB> path <TAB> sha256 or - <TAB> backup path or - <TAB> class <TAB> note
# kind: file | dir | link | unit | enabled | edit | cap | data
# class: REBUILD (our build output), RESET (setting, calibration, registration), KEEP (download), SYS (root)
# uninstall.sh reverses it line by line (last line first).
manifest_init() {
	MANIFEST="$SHARE/manifest.tsv"
	mkdir -p "$SHARE"
	if [[ ! -f "$MANIFEST" ]]; then
		printf '# omarchyvr install manifest v1 (kind\tpath\tsha256\tbackup\tclass\tnote)\n' >"$MANIFEST"
	fi
}
# record KIND PATH CLASS [NOTE] [BACKUP]: adds or replaces the line of PATH
record() {
	local kind=$1 path=$2 class=$3 note=${4:-} backup=${5:--} sum=-
	[[ "$kind" == file && -f "$path" && ! -L "$path" ]] && sum=$(sha256 "$path")
	if [[ "$DRY" == 1 ]]; then
		printf 'WOULD record\t%s\t%s\t%s\n' "$kind" "$path" "$class"
		return 0
	fi
	local tmp="$MANIFEST.tmp"
	awk -F'\t' -v p="$path" -v k="$kind" '!($2 == p && $1 == k)' "$MANIFEST" >"$tmp" && mv -f "$tmp" "$MANIFEST"
	printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$kind" "$path" "$sum" "$backup" "$class" "$note" >>"$MANIFEST"
}

# backup_once PATH: keep the original of a file we are about to replace (once; the first original wins)
backup_once() {
	local p=$1 b
	[[ -e "$p" || -L "$p" ]] || { echo -; return 0; }
	b="$SHARE/backup${p}"
	if [[ "$DRY" != 1 && ! -e "$b" && ! -L "$b" ]]; then
		mkdir -p "$(dirname "$b")"
		cp -a "$p" "$b"
	fi
	echo "$b"
}

# put SRC DEST MODE CLASS [NOTE]: atomic install of one file (temp file + rename), recorded
put() {
	local src=$1 dst=$2 mode=$3 class=$4 note=${5:-} bk=-
	if [[ -f "$dst" ]] && cmp -s "$src" "$dst"; then
		record file "$dst" "$class" "$note"
		return 0
	fi
	if [[ -e "$dst" ]] && ! grep -qF "$dst" "$MANIFEST" 2>/dev/null; then
		bk=$(backup_once "$dst")
	fi
	if [[ "$DRY" == 1 ]]; then
		printf 'WOULD install\t%s -> %s\n' "$src" "$dst"
		return 0
	fi
	mkdir -p "$(dirname "$dst")"
	install -m "$mode" "$src" "$dst.ovr-new"
	mv -f "$dst.ovr-new" "$dst"
	record file "$dst" "$class" "$note" "$bk"
}

# ---- the realtime permission across a rebuild (2026-10-17) ----------------------------------------------------------
# Monado is built in its installed place. A link writes a NEW monado-service and the kernel drops the file capability
# (cap_sys_nice) that the one password set: every ./install.sh asked for the password again, even when the binary had
# not changed (a relink caused only by touched inputs). cap_keep_begin keeps the old file under a second name (a hard
# link: same inode, same capability); after the build cap_keep_end puts it back when the new binary has the same bytes.
# No root is needed for either step.
has_nice_cap() { getcap "$1" 2>/dev/null | grep -q cap_sys_nice; } # tests replace it (setcap needs root)
cap_keep_begin() { # cap_keep_begin SERVICE
	local s=$1
	rm -f -- "$s.cap-keep"
	# best effort: without the link the install goes on and the app asks for the permission once (never fatal)
	if [[ -f $s ]] && has_nice_cap "$s"; then ln -- "$s" "$s.cap-keep" 2>/dev/null || true; fi
	return 0
}
# cap_keep_end SERVICE: prints one word
#   unchanged  not relinked: the same file, the permission as before
#   kept       relinked to the same bytes: the old file (with its permission) is back in place
#   changed    a real update: the permission is gone and must be set again (one password, in the app)
#   none       the file had no permission before the build (first install, or it was lost earlier)
cap_keep_end() {
	local s=$1 k=$1.cap-keep
	if [[ ! -e $k ]]; then
		echo none
		return 0
	fi
	if [[ ! -e $s ]]; then
		mv -f -- "$k" "$s"
		echo unchanged
	elif [[ $(stat -c %i -- "$k") == "$(stat -c %i -- "$s")" ]]; then
		rm -f -- "$k"
		echo unchanged
	elif cmp -s -- "$k" "$s"; then
		# rename keeps the inode and its capability; the new mtime keeps ninja from relinking it next time (a time
		# change does not drop a file capability, only a write or a chown does)
		mv -f -- "$k" "$s"
		touch -c -- "$s"
		echo kept
	else
		rm -f -- "$k"
		echo changed
	fi
}

# ---- machine facts (read only) -------------------------------------------------------------------------------------
os_id() { (. /etc/os-release 2>/dev/null; echo "${ID:-unknown} ${ID_LIKE:-}"); }
is_arch() { [[ " $(os_id) " == *" arch "* ]]; }
is_omarchy() { [[ -d "$HOME/.local/share/omarchy" ]] || command -v omarchy-version >/dev/null 2>&1; }
compositor() {
	if [[ -n "${HYPRLAND_INSTANCE_SIGNATURE:-}" ]] || pgrep -x Hyprland >/dev/null 2>&1; then echo Hyprland
	elif pgrep -x kwin_wayland >/dev/null 2>&1; then echo KWin
	elif pgrep -x gnome-shell >/dev/null 2>&1; then echo GNOME
	elif pgrep -x sway >/dev/null 2>&1; then echo Sway
	else echo unknown; fi
}
# GPU vendors present (nvidia amd intel), from sysfs
gpu_vendors() {
	local v out=()
	for v in /sys/class/drm/card[0-9]/device/vendor; do
		[[ -r "$v" ]] || continue
		case "$(cat "$v")" in
		0x10de) out+=(nvidia) ;; 0x1002) out+=(amd) ;; 0x8086) out+=(intel) ;;
		esac
	done
	printf '%s\n' "${out[@]}" | sort -u | tr '\n' ' '
}
nvidia_pkg_version() { { pacman -Q "$NVIDIA_PACKAGE" 2>/dev/null || true; } | awk '{print $2}' | sed 's/-[0-9]*$//'; }
nvidia_loaded_version() { cat /sys/module/nvidia/version 2>/dev/null; }
# open kernel modules? (the open flavour reports "Dual MIT/GPL")
nvidia_is_open() { modinfo -F license nvidia 2>/dev/null | grep -q 'Dual MIT/GPL'; }
secure_boot() {
	local f
	f=$(ls /sys/firmware/efi/efivars/SecureBoot-* 2>/dev/null | head -1)
	[[ -n "$f" ]] || { echo unknown; return; }
	[[ "$(od -An -t u1 -j4 -N1 "$f" 2>/dev/null | tr -d ' ')" == 1 ]] && echo on || echo off
}
# USB root for the detection (tests point it at a fake sysfs)
usb_root() { echo "${OVR_USB_ROOT:-/sys/bus/usb/devices}"; }
# every USB device: lines "vid:pid name"
all_usb() {
	local d
	for d in "$(usb_root)"/*; do
		[[ -r "$d/idVendor" ]] || continue
		printf '%s:%s %s\n' "$(cat "$d/idVendor")" "$(cat "$d/idProduct" 2>/dev/null)" "$(cat "$d/product" 2>/dev/null || echo '?')"
	done | sort -u
}
# Pimax devices on USB: lines "vid:pid name"
pimax_usb() { all_usb | grep "^$HEADSET_VENDOR:" || true; }
# The sleeping signature of the tested Dream Air (2026-10-17): while it sleeps (no keepalive yet) 34a4:0044 is NOT on
# USB; only the control device 34a4:0043 together with the camera sensor 34a4:0014 and/or the eye chip 2104:0561.
# One rule, shared with the app (rust/omarchyvr-gui-core/src/install.rs headset_state_from_usb; same test vectors).
HEADSET_SLEEP_CTRL=34a4:0043
HEADSET_SLEEP_PEERS="34a4:0014 2104:0561"
headset_state() { # tested (awake) | asleep (tested, 0044 hidden) | untested | absent
	local l ids p
	l=$(pimax_usb)
	ids=$(all_usb | awk '{print $1}')
	if grep -qx "$HEADSET_TESTED" <<<"$ids"; then echo tested; return; fi
	if grep -qx "$HEADSET_SLEEP_CTRL" <<<"$ids"; then
		for p in $HEADSET_SLEEP_PEERS; do
			grep -qx "$p" <<<"$ids" && { echo asleep; return; }
		done
	fi
	[[ -z "$l" ]] && { echo absent; return; }
	echo untested
}
# headset_connector lives in headset-connector.sh (shared with the root job)
# shellcheck source=headset-connector.sh
. "$(dirname "${BASH_SOURCE[0]}")/headset-connector.sh"

# ---- packages ------------------------------------------------------------------------------------------------------
# BUILD packages: only ./install.sh --from-source (and the release container) needs them.
OVR_BUILD_PKGS=(git base-devel cmake ninja rust clang pkgconf python patch eigen opencv onetbb boost boost-libs fmt lz4
	vulkan-headers vulkan-icd-loader glslang shaderc hidapi libusb sdl2-compat gstreamer gst-plugins-base libjpeg-turbo
	libbsd bluez-libs wayland wayland-protocols libdrm libxkbcommon libx11 libxcb libxrandr openxr pipewire dbus
	systemd-libs alsa-lib fontconfig libepoxy qt6-base)
# RUNTIME packages: the libraries a PREBUILT release leaves to the system (the "core" list of
# packaging/release/bundle-libs.py: stable sonames). Everything else (OpenCV, tbb, fmt, ...) travels inside the release.
OVR_RUNTIME_PKGS=(glibc gcc-libs vulkan-icd-loader wayland libx11 libxext libxrandr libxrender libxfixes libxi libxcursor
	libxinerama libxss libxxf86vm libsm libice libxcb libxkbcommon libxkbcommon-x11 sdl2-compat gstreamer
	gst-plugins-base-libs glib2 dbus systemd-libs libusb hidapi libdrm zlib bluez-libs libjpeg-turbo libbsd pipewire
	alsa-lib libinput openxr libglvnd libpng zstd lz4 xz libffi pcre2 fontconfig freetype2 expat)

# ---- prebuilt release ----------------------------------------------------------------------------------------------
# A release tarball unpacks to a directory with MANIFEST.txt, bin/, lib/omarchyvr/ and this installer. Sections of
# MANIFEST.txt after the "key: value" header: [markers] [bundled-libs] [links] (path TAB target) [sha256] (sha256sum -c
# format, last). packaging/release/manifest.sh writes it; verify_release reads it.
release_mode() { # release_mode DIR: prebuilt | source
	if [[ -f "$1/MANIFEST.txt" && -d "$1/lib/omarchyvr" ]]; then echo prebuilt; else echo source; fi
}
manifest_field() { sed -n "s/^$2: //p" "$1/MANIFEST.txt" | head -1; }
# verify_release DIR: every listed file has its checksum, no unlisted file, every link points where it said. Prints
# the failures; returns 1 on any.
verify_release() {
	local dir=$1 m=$1/MANIFEST.txt tmp bad=0 out
	[[ -f $m ]] || { warn "no MANIFEST.txt in $dir"; return 1; }
	tmp=$(mktemp -d)
	sed -n '/^\[sha256\]$/,$p' "$m" | tail -n +2 >"$tmp/sums"
	[[ -s $tmp/sums ]] || { warn "MANIFEST.txt has no [sha256] list"; rm -rf "$tmp"; return 1; }
	if ! out=$(cd "$dir" && sha256sum -c --quiet "$tmp/sums" 2>&1); then
		warn "checksum mismatch:"
		printf '%s\n' "$out" | head -10 | sed 's/^/    /' >&2
		bad=1
	fi
	cut -c67- "$tmp/sums" | LC_ALL=C sort >"$tmp/listed"
	(cd "$dir" && find . -type f ! -path ./MANIFEST.txt | sed 's|^\./||' | LC_ALL=C sort) >"$tmp/present"
	if [[ -n $(LC_ALL=C comm -13 "$tmp/listed" "$tmp/present") ]]; then
		warn "files that are not in MANIFEST.txt:"
		LC_ALL=C comm -13 "$tmp/listed" "$tmp/present" | head -10 | sed 's/^/    /' >&2
		bad=1
	fi
	local p t
	while IFS=$'\t' read -r p t; do
		[[ -z $p || $p == \[* ]] && continue
		if ! [[ -L "$dir/$p" && "$(readlink "$dir/$p")" == "$t" ]]; then warn "link $p should point to $t"; bad=1; fi
	done < <(sed -n '/^\[links\]$/,/^\[sha256\]$/p' "$m")
	rm -rf "$tmp"
	return $bad
}
# install_tree SRC DST CLASS NOTE: copy a directory tree (files, links) into DST. A file with the same bytes stays as it
# is (its inode and its file capability are kept); changed files are replaced atomically; files of an older install
# that SRC does not have are removed. DST is recorded as one directory for the uninstaller.
install_tree() {
	local src=$1 dst=$2 class=$3 note=${4:-} rel t mode
	if [[ "$DRY" == 1 ]]; then printf 'WOULD install tree\t%s -> %s\n' "$src" "$dst"; return 0; fi
	mkdir -p "$dst"
	(cd "$src" && find . -mindepth 1 -type d -print0) | while IFS= read -r -d '' rel; do mkdir -p "$dst/$rel"; done
	while IFS= read -r -d '' rel; do
		rel=${rel#./}
		if [[ -L "$src/$rel" ]]; then
			t=$(readlink "$src/$rel")
			if [[ -L "$dst/$rel" && "$(readlink "$dst/$rel")" == "$t" ]]; then continue; fi
			rm -rf -- "${dst:?}/$rel"
			ln -s -- "$t" "$dst/$rel"
		elif [[ -f "$dst/$rel" && ! -L "$dst/$rel" ]] && cmp -s "$src/$rel" "$dst/$rel"; then
			continue
		else
			mode=$(stat -c %a "$src/$rel")
			rm -rf -- "${dst:?}/$rel.ovr-new"
			install -m "$mode" "$src/$rel" "$dst/$rel.ovr-new"
			mv -f -- "$dst/$rel.ovr-new" "$dst/$rel"
		fi
	done < <(cd "$src" && find . -mindepth 1 \( -type f -o -type l \) -print0)
	(cd "$dst" && find . -mindepth 1 \( -type f -o -type l \) ! -name '*.cap-keep' -print0) |
		while IFS= read -r -d '' rel; do [[ -e "$src/$rel" || -L "$src/$rel" ]] || rm -f -- "${dst:?}/$rel"; done
	find "$dst" -mindepth 1 -depth -type d -empty -delete
	record dir "$dst" "$class" "$note"
}
# write_runtime_json MONADO_DIR: the OpenXR runtime manifest with absolute paths into THIS prefix (it cannot be shipped:
# the paths are the user's). Same shape as the file Monado's CMake wrote for a build tree.
write_runtime_json() {
	local d=$1 oxr lm
	oxr=$d/src/xrt/targets/openxr/libopenxr_monado.so
	[[ -f $oxr ]] || return 1
	lm=$(find "$d/src/xrt/targets/libmonado" -maxdepth 1 -name 'libmonado.so.*.*.*' 2>/dev/null | sort | head -1)
	python3 -I - "$d/openxr_monado-dev.json" "$oxr" "$lm" <<'PY'
import json, sys
out, lib, lm = sys.argv[1:4]
rt = {"name": "Monado", "library_path": lib}
if lm:
    rt["MND_libmonado_path"] = lm
with open(out + ".ovr-new", "w") as f:
    json.dump({"file_format_version": "1.0.0", "runtime": rt}, f, indent=4)
    f.write("\n")
PY
	mv -f -- "$d/openxr_monado-dev.json.ovr-new" "$d/openxr_monado-dev.json"
}
# vr_free: nothing is built or installed while a VR session runs (builds take the CPU and the graphics card from the
# compositor and made VR drop frames: docs/research/perf-2026-10-17.md); checked before every step.
vr_free() {
	[[ ${OMARCHYVR_ALLOW_DURING_VR:-0} == 1 ]] && return 0
	local what
	what=$("$REPO/tools/vr-busy.sh") && return 0
	die "a VR session is running (${what#busy: }): nothing is built or installed during a session (it makes VR drop frames). Stop VR in the app and run ./install.sh again (the finished parts are reused)."
}
