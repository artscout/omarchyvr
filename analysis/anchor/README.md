# Room anchor (AprilTag on the desk)

Brings the VR windows back to the same real spot every session. The SLAM world origin of Monado is wherever the headset
was at start; a printed tag on the desk is the fixed reference.

## How it works
1. `pimax-anchor` reads tracking frames from `pimax-cam broker`, finds the tag36h11 tag in each fisheye tile (resampled
   into pinhole views for the AprilTag detector), refines the tag pose on the KB4 reprojection error jointly over all
   cameras that see it (pose in the head frame).
2. Monado (branch `desk`, `PIMAX_ANCHOR=1`) keeps a ring of its raw head poses (`$XDG_RUNTIME_DIR/pimax-anchor.head`), so
   the tag pose in the SLAM world is `world_from_head(frame time) * head_from_tag`.
3. `calibrate` saves that pose (in the app world) to `~/.config/pimax/anchor.toml`. `apply` computes the offset that puts
   the tag back there (yaw + translation; SLAM is gravity aligned) and writes `$XDG_RUNTIME_DIR/pimax-anchor.offset`.
   Monado applies it to the head pose (`app_from_slam = offset * slam`), so controllers and the gaze ray follow, and
   WayVR's stage (with every window placed in it) lands on the same real spot. No WayVR file is touched.
   (Windows saved relative to the stage/head stay as WayVR stored them; `calibrate --window NAME=pos,quat` also records
   a window relative to the tag in `anchor.toml` for tools that want it.)
4. The offset is per Monado run: Monado deletes the offset file at start and ignores one with another pid.

## Steps
- Print `tag36h11-id0-150mm.pdf` (A4, 100 %, no "fit to page"; the ruler must measure 100 mm, the black square 150 mm).
  Matt paper, glue it flat on a board. Put it on the desk about 40-70 cm in front of you, in light, and do not move it.
  A different size: `pimax-anchor tag --size-mm 200 --out-dir DIR` and pass `--size-mm 200` to calibrate.
- Start Monado with `PIMAX_ANCHOR=1` in its environment (needs the rebuilt `desk` build, then `setcap` again).
- First time, with the windows where you like them: `pimax-anchor calibrate` (hold still, look at the tag for 4 s).
- Every later session, after the headset is tracking (6DoF): look at the tag, `pimax-anchor apply`.
- `pimax-anchor detect` shows what the cameras see, `status` the files, `clear` removes the offset.

Tests: `cargo test -p pimax-anchor --release -- --nocapture` (synthetic tags through the KB4 model, two SLAM origins).
