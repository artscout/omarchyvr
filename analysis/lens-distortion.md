# Dream Air lens distortion and FOV (Monado `omarchyvr` driver)

Files: the model, the `lens.conf` parser, the visibility mask and the debug grid are Rust since 2026-10-14 (`rust/pimax-lens`, see "Rust model and test tooling" at the end); `~/src/monado/src/xrt/drivers/pimax/pimax_lens.{c,h}` are thin C glue around it, `pimax_hmd.c` hooks it in, config template `analysis/tools/lens.conf.example` (installed at `~/.config/omarchyvr/lens.conf`), tuning client `analysis/tools/xr_grid.c` (old) and `rust/pimax-lenstest` (new, `analysis/tools/run_lens_test.sh`). Commit on branch `omarchyvr` (Rust move: branch `lens`).

## Model
How Monado drivers do it: `xdev->compute_distortion(view, u, v, &uv_triplet)` returns, for each point (u, v) of the view's display, the texture coordinate to sample for R, G and B; `u_distortion_mesh_fill_in_compute()` bakes it (vive/rift_s/WMR poly_3k use a per-channel polynomial in the same way, north_star uses a lookup). The compositor already calls the callback with (u, v) in the *user-facing, pre-rotated* display space (3840x3552 per eye) and applies `views[].rot` (left 90, right 270) itself, so the model needs no rotation code of its own and the config is written as the user sees each eye upright.

For a point of the display, p = ((u-0.5)*2*3840/3552, (0.5-v)*2) - centre (units of half the display height, +x right, +y up), p' = p / ca_channel, r = |p'|:

    ray tangent = base * scale * p' * (1 + k1 r^2 + k2 r^4 + k3 r^6)

The client renders a normal pinhole image covering `fov` (left/right/up/down half-angles, advertised through `get_view_poses`), so the sampled texture coordinate is the linear tangent-space map of the ray. k1..k3 are per colour channel, `ca_*` is a per-channel radial divisor (lateral chromatic aberration), `center` is the optical axis position on the panel per eye, `base`/`scale` set the angular magnification. `distortion = off` (or env `PIMAX_LENS_OFF=1`) gives an identity mesh: the pinhole image stretched over the panel, FOV still taken from the config.

## Starting values and sources
* Angle to panel-radius table "Dream Air like Pimax" (fitted by sboys3/CustomHeadsetOpenVR to Pimax's own mesh within 0.1 deg; `analysis/customheadsetopenvr.md` section 2a; radius in percent of half the smaller panel dimension, 1776 px here). Only the numbers were used. Pure-python least squares of `tan(angle) = base*r*(1+k1 r^2+k2 r^4+k3 r^6)` (weighted for angle error) gave **base 0.65307, k1 0.60779, k2 -0.72489, k3 0.58858**, max error 0.28 deg over 2.5..55 deg (the polynomial is only meaningful inside r <= 1.2, i.e. the lens area).
* Chromatic aberration: the red/blue tables of the same document are tiny radial offsets (+0.3..0.55 % red, -0.2..-0.7 % blue); turned into `ca_r = 1.0045`, `ca_b = 0.995` (same k for all channels). Unverified sign.
* Optical centre: reference offset 2.71 % toward the nose, 0.9 % up: `center = +-0.0271 0.009` (left eye +x, right eye -x). Unverified, and its unit (half-height) is an assumption.
* **Measured 2026-10-14** (lens_align.sh, worn, confirmed in xr_room: far lines single): `center = +-0.1060 0.0090` (vergence +2.95 deg per eye vs. the reference). Now the default in `lens.conf.example` and `~/.config/omarchyvr/lens.conf`. Valid for the lens IPD position the headset was in (Linux never sets the IPD motor).
* FOV: panel edge radius 1.0 (half height) maps to 43.9 deg and 1.081 (half width) to 49.5 deg with the fit; reference says recommended 98 x 87. Defaults 50/50/44.5/44.5 so the panel edge is not clipped.
* Not found: `profile.json` only has runtime options (fov_level 0, custom FOV percentages all 0, foveation), no lens data; the factory calibration XML only has camera data; `lens_rd` (cmd 0x27, `0f 0f 00 00 00 05 00 99 1d 99 1d f6 ff f6 ff 01`, i.e. 0x1d99 = 7577 twice and 0xfff6 = -10 twice) is probably per-eye optical-centre / panel offset in some unit (the identical pairs and the small negative -10 look like x/y offsets), but the meaning is unknown; it is not used.

## Config
`~/.config/omarchyvr/lens.conf` (override: `PIMAX_LENS_CONF=path`). INI-like, read **once at monado-service start** (restart to apply). Template with comments: `analysis/tools/lens.conf.example`. Keys: `distortion` (on/off), `fov` (L R U D degrees), `center` (x y), `base`, `scale`, `k` (all channels) / `k_r` `k_g` `k_b` (three numbers each), `ca_r` `ca_g` `ca_b`. Sections `[common]`/`[both]` (or none) apply to both eyes, `[left]`/`[right]` override per eye; later lines win. Bad lines are logged as warnings (`XRT_LOG=info` shows the loaded values: `grep "pimax lens" monado.log`). Env `PIMAX_NO_DEVICE=1` (debug) skips opening the headset so windowed tests do not touch USB.

## Tuning procedure
`xr_grid` is head-locked and ray-cast for the FOV the runtime reports: square grid every 0.2 m on a wall at 2 m (bright every 1 m), red centre cross, circles at 10 (white), 20 (yellow), 30 (green), 40 (cyan), 50 (magenta) deg from the axis, orange frame = edge of the rendered FOV, left eye bluish background, right reddish.

    mkdir -p ~/.config/omarchyvr && cp ~/pimax-re/analysis/tools/lens.conf.example ~/.config/omarchyvr/lens.conf
    SECS=90 ~/pimax-re/analysis/tools/run_lens_tune.sh       # direct mode, logs in /tmp/pimax-lens-tune
    # edit ~/.config/omarchyvr/lens.conf, run again. (Windowed: see Verification.)

Do it with the head still, one eye at a time (close the other), eye on the lens centre:
1. Lines that should be straight: if grid lines bow **outward** (barrel, corners look pushed away / image bulges) raise k1 (try +-0.05 steps, same sign change for the 30-50 deg ring); if they bow **inward** (pincushion) lower k1. Fix the middle ring (20-30 deg) with k1, the outer rings (40-50 deg) with k2 (and k3 for the very edge); when changing k2, k1 usually needs the opposite nudge.
2. Circles must be round and equally spaced in angle as the numbers suggest: if the 10 deg ring is right but 50 deg is too small/large, change k2/k3; if all rings are too small, raise `base`/`scale`; too large, lower them. Check by measuring something of known angle if you have it (e.g. the 30 deg ring should look 30 deg off axis when you turn your eye).
3. Centre: look at the red cross while the other eye is closed; if the cross is not on the axis of your pupil/lens, change `center`: positive x moves the optical centre toward the right of that eye's image (left eye nose side is +x). Use +-0.01 steps; mirror the sign for the other eye. Vertical: positive y moves up.
4. Colour fringes: coloured edges on lines toward the periphery, red outside and blue inside (or the reverse) mean `ca_r`/`ca_b` are wrong. If the red copy of a line sits farther from the centre than the blue/green copy, raise `ca_r` (it divides the radius) or lower `ca_b`; reverse otherwise. Steps of 0.002; keep `ca_g = 1`.
5. FOV: if the image is cut off black before the lens edge, raise `fov` (render area) by 1-2 deg; if you never see the orange frame it is outside the visible area and you can lower it to save render time. Changing `fov` alone does not change the geometry, only coverage and resolution.
6. Fit both eyes; the left and right values should be mirror images except for lens/IPD manufacturing noise. Keep a copy of the working file.

## Verification (windowed only; direct mode was not run by this work)
`XDG_RUNTIME_DIR` was pointed at a private dir so the service did not collide with others; `PIMAX_NO_DEVICE=1`, `XRT_COMPOSITOR_FORCE_XCB=1`, `PIMAX_DEBUG_DIV=4`, `XRT_COMPOSITOR_SCALE_PERCENTAGE=100`, `xr_grid` for 14 s, `grim -g "<hyprctl clients -j geometry>"`. Screenshots in `analysis/frames/monado/`:
* `lens_default.png` / `lens_conf.png`: defaults (built-in and from the installed file are identical): barrel-pre-distorted grid, circles round at the centre, small CA fringes at the edge, panel outline rounded.
* `lens_off.png`: `distortion = off`: straight grid, circles stretched into ellipses (display aspect), no warp (a foreign terminal window overlaps part of the left eye).
* `lens_mod.png`: k = 0, base 0.8, centre moved +-0.15/0.1, ca 1.03/0.97: lines almost straight, strong red/blue fringes, centre cross moved. The mesh therefore follows the config.
In windowed mode the window shows the framebuffer, so the compositor's 90/270 deg pre-rotation is not visible as such; distortion on real optics is **unverified**.

## Rust model and test tooling (2026-10-14, branch `lens`)

**What moved.** `rust/pimax-lens` (crate, `staticlib` + `rlib`) holds the distortion math, the lens.conf parser, the visibility mask and the
debug grid. `pimax_lens.c` in Monado only converts types (`FOV` to radians, the UV triplet, the `xrt_visibility_mask` struct) and calls the
C ABI of `rust/pimax-lens/include/pimax_lens_rs.h`. Build: `(cd ~/pimax-re/rust && cargo build --release -p pimax-lens)` puts
`libpimax_lens.a` next to `libpimax_capi.a`; the pimax CMake block links both from `PIMAX_RUST_TARGET_DIR`, and stops with the cargo command
if the library is missing. Two Rust staticlibs in one binary link without symbol clashes (checked with the real `monado-service`).

**Proof that nothing changed for the current lens.conf.** `rust/pimax-lens-difftest` compiles the ORIGINAL C file (Monado commit 1da3626,
byte for byte, three symbols renamed) next to the Rust crate and compares them bit for bit: the current lens.conf on the compositor's whole
128x128 distortion grid through all four rotations (65 536 points per eye), 3000 random configs of the old file format (including junk
lines, comments, sections, `distortion` values) for both the numbers and the log lines, `PIMAX_LENS_OFF`, the config path rules, the new keys at
neutral values, and the real `pimax_lens.c` glue through its C ABI (`tests/glue.rs`). A negative control shows the comparison fails on a one
ulp change. Deliberate deviations (tests name them): `distortion = OFF` in capitals now switches it off, `nan`/`inf` values are rejected, and a
`k` line with more than three numbers is no longer truncated to three (k4, k5 exist now), and implausible numbers (FOV half angle outside 0.5..89.5 deg, `ca_*` outside 0.5..2, `base` outside 0.01..10, `scale` outside 0.05..20, centre beyond 2) are rejected with a warning instead of producing NaN UVs.

**New lens.conf keys** (all optional, documented in `analysis/tools/lens.conf.example`):

| key | meaning |
|---|---|
| `k`, `k_r`, `k_g`, `k_b` | 3 to 5 numbers: k1..k5 multiply r^2 .. r^10, independent polynomial per colour channel |
| `base_r/g/b`, `shift_r/g/b` | per-channel focal term and per-channel optical-centre shift (`base`, `center` still set all) |
| `fit_range = R` | beyond panel radius R the radial curve continues as its tangent line (0 = off) |
| `blackout = w` or `L R T B` | black margin at the panel edges, half-heights |
| `mask = ellipse a b` (`rect a b`, `superellipse a b n`, `off`), `mask_feather = w` | black outside a shape around the optical centre; also served as the `XR_KHR_visibility_mask` mesh |

Black-out works by pushing the sampled UV of hidden pixels out of the client image: Monado's compositor samples it with a clamp-to-border
black sampler, so hidden pixels are black. The compositor bakes the model into a 128x128 table per channel and interpolates it, so a mask
edge can only follow the configured shape to about one table cell (30 px of the 3840 px wide panel): measured on an elliptical mask with the
CPU model of the compositor (`pimax-lenstest simulate`), the default `mask_feather = 0.03` keeps the visible edge within about 4 px (mean) of the
ideal ellipse at half panel resolution, a hard edge (`mask_feather = 0`) up to a full cell with visible steps, and a wide feather (0.08) moves
the edge outward by up to a few pixels. It is a debug and fine-tuning tool for "this part of the panel is junk", not a pixel exact stencil.

**Fit range and the corners.** The Dream Air table behind the default coefficients ends at panel radius 1.148 (55 deg). The panel corner is at
r = 1.47. The cubic polynomial rises with a tan slope of about 12 there (the table itself ends with 4.0..4.1), reaching 79 deg off axis at the
corner. `fit_range = 1.15` keeps the polynomial where the data is and continues with its tangent line (slope 4.3, 70 deg at the corner):
`analysis/tools/lens.conf.corners.example`. The old fit is reproduced exactly by `pimax-lens fit --terms 3 --rmax 0` (base 0.65307, k
0.60779 -0.72489 0.58858, max error 0.279 deg), which also validates the table and the weights. 4 or 5 terms reach 0.2 deg on the table but
cannot also follow a linear continuation to r = 1.5 (0.6 deg error on the table), so no new default is proposed. Nothing is measured beyond
55 deg: the corners are probably hidden by the lens anyway.

**What the model predicts for the current file** (`pimax-lens show`): panel radius of the 10/20/30/40/50 deg rings 0.260 / 0.500 / 0.732 /
0.936 / 1.087 half-heights (462 / 887 / 1300 / 1663 / 1931 px of 3552), red 2..9 px outside and blue 2..10 px inside green; 85.7 % of the
panel shows image content, the rest (corners) is outside the rendered FOV. Finding: the optical centre is 0.0271 towards the nose, so the
temple edge of each panel is at 51.6 deg while the default FOV is 50: a black sliver of about 36 px at the temple side of each eye. If the lens
shows that part of the panel, `[left] fov = 52 50 44.5 44.5`, `[right] fov = 50 52 44.5 44.5` renders it. (This corrects the "panel edge is not
clipped" remark above, which ignored the centre offset.)

**Compositor facts checked in the code** (answers to open questions of `research/lens-distortion-dreamair.md` section 4):
the compute path samples the client image with a `clamp_to_border_black` sampler in both the fast path and after layer squashing, so UVs
outside the rendered FOV are black, not smeared or wrapped; the distortion table is 128x128 R32G32 per channel, interpolated bilinearly.

**Tools.**
* `pimax-lens show|grid|fit` (rust/pimax-lens): effective config and model numbers, the UV grid picture of both eyes (iso-lines of the client
  image UV per colour channel, angle rings, optical axis, mask outline) without a compositor, polynomial fit with optional continuation.
  `PIMAX_LENS_DEBUG_GRID=<dir>` makes monado-service write the same pictures at start-up (`PIMAX_LENS_DEBUG_GRID_DIV` scales them).
* `pimax-lenstest` (rust/pimax-lenstest, OpenXR + Vulkan via the `openxr` and `ash` crates): straight-line grid, angle rings 10..50 deg with
  labels (from the per-eye FOV), per-channel grids (all/R/G/B), FOV edge frames and white/grey fill, a 5 deg square moving from the centre to the
  edge at 100 % and at 25 % (4 directions), corner frames with checker patches; eye select (both/left/right); controllers A/B/stick (same
  actions and Touch bindings as ctrlcli.c), keyboard, timed `--auto`; prints the active config (runtime, FOV per eye, swapchain format,
  lens.conf crc32, ring predictions). `pimax-lenstest list` prints what to look for in each pattern, `preview DIR` writes all patterns as PNG
  without a headset, `--capture DIR` saves the compositor's final output (after the lens model) per pattern as PNG via
  `XRT_COMPOSITOR_DUMP_TRIGGER` (Monado commit on branch `lens`), `dump2png` and `dumpdiff` convert and compare raw dumps.
* `analysis/tools/run_lens_test.sh` starts monado-service (direct mode, current lens.conf) and the client; `analysis/lens-test-checklist.md`
  is the sheet for the observations; `analysis/overnight/lens.md` has the procedure and the results of the overnight run.
