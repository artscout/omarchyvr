#!/usr/bin/env bash
# vr-busy.sh: is a VR session running? Builds, tests, benches and AI audits must never run during one: they share the
# CPU and the graphics card with the compositor and made VR drop frames (8 % on 2026-10-16; SteamVR dropped 5610 of
# 7577 frames on 2026-10-17 14:43-14:46 while agents' cargo builds ran; docs/research/perf-2026-10-17.md,
# ai/CLAUDE.md section 4, ai/LESSONS.md).
#
#   tools/vr-busy.sh                  exit 0 = free, 1 = a session runs (says what runs)
#   tools/vr-busy.sh --wait [MAX_S]   check every 60 s until free (exit 0), or give up after MAX_S seconds (exit 1)
#   tools/vr-busy.sh --guard CMD...   run CMD (at nice 19) only when free; during a session exit 75 with a message.
#                                     .cargo/config.toml makes it cargo's rustc-wrapper and test/run runner, so every
#                                     cargo build, test and run inside this repo refuses while VR runs.
#
# OMARCHYVR_ALLOW_DURING_VR=1 skips the check. Only for a run the USER asked for during a session; agents never set it.
# OMARCHYVR_VR_BUSY_PROCS overrides the process names and OMARCHYVR_VR_BUSY_POLL_S the 60 s poll (tests).
set -u

procs=${OMARCHYVR_VR_BUSY_PROCS:-"monado-service vrserver vrcompositor"}

running() { # prints "name pid ..." of each running VR process, returns 0 when one runs
	local out="" p pids
	for p in $procs; do
		pids=$(pgrep -x "$p" 2>/dev/null | tr '\n' ' ')
		[[ -n $pids ]] && out+="$p (pid ${pids% }) "
	done
	[[ -n $out ]] && { printf '%s\n' "${out% }"; return 0; }
	return 1
}

busy_msg() {
	echo "vr-busy: a VR session is running: $1." >&2
	echo "vr-busy: builds, tests and benches wait until it ends (they make VR drop frames). Run again after the session," >&2
	echo "vr-busy: or use 'tools/vr-busy.sh --wait' to wait for it. (OMARCHYVR_ALLOW_DURING_VR=1 only if the user asked.)" >&2
}

allowed() { [[ ${OMARCHYVR_ALLOW_DURING_VR:-0} == 1 ]]; }

case "${1:-}" in
"")
	if what=$(running); then
		echo "busy: $what"
		exit 1
	fi
	echo "free"
	exit 0
	;;
--wait)
	max=${2:-0}
	start=$SECONDS
	while what=$(running); do
		if [[ $max -gt 0 && $((SECONDS - start)) -ge $max ]]; then
			echo "vr-busy: still running after ${max} s: $what" >&2
			exit 1
		fi
		sleep "${OMARCHYVR_VR_BUSY_POLL_S:-60}"
	done
	exit 0
	;;
--guard)
	shift
	[[ $# -gt 0 ]] || { echo "vr-busy: --guard needs a command" >&2; exit 2; }
	if ! allowed && what=$(running); then
		busy_msg "$what"
		exit 75
	fi
	exec nice -n 19 "$@"
	;;
-h | --help)
	sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'
	exit 0
	;;
*)
	echo "vr-busy: unknown option $1 (see --help)" >&2
	exit 2
	;;
esac
