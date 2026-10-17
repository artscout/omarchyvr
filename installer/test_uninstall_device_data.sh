#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Test: the uninstaller keeps the user's own per-device data (controller models, lens, IPD, eye calibration, controller
# fits, gyro bias, room map; also from the older ~/.config/pimax places) and removes the settings, and the settings
# backup never holds ai.toml. A dry run, then a REAL run in a scratch HOME with shims for systemctl/pgrep/pkexec (no
# password, no root job, the real home and the real user services are never touched).
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
fails=0
ok() { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=$((fails + 1)); }
T=$(mktemp -d "${TMPDIR:-/tmp}/uninstall-device-test.XXXXXX")
trap 'rm -rf "$T"' EXIT

# shims: no user service exists, nothing runs, no password
mkdir -p "$T/bin"
printf '#!/bin/sh\n[ "$2" = cat ] && exit 1\nexit 0\n' >"$T/bin/systemctl"
printf '#!/bin/sh\nexit 1\n' >"$T/bin/pgrep"
printf '#!/bin/sh\necho "pkexec must not run in this test" >&2\nexit 99\n' >"$T/bin/pkexec"
chmod +x "$T/bin/"*

H=$T/home
put() { mkdir -p "$(dirname "$1")"; printf '%s\n' "$2" >"$1"; }
put "$H/.config/omarchyvr/controllers/SN1/controller_params.json" '{"Hand":"Left"}'
put "$H/.config/omarchyvr/controllers/SN2/controller_params.json" '{"Hand":"Right"}'
put "$H/.config/omarchyvr/lens.conf" 'fov = 40 41 42 43'
put "$H/.config/omarchyvr/ipd.env" 'OXR_DEBUG_IPD_MM=64.0'
put "$H/.config/omarchyvr/eye-calibration.toml" 'version = 2'
put "$H/.config/omarchyvr/games.toml" 'runtime = "steamvr"'
put "$H/.config/omarchyvr/gui-state.toml" 'onboarded = true'
put "$H/.config/omarchyvr/ai.toml" 'api_key = "test-not-a-key"'
put "$H/.config/pimax/ctrl-imu.json" '{"c":[1,2,3]}'
put "$H/.cache/omarchyvr/gyro-bias" '0.1157 0.6422 0.3204'
put "$H/.local/share/omarchyvr/roommap/home/map.bin" 'MAP'
mkdir -p "$H/.local/state"

run() { # run ARGS...: the uninstaller with the scratch HOME and the shims
	env -u XDG_CONFIG_HOME -u XDG_DATA_HOME -u XDG_STATE_HOME -u XDG_CACHE_HOME HOME="$H" TMPDIR="$T" PATH="$T/bin:$PATH" \
		"$HERE/uninstall.sh" --no-root --yes "$@" 2>&1
}

out=$(run --dry-run)
for p in "$H/.config/omarchyvr/controllers" "$H/.config/omarchyvr/lens.conf" "$H/.config/omarchyvr/ipd.env" \
	"$H/.config/omarchyvr/eye-calibration.toml" "$H/.cache/omarchyvr/gyro-bias" "$H/.local/share/omarchyvr/roommap"; do
	grep -qF "keep      $p (your device data) -> $p" <<<"$out" && ok "dry run keeps $(basename "$p")" || bad "not kept: $p"
done
grep -qF "$H/.config/pimax/ctrl-imu.json (your device data) -> $H/.config/omarchyvr/ctrl-imu.json" <<<"$out" &&
	ok "an older place comes back in the current place" || bad "legacy ctrl-imu.json not kept: $out"
grep -q "reset .*$H/.config/omarchyvr\$" <<<"$out" && ok "the settings are still reset" || bad "settings not reset"
[[ -f $H/.config/omarchyvr/games.toml ]] && ok "the dry run changed nothing" || bad "the dry run removed a file"
out=$(run --dry-run --reset-calibration)
if grep -q "your device data" <<<"$out"; then bad "--reset-calibration still keeps device data"; else ok "--reset-calibration lets it go"; fi

out=$(run)
rc=$?
[[ $rc == 0 ]] && ok "the real run ends cleanly" || bad "real run rc=$rc: $(tail -8 <<<"$out")"
same() { [[ -f $1 && "$(cat "$1")" == "$2" ]]; }
same "$H/.config/omarchyvr/controllers/SN1/controller_params.json" '{"Hand":"Left"}' && ok "controller models kept" || bad "controller models lost"
same "$H/.config/omarchyvr/lens.conf" 'fov = 40 41 42 43' && ok "lens kept" || bad "lens lost"
same "$H/.config/omarchyvr/ipd.env" 'OXR_DEBUG_IPD_MM=64.0' && ok "IPD kept" || bad "IPD lost"
same "$H/.config/omarchyvr/eye-calibration.toml" 'version = 2' && ok "eye calibration kept" || bad "eye calibration lost"
same "$H/.config/omarchyvr/ctrl-imu.json" '{"c":[1,2,3]}' && ok "the older place's file is in the current place" || bad "legacy ctrl-imu.json lost"
same "$H/.cache/omarchyvr/gyro-bias" '0.1157 0.6422 0.3204' && ok "gyro bias kept" || bad "gyro bias lost"
same "$H/.local/share/omarchyvr/roommap/home/map.bin" 'MAP' && ok "room map kept" || bad "room map lost"
same "$H/.config/omarchyvr/ai.toml" 'api_key = "test-not-a-key"' && ok "ai.toml kept" || bad "ai.toml lost"
[[ ! -e $H/.config/omarchyvr/games.toml && ! -e $H/.config/omarchyvr/gui-state.toml ]] && ok "the settings are gone" || bad "settings left"
[[ ! -e $H/.config/pimax ]] && ok "the older config dir is gone" || bad "~/.config/pimax left"
tarf=$(ls "$H"/omarchyvr-settings-backup-*.tar.gz 2>/dev/null | head -1)
if [[ -n $tarf ]]; then
	list=$(tar -tzf "$tarf" 2>/dev/null)
	grep -q "games.toml" <<<"$list" && ok "the backup holds the settings" || bad "games.toml not in the backup"
	if grep -q "ai.toml" <<<"$list"; then bad "the backup holds ai.toml"; else ok "the backup never holds ai.toml"; fi
	[[ "$(stat -c %a "$tarf")" == 600 ]] && ok "the backup is mode 600" || bad "backup mode $(stat -c %a "$tarf")"
else
	bad "no settings backup written"
fi

echo
if [[ $fails == 0 ]]; then echo "all passed"; else echo "$fails failed"; exit 1; fi
