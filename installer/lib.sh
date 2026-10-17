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
	PIMAX_ANCHOR_OWNER PIMAX_CTRL_INHAND PIMAX_HANDS PIMAX_AE_EXTENDED)
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
# Pimax devices on USB: lines "vid:pid name"
pimax_usb() {
	local d
	for d in /sys/bus/usb/devices/*; do
		[[ -r "$d/idVendor" ]] || continue
		[[ "$(cat "$d/idVendor")" == "$HEADSET_VENDOR" ]] || continue
		printf '%s:%s %s\n' "$(cat "$d/idVendor")" "$(cat "$d/idProduct")" "$(cat "$d/product" 2>/dev/null || echo '?')"
	done | sort -u
}
headset_state() { # tested | untested | absent
	local l
	l=$(pimax_usb)
	[[ -z "$l" ]] && { echo absent; return; }
	grep -q "^$HEADSET_TESTED " <<<"$l" && echo tested || echo untested
}
# headset_connector lives in headset-connector.sh (shared with the root job)
# shellcheck source=headset-connector.sh
. "$(dirname "${BASH_SOURCE[0]}")/headset-connector.sh"
