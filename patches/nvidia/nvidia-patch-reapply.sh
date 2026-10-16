#!/bin/bash
# Re-apply the Pimax NVIDIA patches to the installed nvidia-open-dkms source tree, in order:
#   1. nvidia-2h1or-viewport.patch               (right eye shows the right half: PR #1262)
#   2. nvidia-2h1or-flip-fix.patch               (merged_head_sync: flips on 2Heads1OR; adds the module parameter)
#   3. nvidia-evo-makeroom-bounded-wait.patch    (bounded wait in the EVO push-buffer make-room loop)
# Idempotent: a patch that is already in (patch -R --dry-run succeeds) is skipped.
#
# Why: the Pimax Dream Air needs 7104x3840 = "2 heads, 1 OR" on NVIDIA. Every nvidia-open-dkms upgrade replaces
# /usr/src/nvidia-<ver> with the pristine source, so the fixes have to be applied again BEFORE dkms builds the module.
# See linux/DISPLAY.md for the install commands (needs sudo).
#
# All patches are first applied to a scratch copy of the files they touch; the real tree is changed only if every
# patch applied (or was already in), so a conflict never leaves a half-patched tree.
#
# Usage: nvidia-patch-reapply.sh [--check] [--hook] [--rebuild] [--verify] [--version V] [--src-root DIR] [--patch FILE]...
#   --check       change nothing; exit 0 if all trees are patched, 2 if a tree needs a patch, 1 on conflict/errors
#   --hook        pacman hook mode: loud banner and a syslog line whenever the module would be built unpatched (exit 1)
#   --rebuild     after patching: dkms build + install --force for the running kernel, then rebuild the boot image
#                 (limine-mkinitcpio on Omarchy/Limine, else mkinitcpio -P), then --verify. The pacman hook does not
#                 need this: the dkms and initramfs hooks run after it.
#   --fallback    never leave the machine without a working module: if the patched dkms build fails (--rebuild),
#                 or (with --verify, the pacman hook after the dkms build) dkms has no installed module for the
#                 running kernel, put the stock files back (*.pimax-orig), build and install the stock module and
#                 the boot image, and report it loudly (exit 1). The desktop keeps working; the headset shows the
#                 left half in both eyes until the patches are fixed.
#   --force-build with --rebuild: build even when every patch was already in (the module lacked the fix)
#   --verify      only check the BUILT module: modinfo of nvidia-modeset must list the merged_head_sync parameter
#                 (exit 1 and a banner if not). Run after the dkms build (the 75 pacman hook does this).
#   --version V   only /usr/src/nvidia-V (default: every /usr/src/nvidia-<digits> tree)
#   --src-root D  directory holding the nvidia-<ver> trees (default /usr/src, or $NVIDIA_SRC_ROOT)
#   --patch FILE  patch to use, repeatable, applied in the order given (default: the three above, next to this script)
# Exit codes: 0 ok (already patched or patched now), 1 error or conflict, 2 --check found an unpatched tree.
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
SRC_ROOT=${NVIDIA_SRC_ROOT:-/usr/src}
PATCH_FILES=()
CHECK=0 HOOK=0 REBUILD=0 VERIFY=0 FALLBACK=0 FORCE_BUILD=0 ONLY_VERSION=""
VIEWPORT_TARGET=src/nvidia-modeset/src/nvkms-evo.c
FLIP_MARKER_FILE=kernel-open/nvidia-modeset/nvidia-modeset-linux.c # carries the merged_head_sync parameter
BACKUP_SUFFIX=.pimax-orig
# the commands --rebuild / --verify run (overridable so that the test never touches the real ones)
DKMS=${NVIDIA_PATCH_DKMS:-dkms}
LIMINE_MKINITCPIO=${NVIDIA_PATCH_LIMINE:-limine-mkinitcpio}
MKINITCPIO=${NVIDIA_PATCH_MKINITCPIO:-mkinitcpio}
MODINFO=${NVIDIA_PATCH_MODINFO:-modinfo}

while [ $# -gt 0 ]; do
	case $1 in
	--check) CHECK=1 ;;
	--hook) HOOK=1 ;;
	--rebuild) REBUILD=1 ;;
	--verify) VERIFY=1 ;;
	--fallback) FALLBACK=1 ;;
	--force-build) FORCE_BUILD=1 ;;
	--version) ONLY_VERSION=${2:?--version needs a value}; shift ;;
	--src-root) SRC_ROOT=${2:?--src-root needs a value}; shift ;;
	--patch) PATCH_FILES+=("${2:?--patch needs a value}"); shift ;;
	-h | --help) awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "${BASH_SOURCE[0]}"; exit 0 ;;
	*) echo "unknown option $1 (try --help)" >&2; exit 1 ;;
	esac
	shift
done
if [ ${#PATCH_FILES[@]} -eq 0 ]; then
	PATCH_FILES=("$HERE/nvidia-2h1or-viewport.patch" "$HERE/nvidia-2h1or-flip-fix.patch" "$HERE/nvidia-evo-makeroom-bounded-wait.patch")
fi

say() { echo "==> nvidia-patch-reapply: $*"; }

banner() { # banner WHAT REASON
	{
		echo "################################################################################"
		echo "# PIMAX: $1"
		echo "# ($2)"
		echo "# The nvidia module that dkms builds next is NOT fully patched: the Dream Air will show the"
		echo "# left half in both eyes and/or hang the display engine (Xid / \"Idling display engine\")."
		echo "# Check whether NVIDIA fixed this upstream (then nothing is needed) or regenerate the patches in"
		echo "# patches/nvidia/ of omarchyvr for the new source."
		echo "################################################################################"
	} >&2
	if [ "$HOOK" = 1 ] && [ -z "${NVIDIA_PATCH_NO_SYSLOG:-}" ] && command -v logger >/dev/null 2>&1; then
		logger -t pimax-nvidia-patch -p user.err "$1 ($2)" || true
	fi
}

# --verify: the built module must carry the flip-fix parameter
verify_module() {
	local kver out
	kver=$(uname -r)
	if ! out=$("$MODINFO" -k "$kver" nvidia-modeset 2>&1); then
		banner "cannot verify the nvidia-modeset module for $kver" "modinfo failed: ${out:0:200}"
		return 1
	fi
	if printf '%s\n' "$out" | grep -q 'merged_head_sync'; then
		say "verified: nvidia-modeset for $kver has the merged_head_sync parameter"
	else
		banner "nvidia-modeset for $kver has NO merged_head_sync parameter" "the module was built without nvidia-2h1or-flip-fix.patch"
		return 1
	fi
}

# restore_stock DIR: the *.pimax-orig files back in place (the stock source), then dkms build + install for the running
# kernel and the boot image. Used by --fallback only.
restore_stock() {
	local d=$1 ver o kver
	ver=${d##*/nvidia-}
	kver=$(uname -r)
	while IFS= read -r o; do
		mv -f -- "$o" "${o%"$BACKUP_SUFFIX"}"
	done < <(find "$d" -name "*$BACKUP_SUFFIX" -type f)
	say "fallback: stock source restored in $d; building the stock module for $kver"
	"$DKMS" build "nvidia/$ver" -k "$kver" --force && "$DKMS" install "nvidia/$ver" -k "$kver" --force || {
		banner "the STOCK nvidia module could not be built either" "dkms build/install nvidia/$ver failed"
		return 1
	}
	if command -v "$LIMINE_MKINITCPIO" >/dev/null 2>&1; then "$LIMINE_MKINITCPIO"; else "$MKINITCPIO" -P; fi
	banner "the patched nvidia module failed to build: the STOCK module is installed instead" "the desktop works; the headset needs the patches (see the pacman log)"
}
module_installed() { "$DKMS" status "nvidia/$1" -k "$(uname -r)" 2>/dev/null | grep -q 'installed'; }

if [ "$VERIFY" = 1 ]; then
	if [ "$FALLBACK" = 1 ]; then
		for d in "$SRC_ROOT"/nvidia-[0-9]*; do
			[ -d "$d" ] || continue
			case $d in *.orig | *.pimax-orig) continue ;; esac
			v=${d##*/nvidia-}
			if ! module_installed "$v" && [ -n "$(find "$d" -name "*$BACKUP_SUFFIX" -type f -print -quit)" ]; then
				restore_stock "$d" || true
				exit 1
			fi
		done
	fi
	verify_module
	exit $?
fi

command -v patch >/dev/null 2>&1 || { echo "patch(1) is not installed (pacman -S patch)" >&2; exit 1; }
# patch(1) is run with -d <dir>, which changes directory before it opens the -i file: make the paths absolute
for i in "${!PATCH_FILES[@]}"; do
	[ -f "${PATCH_FILES[$i]}" ] || { echo "patch file ${PATCH_FILES[$i]} not found" >&2; exit 1; }
	PATCH_FILES[$i]=$(readlink -f -- "${PATCH_FILES[$i]}")
done

# One run at a time: the pacman hook and a manual --rebuild must not both see "unpatched" and then fight over the
# backup files. The lock lives on the first patch file (readable by everybody, so --check works without root).
if command -v flock >/dev/null 2>&1; then
	exec 9<"${PATCH_FILES[0]}"
	flock 9
fi

# files touched by a patch (a/ b/ stripped)
patch_files_of() { sed -n 's|^+++ b/||p' "$1" | sed 's/\t.*//'; }

SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/nvpatch-scratch.XXXXXX")
trap 'rm -rf "$SCRATCH"' EXIT

pdry() { patch -p1 --dry-run --fuzz=0 -s -d "$1" -i "$2" >/dev/null 2>&1; }       # forward applies cleanly?
pdry_r() { patch -R -p1 --dry-run --fuzz=0 -s -d "$1" -i "$2" >/dev/null 2>&1; }  # already in?

# Plan the tree in a scratch copy. Sets PLAN_STATE[i] (patched|apply|fixed-upstream), PLAN_FILES (files to copy over),
# PLAN_ERR (reason, when it returns 1). Returns 0 ok, 1 conflict, 2 not a tree we can handle (missing target).
plan_tree() {
	local dir=$1 s=$SCRATCH/plan n=${#PATCH_FILES[@]} i f k p
	rm -rf "$s"
	mkdir -p "$s"
	PLAN_STATE=() PLAN_FILES=() PLAN_ERR=""
	[ -f "$dir/$VIEWPORT_TARGET" ] || return 2
	local all=()
	for p in "${PATCH_FILES[@]}"; do
		while IFS= read -r f; do all+=("$f"); done < <(patch_files_of "$p")
	done
	for f in $(printf '%s\n' "${all[@]}" | sort -u); do
		if [ -f "$dir/$f" ]; then
			mkdir -p "$s/$(dirname "$f")"
			cp -p "$dir/$f" "$s/$f"
		fi
	done
	# peel the trailing patches that are already in (a later patch may touch the same lines as an earlier one)
	k=$n
	while [ $k -gt 0 ] && pdry_r "$s" "${PATCH_FILES[$((k - 1))]}"; do
		patch -R -p1 --fuzz=0 -s -d "$s" -i "${PATCH_FILES[$((k - 1))]}" >/dev/null
		k=$((k - 1))
	done
	for ((i = k; i < n; i++)); do PLAN_STATE[$i]=patched; done
	for ((i = 0; i < k; i++)); do
		p=${PATCH_FILES[$i]}
		if pdry "$s" "$p"; then
			patch -p1 --fuzz=0 -s -d "$s" -i "$p" >/dev/null
			PLAN_STATE[$i]=apply
		elif pdry_r "$s" "$p"; then
			PLAN_STATE[$i]=patched
		elif [ "${p##*/}" = nvidia-2h1or-viewport.patch ] &&
			awk '/^void nvSetViewPortsEvo/,/^}/' "$s/$VIEWPORT_TARGET" | tr '\n' ' ' | grep -Eq 'EvoSetViewportPointIn\([^;]*mergeHeadSection'; then
			# the call (it may span two lines) no longer passes a constant x offset: upstream, or somebody, fixed it another way
			PLAN_STATE[$i]=fixed-upstream
		else
			PLAN_ERR="${p##*/} does not apply: the code it patches changed"
			return 1
		fi
	done
	# re-apply the peeled patches on top (they were in before, so this works unless an earlier patch was missing)
	for ((i = k; i < n; i++)); do
		if ! patch -p1 --fuzz=0 -s -d "$s" -i "${PATCH_FILES[$i]}" >/dev/null 2>&1; then
			PLAN_ERR="${PATCH_FILES[$i]##*/} was in, but no longer fits on top of the patches before it"
			return 1
		fi
	done
	# which real files change
	while IFS= read -r f; do
		[ -f "$s/$f" ] && ! cmp -s "$s/$f" "$dir/$f" && PLAN_FILES+=("$f")
	done < <(cd "$s" && find . -type f | sed 's|^\./||' | sort)
	return 0
}

trees=()
shopt -s nullglob
for d in "$SRC_ROOT"/nvidia-[0-9]*; do
	[ -d "$d" ] || continue
	case $d in *.orig | *.pimax-orig) continue ;; esac
	ver=${d##*/nvidia-}
	[ -z "$ONLY_VERSION" ] || [ "$ver" = "$ONLY_VERSION" ] || continue
	trees+=("$d")
done
shopt -u nullglob
if [ ${#trees[@]} -eq 0 ]; then
	if [ "$HOOK" = 1 ]; then
		# the hook runs because nvidia-open-dkms was just installed: a missing tree means the dkms build has nothing to patch
		banner "the Pimax NVIDIA patches could NOT be applied to $SRC_ROOT/nvidia-<version>" "no nvidia source tree found${ONLY_VERSION:+ for version $ONLY_VERSION}"
		exit 1
	fi
	say "no nvidia-<version> source tree in $SRC_ROOT${ONLY_VERSION:+ for version $ONLY_VERSION}: nothing to do"
	exit 0
fi

rc=0
patched_now=()
for d in "${trees[@]}"; do
	plan_rc=0
	plan_tree "$d" || plan_rc=$?
	if [ $plan_rc = 2 ]; then
		if [ "$HOOK" = 1 ] && [ -f "$d/dkms.conf" ]; then
			# a real dkms tree whose layout changed: dkms would build it unpatched
			banner "the Pimax NVIDIA patches could NOT be applied to $d" "$VIEWPORT_TARGET not found in a dkms source tree"
			rc=1
		else
			say "$d: $VIEWPORT_TARGET not found, skipped"
		fi
		continue
	elif [ $plan_rc = 1 ]; then
		banner "the Pimax NVIDIA patches could NOT be applied to $d" "$PLAN_ERR"
		rc=1
		continue
	fi
	todo=()
	for i in "${!PATCH_FILES[@]}"; do
		nm=${PATCH_FILES[$i]##*/}
		case ${PLAN_STATE[$i]} in
		patched) say "$d: $nm already patched" ;;
		fixed-upstream) say "$d: $nm: the source already passes a per-head x offset to EvoSetViewportPointIn; not patching. Verify with vk_display_test, then retire this patch." ;;
		apply) todo+=("$nm") ;;
		esac
	done
	if [ ${#todo[@]} -eq 0 ]; then
		say "$d: already patched"
		continue
	fi
	if [ "$CHECK" = 1 ]; then
		say "$d: NOT patched, needs: ${todo[*]} (they apply cleanly)"
		[ $rc -ne 1 ] && rc=2
		continue
	fi
	# the files are replaced in place and a .pimax-orig backup is created next to each
	nowrite=0
	for f in "${PLAN_FILES[@]}"; do
		[ -w "$d/$f" ] && [ -w "$d/$(dirname "$f")" ] || nowrite=1  # the directory too: the backup is created next to the file
	done
	if [ $nowrite = 1 ]; then
		echo "$d (a file to patch) is not writable: run as root (sudo)" >&2
		rc=1
		continue
	fi
	for f in "${PLAN_FILES[@]}"; do
		[ -e "$d/$f$BACKUP_SUFFIX" ] || cp -p "$d/$f" "$d/$f$BACKUP_SUFFIX"
		cp "$SCRATCH/plan/$f" "$d/$f"
	done
	# the real tree must now equal the plan, and carry the flip-fix parameter if that patch is in the list
	again=0
	plan_tree "$d" || again=1
	if [ $again = 0 ]; then
		for i in "${!PATCH_FILES[@]}"; do [ "${PLAN_STATE[$i]}" = apply ] && again=1; done
	fi
	if [ $again = 1 ]; then
		banner "the Pimax NVIDIA patches could NOT be applied to $d" "the files do not look patched after copying the result"
		rc=1
		continue
	fi
	for p in "${PATCH_FILES[@]}"; do
		if [ "${p##*/}" = nvidia-2h1or-flip-fix.patch ] && ! grep -q merged_head_sync "$d/$FLIP_MARKER_FILE" 2>/dev/null; then
			banner "the Pimax NVIDIA patches could NOT be applied to $d" "merged_head_sync is missing from $FLIP_MARKER_FILE after patching"
			rc=1
			continue 2
		fi
	done
	say "$d: patched ${todo[*]} (originals kept as <file>$BACKUP_SUFFIX)"
	patched_now+=("$d")
done

if [ "$REBUILD" = 1 ] && [ "$FORCE_BUILD" = 1 ] && [ "${#patched_now[@]}" -eq 0 ]; then
	patched_now=("${trees[@]}")
fi
if [ "$REBUILD" = 1 ] && [ "${#patched_now[@]}" -gt 0 ] && [ "$CHECK" = 0 ]; then
	for d in "${patched_now[@]}"; do
		ver=${d##*/nvidia-}
		say "dkms build + install nvidia/$ver for $(uname -r)"
		if ! { "$DKMS" build "nvidia/$ver" -k "$(uname -r)" --force && "$DKMS" install "nvidia/$ver" -k "$(uname -r)" --force; }; then
			if [ "$FALLBACK" = 1 ]; then
				restore_stock "$d" || true
				exit 1
			fi
			banner "dkms could not build the patched nvidia module" "see the dkms output above"
			exit 1
		fi
	done
	if command -v "$LIMINE_MKINITCPIO" >/dev/null 2>&1; then
		# Omarchy/Limine boots a UKI that contains the nvidia modules; /etc/mkinitcpio.d has no presets for mkinitcpio -P
		say "limine-mkinitcpio (rebuilds the UKI with the new nvidia modules)"
		"$LIMINE_MKINITCPIO"
	else
		say "mkinitcpio -P (the nvidia modules load from the initramfs)"
		"$MKINITCPIO" -P
	fi
	verify_module || rc=1
	say "done: reboot to load the patched nvidia modules"
fi
exit $rc
