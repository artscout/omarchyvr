#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# omarchyvr-root.sh: the ONE root job of an omarchyvr install or uninstall (run through one pkexec prompt).
#
#   omarchyvr-root.sh plan      PLAN.env   print exactly what `apply` would change (any user, changes nothing)
#   omarchyvr-root.sh apply     PLAN.env   do it (root); writes /var/lib/omarchyvr/manifest.tsv
#   omarchyvr-root.sh edid      PLAN.env   only the EDID override step (root): finds the headset's display (OVR_EDID=auto
#                                          logic, waits up to OVR_EDID_WAIT s), writes the files, rebuilds the boot image;
#                                          exit 3 = the headset display was not seen (nothing changed)
#   omarchyvr-root.sh uninstall-plan [PLAN.env]  print what `uninstall` would remove or restore (any user)
#   omarchyvr-root.sh uninstall [PLAN.env]       reverse every system change (root), incl. installs from before the
#                                                manifest existed (known paths only)
#
# PLAN.env is written by install.sh / uninstall.sh. It is parsed, never sourced: only KEY=VALUE lines with the keys
# below and values from a safe character set are accepted.
#   OVR_USER, OVR_HOME          the installing user and home
#   OVR_DATA                    the installed data dir (<share>/omarchyvr/data: linux/, patches/, installer/)
#   OVR_SERVICE                 the installed monado-service (gets cap_sys_nice+ep once)
#   OVR_PACKAGES                pacman packages to install (space separated; empty = none)
#   OVR_NVIDIA                  patch | skip   (install.sh decides: open modules + headset or opt-in; a version that is
#                               not tested is still "patch": the next key decides whether the patch is applied)
#   OVR_NVIDIA_VERSION          the exact nvidia-open-dkms version
#   OVR_NVIDIA_TESTED           tested versions (space separated; patches/nvidia/tested-versions.txt wins when present)
#   OVR_NVIDIA_ALLOW_UNTESTED   yes | no (default no): the user's switch "Allow the patch on untested NVIDIA drivers".
#                               Written to /etc/omarchyvr/nvidia.conf so later pacman hook runs honour it.
#   OVR_EDID                    the NVKMS override key, e.g. override.[0000:01:00.0].DP-0 (detected; empty = no EDID step;
#                               auto = look for the headset's display during the job, after the udev step woke it, up to 30 s)
#   OVR_STEAMVR_LAUNCHER        SteamVR's vrcompositor-launcher (empty = skip); gets cap_sys_nice=eip like SteamVR's own prompt
#   OVR_SNAPSHOT                yes | no  (snapper snapshot of / first, when snapper has a "root" config)
#   OVR_KEEP_LEGACY_CAPS        yes | no  (uninstall: leave the caps of old Monado builds in ~/src alone)
#
# Safety: fixed PATH; every write goes to a fixed list of destinations; originals that are not ours are saved under
# /var/lib/omarchyvr/backup before they are replaced and put back on uninstall; NVIDIA is patched only on a tested
# version (or an untested one the user allowed) and falls back to the stock module if the patched build fails, so the
# desktop keeps working.
set -euo pipefail
export PATH=/usr/local/sbin:/usr/local/bin:/usr/bin:/usr/sbin:/bin
umask 022

STATE=/var/lib/omarchyvr
LIBDIR=/usr/local/lib/omarchyvr
LEGACY_LIBDIR=/usr/local/lib/pimax
HOOK_PATCH=/etc/pacman.d/hooks/65-nvidia-pimax-patch.hook
HOOK_VERIFY=/etc/pacman.d/hooks/75-nvidia-pimax-verify.hook
UDEV_RULE=/etc/udev/rules.d/70-pimax.rules
EDID_DIR=/etc/pimax
CONF_DIR=/etc/omarchyvr
NVIDIA_CONF=$CONF_DIR/nvidia.conf
NVIDIA_STATUS=$STATE/nvidia-patch-status
MODPROBE=/etc/modprobe.d/nvidia-pimax.conf
MKINITCPIO=/etc/mkinitcpio.conf.d/pimax.conf

# shellcheck source=headset-connector.sh
. "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")/headset-connector.sh" 2>/dev/null || headset_connector() { return 1; }

MODE=${1:-}
PLAN=${2:-}
[[ $MODE == restore-stock || $MODE == edid-key ]] && PLAN="" # its argument is a source tree, not a plan
DRY=1
case $MODE in
apply | uninstall | edid) DRY=0 ;;
plan | uninstall-plan) DRY=1 ;;
edid-key) DRY=1 ;; # internal, tests: edid-key [SECONDS]: print the headset's override key once its EDID shows (OVR_DRM_ROOT)
restore-stock) DRY=0 ;; # internal, also used by the tests on a scratch tree: restore-stock SRC_TREE [PATCH_DIR]
*) sed -n '3,28p' "$0"; exit 2 ;;
esac
# OVR_TEST_NO_ROOT=1 (tests of the not-found path only, which writes nothing; pkexec drops the environment)
if [[ $DRY == 0 && $EUID -ne 0 && $MODE != restore-stock && ! ( $MODE == edid && ${OVR_TEST_NO_ROOT:-} == 1 ) ]]; then
	echo "omarchyvr-root.sh $MODE must run as root (the app runs it through pkexec)" >&2
	exit 1
fi

# ---- the plan ------------------------------------------------------------------------------------------------------
OVR_USER="" OVR_HOME="" OVR_DATA="" OVR_SERVICE="" OVR_PACKAGES="" OVR_NVIDIA=skip OVR_NVIDIA_VERSION=""
OVR_NVIDIA_TESTED="" OVR_NVIDIA_ALLOW_UNTESTED=no OVR_EDID="" OVR_STEAMVR_LAUNCHER="" OVR_SNAPSHOT=yes OVR_KEEP_LEGACY_CAPS=no
if [[ -n $PLAN ]]; then
	[[ -f $PLAN ]] || { echo "plan $PLAN not found" >&2; exit 1; }
	while IFS= read -r line || [[ -n $line ]]; do
		[[ -z $line || $line == \#* ]] && continue
		k=${line%%=*} v=${line#*=}
		v=${v#\"} v=${v%\"}
		[[ $v =~ ^[][A-Za-z0-9_./:@+\ -]*$ ]] || { echo "plan: unsafe value for $k" >&2; exit 1; }
		case $k in
		OVR_USER | OVR_HOME | OVR_DATA | OVR_SERVICE | OVR_PACKAGES | OVR_NVIDIA | OVR_NVIDIA_VERSION | OVR_NVIDIA_TESTED | OVR_NVIDIA_ALLOW_UNTESTED | \
			OVR_EDID | OVR_STEAMVR_LAUNCHER | OVR_SNAPSHOT | OVR_KEEP_LEGACY_CAPS) printf -v "$k" '%s' "$v" ;;
		*) echo "plan: unknown key $k" >&2; exit 1 ;;
		esac
	done <"$PLAN"
fi

N=0 REBOOT=0 INITRAMFS=0
step() { N=$((N + 1)); printf '%2d. %s\n' "$N" "$*"; }
act() { # act DESCRIPTION -- COMMAND...: print, and run it unless this is a plan
	local d=$1; shift; [[ $1 == -- ]] && shift
	if [[ $DRY == 1 ]]; then printf '      would: %s\n' "$d"; else printf '      %s\n' "$d"; "$@"; fi
}
info() { printf '      %s\n' "$*"; }
mrec() { # mrec KIND PATH NOTE [BACKUP]: the root manifest
	[[ $DRY == 1 ]] && return 0
	mkdir -p "$STATE"
	[[ -f $STATE/manifest.tsv ]] || printf '# omarchyvr root manifest v1 (kind\tpath\tbackup\tnote)\n' >"$STATE/manifest.tsv"
	awk -F'\t' -v p="$2" -v k="$1" '!($2 == p && $1 == k)' "$STATE/manifest.tsv" >"$STATE/manifest.tsv.new"
	mv -f "$STATE/manifest.tsv.new" "$STATE/manifest.tsv"
	printf '%s\t%s\t%s\t%s\n' "$1" "$2" "${4:--}" "$3" >>"$STATE/manifest.tsv"
}
in_manifest() { [[ -f $STATE/manifest.tsv ]] && awk -F'\t' -v p="$1" '$2 == p { f = 1 } END { exit !f }' "$STATE/manifest.tsv"; }
backup_of() { echo "$STATE/backup$1"; }
# files whose names are ours (an older omarchyvr/pimax install wrote them): replaced, never kept as "originals"
ours_by_name() {
	case $1 in
	"$UDEV_RULE" | "$HOOK_PATCH" | "$HOOK_VERIFY" | "$NVIDIA_CONF" | "$MODPROBE" | "$MKINITCPIO" | "$EDID_DIR"/* | "$LIBDIR"/* | "$LEGACY_LIBDIR"/*) return 0 ;;
	esac
	return 1
}
save_original() { # keep a file that is not ours before replacing it (first original wins)
	local p=$1 b
	b=$(backup_of "$p")
	[[ -e $p ]] || return 0
	in_manifest "$p" && return 0
	ours_by_name "$p" && return 0
	[[ -e $b ]] && return 0
	mkdir -p "$(dirname "$b")"
	cp -a "$p" "$b"
}
place() { # place SRC DEST MODE NOTE: install a file if it differs (backup of a foreign original)
	local src=$1 dst=$2 mode=$3 note=$4
	if [[ -f $dst ]] && cmp -s "$src" "$dst"; then
		info "unchanged: $dst"
		[[ $DRY == 0 ]] && mrec file "$dst" "$note"
		return 1
	fi
	if [[ $DRY == 1 ]]; then
		info "would write $dst ($note)$([[ -e $dst ]] && ! ours_by_name "$dst" && echo ', the current file is saved first')$([[ -e $dst ]] && ours_by_name "$dst" && echo ', replacing an older omarchyvr/pimax copy')"
		return 0
	fi
	save_original "$dst"
	install -D -m "$mode" "$src" "$dst"
	mrec file "$dst" "$note" "$([[ -e $(backup_of "$dst") ]] && backup_of "$dst" || echo -)"
	info "wrote $dst"
	return 0
}
text_to() { # text_to DEST NOTE TEXT: like place, from a string
	local t
	t=$(mktemp)
	printf '%s\n' "$3" >"$t"
	local rc=0
	place "$t" "$1" 644 "$2" || rc=$?
	rm -f "$t"
	return $rc
}
# wait_headset_edid_key SECONDS: poll every 0.5 s for the headset's EDID (the udev step woke it); prints the NVKMS key
# "override.[<pci>].<NVKMS connector>" or fails on timeout. OVR_DRM_ROOT (tests) moves the sysfs root.
wait_headset_edid_key() {
	local max=$1 i=0 c pci nv
	while :; do
		if c=$(headset_connector); then
			read -r pci _ nv <<<"$c"
			echo "override.[$pci].$nv"
			return 0
		fi
		[[ $i -ge $((max * 2)) ]] && return 1
		sleep "${OVR_EDID_POLL:-0.5}"
		i=$((i + 1))
	done
}
if [[ $MODE == edid-key ]]; then
	wait_headset_edid_key "${2:-30}"
	exit
fi
# edid_step DATA: the EDID override (headset display mode at boot): files, initramfs flag, reboot flag. OVR_EDID decides:
# "auto" = look for the headset's display now (the udev step woke it), a key = use it, empty = no step.
# Returns 3 when "auto" found no headset display (the rest of the job is not affected).
edid_step() {
	local d=$1 rc=0 edid=$OVR_EDID
	if [[ $edid == auto ]]; then
		step "Headset display mode at boot: EDID override (connector found after the headset wakes, up to ${OVR_EDID_WAIT:-30} s)"
		if [[ $DRY == 1 ]]; then
			info "would: wait for the headset's display (EDID id PVR) to appear, then write the override for its connector"
			info "the headset wakes once its access rule is in; if it does not show up the step is skipped (press Fix in the app later)"
			edid=""
		else
			edid=$(wait_headset_edid_key "${OVR_EDID_WAIT:-30}") || edid=""
			if [[ -z $edid ]]; then
				info "headset display not seen: put the headset on / turn it on, then press Fix in the app (EDID override skipped, nothing else is affected)"
				rc=3
			else
				info "headset display found: ${edid#override.}"
			fi
		fi
	elif [[ -n $edid ]]; then
		step "Headset display mode at boot: EDID override for ${edid#override.} (non-desktop, native modes)"
	fi
	if [[ -n $edid ]]; then
		local ch=0
		place "$d/linux/pimax-dp-edid-msvsdb.bin" "$EDID_DIR/dp-edid.bin" 644 "headset EDID" && ch=1
		text_to "$EDID_DIR/nvidia-modeset.conf" "EDID override key" "$edid=$EDID_DIR/dp-edid.bin" && ch=1
		text_to "$MODPROBE" "nvidia-modeset options" "options nvidia-modeset config_file=$EDID_DIR/nvidia-modeset.conf" && ch=1
		text_to "$MKINITCPIO" "initramfs files" "FILES+=($EDID_DIR/nvidia-modeset.conf $EDID_DIR/dp-edid.bin)" && ch=1
		if [[ $ch == 1 ]]; then INITRAMFS=1; REBOOT=1; fi
	fi
	return $rc
}
rebuild_initramfs() {
	if command -v limine-mkinitcpio >/dev/null 2>&1; then
		act "limine-mkinitcpio (boot image with the nvidia modules and the EDID files)" -- limine-mkinitcpio
	else
		act "mkinitcpio -P (boot image with the nvidia modules and the EDID files)" -- mkinitcpio -P
	fi
}
valid_service() { # the installed monado-service: absolute, inside the user's home, a regular file owned by the user
	local s=$1
	[[ $s == /* && $s == */monado-service && $s != *..* ]] || return 1
	[[ -n $OVR_HOME && $s == "$OVR_HOME"/* ]] || return 1
	[[ $DRY == 1 && ! -e $s ]] && return 0
	[[ -f $s && ! -L $s ]] || return 1
	[[ $(stat -c %U "$s") == "$OVR_USER" ]]
}

# ==== install ========================================================================================================
do_apply() {
	local d=$OVR_DATA
	[[ -n $OVR_USER && -n $OVR_HOME && -d $d/linux && -d $d/patches/nvidia ]] || { echo "plan: OVR_USER/OVR_HOME/OVR_DATA missing or $d has no linux/ and patches/nvidia" >&2; exit 1; }
	echo "omarchyvr system setup ($([[ $DRY == 1 ]] && echo 'plan only, nothing changes' || echo applying)):"

	if [[ $OVR_SNAPSHOT == yes ]] && command -v snapper >/dev/null 2>&1 && snapper list-configs 2>/dev/null | grep -q '^root '; then
		step "Restore point: a snapper snapshot of / (roll back from the boot menu if anything goes wrong)"
		act "snapper -c root create -d 'before omarchyvr install'" -- snapper -c root create -d "before omarchyvr install"
	fi

	if [[ -n $OVR_PACKAGES ]]; then
		step "Packages from the Arch repositories: $OVR_PACKAGES"
		local p before=()
		for p in $OVR_PACKAGES; do pacman -Q "$p" >/dev/null 2>&1 || before+=("$p"); done
		if [[ ${#before[@]} -gt 0 ]]; then
			act "pacman -S --needed --noconfirm ${before[*]}" -- pacman -S --needed --noconfirm "${before[@]}"
			if [[ $DRY == 0 ]]; then for p in "${before[@]}"; do mrec pkg "$p" "installed by omarchyvr (not removed on uninstall)"; done; fi
		else
			info "all present"
		fi
	fi

	step "Headset access: udev rule $UDEV_RULE (only Pimax devices 34a4:* and the eye tracker 2104:0561; keeps the hubs above the headset awake)"
	place "$d/installer/pimax-hub-power.sh" "$LIBDIR/pimax-hub-power.sh" 755 "hub power helper (udev RUN)" || true
	if place "$d/linux/70-pimax.rules" "$UDEV_RULE" 644 "headset udev rule"; then
		act "udevadm control --reload; udevadm trigger for the Pimax devices" -- sh -c 'udevadm control --reload && udevadm trigger --subsystem-match=usb --attr-match=idVendor=34a4 && udevadm trigger --subsystem-match=hidraw'
	fi

	step "Uninstall helper: $LIBDIR/omarchyvr-root.sh (root-owned copy of this script, used by the uninstaller)"
	place "$d/installer/omarchyvr-root.sh" "$LIBDIR/omarchyvr-root.sh" 755 "root job (uninstall)" || true

	if [[ $OVR_NVIDIA == patch ]]; then
		local v=$OVR_NVIDIA_VERSION tested=no allow=no flag=() f
		local tlist=$OVR_NVIDIA_TESTED
		[[ -r $d/patches/nvidia/tested-versions.txt ]] && tlist=$(sed 's/#.*//' "$d/patches/nvidia/tested-versions.txt" | tr -s ' \t\n' ' ')
		[[ " $tlist " == *" $v "* ]] && tested=yes
		[[ $OVR_NVIDIA_ALLOW_UNTESTED == yes ]] && allow=yes
		if [[ $tested == yes ]]; then
			step "NVIDIA $v (tested): display patches for the headset's two-head mode, re-applied after driver updates"
		elif [[ $allow == yes ]]; then
			step "NVIDIA $v (NOT tested; you allowed the patch on untested drivers): display patches, re-applied after driver updates"
			info "WARNING: this driver version was never tested with the patches. The patched build may fail, or the headset picture may break."
			info "The stock module is built as a fallback if the patched build fails, so the desktop keeps working. A restore snapshot is taken first."
			flag=(--allow-untested)
		else
			step "NVIDIA $v (NOT tested): the patch is NOT applied; the stock module stays (the desktop works, the headset picture will not)"
			info "To patch it anyway: turn on 'Allow the patch on untested NVIDIA drivers' in the app (the setup screen, NVIDIA driver patch card) and press Run setup again."
		fi
		for f in nvidia-patch-reapply.sh nvidia-2h1or-viewport.patch nvidia-2h1or-flip-fix.patch nvidia-evo-makeroom-bounded-wait.patch tested-versions.txt; do
			[[ -f $d/patches/nvidia/$f ]] || continue
			place "$d/patches/nvidia/$f" "$LIBDIR/$f" "$([[ $f == *.sh ]] && echo 755 || echo 644)" "nvidia patch set" || true
		done
		place "$d/patches/nvidia/nvidia-pimax-patch.hook" "$HOOK_PATCH" 644 "pacman hook: re-apply after driver updates" || true
		place "$d/patches/nvidia/nvidia-pimax-verify.hook" "$HOOK_VERIFY" 644 "pacman hook: verify, stock fallback" || true
		# the user's choice, read by the pacman hook on later driver updates (parsed, never sourced)
		text_to "$NVIDIA_CONF" "NVIDIA patch: allow untested drivers" "# omarchyvr: written by the system setup job (app switch: Allow the patch on untested NVIDIA drivers)
allow_untested=$([[ $allow == yes ]] && echo 1 || echo 0)" || true
		if [[ -d $LEGACY_LIBDIR ]]; then
			act "remove the older copy $LEGACY_LIBDIR (the hooks now use $LIBDIR)" -- rm -r -- "$LEGACY_LIBDIR"
		fi
		if [[ $tested == no && $allow == no ]]; then
			act "record the state for the app (untested driver, patch not applied)" -- "$LIBDIR/nvidia-patch-reapply.sh" --version "$v"
			[[ $DRY == 0 ]] && mrec nvidia "/usr/src/nvidia-$v" "untested driver, not patched (hooks installed)"
		else
		# which fixes the source still needs (a newer source may already contain them: they are skipped)
		local chk rc=0
		chk=$("$d/patches/nvidia/nvidia-patch-reapply.sh" --check --version "$v" "${flag[@]}" 2>&1) || rc=$?
		printf '%s\n' "$chk" | sed 's/^/      /'
		if [[ $rc == 2 ]]; then
			act "patch /usr/src/nvidia-$v (originals kept as *.pimax-orig), dkms build + install; stock module if the build fails" -- \
				"$LIBDIR/nvidia-patch-reapply.sh" --version "$v" "${flag[@]}" --rebuild --fallback
			[[ $DRY == 0 ]] && mrec nvidia "/usr/src/nvidia-$v" "patched; uninstall restores the *.pimax-orig files and rebuilds stock"
			REBOOT=1
		elif [[ $rc == 0 ]]; then
			info "source already patched (or fixed upstream): nothing to rebuild"
			[[ $DRY == 0 ]] && mrec nvidia "/usr/src/nvidia-$v" "already patched before this install"
			if ! "$LIBDIR/nvidia-patch-reapply.sh" --verify >/dev/null 2>&1 && [[ $DRY == 0 ]]; then
				act "the built module lacks the fix: dkms rebuild" -- "$LIBDIR/nvidia-patch-reapply.sh" --version "$v" "${flag[@]}" --rebuild --force-build --fallback
				REBOOT=1
			fi
		else
			echo "      the patches do not apply to this source: nothing changed (see the message above)" >&2
		fi
		fi
		edid_step "$d" || true # returns 3 when the headset display was not seen: only this step is skipped
	else
		step "NVIDIA: no driver patch (not needed or not a tested version; see the installer's message)"
	fi
	if [[ $INITRAMFS == 1 && $OVR_NVIDIA == patch ]]; then rebuild_initramfs; fi

	if [[ -n $OVR_SERVICE ]]; then
		step "Realtime priority for the VR compositor: cap_sys_nice on $OVR_SERVICE (once; Play never asks again)"
		valid_service "$OVR_SERVICE" || { echo "refusing setcap: $OVR_SERVICE is not the user's installed monado-service" >&2; exit 1; }
		if [[ -e $OVR_SERVICE ]] && getcap "$OVR_SERVICE" 2>/dev/null | grep -q cap_sys_nice; then
			info "already set"
		else
			act "setcap cap_sys_nice+ep $OVR_SERVICE" -- setcap cap_sys_nice+ep "$OVR_SERVICE"
			[[ $DRY == 0 ]] && mrec cap "$OVR_SERVICE" "cap_sys_nice+ep"
		fi
	fi
	if [[ -n $OVR_STEAMVR_LAUNCHER ]]; then
		step "SteamVR's compositor priority (the same setcap SteamVR asks for on its first start)"
		if [[ $OVR_STEAMVR_LAUNCHER != "$OVR_HOME"/*/SteamVR/bin/linux64/vrcompositor-launcher || $OVR_STEAMVR_LAUNCHER == *..* ]]; then
			echo "refusing: $OVR_STEAMVR_LAUNCHER is not SteamVR's vrcompositor-launcher" >&2; exit 1
		fi
		if getcap "$OVR_STEAMVR_LAUNCHER" 2>/dev/null | grep -q cap_sys_nice; then
			info "already set"
		else
			act "setcap cap_sys_nice=eip $OVR_STEAMVR_LAUNCHER" -- setcap cap_sys_nice=eip "$OVR_STEAMVR_LAUNCHER"
			[[ $DRY == 0 ]] && mrec cap "$OVR_STEAMVR_LAUNCHER" "cap_sys_nice=eip (SteamVR's own)"
		fi
	fi
	echo "REBOOT=$([[ $REBOOT == 1 ]] && echo yes || echo no)"
}

# The EDID override alone (the app's "Fix" for the headset display mode): the same wait and the same files as the
# EDID step of `apply`, so one click fixes it after a headset that was off during the setup.
do_edid() {
	local d=$OVR_DATA rc=0
	[[ -n $OVR_USER && -n $OVR_HOME && -d $d/linux ]] || { echo "plan: OVR_USER/OVR_HOME/OVR_DATA missing or $d has no linux/" >&2; exit 1; }
	echo "omarchyvr headset display mode (EDID override only):"
	[[ -n $OVR_EDID ]] || OVR_EDID=auto
	edid_step "$d" || rc=$?
	if [[ $INITRAMFS == 1 ]]; then rebuild_initramfs; fi
	echo "REBOOT=$([[ $REBOOT == 1 ]] && echo yes || echo no)"
	exit $rc
}

# ---- stock NVIDIA source ---------------------------------------------------------------------------------------------
PATCH_NAMES=(nvidia-2h1or-viewport.patch nvidia-2h1or-flip-fix.patch nvidia-evo-makeroom-bounded-wait.patch)
# the files the three patches touch (used only when the patch files themselves cannot be found)
PATCH_TOUCHED=(kernel-open/nvidia-drm/nvidia-drm-drv.c kernel-open/nvidia-modeset/nvidia-modeset-linux.c
	kernel-open/nvidia-modeset/nvidia-modeset-os-interface.h src/nvidia-modeset/include/nvkms-dma.h
	src/nvidia-modeset/include/nvkms-hw-flip.h src/nvidia-modeset/include/nvkms-types.h
	src/nvidia-modeset/os-interface/include/nvidia-modeset-os-interface.h src/nvidia-modeset/src/nvkms-dma.c
	src/nvidia-modeset/src/nvkms-evo.c src/nvidia-modeset/src/nvkms-flip.c src/nvidia-modeset/src/nvkms-hw-flip.c
	src/nvidia-modeset/src/nvkms-rm.c)
patch_dir_ok() { local n; for n in "${PATCH_NAMES[@]}"; do [[ -f $1/$n ]] || return 1; done; }
find_patch_dir() { # first of the given directories that holds all three patches
	local c
	for c in "$@"; do [[ -n $c ]] && patch_dir_ok "$c" && { echo "$c"; return 0; }; done
	return 1
}
# patch_markers TREE: one line per piece of patch code still in the source tree (empty = stock)
patch_markers() {
	local t=$1 ev
	ev=$t/src/nvidia-modeset/src/nvkms-evo.c
	grep -rqsF 'merged_head_sync' "$t/kernel-open" "$t/src/nvidia-modeset" && echo "merged_head_sync (flip-fix module parameter)"
	if [[ -f $ev ]] && awk '/^void nvSetViewPortsEvo/,/^}/' "$ev" | tr '\n' ' ' | grep -Eq 'EvoSetViewportPointIn\([^;]*mergeHeadSection'; then
		echo "per-head viewport offset in nvSetViewPortsEvo (mergeHeadSection)"
	fi
	grep -rqsF 'nvEvoReadChannelGetPut' "$t/src/nvidia-modeset" && echo "nvEvoReadChannelGetPut (flip-fix)"
	grep -rqsE 'NV_EVO_MAKE_ROOM_MAX_TIMEOUTS|pb\.dead' "$t/src/nvidia-modeset" && echo "bounded make-room code (NV_EVO_MAKE_ROOM_MAX_TIMEOUTS, pb.dead)"
	return 0
}
RS_CHANGED=0 RS_LEFT=""
# restore_stock_source SRC [PATCHDIR]: put the stock source back in a /usr/src/nvidia-<ver> tree.
#   1. files with a *.pimax-orig backup are restored from it;
#   2. every patch that is still in (applied by hand, no backup) is reversed, newest first, file by file: dry run
#      (patch -R --dry-run --fuzz=0) first, the real reverse only if the dry run succeeds;
#   3. if a pristine copy SRC.orig exists (and has no patch code itself) the files the patches touch are copied from it
#      when they still differ;
#   4. no patch marker may remain (merged_head_sync, the viewport call, nvEvoReadChannelGetPut, the bounded make-room
#      code): otherwise RS_LEFT lists them and the return value is 1.
# Sets RS_CHANGED=1 when it changed anything.
restore_stock_source() {
	local src=$1 pdir=${2:-} o f p i chunks c n rel
	RS_CHANGED=0 RS_LEFT=""
	local origs=()
	mapfile -t origs < <(find "$src" -name '*.pimax-orig' -type f 2>/dev/null)
	for o in "${origs[@]}"; do
		act "restore ${o%.pimax-orig} (stock)" -- mv -f -- "$o" "${o%.pimax-orig}"
		RS_CHANGED=1
	done
	if [[ $DRY == 1 ]]; then
		[[ -n $(patch_markers "$src") ]] && RS_CHANGED=1
		info "would then reverse every patch still in $src that has no backup (patch -R --fuzz=0, dry run first), use $src.orig as the reference when it exists, and check that no patch code is left"
		return 0
	fi
	[[ -n $pdir ]] || pdir=$(find_patch_dir "$LIBDIR" "${OVR_HOME:+$OVR_HOME/.local/share/omarchyvr/data/patches/nvidia}" "${OVR_DATA:+$OVR_DATA/patches/nvidia}") || pdir=""
	local touched=("${PATCH_TOUCHED[@]}")
	if [[ -n $pdir ]]; then
		mapfile -t touched < <(for n in "${PATCH_NAMES[@]}"; do sed -n 's|^+++ b/||p' "$pdir/$n" | sed 's/\t.*//'; done | sort -u)
		chunks=$(mktemp -d)
		for ((i = ${#PATCH_NAMES[@]} - 1; i >= 0; i--)); do
			n=${PATCH_NAMES[$i]}
			mkdir -p "$chunks/$i"
			# one chunk per file: a patch that is only partly in (some files restored from backups) still reverses file by file
			awk -v d="$chunks/$i" '/^diff --git /{k++} k>0{print > (d "/" k ".diff")}' "$pdir/$n"
			for c in "$chunks/$i"/*.diff; do
				[[ -f $c ]] || continue
				rel=$(sed -n 's|^+++ b/||p' "$c" | sed 's/\t.*//' | head -n1)
				if patch -R -p1 --dry-run --fuzz=0 --no-backup-if-mismatch -s -d "$src" -i "$c" >/dev/null 2>&1; then
					act "reverse $n in $rel (it is still in the source: applied by hand, no backup)" -- \
						patch -R -p1 --fuzz=0 --no-backup-if-mismatch -s -d "$src" -i "$c"
					RS_CHANGED=1
				fi
			done
		done
		rm -rf "$chunks"
	fi
	if [[ -d $src.orig ]]; then
		if [[ -n $(patch_markers "$src.orig") ]]; then
			info "not using $src.orig as the reference: it contains patch code itself"
		else
			for f in "${touched[@]}"; do
				[[ -f $src.orig/$f ]] || continue
				if ! cmp -s "$src.orig/$f" "$src/$f"; then
					act "copy $f back from the pristine copy $src.orig (it still differed)" -- install -m "$(stat -c %a "$src.orig/$f")" -- "$src.orig/$f" "$src/$f"
					RS_CHANGED=1
				fi
			done
		fi
	fi
	RS_LEFT=$(patch_markers "$src")
	if [[ -n $RS_LEFT ]]; then
		{
			echo "################################################################################"
			echo "# omarchyvr: $src STILL CONTAINS PATCH CODE after the restore:"
			while IFS= read -r o; do echo "#   - $o"; done <<<"$RS_LEFT"
			echo "# The files could not be reversed automatically (they were edited by hand?). Reinstall the stock"
			echo "# driver source: reinstall the package (pacman -S nvidia-open-dkms) or compare it with a pristine copy."
			echo "################################################################################"
		} >&2
		return 1
	fi
	info "$src is the stock source (no patch code left)"
	return 0
}

# ==== uninstall ======================================================================================================
NVIDIA_LEFTOVER=()
LEGACY_FILES=("$UDEV_RULE" "$HOOK_PATCH" "$HOOK_VERIFY" "$MODPROBE" "$MKINITCPIO" "$EDID_DIR/nvidia-modeset.conf" "$EDID_DIR/dp-edid.bin")
remove_file() { # remove a system file of ours; put a saved original back
	local p=$1 b
	b=$(backup_of "$p")
	if [[ -e $b ]]; then
		act "restore the original $p" -- cp -a "$b" "$p"
	elif [[ -e $p ]]; then
		act "remove $p" -- rm -f -- "$p"
	fi
}
do_uninstall() {
	echo "omarchyvr system removal ($([[ $DRY == 1 ]] && echo 'plan only, nothing changes' || echo removing)):"
	if [[ $OVR_SNAPSHOT == yes ]] && command -v snapper >/dev/null 2>&1 && snapper list-configs 2>/dev/null | grep -q '^root '; then
		step "Restore point: snapper snapshot of /"
		act "snapper -c root create -d 'before omarchyvr uninstall'" -- snapper -c root create -d "before omarchyvr uninstall"
	fi
	step "Headset udev rule"
	local had_udev=0
	[[ -e $UDEV_RULE ]] && had_udev=1
	remove_file "$UDEV_RULE"
	[[ $had_udev == 1 ]] && act "udevadm control --reload" -- udevadm control --reload

	step "NVIDIA: pacman hooks, patch set, stock driver source and module"
	remove_file "$HOOK_PATCH"
	remove_file "$HOOK_VERIFY"
	remove_file "$NVIDIA_CONF"
	[[ -d $CONF_DIR ]] && act "remove $CONF_DIR if empty" -- rmdir --ignore-fail-on-non-empty -- "$CONF_DIR"
	remove_file "$NVIDIA_STATUS"
	local src v restored=0
	for src in /usr/src/nvidia-[0-9]*; do
		[[ -d $src ]] || continue
		case $src in *.orig | *.pimax-orig) continue ;; esac
		v=${src##*/nvidia-}
		local rs_rc=0
		restore_stock_source "$src" || rs_rc=$?
		if [[ $RS_CHANGED == 1 ]]; then
			act "dkms build + install nvidia/$v for $(uname -r) (stock module)" -- sh -c "dkms build nvidia/$v -k $(uname -r) --force && dkms install nvidia/$v -k $(uname -r) --force"
			restored=1
		fi
		if [[ $rs_rc != 0 ]]; then
			NVIDIA_LEFTOVER+=("$src")
			echo "NVIDIA_STOCK=leftover:$src"
			continue # keep the pristine copy: it is the only reference left
		fi
		if [[ -d $src.orig ]]; then
			# a pristine copy made by hand before the hooks existed: removed only when it equals the restored source
			if [[ $DRY == 1 ]]; then
				info "would compare $src.orig with the restored $src and remove it if they are equal (a hand-made pristine copy)"
			elif diff -rq "$src.orig" "$src" >/dev/null 2>&1; then
				act "remove $src.orig (equal to the stock source)" -- rm -r -- "$src.orig"
			else
				info "kept $src.orig: it differs from the restored source (look at it by hand)"
			fi
		fi
	done
	if [[ $restored == 0 ]]; then info "no patched nvidia source found"; fi

	step "EDID override and initramfs entries"
	local had_edid=0 f
	for f in "$MODPROBE" "$MKINITCPIO" "$EDID_DIR/nvidia-modeset.conf" "$EDID_DIR/dp-edid.bin"; do
		[[ -e $f ]] && had_edid=1
		remove_file "$f"
	done
	if [[ -d $EDID_DIR ]]; then act "remove $EDID_DIR if empty" -- rmdir --ignore-fail-on-non-empty -- "$EDID_DIR"; fi
	if [[ $restored == 1 || $had_edid == 1 ]]; then
		rebuild_initramfs
		REBOOT=1
	fi

	step "File capabilities"
	local p
	if [[ -f $STATE/manifest.tsv ]]; then
		while IFS=$'\t' read -r k p _; do
			[[ $k == cap && -e $p ]] && act "setcap -r $p" -- setcap -r "$p"
		done <"$STATE/manifest.tsv"
	fi
	if [[ $OVR_KEEP_LEGACY_CAPS != yes && -n $OVR_HOME ]]; then
		# monado-service builds from before the manifest (hand builds in ~/src) carry cap_sys_nice+ep
		while IFS= read -r p; do
			[[ $(stat -c %U "$p" 2>/dev/null) == "$OVR_USER" ]] || continue
			getcap "$p" 2>/dev/null | grep -q cap_sys_nice || continue
			act "setcap -r $p (an older Monado build)" -- setcap -r "$p"
		done < <(find "$OVR_HOME/src" -maxdepth 7 -path '*/src/xrt/targets/service/monado-service' -type f 2>/dev/null)
	fi

	step "Our root files: $LIBDIR, $LEGACY_LIBDIR, $STATE"
	[[ -d $LEGACY_LIBDIR ]] && act "remove $LEGACY_LIBDIR" -- rm -r -- "$LEGACY_LIBDIR"
	[[ -d $LIBDIR ]] && act "remove $LIBDIR" -- rm -r -- "$LIBDIR"
	if [[ -f $STATE/manifest.tsv ]] && grep -q '^pkg' "$STATE/manifest.tsv"; then
		info "packages installed by omarchyvr stay (other software may use them): $(awk -F'\t' '$1 == "pkg" { printf "%s ", $2 }' "$STATE/manifest.tsv")"
	fi
	[[ -d $STATE ]] && act "remove $STATE (manifest and saved originals, after restoring them)" -- rm -r -- "$STATE"
	if [[ ${#NVIDIA_LEFTOVER[@]} -gt 0 ]]; then
		echo "WARNING: patch code is STILL in the NVIDIA source: ${NVIDIA_LEFTOVER[*]} (see the message above). The next driver build would be patched."
	fi
	echo "REBOOT=$([[ $REBOOT == 1 ]] && echo yes || echo no)"
}

case $MODE in
plan | apply) do_apply ;;
edid) do_edid ;;
uninstall-plan | uninstall) do_uninstall ;;
restore-stock)
	[[ -d ${2:-} ]] || { echo "restore-stock: ${2:-} is not a directory" >&2; exit 2; }
	restore_stock_source "$2" "${3:-}"
	;;
esac
