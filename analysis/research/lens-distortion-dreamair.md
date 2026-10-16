# Dream Air lens distortion: calibration data, model gaps, test plan

Status: research and test design only. No driver code changed, headset not touched, no services stopped.
Related: `analysis/lens-distortion.md` (current model and tuning procedure), `analysis/customheadsetopenvr.md` section 2a (source of the angle table), `analysis/headset-calibration.xml`.

## 1. Bottom line

* **The factory calibration XML does not contain display distortion data.** `headset-calibration.xml` has four `<Camera>` entries (trackingA, trackingB, Pi-controllerA, Pi-controllerB), each an OV7251 640x480 tracking camera with `model="FISHEYE_4_PARAMETERS"`, a principal point, focal length, 6 radial coefficients, and an IMU (`SFConfig`) block with biases. Nothing about the panel, lens centre, FOV, mesh, chromatic aberration or eye geometry. (The file says "FISHEYE_4" but carries six radial values; that is a label oddity, not lens data.)
* The only distortion source we have is the third-party table in `customheadsetopenvr.md` (SBoys3/CustomHeadsetOpenVR, "Dream Air like Pimax"), fitted to Pimax's own mesh, and `lens_rd` (cmd 0x27) which is unidentified.
* So the current `lens.conf` numbers are **a fit to a secondhand angle table, not a measurement of our unit**. Nothing in the repo has been validated on real optics. The windowed screenshots only prove the mesh follows the config.
* The user's report ("similar to Windows, slight edge reflections") is consistent with a correct-ish radial fit plus either (a) a sampling/visibility problem at the panel corners, or (b) real lens optics (pancake glare). The test plan below separates these.

## 2. Factory calibration fields (headset-calibration.xml)

| Field | Values (from file) | Likely meaning | Usable for display distortion? |
|---|---|---|---|
| `DeviceConfiguration deviceUID` | P511X0X000000000001 | serial / unit id | Identifies unit only |
| `pimax_calibration_version` | Pimax_Calibration_tag_0.3 | Pimax calibration tool version | No |
| `calibration_date` | 2026_07_28_13_05_49 | when tracking cal was run | No |
| `Camera cam_name/id` | trackingA 0, trackingB 1, Pi-controllerA 2, Pi-controllerB 3 | tracking/controller camera roles | No |
| `Calibration size` | 640 480 | camera sensor resolution (OV7251 binned) | No |
| `principal_point` | A 325.38/248.28; B 323.51/246.60; C 321.50/238.79; D 320.91/240.92 | camera optical centre px | No (camera, not lens) |
| `focal_length` | A 234.7/234.7; B 232.6/232.5; C 229.5/229.3; D 233.9/233.7 | camera focal px | No |
| `model` | FISHEYE_4_PARAMETERS | OpenCV-style fisheye model for the camera | No |
| `radial_distortion` (6 values per camera) | A 0.0590 -0.0603 0.0320 -0.0082 0.0012 -0.0001; B 0.0621 -0.0624 0.0300 -0.0071 -0.0014 0.0033; C 0.0594 -0.0613 0.0304 -0.0073 -0.0011 -0.0006; D 0.0593 -0.0603 0.0305 -0.0077 0.0009 0.0002 | tracking camera lens distortion | No (camera optics) |
| `undistortion_limit` | 0.0 | camera undistort cutoff | No |
| `Rig translation / rowMajorRotationMat` | per camera, B/C/D relative to A | camera extrinsics on the headset | No (tracking) |
| `TimeAlignment delta` | 0 | camera-IMU time offset | No |
| `Image format` | interleaved | camera stream format | No |
| `SFConfig Stateinit` (ombc, tbc, aBias, wBias, ka, kg, na, ng, ombg, delta) | e.g. ka 0.00616 0.00159 0.02118; kg 0.00505 0.00299 0.00105; delta 0.00164 | IMU scale/bias/misalignment | No (IMU) |
| `IMUNoise` | all 0.0 | not filled | No |

Conclusion: the file gives **zero** display-side fields. There is no lens centre, no per-eye panel geometry, no FOV, no per-colour polynomial. The Pimax calibration tool used here handles cameras, IMU and controllers only. Display distortion, if stored, lives in Pimax's runtime (pi_server/PVR) and is not in this file (per `customheadsetopenvr.md` section 2a, "Hidden-area mesh is pulled from PVR, not embedded"; issue #57 says raw built-in distortion can be dumped from pi_server on Windows).

Possible follow-up that *could* yield data: dump the Windows-side mesh (`pvr_getHmdDistortedUV`, the "Pimax Builtin" profile) with a Windows install, or decode `lens_rd` (cmd 0x27: `0f 0f 00 00 00 05 00 99 1d 99 1d f6 ff f6 ff 01`, i.e. 0x1d99 = 7577 and 0xfff6 = -10, pairs repeated) by comparing it with a change of IPD or centre in Pimax software. Both are unverified.

## 3. How the fields map onto Monado's distortion models

Monado (`u_distortion_mesh.h`) offers: `u_compute_distortion_vive` (per-channel centre and 4 polynomial coefficients per channel, plus aspect and cutoff radius), `u_compute_distortion_panotools` (coefficients, aberration, scale, lens centre, viewport), `u_compute_distortion_cardboard` (radial K0..K4 applied to r^2..r^8), `u_compute_distortion_ns_p2d` (2D polynomial per view), and mesh/none fills. The `poly_3k` name used in our notes is the Vive/WMR-style 3-term polynomial; I could not confirm a source page that names it exactly, so treat that as an inference from the code.

Our model is already a per-channel 3-term radial polynomial (Vive-style) with a centre, a divisor for chromatic aberration, and a base/scale. The mapping is:

| Source quantity | Our parameter | Status |
|---|---|---|
| Angle to panel radius (green), 2.5..55 deg | `base`, `k` (fit, max error 0.28 deg) | Fitted; only valid to r ~1.2 |
| Red/blue radius offsets (+0.3..0.55 % red, -0.2..-0.7 % blue) | `ca_r` 1.0045, `ca_b` 0.995 | Sign unverified; only a divisor, no separate red/blue fit |
| Offset X -2.71 % (toward nose), Y +0.9 % (up) | `center` +-0.0271 0.009 | Unit (half display height) assumed |
| Recommended max FOV 98 x 87 | `fov` 50 50 44.5 44.5 | Our FOV comes from the fitted panel edge, not from Pimax's FOV |
| Panotools-style centre/scale/viewport | not used | Would be the natural form if centre and scale need separate axes |

## 4. Gaps in the current model

1. **Only one fit, one channel of data.** The k coefficients are fitted to the green table only. The red/blue tables are reduced to a single divisor, which cannot represent different radial curves per colour. Real CA is usually not a pure scale.
2. **Tangential and asymmetric error ignored.** A centre offset is applied, but no tangential term and no per-axis scale. Lenses with a canted or off-axis optical path show trapezoid or skew error at the edges.
3. **The centre and aspect are assumptions.** `aspect = PIMAX_EYE_H/PIMAX_EYE_W = 3840/3552` (user-facing width/height) is only right if the compositor really passes pre-rotated coordinates (`lens-distortion.md` says it does; the Monado comment says "after the compositor's panel pre-rotation"). This was never measured. If the compositor passes native coordinates the whole mesh is rotated and the centre is in the wrong axis.
4. **Fit range.** The polynomial is valid for r <= ~1.2 (half-height units). The panel corners reach r ~ 1.5 (corner at (1.081, 1.0)). Beyond the fit range the mapping is extrapolated, which is the likely source of corner error.
5. **Visibility / sampling at the corners.** The rendered FOV is 50 x 44.5 deg (tangent-space clamp). Any ray beyond the rendered FOV samples outside the texture. If the sampler wraps or the border colour is not black, the edge shows streaks or wrap artefacts that look like reflections. This is a driver/compositor question: check the sampler address mode and whether `tx`/`ty` outside [0,1] are clamped. Not yet verified in code.
6. **No FOV-edge masking.** A "hidden area" or visibility mask (what the panel can really see through the lens) is not used. Pimax's own pipeline uses a hidden-area mesh from PVR.
7. **No calibration source.** Everything is from a third-party table. No measurement of our headset exists.
8. **Compute vs mesh.** The vronlinux guide says Monado's Pimax path uses precalculated meshes (`PIMAX_MESHES_PATH`, default `~/.config/pimax/meshes`, `PIMAX_MESH_NAME`) exported from Pimax's runtime; our branch computes the mesh instead. These two paths should be compared on the same grid if we ever dump a Windows mesh; the dumped mesh is the best reference we can get.

## 5. Causes of "edge reflections" or ghosting (to tell apart)

| Cause | Where it comes from | Looks like | Discriminator |
|---|---|---|---|
| Pancake / Fresnel glare | Hardware (lens coatings, multiple reflections; Pimax concedes glare in high contrast) | Faint halos near bright objects, strong with black background, same position regardless of render | Brightness/contrast change, moving the head, same on Windows |
| Wrong distortion (fit error) | Software model | Straight lines bow at the edge, circles not round; a *shape* error, not a ghost | Grid test (section 6.1, 6.2) |
| FOV clamp / sampling outside texture | Software | Streaks or repeated copies at the panel corners | Border test (6.4): render only to the edge, check colour beyond the rendered FOV |
| Rendering outside visible area | Software (FOV too wide, no visibility mask) | Content visible in a band that is physically hidden, or reflections of the blocked edge | Visibility test (6.4) |
| Chromatic fringes | Model CA wrong or real lens CA | Coloured edges, red/blue copies offset | Chromatic test (6.3) |

The key test is the single bright square (6.5): if the reflection follows the square's *position* (not its distortion) and changes with brightness, it is optics; if it is a copy at a fixed offset from the mapped position, it is software sampling.

## 6. Test plan (to run later, on the headset, one step at a time)

Set up: `analysis/tools/run_lens_tune.sh` (direct mode) or the windowed variant with `PIMAX_NO_DEVICE=1`. One eye at a time, head still, eye on the lens centre, same IPD. Keep a copy of `lens.conf` for each run.

1. **Straight-line grid.** Use the current `xr_grid` grid. Score each visible line segment for straightness at 10, 20, 30, 40, 50 deg rings. Measure with a phone at the same position: bow in pixels per 10 deg of arc. Pass target: bow < 2 px at 1080p-equivalent. Record in a table per ring.
2. **Angle-to-radius check.** Put a known-angle target (a ruler at 2 m with marks at 10/20/30 deg from the axis) on the wall and check where the mesh puts the 30 deg ring. Compare the ring radius with the fitted curve (`tan(angle)` values in section 3 of `lens-distortion.md`). Repeat at 40 and 50 deg; these check the k2/k3 terms.
3. **Chromatic test.** Render three separate grids: red-only, green-only, blue-only (same geometry, one channel each, others zero). Photograph the lens through a phone camera with the same framing and overlay them. Measure the offset of each colour's line at the 40 deg ring. Pass: offset < 1 px at 1080p. This gives the real `ca_r`/`ca_b` and sign.
4. **FOV edge and border test.** Draw a pure white frame exactly at the configured FOV edge and another 2 deg outside it. Check with a phone whether the outer frame is black, shows streaks, or repeats. If repeated or streaked, the sampler is wrapping or clamping at the wrong place (driver bug). Also draw a checkerboard only in the corners (r > 1.1).
5. **Reflection-vs-distortion discriminator.** Full black frame with a single bright square (e.g. 5 deg, white, 100 %). Move the square along the horizontal axis from centre to the edge in 5 steps; at each step photograph through the lens. Then repeat at 25 % brightness. Interpretation: a halo or ghost that scales with brightness and stays at the square's physical position = optics (Fresnel/pancake glare). A copy displaced by a fixed offset that does not scale with brightness = software sampling. Shape distorted in step with the mesh = fit error.
6. **Windows comparison.** On the Windows machine with the same headset, run the same square/grid sequence (Pimax's Built-in profile, then "Normal FOV" and "Large FOV"), same phone position. Compare photos side by side (or with the grid overlay). The Windows result is the target, not the absolute truth; record which profile was used.
7. **Coordinate-frame check.** Render a single white square at each panel corner using the rotated (user-facing) convention. If it appears in the wrong corner, the rotation assumption (`aspect`, `center` axis) is wrong.
8. **Log the result.** Each run writes a table (ring, measured bow, CA offset, border result) to the test folder; compare with the previous run.

## 7. What xr_grid would need

`analysis/tools/xr_grid.c` currently draws a head-locked grid with circles at 10..50 deg, a centre cross and a frame at the rendered FOV edge, all ray-cast through the runtime FOV. Missing for this test plan:

* **Channel select** env (`XR_GRID_CHANNEL=r|g|b|all`) to draw one colour only (step 3).
* **Single-square mode** (`XR_GRID_SQUARE=deg,x_deg,y_deg,size_deg,brightness`) for step 5, with the square moved by a key or by a list of positions run in sequence. A timed sequence (`XR_GRID_SEQUENCE=file`) so the photos can be taken hands-free.
* **Border mode** (`XR_GRID_BORDER=1`) that draws the frame exactly at the FOV edge and at +2 deg outside, plus corner checkerboards (step 4).
* **Ring labels** (text, drawn with a bitmap font) so photos can be read without a log.
* **Log output** of the FOV and config values used (`lens.conf` hash) written to stdout, so each photo can be matched to a config.
* **Per-eye switch** already exists (bluish left, reddish right) but needs `XR_GRID_EYE=left|right|both` to render only one eye with the other black (for reflection tests).

## 8. Recommended fix approach (later)

1. **Measure first.** Run steps 1-6. Do not change the model before the chromatic and border results are known.
2. **Fix the visibility/sampling first** if step 4 shows streaks: clamp or black the sampler outside the rendered FOV, and clip the mesh to the visible area (a visibility mask from the panel edge circle or from a dumped hidden-area mesh). This is cheap and may remove the "reflection" look entirely.
3. **Replace the single-channel divisor with per-channel polynomials** (Vive-style `u_vive_values`, one coefficient set per channel, fitted to the step 3 offsets instead of the red/blue tables). Keep `ca` as a fallback.
4. **Refit the k terms** on the measured rings (steps 1-2) with a least-squares fit in tangent space over r <= 1.4, not 1.2, and check the corners. If the corner error stays, move to a measured mesh (dumped from Windows via `PIMAX_MESHES_PATH`) or a 2D polynomial (`ns_p2d`-style).
5. **Config**: keep `lens.conf` as the override, add `[left]/[right]` per-channel k sets and an optional `mesh = path` key for a dumped mesh. Record the source of every value (measured/fitted/table).
6. **Glare** is not a software fix: if step 5 shows optics, report it as hardware behaviour and tune brightness / black level in the compositor (e.g. a black floor), not the mesh.

## 9. Sources

* Pimax Dream Air setup and distortion profile notes: https://pimax.com/blogs/blogs/sboys3-native-steamvr-driver-setup-guide (default profile only; profiles for other headsets are wrong for this lens)
* Pimax variable binocular overlap (profiles trade FOV for overlap): https://au.pimax.com/blogs/blogs/variable-binocular-overlap-in-pimax-headsets
* Pimax on lens types and glare (marketing, use with care): https://pimax.com/aspheric-vs-pancake-vr-lenses-and-why-glass/ and https://pimax.com/blogs/highlights/real-pimax-dream-air-user-reviews-what-early-owners-actually-think
* Road to VR, Pimax 8K distortion and periphery blur (older model): https://roadtovr.com/ces-2018-pimax-8k-hands-on-long-way-kinks-to-iron/
* DCS forum, Pimax 5K distortion issue (Windows-era, anecdotal): https://forum.dcs.world/topic/203807-pimax-5k-distortion-issue
* Linux Pimax setup and mesh path (`PIMAX_MESHES_PATH`, `PIMAX_MESH_NAME`, precomputed meshes): https://vronlinux.org/docs/hardware/pimax/ and https://wiki.vronlinux.org/docs/fossvr/monado/pimax/
* Monado Pimax HMD support MR: https://gitlab.freedesktop.org/monado/monado/-/merge_requests/2309
* Monado distortion models (Vive/Cardboard/Panotools/NS): https://monado.pages.freedesktop.org/monado/u__distortion__mesh_8h_source.html and https://monado.pages.freedesktop.org/monado/structu__vive__values.html and https://monado.pages.freedesktop.org/monado/structu__cardboard__distortion__values.html
* Monado Vive distortion improvements (DPOLY3 vs SteamVR): https://gitlab.freedesktop.org/monado/monado/-/merge_requests/666
* Monado WMR Poly3K / config MR: https://gitlab.freedesktop.org/monado/monado/merge_requests/136
* Lens distortion background (Projection and Distortion in Wide-FoV HMDs): https://archive.vrroom.studio/?p=22074

Not found: any public Pimax JSON lens-profile format, any PiServer documentation of the "large/normal/small FOV" labels, and any Monado Index/Rift S/Bigscreen Beyond distortion JSON format in the sources I could reach. The per-colour polynomial claims for Index (vive model) and WMR (Poly3K) are from Monado code and MR descriptions; they were not checked against the device JSON files.

Local sources read: `analysis/lens-distortion.md`, `analysis/headset-calibration.xml`, `analysis/customheadsetopenvr.md` (sections 2a, 4-5), `analysis/tools/run_lens_tune.sh`, `analysis/tools/lens.conf.example`, `analysis/tools/xr_grid.c` (header and config env), `~/src/monado/src/xrt/drivers/pimax/pimax_lens.c`, `pimax_hmd.c` lines 58-59 and 402-457.
