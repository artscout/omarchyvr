#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Test of the prebuilt release path (docs/RELEASE.md): MANIFEST.txt write and verify, mode detection, a prebuilt install
# of a FAKE release into a temp prefix with a temp HOME (the real home, ~/.local and ~/.config are never touched; the
# prefix is not ~/.local, so every user-level step is only printed), the generated OpenXR runtime manifest, the
# uninstaller as a dry run, RUNPATH relocation and the absolute RUNPATHs of an install (needs gcc and patchelf; set
# OVR_PATCHELF=/path/to/patchelf when the PC has none), a full user-level install into a temp HOME with a fake systemctl
# (no user bus) and a fake curl (failed speech model download), and the package lists.
# No sudo, no docker, no VR session (a stand-in name keeps vr-busy.sh from looking at the real one).
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/.." && pwd)
fails=0
ok() { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=$((fails + 1)); }
skip() { echo "skip $*"; }
T=$(mktemp -d "${TMPDIR:-/tmp}/release-test.XXXXXX")
trap 'rm -rf "$T"' EXIT
# shellcheck source=lib.sh
. "$HERE/lib.sh"
# shellcheck source=../packaging/release/manifest.sh
. "$REPO/packaging/release/manifest.sh"

PATCHELF=$(command -v patchelf || true)
[[ -z $PATCHELF && -x ${OVR_PATCHELF:-/nonexistent} ]] && PATCHELF=$OVR_PATCHELF
REAL_ELF=0
if [[ -n $PATCHELF ]] && command -v gcc >/dev/null; then REAL_ELF=1; fi

# ---- a fake release: the real installer, tiny stand-ins for the built files --------------------------------------
make_release() { # make_release DIR
	local r=$1 b m l
	mkdir -p "$r/bin" "$r/lib/omarchyvr/monado/src/xrt/targets/"{service,openxr,libmonado} "$r/lib/omarchyvr/monado/steamvr-monado/bin/linux64" \
		"$r/lib/omarchyvr/basalt/lib" "$r/lib/omarchyvr/wayvr" "$r/lib/omarchyvr/deps" "$r/share/omarchyvr/data" "$r/tools" "$r/linux" "$r/packaging" "$r/patches"
	cp "$REPO/install.sh" "$REPO/uninstall.sh" "$r/"
	cp -a "$REPO/installer" "$r/installer"
	cp "$REPO/linux/omarchyvr.desktop" "$REPO/linux/omarchyvr.svg" "$REPO/linux/omarchyvr-keepalived.service" "$r/linux/"
	cp -a "$REPO/packaging/systemd" "$r/packaging/systemd"
	cp -a "$REPO/patches/nvidia" "$r/patches/nvidia"
	cp "$REPO/tools/vr-busy.sh" "$r/tools/"
	for b in "${OVR_BINS[@]}"; do printf '#!/bin/sh\necho %s\n' "$b" >"$r/bin/$b"; chmod 755 "$r/bin/$b"; done
	{ echo '#!/bin/sh'; for m in "${OVR_MARKERS[@]}"; do echo "# $m"; done; } >"$r/lib/omarchyvr/monado/src/xrt/targets/service/monado-service"
	chmod 755 "$r/lib/omarchyvr/monado/src/xrt/targets/service/monado-service"
	mkdir -p "$r/lib/omarchyvr/tools"
	if [[ $REAL_ELF == 1 ]]; then
		# the installer's own tool, and real ELF files with $ORIGIN RUNPATHs like the tarball's: monado-service (needs
		# deps/libfake.so.1) and the library itself
		cp "$PATCHELF" "$r/lib/omarchyvr/tools/patchelf"
		echo 'int fakefn(void){return 3;}' >"$T/fake.c"
		gcc -shared -fPIC -Wl,-soname,libfake.so.1 -o "$r/lib/omarchyvr/deps/libfake.so.1" "$T/fake.c"
		"$PATCHELF" --set-rpath '$ORIGIN' "$r/lib/omarchyvr/deps/libfake.so.1"
		{
			printf '__attribute__((used)) const char markers[] = ""'
			for m in "${OVR_MARKERS[@]}"; do printf ' "%s" "\\0"' "$m"; done
			printf ';\nint fakefn(void);\nint main(void){return fakefn()==3?0:1;}\n'
		} >"$T/svc.c"
		gcc -o "$r/lib/omarchyvr/monado/src/xrt/targets/service/monado-service" "$T/svc.c" -L"$r/lib/omarchyvr/deps" -l:libfake.so.1 -Wl,-rpath,'$ORIGIN/../../../../../deps'
	else
		printf '#!/bin/sh\nexit 0\n' >"$r/lib/omarchyvr/tools/patchelf"
		chmod 755 "$r/lib/omarchyvr/tools/patchelf"
	fi
	echo fake >"$r/lib/omarchyvr/monado/src/xrt/targets/openxr/libopenxr_monado.so"
	echo fake >"$r/lib/omarchyvr/monado/src/xrt/targets/libmonado/libmonado.so.25.1.0"
	ln -s libmonado.so.25.1.0 "$r/lib/omarchyvr/monado/src/xrt/targets/libmonado/libmonado.so.25"
	ln -s libmonado.so.25 "$r/lib/omarchyvr/monado/src/xrt/targets/libmonado/libmonado.so"
	echo '{}' >"$r/lib/omarchyvr/monado/steamvr-monado/driver.vrdrivermanifest"
	echo fake >"$r/lib/omarchyvr/monado/steamvr-monado/bin/linux64/driver_monado.so"
	printf 'PIMAX_CAM:BOOL=ON\n' >"$r/lib/omarchyvr/monado/CMakeCache.txt"
	echo fake >"$r/lib/omarchyvr/basalt/lib/libbasalt.so.2.0.1"
	ln -s libbasalt.so.2.0.1 "$r/lib/omarchyvr/basalt/lib/libbasalt.so"
	printf '#!/bin/sh\n' >"$r/lib/omarchyvr/wayvr/wayvr"; chmod 755 "$r/lib/omarchyvr/wayvr/wayvr"
	echo fake >"$r/lib/omarchyvr/deps/libopencv_core.so.500"
	echo fakecommit >"$r/share/omarchyvr/data/COMMIT"
	mkdir -p "$r/share/omarchyvr/data/installer" "$r/share/omarchyvr/data/linux" "$r/share/omarchyvr/data/patches"
	cp -a "$REPO/installer/." "$r/share/omarchyvr/data/installer/"
	cp -a "$REPO/linux/70-pimax.rules" "$r/share/omarchyvr/data/linux/" 2>/dev/null || true
	cp -a "$REPO/patches/nvidia" "$r/share/omarchyvr/data/patches/nvidia"
	printf 'version: 9.9.9\ncommit: 0123456789abcdef0123456789abcdef01234567\nbuilt: test\n' >"$T/header.txt"
	for m in "${OVR_MARKERS[@]}"; do echo "$m: 1"; done >"$T/markers.txt"
	printf 'core\tlibc.so.6\tsystem\nbundle\tlibopencv_core.so.500\t/usr/lib/libopencv_core.so.500\n' >"$T/libs.tsv"
	write_manifest "$r" "$T/header.txt" "$T/markers.txt" "$T/libs.tsv"
}
R=$T/omarchyvr-9.9.9
make_release "$R"

# ---- 1. mode detection ---------------------------------------------------------------------------------------------
[[ $(release_mode "$R") == prebuilt ]] && ok "a tree with MANIFEST.txt and lib/omarchyvr is a prebuilt release" || bad "release not detected"
[[ $(release_mode "$REPO") == source ]] && ok "the repository (no MANIFEST.txt) is a from-source install" || bad "repo detected as release"
mkdir -p "$T/halfway/lib/omarchyvr"
[[ $(release_mode "$T/halfway") == source ]] && ok "lib/omarchyvr without MANIFEST.txt is not a release" || bad "half release detected"
[[ $(manifest_field "$R" version) == 9.9.9 ]] && ok "manifest field: version 9.9.9" || bad "manifest_field"

# ---- 2. MANIFEST.txt verification ----------------------------------------------------------------------------------
verify_release "$R" 2>"$T/v.err" && ok "a fresh manifest verifies" || bad "fresh manifest fails: $(cat "$T/v.err")"
grep -q '^\[links\]$' "$R/MANIFEST.txt" && grep -q "libbasalt.so	libbasalt.so.2.0.1" "$R/MANIFEST.txt" && ok "links are listed with their targets" || bad "links missing from the manifest"
cp -a "$R" "$T/bad1"
echo tampered >>"$T/bad1/bin/omarchyvr"
verify_release "$T/bad1" 2>"$T/v.err" && bad "a changed file passed" || { grep -q "bin/omarchyvr" "$T/v.err" && ok "a changed file is refused (named)" || bad "no name in: $(cat "$T/v.err")"; }
cp -a "$R" "$T/bad2"
echo extra >"$T/bad2/bin/extra"
verify_release "$T/bad2" 2>"$T/v.err" && bad "an extra file passed" || { grep -q "bin/extra" "$T/v.err" && ok "a file that is not in the manifest is refused" || bad "extra: $(cat "$T/v.err")"; }
cp -a "$R" "$T/bad3"
rm "$T/bad3/lib/omarchyvr/basalt/lib/libbasalt.so"
ln -s elsewhere "$T/bad3/lib/omarchyvr/basalt/lib/libbasalt.so"
verify_release "$T/bad3" 2>"$T/v.err" && bad "a changed link passed" || ok "a link that points elsewhere is refused"
cp -a "$R" "$T/bad4"
rm "$T/bad4/lib/omarchyvr/wayvr/wayvr"
verify_release "$T/bad4" 2>"$T/v.err" && bad "a missing file passed" || ok "a missing file is refused"

# ---- 3. a prebuilt install into a temp prefix with a temp HOME -----------------------------------------------------
H=$T/home
P=$H/prefix # under the temp HOME: the root plan refuses a monado-service outside the user home
mkdir -p "$H"
run_install() {
	env -u XDG_CONFIG_HOME -u XDG_DATA_HOME -u XDG_STATE_HOME -u XDG_CACHE_HOME HOME="$H" TMPDIR="$T" OVR_SKIP_PACKAGES=1 \
		OMARCHYVR_VR_BUSY_PROCS=no-such-process-for-the-release-test OMARCHYVR_VR_BUSY_TEST_ROOTS=/nonexistent/ "$@"
}
if ! is_arch; then
	skip "this is not Arch: the installer's distro check refuses; install tests skipped"
else
	out=$(run_install "$R/install.sh" --prefix "$P" --no-app --no-root --no-downloads 2>&1)
	rc=$?
	if [[ $rc == 0 ]]; then ok "install.sh --prefix (prebuilt, detected from MANIFEST.txt) ends cleanly"; else bad "install rc=$rc: $(tail -25 <<<"$out")"; fi
	grep -q "prebuilt release 9.9.9" <<<"$out" && ok "it says it installs the prebuilt release" || bad "no prebuilt banner: $(head -3 <<<"$out")"
	grep -q "all files match MANIFEST.txt" <<<"$out" && ok "it verified the files first" || bad "no verification line"
	grep -q "forks are missing" <<<"$out" && bad "it asked for the forks" || ok "no forks, no compiler asked for"
	for b in "${OVR_BINS[@]}"; do [[ -x $P/bin/$b ]] || { bad "bin/$b not installed"; break; }; done
	[[ -x $P/bin/xr-pinchtest ]] && ok "all ${#OVR_BINS[@]} binaries are in the prefix" || bad "binaries"
	M=$P/lib/omarchyvr/monado
	[[ -x $M/src/xrt/targets/service/monado-service && -L $M/src/xrt/targets/libmonado/libmonado.so ]] && ok "Monado is installed in the build-tree layout (links kept)" || bad "monado layout"
	[[ -f $P/lib/omarchyvr/deps/libopencv_core.so.500 && -L $P/lib/omarchyvr/basalt/lib/libbasalt.so ]] && ok "bundled libraries and Basalt are installed" || bad "deps/basalt"
	J=$M/openxr_monado-dev.json
	if [[ -f $J ]]; then
		lp=$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["runtime"]["library_path"])' "$J")
		mp=$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["runtime"]["MND_libmonado_path"])' "$J")
		[[ $lp == "$M/src/xrt/targets/openxr/libopenxr_monado.so" && -f $lp ]] && ok "runtime manifest: library_path points into the prefix" || bad "library_path $lp"
		[[ $mp == "$M/src/xrt/targets/libmonado/libmonado.so.25.1.0" && -f $mp ]] && ok "runtime manifest: MND_libmonado_path points into the prefix" || bad "MND path $mp"
		grep -q "$HOME" "$J" && bad "the manifest holds the real home" || ok "the runtime manifest holds only paths of the temp prefix"
	else bad "no openxr_monado-dev.json written"; fi
	grep -q "^$(printf 'dir\t')$M" "$P/share/omarchyvr/manifest.tsv" && ok "Monado is recorded as a directory for the uninstaller" || bad "no dir record for monado"
	grep -q "^$(printf 'file\t')$P/bin/omarchyvr" "$P/share/omarchyvr/manifest.tsv" && ok "binaries are recorded with their checksum" || bad "no file record"
	[[ -f $P/share/omarchyvr/data/COMMIT && -x $P/share/omarchyvr/data/installer/uninstall.sh ]] && ok "the data dir (with the installer copy the app runs) is in place" || bad "data dir"
	grep -q "WOULD" <<<"$out" && ok "user-level registration is printed (WOULD), not done, in a staging prefix" || bad "no WOULD lines"
	[[ ! -e $H/.config && ! -e $H/.local ]] && ok "the temp HOME got no config and no ~/.local" || bad "the temp HOME was written: $(ls -A "$H")"
	# idempotent, and files of an older release go
	touch "$M/stale-from-older-release"
	out2=$(run_install "$R/install.sh" --prefix "$P" --no-app --no-root --no-downloads 2>&1) && ok "a second install ends cleanly" || bad "reinstall: $(tail -5 <<<"$out2")"
	[[ ! -e $M/stale-from-older-release ]] && ok "a file the new release does not have is removed" || bad "stale file kept"
	[[ -f $J ]] && ok "the runtime manifest is back after the reinstall" || bad "json lost on reinstall"
	# the same inode: an unchanged monado-service keeps its realtime permission
	i1=$(stat -c %i "$M/src/xrt/targets/service/monado-service")
	run_install "$R/install.sh" --prefix "$P" --no-app --no-root --no-downloads >/dev/null 2>&1
	[[ $i1 == "$(stat -c %i "$M/src/xrt/targets/service/monado-service")" ]] && ok "an unchanged monado-service keeps its inode (and its file capability)" || bad "monado-service was replaced"
	# ---- absolute RUNPATHs (the setcap/$ORIGIN bug of release 0.1.1: cap_sys_nice makes glibc ignore $ORIGIN) ----
	SVC_R=$R/lib/omarchyvr/monado/src/xrt/targets/service/monado-service
	SVC_I=$M/src/xrt/targets/service/monado-service
	if [[ $REAL_ELF == 1 ]]; then
		rrp() { "$PATCHELF" --print-rpath "$1"; }
		[[ $(rrp "$SVC_R") == '$ORIGIN/../../../../../deps' ]] && ok "the release tree stays relocatable (\$ORIGIN RUNPATH, untouched by the install)" || bad "release RUNPATH changed: $(rrp "$SVC_R")"
		verify_release "$R" 2>/dev/null && ok "the release still matches MANIFEST.txt after the install" || bad "the install changed the release tree"
		[[ $(rrp "$SVC_I") == "$P/lib/omarchyvr/deps" ]] && ok "installed monado-service: RUNPATH is the absolute, normalised path ($P/lib/omarchyvr/deps)" || bad "installed RUNPATH: $(rrp "$SVC_I")"
		[[ $(rrp "$P/lib/omarchyvr/deps/libfake.so.1") == "$P/lib/omarchyvr/deps" ]] && ok "installed library: \$ORIGIN became the absolute directory" || bad "lib RUNPATH: $(rrp "$P/lib/omarchyvr/deps/libfake.so.1")"
		left=$(find "$P" -type f -print0 | xargs -0 -r "$PATCHELF" --print-rpath 2>/dev/null | grep -c 'ORIGIN' || true)
		[[ $left == 0 ]] && ok "no installed ELF has \$ORIGIN in its RUNPATH" || bad "$left ELF files still have \$ORIGIN"
		"$SVC_I" && ok "the installed monado-service runs (finds libfake.so.1 through the absolute RUNPATH)" || bad "installed monado-service does not run"
		grep -q "monado-service" "$P/share/omarchyvr/runpath-rewrite.tsv" && ok "the rewrite is listed in share/omarchyvr/runpath-rewrite.tsv" || bad "no rewrite list"
		python3 -I "$HERE/runpath.py" check "$R" "$P" "$PATCHELF" 2>/dev/null && ok "runpath.py check passes on the install" || bad "runpath.py check failed"
		cp "$SVC_I" "$T/svc-broken" && "$PATCHELF" --set-rpath '$ORIGIN/x' "$T/svc-broken" && cp -a "$P" "$T/prefix-broken" && cp "$T/svc-broken" "$T/prefix-broken/lib/omarchyvr/monado/src/xrt/targets/service/monado-service"
		python3 -I "$HERE/runpath.py" check "$R" "$T/prefix-broken" "$PATCHELF" >/dev/null 2>&1 && bad "the check accepted a relative RUNPATH" || ok "the check refuses an installed file with \$ORIGIN in its RUNPATH"
		# patchelf is deterministic: identical input and arguments give identical bytes (this is what keeps the inode)
		cp "$SVC_R" "$T/d1"; cp "$SVC_R" "$T/d2"
		"$PATCHELF" --set-rpath "$P/lib/omarchyvr/deps" "$T/d1"; "$PATCHELF" --set-rpath "$P/lib/omarchyvr/deps" "$T/d2"
		cmp -s "$T/d1" "$T/d2" && cmp -s "$T/d1" "$SVC_I" && ok "patchelf is deterministic: two patched copies and the installed file are identical" || bad "patchelf output differs between runs"
	else
		skip "absolute RUNPATH checks: need gcc and patchelf (set OVR_PATCHELF)"
	fi
	# refusals
	cp -a "$R" "$T/rel-bad"
	echo x >>"$T/rel-bad/bin/omarchyvr"
	out3=$(run_install "$T/rel-bad/install.sh" --prefix "$H/prefix-bad" --no-app --no-root --no-downloads 2>&1) && bad "a damaged release was installed" || { grep -q "do not match MANIFEST.txt" <<<"$out3" && [[ ! -e $H/prefix-bad/bin ]] && ok "a damaged release is refused before anything is copied" || bad "damaged: $(tail -3 <<<"$out3")"; }
	cp -a "$R" "$T/rel-nomarker"
	sed -i 's/PIMAX_ANCHOR_GLIDE/PIMAX_ANCHOR_GLID_/' "$T/rel-nomarker/lib/omarchyvr/monado/src/xrt/targets/service/monado-service"
	write_manifest "$T/rel-nomarker" "$T/header.txt" "$T/markers.txt" "$T/libs.tsv"
	out4=$(run_install "$T/rel-nomarker/install.sh" --prefix "$H/prefix-nm" --no-app --no-root --no-downloads 2>&1) && bad "a release without a marker was installed" || { grep -q "marker 'PIMAX_ANCHOR_GLIDE' missing" <<<"$out4" && ok "a Monado without a marker is refused" || bad "marker: $(tail -3 <<<"$out4")"; }
	out5=$(run_install "$R/install.sh" --from-source --prefix "$H/prefix-fs" --no-app --no-root --no-downloads 2>&1) && bad "--from-source worked without forks" || { grep -q "forks are missing" <<<"$out5" && ok "--from-source in a release asks for the forks" || bad "from-source: $(tail -3 <<<"$out5")"; }
	out6=$(run_install "$REPO/install.sh" --help 2>&1)
	{ grep -q -- "--from-source" <<<"$out6" && grep -q -- "--stage-only" <<<"$out6"; } && ok "--help lists --from-source and --stage-only" || bad "help lacks the new options"
	out7=$(run_install "$REPO/install.sh" --stage-only 2>&1) && bad "--stage-only without --prefix ran" || { grep -q "needs --prefix" <<<"$out7" && ok "--stage-only needs an explicit --prefix (never ~/.local)" || bad "stage-only: $(tail -2 <<<"$out7")"; }
	out8=$(run_install "$R/install.sh" --stage-only --prefix "$H/prefix-so" 2>&1) && bad "--stage-only ran inside a release" || { grep -q "builds from source" <<<"$out8" && ok "--stage-only refuses to run inside a prebuilt release" || bad "stage-only in release: $(tail -2 <<<"$out8")"; }
	# ---- a full user-level install (prefix = $HOME/.local) in a temp HOME: no user bus, a speech model that cannot be
	# downloaded. The fake systemctl and curl keep every call away from the real user manager and the network.
	if pgrep -x monado-service >/dev/null 2>&1 || [[ -n $(pacman -T "${OVR_RUNTIME_PKGS[@]}" 2>/dev/null) ]]; then
		skip "user-level install: a VR session runs, or runtime packages are missing on this PC"
	else
		H2=$T/home2
		mkdir -p "$H2" "$T/fakebin"
		printf '#!/bin/sh\necho "systemctl $*" >>"%s/systemctl.log"\necho "Failed to connect to bus: No medium found" >&2\nexit 1\n' "$T" >"$T/fakebin/systemctl"
		printf '#!/bin/sh\necho "curl $*" >>"%s/curl.log"\necho "curl: (6) Could not resolve host" >&2\nexit 6\n' "$T" >"$T/fakebin/curl"
		chmod 755 "$T/fakebin/systemctl" "$T/fakebin/curl"
		out9=$(env -u XDG_CONFIG_HOME -u XDG_DATA_HOME -u XDG_STATE_HOME -u XDG_CACHE_HOME HOME="$H2" PATH="$T/fakebin:$PATH" TMPDIR="$T" \
			OMARCHYVR_VR_BUSY_PROCS=no-such-process-for-the-release-test OMARCHYVR_VR_BUSY_TEST_ROOTS=/nonexistent/ \
			"$R/install.sh" --no-app --no-root 2>&1)
		rc=$?
		[[ $rc == 0 ]] && ok "no user systemd bus: the install still ends cleanly" || bad "install without a user bus: rc=$rc: $(tail -12 <<<"$out9")"
		grep -q "no systemd user session here" <<<"$out9" && ok "it says the user services are enabled at the next desktop login" || bad "no user-bus warning"
		grep -q "7/7 Done" <<<"$out9" && ok "the steps after the failed daemon-reload still ran (root plan, done)" || bad "install stopped early: $(tail -4 <<<"$out9")"
		[[ -s $H2/.local/share/omarchyvr/root-plan.env ]] && ok "the root plan was written after the bus failure" || bad "no root plan"
		grep -q "speech model: the download failed" <<<"$out9" && grep -q "run ./install.sh again" <<<"$out9" && ok "a failed speech model download warns and says how to repeat it" || bad "no speech-model warning"
		[[ ! -e $H2/models/parakeet-tdt-0.6b-v3-int8 && ! -e $H2/.cache/omarchyvr/build/dl/parakeet.tar.bz2 ]] && ok "no partial or bad speech model is kept" || bad "a speech model file was kept"
		grep -q "voice activity model: not available" <<<"$out9" && ok "a failed voice activity model download warns too" || bad "no VAD warning"
		# a bad checksum: a tarball that downloads fine but is not the model
		printf '#!/bin/sh\nfor a; do last=$a; done; o=""; while [ $# -gt 0 ]; do [ "$1" = -o ] && o=$2; shift; done\nmkdir -p "%s/fakemodel/x-parakeet-tdt-0.6b-v3-int8"; echo bad >"%s/fakemodel/x-parakeet-tdt-0.6b-v3-int8/encoder.int8.onnx"\ntar -cjf "$o" -C "%s/fakemodel" x-parakeet-tdt-0.6b-v3-int8\n' "$T" "$T" "$T" >"$T/fakebin/curl"
		out10=$(env -u XDG_CONFIG_HOME -u XDG_DATA_HOME -u XDG_STATE_HOME -u XDG_CACHE_HOME HOME="$H2" PATH="$T/fakebin:$PATH" TMPDIR="$T" \
			OMARCHYVR_VR_BUSY_PROCS=no-such-process-for-the-release-test OMARCHYVR_VR_BUSY_TEST_ROOTS=/nonexistent/ \
			"$R/install.sh" --no-app --no-root 2>&1) && ok "a speech model with a wrong checksum does not abort the install" || bad "bad checksum aborted: $(tail -6 <<<"$out10")"
		grep -q "does not match its checksum (refused, not kept)" <<<"$out10" && [[ ! -e $H2/models/parakeet-tdt-0.6b-v3-int8 ]] && ok "the bad model is refused and not kept" || bad "bad model: $(grep -i speech <<<"$out10")"
	fi

	# ---- 4. the uninstaller removes a prebuilt install (dry run only: a real run would talk to the real user services) ----
	un=$(run_install "$R/uninstall.sh" --dry-run --no-root --yes --prefix "$P" 2>&1)
	grep -q "remove .*$M" <<<"$un" && ok "uninstall (dry run) removes the prebuilt Monado directory" || bad "uninstall plan: $(head -12 <<<"$un")"
	grep -q "remove .*$P/bin/omarchyvr" <<<"$un" && grep -q "remove .*$P/lib/omarchyvr/deps" <<<"$un" && ok "uninstall (dry run) removes the binaries and the bundled libraries" || bad "uninstall plan incomplete"
fi

# ---- 5. RUNPATH relocation (bundle-libs.py) ------------------------------------------------------------------------
BL=$REPO/packaging/release/bundle-libs.py
o=$(python3 -I "$BL" origin /r/lib/omarchyvr/monado/steamvr-monado/bin/linux64/driver_monado.so /r/lib/omarchyvr/deps /r/lib/omarchyvr/onnxruntime/lib)
[[ $o == '$ORIGIN/../../../../deps:$ORIGIN/../../../../onnxruntime/lib' ]] && ok "origin: driver_monado.so -> deps and onnxruntime" || bad "origin $o"
o=$(python3 -I "$BL" origin /r/bin/omarchyvr-app /r/lib/omarchyvr/deps)
[[ $o == '$ORIGIN/../lib/omarchyvr/deps' ]] && ok "origin: bin/ -> lib/omarchyvr/deps" || bad "origin $o"
o=$(python3 -I "$BL" origin /r/lib/omarchyvr/deps/libx.so /r/lib/omarchyvr/deps)
[[ $o == '$ORIGIN' ]] && ok "origin: a bundled library finds its neighbours with \$ORIGIN" || bad "origin $o"
PATCHELF=$(command -v patchelf || true)
[[ -z $PATCHELF && -x ${OVR_PATCHELF:-/nonexistent} ]] && PATCHELF=$OVR_PATCHELF
if [[ -z $PATCHELF ]] || ! command -v gcc >/dev/null; then
	skip "relocation run: needs gcc and patchelf (the release container has both; or set OVR_PATCHELF)"
else
	X=$T/reloc
	mkdir -p "$X/b" "$X/src" "$X/tree/bin" "$X/tree/lib/omarchyvr/onnxruntime/lib"
	# a private library of the tree (like onnxruntime) found through an absolute build-time RUNPATH, and a bundled system library
	echo 'int priv(void){return 7;}' >"$X/src/priv.c"
	echo 'int priv(void); int BZ2_bzlibVersion(void); int main(void){return priv()==7?0:1;}' >"$X/src/m.c"
	gcc -shared -fPIC -Wl,-soname,libpriv.so.1 -o "$X/tree/lib/omarchyvr/onnxruntime/lib/libpriv.so.1" "$X/src/priv.c"
	ln -s libpriv.so.1 "$X/tree/lib/omarchyvr/onnxruntime/lib/libpriv.so"
	if gcc -o "$X/tree/bin/prog" "$X/src/m.c" -L"$X/tree/lib/omarchyvr/onnxruntime/lib" -lpriv -Wl,-rpath,"$X/tree/lib/omarchyvr/onnxruntime/lib" -Wl,--no-as-needed -l:libbz2.so.1.0 2>/dev/null; then
		PATH=$(dirname "$PATCHELF"):$PATH python3 -I "$BL" bundle "$X/tree" --report "$X/report.tsv" >"$X/bundle.out" 2>&1 && ok "bundle-libs bundle runs" || bad "bundle: $(cat "$X/bundle.out")"
		rp=$(readelf -d "$X/tree/bin/prog" | sed -n 's/.*\(RUNPATH\|RPATH\).*\[\(.*\)\]/\2/p')
		[[ $rp == *'$ORIGIN/../lib/omarchyvr/onnxruntime/lib'* && $rp == *'$ORIGIN/../lib/omarchyvr/deps'* ]] && ok "RUNPATH is \$ORIGIN-relative ($rp)" || bad "RUNPATH $rp"
		grep -q "$X" <<<"$rp" && bad "the build path is still in the RUNPATH" || ok "the build-time absolute RUNPATH is gone"
		[[ -f $X/tree/lib/omarchyvr/deps/libbz2.so.1.0 ]] && ok "the non-core system library was bundled into deps/" || bad "libbz2 not bundled"
		awk -F'\t' '$1=="core" && $2 ~ /^libc\.so/ {f=1} END{exit !f}' "$X/report.tsv" && ok "libc stays a system (core) library in the decision list" || bad "decision list: $(cat "$X/report.tsv")"
		mv "$X/tree" "$X/moved-elsewhere"
		rm -rf "$X/b"
		"$X/moved-elsewhere/bin/prog" && ok "the moved tree runs (libraries found relative to the binary)" || bad "moved tree does not run"
		python3 -I "$BL" check "$X/moved-elsewhere" >"$X/check.out" 2>&1 && ok "bundle-libs check passes on the moved tree" || bad "check: $(cat "$X/check.out")"
		# and fails when a build path is left in a RUNPATH
		"$PATCHELF" --set-rpath /nonexistent/build/path "$X/moved-elsewhere/bin/prog"
		python3 -I "$BL" check "$X/moved-elsewhere" >"$X/check.out" 2>&1 && bad "check accepted an absolute RUNPATH" || ok "bundle-libs check refuses an absolute RUNPATH"
	else skip "relocation run: gcc could not link against libbz2 here"; fi
fi

# ---- 6. package lists and the container -----------------------------------------------------------------------------
for p in git cmake ninja rust boost opencv qt6-base; do
	[[ " ${OVR_BUILD_PKGS[*]} " == *" $p "* ]] || { bad "OVR_BUILD_PKGS lacks $p"; break; }
done
[[ " ${OVR_BUILD_PKGS[*]} " == *" base-devel "* ]] && ok "build packages: the compiler stack and the libraries (from source and the release container)" || bad "build list"
overlap=""
for p in cmake ninja rust clang boost opencv qt6-base eigen glslang shaderc; do [[ " ${OVR_RUNTIME_PKGS[*]} " == *" $p "* ]] && overlap+="$p "; done
[[ -z $overlap ]] && ok "runtime packages hold no build tool and none of the libraries that are bundled" || bad "runtime list has build/bundled packages: $overlap"
grep -q 'OVR_BUILD_PKGS' "$REPO/packaging/release/Dockerfile" && grep -q 'OVR_RUNTIME_PKGS' "$REPO/packaging/release/Dockerfile" && ok "the Dockerfile reads its package lists from installer/lib.sh (one source)" || bad "Dockerfile does not use lib.sh lists"
if command -v pacman >/dev/null && pacman -Si bash >/dev/null 2>&1; then
	miss=""
	for p in "${OVR_RUNTIME_PKGS[@]}" "${OVR_BUILD_PKGS[@]}"; do pacman -Si "$p" >/dev/null 2>&1 || miss+="$p "; done
	[[ -z $miss ]] && ok "every listed package exists in the Arch repositories" || bad "unknown packages: $miss"
else skip "package names: no pacman sync database here"; fi

echo
if [ "$fails" = 0 ]; then echo "all checks passed"; else echo "$fails check(s) FAILED"; fi
[ "$fails" = 0 ]
