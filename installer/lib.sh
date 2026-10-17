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
