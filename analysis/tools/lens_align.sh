#!/bin/bash
# Live stereo alignment of the two eyes: runs the lens test client with only the `align` pattern.
# Left eye shows a red line + tick on its optical axis, right eye a green one. Look with both eyes and shift the
# targets against each other (in the rendered image, lens.conf is not changed) until they fuse into ONE line.
#
# Keys (type them in THIS terminal):
#   ]  /  [     vergence + / - 0.05 deg per eye (+ : left target moves right, right target moves left, i.e. toward each other)
#   }  /  {     the same in coarse steps of 0.25 deg
#   i  /  k     vertical offset between the eyes + / - 0.05 deg (+ : left up, right down)
#   r           reset all offsets
#   q           quit: prints the [left]/[right] center lines and a command writing ~/.config/pimax/lens-ipd/measured.conf
# Controller (if PIMAX_CTRL_HANDS is not 0): stick left/right = vergence 0.05, stick up/down = vertical, trigger = reset.
# Every change is also printed in the terminal (deg per eye, vertical, equivalent lens.conf center values).
cd "$(dirname "$0")/../.." || exit 1
PIMAX_CTRL_HANDS=0 exec analysis/tools/run_lens_test.sh --patterns align "$@"
