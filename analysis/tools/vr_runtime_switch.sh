#!/bin/bash
# Switch the user's OpenVR/OpenXR runtime config. Usage: vr_runtime_switch.sh xrizer|steamvr|status
# xrizer : openvrpaths.vrpath -> xrizer, OpenXR active_runtime -> our Monado build
# steamvr: openvrpaths.vrpath -> SteamVR (writable); the OpenXR runtime is left as it is
VRP=$HOME/.config/openvr/openvrpaths.vrpath
XRP=$HOME/.config/openxr/1/active_runtime.json
BK=$HOME/.config/pimax-vr-backup
XRIZER=${XRIZER:-$HOME/src/xrizer/target/release}
MONADO_JSON=${MONADO_JSON:-$HOME/src/monado/build/openxr_monado-dev.json}
mkdir -p "$BK" "$(dirname "$VRP")" "$(dirname "$XRP")"
case "$1" in
xrizer)
  # back up only once (never overwrite the original backup)
  if [ ! -e "$BK/.saved" ]; then
    for f in "$VRP" "$XRP"; do
      n=$(basename "$f")
      if [ -e "$f" ] || [ -L "$f" ]; then cp -a "$f" "$BK/$n"; else touch "$BK/$n.absent"; fi
    done
    touch "$BK/.saved"
  fi
  tmp="$VRP.tmp.$$"
  cat >"$tmp" <<J
{
	"jsonid": "vrpathreg",
	"runtime": [
		"$XRIZER"
	],
	"version": 1
}
J
  chmod 444 "$tmp" && mv -f "$tmp" "$VRP"
  ln -sfn "$MONADO_JSON" "$XRP"
  echo "OpenVR -> xrizer ($XRIZER); OpenXR -> $MONADO_JSON" ;;
steamvr)
  # The app uses `pimax vr games steamvr` (same rules). Works without a backup, never touches the OpenXR runtime,
  # leaves the file writable: SteamVR 2.17 exits with "Saving Path Registry failed" on a read-only one.
  SVR=$HOME/.local/share/Steam/steamapps/common/SteamVR
  [ -e "$SVR/bin/linux64/vrclient.so" ] || { echo "SteamVR is not installed ($SVR): install it in Steam"; exit 1; }
  [ -e "$VRP" ] && grep -q xrizer "$VRP" && cp -f "$VRP" "$VRP.pimax-xrizer"
  tmp="$VRP.tmp.$$"
  cat >"$tmp" <<J
{
	"config": [ "$HOME/.local/share/Steam/config" ],
	"external_drivers": null,
	"jsonid": "vrpathreg",
	"log": [ "$HOME/.local/share/Steam/logs" ],
	"runtime": [ "$SVR" ],
	"version": 1
}
J
  chmod 644 "$tmp" && mv -f "$tmp" "$VRP"
  echo "OpenVR -> SteamVR ($SVR), file writable; OpenXR runtime unchanged" ;;
status|*)
  echo "== $VRP"; cat "$VRP" 2>&1; echo "== $XRP"; ls -l "$XRP" 2>&1 ;;
esac
