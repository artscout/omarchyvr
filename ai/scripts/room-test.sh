#!/usr/bin/env bash
# Room map live test: map the room, restart the desk session, recognise the room and restore the world origin.
#   tools/room-test.sh            build the map (if missing), restart, relocalize + apply
#   tools/room-test.sh --rebuild  map the room again first
#   tools/room-test.sh --reloc    only relocalize + apply in the running session
# Env: ROOM_NAME (home), MAP_SECONDS (90), DESK_ARGS (the desk start flags).
set -euo pipefail
NAME=${ROOM_NAME:-home}
SECS=${MAP_SECONDS:-90}
DESK_ARGS=${DESK_ARGS:---6dof --ctrl-track --preset work --anchor}
RM=~/pimax-re/rust/target/release/pimax-roommap
MAP=~/.local/share/pimax/roommap/$NAME/map.bin
mode=${1:-}

say() { printf '\n\033[1;34m== %s\033[0m\n' "$*"; command -v notify-send >/dev/null && notify-send -t 4000 "Room test" "$*" || true; }
countdown() { for ((i=$1; i>0; i--)); do printf '\r   %s in %2d s ' "$2" "$i"; sleep 1; done; printf '\r%*s\r' 40 ''; }

pimax desk status >/dev/null 2>&1 && pgrep -x monado-service >/dev/null || { echo "no desk session running; start it first: pimax desk start $DESK_ARGS"; exit 1; }

reloc() {
  say "Recognising the room: look around slowly (about 15 s)"
  "$RM" relocalize --source broker --seconds 15 --name "$NAME" --apply
  say "Done. Screens should be back in the same real place"
}

if [[ $mode == --reloc ]]; then reloc; exit; fi

if [[ ! -f $MAP || $mode == --rebuild ]]; then
  say "Mapping the room for $SECS s: walk slowly, look at every wall, return to where you started"
  countdown 5 "mapping starts"
  "$RM" build --source broker --seconds "$SECS" --name "$NAME"
  "$RM" info --name "$NAME" || true
fi

say "Note where your screens are now. Restarting the desk session"
countdown 10 "restart"
pimax desk stop
pimax desk start $DESK_ARGS
countdown 10 "relocalize"
reloc
