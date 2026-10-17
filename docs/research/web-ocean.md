# Research: web-ocean as a Work background (2026-10-17)

Request: "https://github.com/Noniv/web-ocean: have it as the background in Work mode. Investigate and implement a
Rust version, or a good version of it." Result: implemented as "Ocean (animated)" (docs/BACKGROUNDS.md).

## 1. The project and its licence
- `Noniv/web-ocean`, cloned shallow to a scratch folder, read only (no `npm install`, no build script run).
  Files: `js/main.js` (877 lines), `js/wgsl/{common,sim,sky,ocean,post}.js` (about 1500 lines of WGSL), `build.mjs`,
  `server.mjs`, a README with three screenshots.
- **Licence: MIT, Copyright (c) 2026 Maksymilian Dendura** (`LICENSE`; the README says "MIT"). The source files carry no
  per-file headers. MIT lets us port and redistribute if the notice is kept. omarchyvr is MIT OR Apache-2.0 and
  WayVR (where the code lives) is GPL-3.0, both compatible with MIT.
- The README says it was written by Claude Code (Opus 5.5) from a prompt; the author's copyright claim is the MIT file.
  We did not rely on that either way.
- **Decision: clean-room.** No line or shader of web-ocean was copied. Its README was used for the list of techniques
  and the performance number; the implementation is written from the published papers (section 6). So no MIT notice
  is required; `THIRD-PARTY.md` still names the project as the inspiration and lists the references.

## 2. What web-ocean renders and how
WebGPU, no engine, a free-fly camera. From its README:
- **Waves**: Tessendorf FFT sea, **6 cascades of 256x256** (patches 1103 m down to 0.26 m), fetch-limited JONSWAP wind
  sea with Donelan-Banner spreading plus a narrow swell, capillary term in the dispersion, choppy displacement, radix-2
  inverse FFT compute shaders in workgroup memory.
- **Surface**: CDLOD quadtree mesh, 17 levels, 6.25 cm vertices near the camera, reaching 160 km, vertex morphing.
- **Shading**: LEAN mapping (mip-mapped slope moments) for anisotropic roughness at any distance, GGX-like sun glitter
  with Smith masking and the sun's disc size, sky reflection from a per-frame environment cube map, turbid blue-green
  body with a subsurface glow, whitecaps from the Jacobian fold, foam kept in two camera-centred world-space maps.
- **Sky**: Hillaire atmosphere (transmittance / multiple-scattering / sky-view LUTs), two cloud layers (cumulus and
  altostratus) with self-shadowing and silver linings.
- **Post**: HDR with 4x MSAA, 8-level bloom, AgX tone mapping.
- **Cost (its README)**: 6 to 8 ms per frame at 2560x1440 on an RTX 5070 Ti, with other programs on the GPU.

That is a showcase renderer: a full frame at monitor resolution costs 6 to 8 ms, more than our whole Work budget
(a 90 Hz frame is 11.1 ms and WayVR, the screens and Monado need most of it). A direct port to the headset (two eyes
at 3552x3840) would cost about 4 to 5 times that. It cannot be the background as it is; its ideas can.

## 3. How backgrounds work in Work today
- `rust/omarchyvr-backgrounds` keeps the library (`~/.local/share/omarchyvr/backgrounds/<theme>/*.dds`, 66 equirect 360
  pictures, 8K/4K/2K, BC1/BC7) and the choice `~/.config/omarchyvr/background.toml` (`enabled`, `id`, `res`), shared by
  the app's Backgrounds page, voice ("Max, set background ...") and `omarchyvr desk start`.
- Applying = `wayvrctl skybox <path>` (sets WayVR's `skybox_texture` over IPC `WlxSetSkybox`), then the passthrough
  camera goes off (`omarchyvr-passthrough`).
- WayVR (`forks/wayvr`, Rust, **vulkano** on Vulkan 1.3 with dynamic rendering, OpenXR via the `openxr` crate):
  `wayvr/src/backend/openxr/skybox.rs`. A swapchain image (the picture's size) is rendered **once** (immutable
  `STATIC_IMAGE` swapchain) by a fragment pass over a full-screen quad (the DDS sampler, or the built-in procedural
  `sky.frag`), and shown as an `XrCompositionLayerEquirect2KHR` (radius 100 m, whole sphere, pose = head translation in
  the stage space, orientation fixed: world-locked). The skybox only exists while the blend mode is OPAQUE (the
  passthrough is off) and `use_skybox` is set.
- Monado composes the layer with the screens (the other layers) and distorts for the lenses. WayVR does not render
  per eye for the background: one image serves both eyes.

## 4. Where an animated 3D background fits
| Option | Verdict |
|---|---|
| **In WayVR, into the equirect skybox swapchain, redrawn per frame** | **Chosen.** WayVR already owns the layer, the Vulkan device, the swapchain code, the IPC name (`wayvrctl skybox`), and "Work only" (WayVR runs only in Work; Play is SteamVR, with no WayVR skybox). The sky layer follows the head position and is world-locked in orientation, behind everything. One image for both eyes: the cost does not double. Change: a new module plus one hook in `skybox.rs`. |
| Per-eye projection layer in WayVR (real parallax and depth) | Not needed for a background: parallax of a horizon-distance sea is below what a seated user can notice, and it would cost two eye renders at display resolution. Possible later for a near-water pier. |
| Separate OpenXR client (own layer) | Rejected: a second app and Vulkan device, a second session (XR_EXTX_overlay) to start and stop, no access to the user's saved setting, and it would sit at an undefined z-order against the WayVR layers. |
| Pre-rendered looping 360 video | Fallback only (no live sun/head interaction, big files, DDS-only skybox in WayVR). Not needed. |

Seen from the head the sphere has no depth, so the ocean is **ray-traced from the head** per texel: the direction of
each texel meets a water plane that is fixed in the stage (3 m under the floor), so sitting down or standing up changes
the view of the sea correctly, and the waves are anchored to the world (the head's x,z is added to the wave phase).
The horizon is shown 4 degrees below eye level (the rays are tilted up by 4 degrees before they hit the plane: the
look of a pier above a curved sea), so it clears the desk screens instead of cutting through them.

## 5. Technique chosen and why not FFT
Budget: below 1.5 ms of GPU per frame at the intended quality, CPU near zero, on an RTX 4090, never in Play.
- **FFT cascades** (web-ocean's way): needs several compute passes per frame (2x 2D FFT per cascade), 6 storage
  images, mip chains and foam textures. WayVR's wgui layer only wraps graphics pipelines (no compute, no storage images
  or barriers helpers); adding them to a vendored renderer is a large, risky change and the result would be mainly
  seen as a smooth pattern in the lower 25% of the view.
- **Sum of directional waves from a spectrum (statistical spectral synthesis)** evaluated in the fragment shader:
  N plane waves (20 / 40 / 64) with wavelengths from 10 cm to 120 m drawn from a JONSWAP spectrum, cos^2 directional
  spreading that widens for short waves, two long swells, capillary dispersion, an analytic normal and the Jacobian of
  the Gerstner displacement for whitecaps. This is the same sea statistics as Tessendorf's FFT (it is the same
  spectrum; the FFT is only a fast way to sum a very large number of waves) with a small N: periodic repetition is not
  visible because the wavelengths are incommensurate. No textures, no passes, no state.
- **Anti-aliasing instead of cascades**: each wave fades out when its wavelength is below a few texels at the point
  it is seen (computed from the texel's footprint on the water), and the slope variance of the faded waves (Cox-Munk
  style) is added to the GGX roughness of the sun glitter, so the glitter path widens with distance as web-ocean's LEAN
  mapping does, and the far sea does not shimmer.
- Sky: closed-form single scattering (Rayleigh + Mie phase, sun reddening by air mass), flat cloud layer with fbm noise
  lit from the sun side (octaves by quality), sun disc, horizon haze.
- Frame rate: the image is redrawn every 2nd or 3rd frame (quality), the layer shows the last image in between: waves
  move slowly so 45 Hz or 30 Hz of animation is not visible, and it halves or thirds the cost.

## 6. References (clean-room, none copied)
- J. Tessendorf, "Simulating Ocean Water", SIGGRAPH 2001 course notes (statistical sea, Gerstner/Jacobian foam).
- W. J. Pierson, L. Moskowitz, J. Geophys. Res. 69 (1964) (fully developed sea, peak wavenumber).
- K. Hasselmann et al., "Measurements of wind-wave growth and swell decay during JONSWAP", 1973.
- C. Cox, W. Munk, "Measurement of the roughness of the sea surface from photographs of the sun's glitter", 1954
  (used as the unit test of the roughness).
- T. Nishita et al., "Display of the Earth Taking into Account Atmospheric Scattering", SIGGRAPH 1993.
- B. Walter et al., "Microfacet Models for Refraction through Rough Surfaces", EGSR 2007 (GGX, Smith masking).
- OpenXR 1.0 specification, `XR_KHR_composition_layer_equirect2`.

## 7. Measured
RTX 4090, headless, Vulkan timestamp queries around the draw (no VR session running; a Sunshine encode was also
using the GPU at about 30 %), median of 60 frames, test `ocean::tests::gpu::offscreen_render_png_and_gpu_time`:

| Quality | Image | Waves | GPU per drawn frame | Drawn every | Per display frame |
|---|---|---|---|---|---|
| Low | 2048x1024 | 20 | 0.062 ms | 3rd | 0.02 ms |
| Medium | 4096x2048 | 40 | 0.383 ms | 2nd | 0.19 ms |
| High | 6144x3072 | 64 | 1.201 ms | 2nd | 0.60 ms |

All within the 1.5 ms budget even on a drawn frame. CPU: one uniform upload of 1.3 KB and one command buffer per drawn
frame. Not measured here (needs a headset session): Monado's cost of sampling the equirect layer into both eyes at
7104x3840 (a texture fetch per pixel, expected well below 0.2 ms) and the real VR frame times; both are to check in the
headset with the performance readout on the right wrist (`omarchyvr-perf`).

Pictures: `docs/img/ocean-high-view-sun.png` (looking at the sun), `ocean-high-view-away.png`,
`backgrounds-ocean-page.png` (the app page).

## 8. Known limits / next steps
- Voice: "animated ocean" resolves in `omarchyvr-backgrounds` (`voice_path`/`remember`), but the voice grammar in
  `omarchyvr-voice` was not touched or tested.
- No parallax and no depth (it is an image on a sphere): moving the head 1 m does not show nearer/farther waves.
- Foam is stateless (from the surface fold only): no persistent wakes.
- Fixed golden-hour light (sun 11 degrees up, 50 degrees to the right of straight ahead in the stage space) and fixed
  weather (7 m/s wind). `OceanParams` already has wind, sun, haze, cloud and exposure; exposing them in the app is a
  follow-up (time of day, calm/rough).
- The sky is cooler and the water darker than web-ocean's sunset; tune after looking in the headset.
- Equirect resolution: High is 17 pixels per degree, the Dream Air shows about 35: the sea is softer than the screens.
  A higher-resolution band around the horizon (two equirect layers) is the next step if it is needed.
