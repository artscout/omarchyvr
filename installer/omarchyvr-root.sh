#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# omarchyvr-root.sh: the ONE root job of an omarchyvr install or uninstall (run through one pkexec prompt).
#
#   omarchyvr-root.sh plan      PLAN.env   print exactly what `apply` would change (any user, changes nothing)
#   omarchyvr-root.sh apply     PLAN.env   do it (root); writes /var/lib/omarchyvr/manifest.tsv
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
#   OVR_NVIDIA                  patch | skip   (install.sh decides: tested version + open modules + headset or opt-in)
#   OVR_NVIDIA_VERSION          the exact nvidia-open-dkms version (must be in OVR_NVIDIA_TESTED)
#   OVR_NVIDIA_TESTED           tested versions (space separated)
#   OVR_EDID                    the NVKMS override key, e.g. override.[0000:01:00.0].DP-0 (detected; empty = no EDID step)
#   OVR_STEAMVR_LAUNCHER        SteamVR's vrcompositor-launcher (empty = skip); gets cap_sys_nice=eip like SteamVR's own prompt
#   OVR_SNAPSHOT                yes | no  (snapper snapshot of / first, when snapper has a "root" config)
#   OVR_KEEP_LEGACY_CAPS        yes | no  (uninstall: leave the caps of old Monado builds in ~/src alone)
#
# Safety: fixed PATH; every write goes to a fixed list of destinations; originals that are not ours are saved under
# /var/lib/omarchyvr/backup before they are replaced and put back on uninstall; NVIDIA is patched only on a tested
# version and falls back to the stock module if the patched build fails, so the desktop keeps working.
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
MODPROBE=/etc/modprobe.d/nvidia-pimax.conf
MKINITCPIO=/etc/mkinitcpio.conf.d/pimax.conf

MODE=${1:-}
PLAN=${2:-}
DRY=1
case $MODE in
apply | uninstall) DRY=0 ;;
plan | uninstall-plan) DRY=1 ;;
*) sed -n '3,25p' "$0"; exit 2 ;;
esac
if [[ $DRY == 0 && $EUID -ne 0 ]]; then
	echo "omarchyvr-root.sh $MODE must run as root (the app runs it through pkexec)" >&2
	exit 1
fi

# ---- the plan ------------------------------------------------------------------------------------------------------
OVR_USER="" OVR_HOME="" OVR_DATA="" OVR_SERVICE="" OVR_PACKAGES="" OVR_NVIDIA=skip OVR_NVIDIA_VERSION=""
OVR_NVIDIA_TESTED="" OVR_EDID="" OVR_STEAMVR_LAUNCHER="" OVR_SNAPSHOT=yes OVR_KEEP_LEGACY_CAPS=no
if [[ -n $PLAN ]]; then
	[[ -f $PLAN ]] || { echo "plan $PLAN not found" >&2; exit 1; }
	while IFS= read -r line || [[ -n $line ]]; do
		[[ -z $line || $line == \#* ]] && continue
		k=${line%%=*} v=${line#*=}
		v=${v#\"} v=${v%\"}
		[[ $v =~ ^[][A-Za-z0-9_./:@+\ -]*$ ]] || { echo "plan: unsafe value for $k" >&2; exit 1; }
		case $k in
		OVR_USER | OVR_HOME | OVR_DATA | OVR_SERVICE | OVR_PACKAGES | OVR_NVIDIA | OVR_NVIDIA_VERSION | OVR_NVIDIA_TESTED | \
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
save_original() { # keep a file that is not ours before replacing it (first original wins)
	local p=$1 b
	b=$(backup_of "$p")
	[[ -e $p ]] || return 0
	in_manifest "$p" && return 0
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
		info "would write $dst ($note)${dst:+$([[ -e $dst ]] && echo ', the current file is saved first')}"
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
		local v=$OVR_NVIDIA_VERSION
		step "NVIDIA $v (tested): display patches for the headset's two-head mode, re-applied after driver updates"
		[[ " $OVR_NVIDIA_TESTED " == *" $v "* ]] || { echo "refusing: $v is not a tested version" >&2; exit 1; }
		local f
		for f in nvidia-patch-reapply.sh nvidia-2h1or-viewport.patch nvidia-2h1or-flip-fix.patch nvidia-evo-makeroom-bounded-wait.patch; do
			place "$d/patches/nvidia/$f" "$LIBDIR/$f" "$([[ $f == *.sh ]] && echo 755 || echo 644)" "nvidia patch set" || true
		done
		place "$d/patches/nvidia/nvidia-pimax-patch.hook" "$HOOK_PATCH" 644 "pacman hook: re-apply after driver updates" || true
		place "$d/patches/nvidia/nvidia-pimax-verify.hook" "$HOOK_VERIFY" 644 "pacman hook: verify, stock fallback" || true
		if [[ -d $LEGACY_LIBDIR ]]; then
			act "remove the older copy $LEGACY_LIBDIR (the hooks now use $LIBDIR)" -- rm -r -- "$LEGACY_LIBDIR"
		fi
		# which fixes the source still needs (a newer source may already contain them: they are skipped)
		local chk rc=0
		chk=$("$d/patches/nvidia/nvidia-patch-reapply.sh" --check --version "$v" 2>&1) || rc=$?
		printf '%s\n' "$chk" | sed 's/^/      /'
		if [[ $rc == 2 ]]; then
			act "patch /usr/src/nvidia-$v (originals kept as *.pimax-orig), dkms build + install; stock module if the build fails" -- \
				"$LIBDIR/nvidia-patch-reapply.sh" --version "$v" --rebuild --fallback
			[[ $DRY == 0 ]] && mrec nvidia "/usr/src/nvidia-$v" "patched; uninstall restores the *.pimax-orig files and rebuilds stock"
			REBOOT=1
		elif [[ $rc == 0 ]]; then
			info "source already patched (or fixed upstream): nothing to rebuild"
			[[ $DRY == 0 ]] && mrec nvidia "/usr/src/nvidia-$v" "already patched before this install"
			if ! "$LIBDIR/nvidia-patch-reapply.sh" --verify >/dev/null 2>&1 && [[ $DRY == 0 ]]; then
				act "the built module lacks the fix: dkms rebuild" -- "$LIBDIR/nvidia-patch-reapply.sh" --version "$v" --rebuild --force-build --fallback
				REBOOT=1
			fi
		else
			echo "      the patches do not apply to this source: nothing changed (see the message above)" >&2
		fi
		if [[ -n $OVR_EDID ]]; then
			step "Headset display mode at boot: EDID override for ${OVR_EDID#override.} (non-desktop, native modes)"
			local ch=0
			place "$d/linux/pimax-dp-edid-msvsdb.bin" "$EDID_DIR/dp-edid.bin" 644 "headset EDID" && ch=1
			text_to "$EDID_DIR/nvidia-modeset.conf" "EDID override key" "$OVR_EDID=$EDID_DIR/dp-edid.bin" && ch=1
			text_to "$MODPROBE" "nvidia-modeset options" "options nvidia-modeset config_file=$EDID_DIR/nvidia-modeset.conf" && ch=1
			text_to "$MKINITCPIO" "initramfs files" "FILES+=($EDID_DIR/nvidia-modeset.conf $EDID_DIR/dp-edid.bin)" && ch=1
			if [[ $ch == 1 ]]; then INITRAMFS=1; REBOOT=1; fi
		fi
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

# ==== uninstall ======================================================================================================
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
	local src v restored=0 o
	for src in /usr/src/nvidia-[0-9]*; do
		[[ -d $src ]] || continue
		case $src in *.orig | *.pimax-orig) continue ;; esac
		v=${src##*/nvidia-}
		mapfile -t origs < <(find "$src" -name '*.pimax-orig' -type f 2>/dev/null)
		if [[ ${#origs[@]} -gt 0 ]]; then
			for o in "${origs[@]}"; do act "restore ${o%.pimax-orig} (stock)" -- mv -f -- "$o" "${o%.pimax-orig}"; done
			act "dkms build + install nvidia/$v for $(uname -r) (stock module)" -- sh -c "dkms build nvidia/$v -k $(uname -r) --force && dkms install nvidia/$v -k $(uname -r) --force"
			restored=1
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
	echo "REBOOT=$([[ $REBOOT == 1 ]] && echo yes || echo no)"
}

case $MODE in
plan | apply) do_apply ;;
uninstall-plan | uninstall) do_uninstall ;;
esac
