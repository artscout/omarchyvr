#!/bin/bash
# Pupil-swim and distortion residual check: runs the lens test client with the `swim` (world-locked room), `grid` and
# `flat` (colour balance) patterns. Controllers are off so the keys below go to THIS terminal.
#
# swim: look at the centre of the room and turn your head slowly left/right and up/down. The lines must stay straight
# and still; if they bend or bulge as they move toward the edge, that is a barrel/pincushion residual.
# Rendering is CPU-side every frame: if the room lags behind your head, add --res-div 2.
#
# Keys (type them in THIS terminal), for grid and swim:
#   ;  /  '     dk1 (radial residual, like lens.conf k1) - / + 0.01        :  /  "   the same in steps of 0.05
#   -  /  =     residual scale - / + 0.5 %                                  _  /  +   the same in steps of 2 %
#   c           swim: put the room around the head's current position and heading
#   r           reset the tuning (and the align offsets)
#   n / p       next / previous pattern      . / ,  step (flat: white 255, grey 192/128/64, red, green, blue)
#   h           text on/off (flat: only a small level name in one corner)
#   q           quit: prints the equivalent lens.conf scale and k_g/k_r/k_b (pre-warp fitted into the lens model)
# The tuning is a pre-warp of the client image: lens.conf and the compositor are not changed. To keep a result, put the
# printed values into lens.conf and restart monado-service.
cd "$(dirname "$0")/../.." || exit 1
PIMAX_CTRL_HANDS=0 exec analysis/tools/run_lens_test.sh --patterns swim,grid,flat "$@"
