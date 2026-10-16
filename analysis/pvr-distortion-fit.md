# Pimax PVR distortion dump: findings and fit (2026-10-14)

Data: `pimax-data/windows-20261014/.../pimax-distortion-P511X0X000000000001.json` (Dream Air, PVR 1.33.1). Fitter: `pimax-lens fit-pvr`
(`rust/pimax-lens/src/pvrfit.rs`). Variant written to `~/.config/omarchyvr/lens-ipd/pvr.conf` (lens.conf untouched).

## 1. The data

- Per eye: 129x129 grid (row-major, v outer) and 4 radial lines of 1024 for each of R/G/B. 0 failed calls, no NaN, no sentinels.
- Outside the visible area the runtime still returns finite, extrapolated numbers (render UV from -0.67 to 1.71). The visible area is exactly
  "output UV inside [0,1]" (13972 of 16641 grid points). The visible-area mesh is a rounded square in panel UV (corner radius 0.2);
  masking with the border polygon removes only 50 more points. The fitter masks on output in [-0.002, 1.002].
- Direction: panel UV (0..1 over the eye's native 3552x3840 panel, before rotation) -> render UV (0..1) over the FOV port, linear in tangent:
  tan_x = -left + u*(left+right), tan_y = -down + v*(up+down) (V=0 is the bottom; with up=down only the sign of the small vertical offset depends on it).
  Evidence: the panel edge centre lines hit render UV 0 and 1 exactly (FOV port = panel extent), tan grows ~linearly near the centre
  (0.69 per half-panel-height unit) and super-linearly toward the edge.
- Orientation: the panel is rotated by 90 deg, mirrored between the eyes.
  Left: user x = panel v, user y = -panel u. Right: user x = -panel v, user y = +panel u (opposite rotations, centres mirrored in x). This is the same as the
  driver's left 90 / right 270 pre-rotation. Panel size convention that fits: 3552 (u) x 3840 (v), unit = half of 3552 = 1776 px,
  i.e. user-facing display 3840x3552, aspect 1.081 as in `pimax_lens_load`.
- The right eye dump is the exact mirror of the left (same values with x negated), including the FOV ports (left eye: outer = left).
- R/G/B: same shape, lateral CA only: red is shown at a ~0.3% larger radius divisor, blue ~0.4% smaller (fit ca_r 1.0032, ca_b 0.9963;
  at 50 deg red +6 px, blue -7 px vs green). Our tuned default had 1.0045 / 0.995.
- Centre of the panel maps to tan (-0.018, -0.012), not (0,0): the optical axis is not at the panel middle (see below).

## 2. What was wrong with the fitter

The orientation search fits every candidate (8 flips/rotations x 4 panel conventions) with a free centre and takes the lowest cost. For the
right eye a runaway candidate (panel 3840x3552, orient 0: centre (-80, 35), a0 = 1e5, k = -40...) cancelled numerically to cost 0 and won; the
following inlier rounds then failed ("did not converge"). The left eye got lucky. Fixes in `fit_core`: reject candidates whose centre is more
than 0.35 half-heights from the panel middle or whose focal term is outside 0.1..5. Also the default inlier threshold was 0.5 deg, which
clipped a systematic residual of ~0.4 deg and made the polynomial wild (k up to 10, fov 37 deg); the data has no outliers inside the visible area, so it is now 3 deg. The
written `fov` is now Pimax's FOV port (exact), not an estimate. Regression test: `fits_the_real_dream_air_dump_for_both_eyes`
with `rust/pimax-lens/tests/data/pvr-dump-sub.json` (every 8th grid point, 344 samples per eye).

## 3. Fit result (green, both eyes identical up to the mirror)

| | left | right |
|---|---|---|
| centre (half-heights) | +0.02699, +0.00902 | -0.02699, +0.00902 |
| base*scale | 0.68595 | 0.68595 |
| k1..k5 | 0.262818 -0.181637 1.035929 -1.258917 0.562678 | same |
| ca R / B | 1.00319 / 0.99632 | same |
| shift R / B | +0.00009,+0.00011 / -0.00011,-0.00010 | x mirrored |
| fov L R U D | 52.58 47.54 44.55 44.55 | 47.54 52.58 44.55 44.55 |
| cant_deg | 2.0 (poses: left yaw +2, right yaw -2) | |

Residual (angle between Pimax ray and model ray, whole visible area, 13922 samples): rms 0.395, median 0.35, 95% 0.66, max 1.66 deg.

| rho (half-heights) | median | 95% | max |
|---|---|---|---|
| 0.0-0.3 | 0.34 | 0.39 | 0.39 |
| 0.3-0.6 | 0.33 | 0.41 | 0.41 |
| 0.6-0.8 | 0.27 | 0.44 | 0.46 |
| 0.8-1.0 | 0.35-0.41 | 0.51-0.71 | 0.82 |
| 1.0-1.2 | 0.54-0.60 | 0.97-1.32 | 1.66 |

Symmetry verdict: NOT radially symmetric to 0.1 deg. The residual is a smooth, structured field (twist rms 1.9 deg), not noise. Offline test with an
extended model (python, same data, left eye): anisotropy (user-y scale 1.0155 relative to x) alone gives median 0.19 deg; a constant additive tangent
offset (0.0007, -0.0183) alone gives median 0.26; both together rms 0.17, median 0.06, 95% 0.36, max 1.0 deg (rotation of the rays instead of the
additive offset is worse: median 0.14). So the radial polynomial itself is fine; what our model lacks is (a) a 1.5% x/y scale difference and (b) a
constant pointing offset of ~1 deg vertical (Pimax's optical axis, tan = 0, sits at user y = +0.018, our radial-only fit compromises at 0.009, which leaves
~0.35 deg error at the centre). The lens.conf model cannot express either; to go below 0.1 deg a per-eye mesh/LUT mode (or two extra keys: y-aspect and a
tangent offset) is needed. The remaining 0.2-0.4 deg is small compared with the swim tuning error budget and the centre estimate agrees with the by-eye tune.

## 4. FOV

Pimax ports: left eye L 52.58 / R 47.54, right eye L 47.54 / R 52.58, U = D = 44.55 (outer is the temple side: left of the left eye, right of the right eye).
Our `fov = L R U D` (tangent frame, x right, y up) per eye: left `52.58 47.54 44.55 44.55`, right `47.54 52.58 44.55 44.55`. The data's tangent extent
is exactly this rectangle, so it is the minimal FOV that covers the whole panel. The symmetric 50 50 44.5 44.5 is NOT the cause of the centre tuning: the
centre is a panel-space quantity and the FOV only sets the image-UV <-> tangent map; the by-eye centre (0.0271, 0.009) equals the Pimax fit (0.0270, 0.0090).
The symmetric 50/50 clips the outer 2.6 deg of the temple side (black) and wastes 2.5 deg on the nose side. Note with the exact ports the
radial fit's edge samples land at image UV 1.003 / 1.005 (a ~0.3 deg misfit), so a thin black rim on the inner/bottom edge is possible; add 0.3 deg if it shows.

## 5. Pimax curve vs our tuned default (0..60 deg, green, radius in half-heights; "mag" = dr/dtheta relative to the centre, 1 = no distortion)

Tuned = `~/.config/omarchyvr/lens.conf` (base 0.65307, scale 1.035, k .33692 -.36351 .48055 -.03315 -.02408). Pimax data covers up to ~52 deg (outer) / 44 deg (up/down); rows above are the polynomial extrapolation.

| angle | r Pimax | r tuned | tuned vs Pimax | mag Pimax | mag tuned |
|---|---|---|---|---|---|
| 0 | 0 | 0 | | 1.000 | 1.000 |
| 5 | 0.1270 | 0.1287 | +1.4% (+3 px) | 1.005 | 1.009 |
| 10 | 0.2529 | 0.2556 | +1.1% (+5 px) | 1.017 | 1.027 |
| 15 | 0.3770 | 0.3802 | +0.9% (+6 px) | 1.036 | 1.043 |
| 20 | 0.4980 | 0.5034 | +1.1% (+10 px) | 1.071 | 1.053 |
| 25 | 0.6138 | 0.6250 | +1.8% (+20 px) | 1.129 | 1.075 |
| 30 | 0.7232 | 0.7420 | +2.6% (+33 px) | 1.197 | 1.145 |
| 35 | 0.8267 | 0.8489 | +2.7% (+39 px) | 1.262 | 1.283 |
| 40 | 0.9243 | 0.9432 | +2.0% (+33 px) | 1.358 | 1.464 |
| 45 | 1.0120 | 1.0264 | +1.4% (+26 px) | 1.577 | 1.635 |
| 50 | 1.0848 | 1.1025 | +1.6% (+31 px) | 1.953 | 1.748 |
| 55 | 1.1437 | 1.1756 | +2.8% (+57 px) | 2.381 | 1.770 |
| 60 | 1.1934 | 1.2499 | +4.7% (+100 px) | 2.720 | 1.682 |

Same by panel radius (angle shown): rho 0.3: 11.89 vs 11.77 deg; 0.6: 24.39 vs 23.96; 0.8: 33.68 vs 32.64; 1.0: 44.26 vs 43.35; 1.1: 51.20 vs 49.83.
Angle difference (tuned - Pimax) rms over rho <= 0.6 / <= 1.1: original default (before tuning) 0.19 / 0.35 deg, after swim1 0.27 / 0.43, current 0.17 / 0.65 deg
(max 0.42 / 1.37).

Verdict: the by-eye tuning found Pimax's optical centre almost exactly (x 0.0271 vs 0.0270, y 0.009 vs 0.0090), and the central 0.6 half-heights are
about as close as before (0.17 vs 0.19 deg rms). But it moved AWAY from Pimax's curve in the periphery: the current default shows 0.4 to 1.4 deg less angle than
Pimax at rho 0.6-1.1 (the original curve was within 0.7 deg there), and its dr/dtheta is too flat from 20 to 30 deg and too low beyond 50. The tuned base*scale
(0.676) is 1.5% below Pimax's 0.686, which makes the world look slightly smaller/farther in the centre. Using the PVR fit as the baseline and tuning only the
residual (centre-weighted) would be the more principled path.

## 6. Exact LUT mode (`lut = FILE`)

The radial model leaves rms 0.4 deg because Pimax's mapping has a 1.5% y/x aspect and a vertical tangent offset it cannot express. LUT mode skips the model and
samples Pimax's own dump.

- Build: `pimax-lens make-lut <pimax-distortion-*.json> --out ~/.config/omarchyvr/lens-ipd/pvr-lut.bin` (800 KB, `rust/pimax-lens/src/lut.rs`). Binary, little
  endian, versioned, CRC32: header (version, serial, grid 129, per-eye orientation index into `pvrfit::ORIENTS`, panel size, FOV port tangents), then per eye and
  R/G/B the 129x129 grid of render **tangents** (x right, y up), in the dump's panel order, so a FOV change never invalidates it. Loading resamples (exact node
  permutation) into the user-display grid with the eye's orientation (left 3: user x = panel v, user y = -panel u; right 1: mirrored).
- lens.conf: `lut = path` (global or per eye; `none` disables for an eye; relative paths are relative to the conf file; `~/` works), `lut_margin = deg`
  (widens the rendered FOV beyond the LUT's port, e.g. 0.3 if a thin black rim shows), `lut_interp = bicubic|bilinear` (default bicubic, Catmull-Rom). With a
  LUT the eye's `fov` is the dump's port (+ margin), `center` is ignored (the LUT carries the optical axis), `cant_deg` still comes from the conf. Visibility:
  `Lens::compute` output outside [0,1] is black (same as the dump's visible area), and the OpenXR visibility mask is traced from it. A missing or invalid file logs
  a warning per eye and the eye keeps the radial model (pvr-lut.conf keeps pvr.conf's radial sections as that fallback). The C ABI is unchanged.
- Interpolation error against the 4x1024 radial lines of the real dump (24548 visible samples, ray angle): bilinear rms 0.0046 deg / max 0.084; bicubic rms
  0.0071 / max 0.096. Both maxima sit within 2.5% of the line end, right at the visible-area edge where the data has a kink; the lines also run along grid rows,
  so this tests only 1-D interpolation. A 2-D proxy (half resolution 65x65 LUT against the 129 nodes): bilinear rms 0.021, bicubic 0.014-0.018 deg, so at 129
  the 2-D interpolation error is about 0.005 deg rms. Bilinear exceeds 0.02 deg at the edge, so bicubic is the default; the difference is not visible.
- Variant: `~/.config/omarchyvr/lens-ipd/pvr-lut.conf`. Tests: `cargo test -p pimax-lens lut` (round trip, corruption, orientation of both eyes, node-exact `compute`,
  margin, per-eye `none`, fallback, CLI).
