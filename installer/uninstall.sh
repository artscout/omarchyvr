#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# omarchyvr uninstaller (./uninstall.sh at the repo root, and the app's Ready > Uninstall omarchyvr button).
#
# Reverses the install manifest exactly (every file it lists; originals put back), and also removes what installs
# from before the manifest left behind (installer/legacy-inventory.tsv: the old pimax-* names). Then ONE root job
# (one password prompt): udev rule, pacman hooks and patch set, stock NVIDIA source and module, EDID override and
# initramfs entries, file capabilities. Says when a reboot is needed. Steam and SteamVR are never removed.
#
#   ./uninstall.sh [--dry-run] [--remove-downloads] [--keep-settings] [--remove-keys] [--remove-lab-builds]
#                  [--no-root] [--prefix DIR] [--yes]
#
#   --dry-run            list exactly what would be removed, restored or kept; change nothing (no password)
#   --remove-downloads   also delete the downloads (AI models, voice models, backgrounds, hand models); default: kept
#   --keep-settings      keep settings and calibration; default: saved into one tar in your home, then removed
#   --remove-keys        also delete ai.toml (your cloud AI keys); default: kept
#   --remove-lab-builds  also delete the lab source trees and build dirs in ~/src (~150 GB); default: only listed
#   --no-root            skip the root job (system files stay)
#   --prefix DIR         the prefix used at install time (default ~/.local)
#   --yes                do not ask before starting
set -euo pipefail
REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# run from a private copy: the installed data dir (where the app starts this script) is removed on the way
if [[ -z ${OVR_UNINSTALL_COPY:-} ]]; then
	tmp=$(mktemp -d "${TMPDIR:-/tmp}/omarchyvr-uninstall.XXXXXX")
	cp -a "$REPO/installer" "$tmp/installer"
	OVR_UNINSTALL_COPY=$tmp OVR_SRC_REPO=$REPO exec "$tmp/installer/uninstall.sh" "$@"
fi
trap 'rm -rf "$OVR_UNINSTALL_COPY"' EXIT
# shellcheck source=lib.sh
. "$REPO/installer/lib.sh"

DRY=0 RM_DL=0 KEEP_SET=0 RM_KEYS=0 RM_LAB=0 NO_ROOT=0 YES=0 PREFIX="$HOME/.local"
while [[ $# -gt 0 ]]; do
	case $1 in
	--dry-run) DRY=1 ;;
	--remove-downloads) RM_DL=1 ;;
	--keep-settings) KEEP_SET=1 ;;
	--remove-keys) RM_KEYS=1 ;;
	--remove-lab-builds) RM_LAB=1 ;;
	--no-root) NO_ROOT=1 ;;
	--prefix) PREFIX=$(realpath -m "${2:?}"); shift ;;
	--yes) YES=1 ;;
	-h | --help) sed -n '3,22p' "$0"; exit 0 ;;
	*) die "unknown option $1" ;;
	esac
	shift
done
[[ $EUID -ne 0 ]] || die "run uninstall.sh as your user (the root part asks for the password once)"
SHARE="$PREFIX/share/omarchyvr"
CONF="${XDG_CONFIG_HOME:-$HOME/.config}"
MANIFEST="$SHARE/manifest.tsv"
STAMP=$(date +%Y%m%d-%H%M%S)
SETTINGS_TAR="$HOME/omarchyvr-settings-backup-$STAMP.tar.gz"

declare -A COUNT=()
n_rm=0 n_keep=0 n_restore=0
line() { printf '  %-8s %-9s %s\n' "$1" "$2" "$3"; COUNT[$1]=$((${COUNT[$1]:-0} + 1)); }
do_rm() { # do_rm CLASS PATH NOTE
	local c=$1 p=$2 note=$3
	line "$c" remove "$p${note:+  ($note)}"
	n_rm=$((n_rm + 1))
	[[ $DRY == 1 ]] && return 0
	if [[ -d $p && ! -L $p ]]; then rm -rf -- "$p"; else rm -f -- "$p"; fi
}
do_keep() { line "$1" keep "$2${3:+  ($3)}"; n_keep=$((n_keep + 1)); }
do_restore() { # do_restore CLASS PATH BACKUP
	line "$1" restore "$2  (original from $3)"
	n_restore=$((n_restore + 1))
	[[ $DRY == 1 ]] && return 0
	rm -rf -- "$2"
	cp -a -- "$3" "$2"
}

say "omarchyvr uninstall$([[ $DRY == 1 ]] && echo ' (dry run: nothing changes)')"
if [[ $DRY == 0 ]]; then
	RUN=""
	for p in monado-service vrserver wayvr; do pgrep -x "$p" >/dev/null 2>&1 && RUN+="$p "; done
	[[ -z $RUN ]] || die "a VR session is running ($RUN): stop VR in the app first"
	if [[ $YES == 0 && -t 0 ]]; then
		read -r -p "Remove omarchyvr (downloads are $([[ $RM_DL == 1 ]] && echo removed || echo kept))? [y/N] " a
		[[ $a == [yY]* ]] || exit 1
	fi
fi

# ---- 0. the app and the services ---------------------------------------------------------------------------------
say "Stopping the app and the user services"
for comm in omarchyvr-app pimax-setup; do
	for pid in $(pgrep -x -u "$(id -u)" "$comm" 2>/dev/null || true); do
		line APP stop "$comm pid $pid"
		[[ $DRY == 0 && $pid != "${OMARCHYVR_APP_PID:-}" ]] && kill -TERM "$pid" 2>/dev/null || true
	done
done
UNITS=(omarchyvr-keepalived.service omarchyvrd.service omarchyvr-autod.service omarchyvr-ai.service
	pimax-keepalived.service pimaxd.service pimax-autod.service pimax-ai.service)
for u in "${UNITS[@]}"; do
	if systemctl --user cat "$u" >/dev/null 2>&1; then
		line UNIT stop "$u (disable --now)"
		[[ $DRY == 0 ]] && systemctl --user disable --now "$u" >/dev/null 2>&1 || true
	fi
done

# ---- 1. registration (before the binaries go: steam-setup off is done by our own launcher) -----------------------
say "OpenXR / OpenVR / Steam registration"
L=""
for c in "$PREFIX/bin/omarchyvr" "$HOME/.local/bin/pimax"; do [[ -x $c ]] && { L=$c; break; }; done
if [[ -n $L ]]; then
	line REG run "$(basename "$L") vr steam-setup off (Steam entry copy and session variable)"
	[[ $DRY == 0 ]] && "$L" vr steam-setup off >/dev/null 2>&1 || true
fi
# OpenVR: drop the external driver entries that point at a Monado build; put a saved file back when we have one
VRP="$HOME/.config/openvr/openvrpaths.vrpath"
if [[ -f $VRP ]] && grep -q 'steamvr-monado' "$VRP"; then
	line REG edit "$VRP (remove the external_drivers entries of our Monado builds)"
	if [[ $DRY == 0 ]]; then
		python3 -I - "$VRP" <<'PY'
import json, sys
p = sys.argv[1]
d = json.load(open(p))
d["external_drivers"] = [x for x in d.get("external_drivers") or [] if "steamvr-monado" not in x]
if not d["external_drivers"]:
    d.pop("external_drivers")
open(p + ".ovr-new", "w").write(json.dumps(d, indent="\t") + "\n")
PY
		mv -f "$VRP.ovr-new" "$VRP"
	fi
fi
# SteamVR settings our app wrote (refresh rate, render scale, colour gain): the keys go, SteamVR's defaults return
VRS="$HOME/.local/share/Steam/config/steamvr.vrsettings"
OUR_KEYS="preferredRefreshRate supersampleManualOverride supersampleScale hmdDisplayColorGainR hmdDisplayColorGainG hmdDisplayColorGainB"
if [[ -f $VRS ]] && python3 -I -c 'import json,sys; d=json.load(open(sys.argv[1])).get("steamvr",{}); sys.exit(0 if any(k in d for k in sys.argv[2].split()) else 1)' "$VRS" "$OUR_KEYS" 2>/dev/null; then
	line REG edit "$VRS (remove the keys our app set: $OUR_KEYS)"
	if [[ $DRY == 0 ]]; then
		python3 -I - "$VRS" "$OUR_KEYS" <<'PY'
import json, sys
p, keys = sys.argv[1], sys.argv[2].split()
d = json.load(open(p))
for k in keys:
    d.get("steamvr", {}).pop(k, None)
open(p + ".ovr-new", "w").write(json.dumps(d, indent=3) + "\n")
PY
		mv -f "$VRS.ovr-new" "$VRS"
	fi
fi
# OpenXR: a link to one of our Monado builds goes; SteamVR's own runtime link stays (SteamVR manages it)
AR="$HOME/.config/openxr/1/active_runtime.json"
if [[ -L $AR ]]; then
	t=$(readlink "$AR")
	case $t in
	*openxr_monado*) if [[ -e $AR.pimax-backup ]]; then do_restore REG "$AR" "$AR.pimax-backup"; else do_rm REG "$AR" "points at our Monado build"; fi ;;
	*) do_keep REG "$AR" "-> $t (SteamVR's runtime, left as it is)" ;;
	esac
fi

# ---- 2. the manifest (newest line first) --------------------------------------------------------------------------
if [[ -f $MANIFEST ]]; then
	say "Install manifest $MANIFEST"
	while IFS=$'\t' read -r kind path sum backup class note; do
		[[ -z $kind || $kind == \#* ]] && continue
		case $kind in
		file | link | dir)
			if [[ $backup != - && -e $backup ]]; then do_restore "$class" "$path" "$backup"
			elif [[ -e $path || -L $path ]]; then
				if [[ $kind == file && $sum != - && -f $path && "$(sha256 "$path")" != "$sum" ]]; then note="$note; changed since install"; fi
				do_rm "$class" "$path" "$note"
			fi ;;
		edit) [[ $backup != - && -e $backup ]] && do_restore "$class" "$path" "$backup" ;;
		enabled) ;; # disabled above
		data)
			if [[ $RM_DL == 1 && -e $path ]]; then do_rm KEEP "$path" "$note"; elif [[ -e $path ]]; then do_keep KEEP "$path" "$note"; fi ;;
		esac
	done < <(tac "$MANIFEST")
else
	note "no install manifest at $MANIFEST (an install from before the manifest): the known paths below are used"
fi

# ---- 3. what older installs left (known paths) ---------------------------------------------------------------------
say "Known paths of older installs (installer/legacy-inventory.tsv)"
SETTINGS=()
declare -A SEEN=()
while IFS=$'\t' read -r class pat note; do
	[[ -z $class || $class == \#* ]] && continue
	pat=${pat//\$HOME/$HOME}
	for p in $pat; do
		[[ -e $p || -L $p ]] || continue
		[[ -n ${SEEN[$p]:-} ]] && continue
		SEEN[$p]=1
		case $class in
		BIN | UNIT | ENTRY | BUILD | LINK) do_rm "$class" "$p" "$note" ;;
		REG)
			case $p in
			*/active_runtime.json | */openvrpaths.vrpath | */steam.desktop | */environment.d/*) ;; # handled above / by steam-setup off
			*/chaperone_info.vrchap.pimax-backup) do_restore REG "${p%.pimax-backup}" "$p"; [[ $DRY == 0 ]] && rm -f -- "$p" ;;
			*/Steam/config/monado) if [[ -z "$(ls -A "$p" 2>/dev/null)" ]]; then do_rm REG "$p" "$note"; fi ;;
			*) do_rm REG "$p" "$note" ;;
			esac ;;
		RESET)
			if [[ $KEEP_SET == 1 ]]; then do_keep RESET "$p" "--keep-settings"; else SETTINGS+=("$p"); line RESET reset "$p  ($note)"; fi ;;
		KEEP)
			if [[ $RM_DL == 1 ]]; then do_rm KEEP "$p" "$note"; else do_keep KEEP "$p" "$note"; fi ;;
		KEEPDATA) do_keep KEEPDATA "$p" "$note" ;;
		LAB)
			if [[ $RM_LAB == 1 ]]; then do_rm LAB "$p" "$note"; else line LAB "listed" "$p  ($note; --remove-lab-builds deletes it)"; fi ;;
		esac
	done
done <"$REPO/installer/legacy-inventory.tsv"
# the new-name settings, state and caches of a manifest install
for p in "$CONF/omarchyvr" "$HOME/.local/state/omarchyvr" "$HOME/.cache/omarchyvr"; do
	[[ -e $p ]] || continue
	if [[ $KEEP_SET == 1 ]]; then do_keep RESET "$p" "--keep-settings"; else SETTINGS+=("$p"); line RESET reset "$p"; fi
done
for p in "$SHARE"/{eye-calibration,eye-accuracy,roommap,ctrl-sessions,wayvr-home}; do
	[[ -e $p ]] || continue
	if [[ $KEEP_SET == 1 ]]; then do_keep RESET "$p" "--keep-settings"; else SETTINGS+=("$p"); line RESET reset "$p"; fi
done
if [[ ${#SETTINGS[@]} -gt 0 && $KEEP_SET == 0 ]]; then
	KEYS=()
	for k in "$HOME/.config/pimax/ai.toml" "$CONF/omarchyvr/ai.toml"; do [[ -f $k ]] && KEYS+=("$k"); done
	line RESET backup "${#SETTINGS[@]} settings/calibration paths -> $SETTINGS_TAR (mode 600)"
	if [[ $DRY == 0 ]]; then
		(umask 077; tar -czf "$SETTINGS_TAR" --absolute-names "${SETTINGS[@]}" 2>/dev/null) || warn "settings backup incomplete"
	fi
	if [[ ${#KEYS[@]} -gt 0 && $RM_KEYS == 0 ]]; then line RESET keep "ai.toml (your keys) -> $CONF/omarchyvr/ai.toml"; fi
	KEYTMP=""
	if [[ $DRY == 0 && ${#KEYS[@]} -gt 0 && $RM_KEYS == 0 ]]; then KEYTMP=$(mktemp); cp -p "${KEYS[0]}" "$KEYTMP"; fi
	for p in "${SETTINGS[@]}"; do
		n_rm=$((n_rm + 1))
		[[ $DRY == 0 ]] && rm -rf -- "$p"
	done
	if [[ -n $KEYTMP ]]; then mkdir -p "$CONF/omarchyvr"; mv -f "$KEYTMP" "$CONF/omarchyvr/ai.toml"; chmod 600 "$CONF/omarchyvr/ai.toml"; fi
fi
# downloads kept from the old data dir move to the new one, so the next install reuses them
if [[ $RM_DL == 0 ]]; then
	for d in ai backgrounds slam-recordings tracking-recordings; do
		if [[ -d "$HOME/.local/share/pimax/$d" && ! -e "$SHARE/$d" ]]; then
			line KEEP move "~/.local/share/pimax/$d -> $SHARE/$d (reused by the next install)"
			[[ $DRY == 0 ]] && { mkdir -p "$SHARE"; mv "$HOME/.local/share/pimax/$d" "$SHARE/$d"; }
		fi
	done
fi
[[ $DRY == 0 && -d "$HOME/.local/share/pimax" ]] && rmdir --ignore-fail-on-non-empty "$HOME/.local/share/pimax"
[[ $DRY == 0 ]] && systemctl --user daemon-reload 2>/dev/null || true

# ---- 4. the root job (one password) -------------------------------------------------------------------------------
say "System files (root, one password prompt)"
ROOT_SCRIPT=/usr/local/lib/omarchyvr/omarchyvr-root.sh
[[ -x $ROOT_SCRIPT ]] || ROOT_SCRIPT="$REPO/installer/omarchyvr-root.sh"
PLAN=$(mktemp --suffix=.env)
printf 'OVR_USER=%s\nOVR_HOME=%s\nOVR_SNAPSHOT=yes\nOVR_KEEP_LEGACY_CAPS=%s\n' "$(id -un)" "$HOME" "$([[ $RM_LAB == 1 ]] && echo yes || echo no)" >"$PLAN"
REBOOT=no
if [[ $DRY == 1 || $NO_ROOT == 1 ]]; then
	"$ROOT_SCRIPT" uninstall-plan "$PLAN" | sed 's/^/    /'
	[[ $NO_ROOT == 1 && $DRY == 0 ]] && note "--no-root: the system files stay"
else
	out=$(pkexec "$ROOT_SCRIPT" uninstall "$PLAN") || { echo "$out"; die "the root part failed or was cancelled (user files are already removed; run ./uninstall.sh again)"; }
	echo "$out" | sed 's/^/    /'
	grep -q '^REBOOT=yes' <<<"$out" && REBOOT=yes
fi
rm -f "$PLAN"
# the manifest itself and the empty install dirs go last
if [[ $DRY == 0 && -f $MANIFEST ]]; then
	rm -f "$MANIFEST" "$SHARE/root-plan.env" "$SHARE/root-plan.txt"
	rm -rf "$SHARE/backup"
	rmdir --ignore-fail-on-non-empty "$SHARE" "$PREFIX/lib/omarchyvr" 2>/dev/null || true
fi

say "Summary: $n_rm removed, $n_restore restored, $n_keep kept$([[ $DRY == 1 ]] && echo ' (dry run)')"
for k in "${!COUNT[@]}"; do printf '    %-9s %d\n' "$k" "${COUNT[$k]}"; done | sort
[[ ${#SETTINGS[@]} -gt 0 && $KEEP_SET == 0 && $DRY == 0 ]] && note "settings backup: $SETTINGS_TAR"
note "Steam and SteamVR were not touched."
if [[ $REBOOT == yes ]]; then say "REBOOT NEEDED: the stock NVIDIA module and the boot image without the EDID override load at the next start."; fi
