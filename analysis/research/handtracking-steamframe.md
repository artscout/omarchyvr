# Hand tracking research: Steam Frame, Monado Mercury, SteamVR (as of 2026-10-13)

Confidence tags: CONFIRMED = read in a fetched primary/official page. SECONDARY = from news/spec sites or search snippets. UNVERIFIED = could not confirm.

## Bottom line

- **Steam Frame does not ship controller-free hand tracking.** Two spec sites say so (SECONDARY). Its controllers are the input device.
- **What Valve actually opened in 2026 is Lepton** (Android compatibility layer, MIT for Valve's code, GPL-3.0 for the bundled Android rootfs). It has nothing to do with tracking.
- **No Valve release of Frame SLAM, tracking, hand-tracking models, or datasets was found** (UNVERIFIED negative; search was not exhaustive). A FOSDEM 2026 talk speaker said they doubt Valve will open-source Frame SLAM or controller tracking (SECONDARY, speaker opinion).
- **The directly reusable open hand tracker is Monado's Mercury** (optical, ONNX Runtime, CPU, BSL-1.0 Monado code; model license UNVERIFIED). It is already in Monado and needs a calibrated stereo camera pair.

## 1. Steam Frame

Hardware (SECONDARY, spec sites citing Valve's announcement):
- Four onboard greyscale tracking cameras for inside-out 6DoF of the headset and controllers; two can give monochrome passthrough. Source: https://www.uploadvr.com/valve-steam-frame-official-announcement-features-details (cited via search summary). Also https://vr.org/steam-frame-specs
- Two interior cameras for eye tracking and foveated streaming, with IR illuminators. Source: https://www.ifixit.com/Device/Steam_Frame
- Snapdragon 8 Gen 3, 16 GB LPDDR5X, 2160x2160 LCD per eye, PCIe Gen 4 and a MIPI camera interface (SECONDARY): https://vr.org/steam-frame-specs
- **Tracking camera resolution and frame rate: UNVERIFIED.** Spec sites give none for the tracking cameras; one lists 1280x1024 for passthrough, which may not be the tracking camera.
- **Hand tracking: not supported** per two spec sites (SECONDARY): https://vr.org/steam-frame-specs and a guide that says a controller is required. The controllers have finger/capacitive sensing and full 6DoF.
- Whether any tracking runs on-device or on host: UNVERIFIED. The headset is standalone Arm64 SteamOS (Arch-based), so on-device is the likely default, but no primary source confirms the tracking pipeline's location.
- Release status conflicts across sources (vr.org Aug/Sep 2026 says no price or date; Wikipedia says launch 14 Sep 2026; a GamingOnLinux Sep 2026 article mentions pre-orders). **UNVERIFIED.** Sources: https://en.wikipedia.org/wiki/Steam_Frame , https://mixed-news.com/en/?p=31540

Open source status, by category:

| Component | Open? | Evidence | License |
|---|---|---|---|
| Lepton (Android compat layer, Waydroid fork) | YES, source on Valve GitLab | https://www.gamingonlinux.com/2026/09/lepton-from-valve-to-run-android-games-on-linux-is-now-open-source/ (published 17 Sep 2026); repo gitlab.steamos.cloud/frame-public/lepton/ | MIT (Valve code); GPL-3.0 (Android rootfs) |
| SteamOS Manager | YES (opened May 2025); Frame support commit added | https://gamingonlinux.com/2026/05/were-inching-closer-to-the-steam-frame-release-with-a-steamos-manager-update | UNVERIFIED (not checked) |
| FEX | UNVERIFIED whether it is "open" as a 2026 release; the open-source FEX project predates Frame | https://vr.org/articles/valve-lepton-fex-steam-frame-translation-layers-2026 | UNVERIFIED |
| Proton | Open source since before Frame (Steam Deck era) | general knowledge, not fetched | UNVERIFIED here |
| Frame SLAM / tracking / hand models / datasets | NOT FOUND | no search hit; FOSDEM talk says Valve likely won't open these (SECONDARY) | n/a |

"Just runs on Linux" (not open source): Steam Frame runs SteamOS, and Steam/Proton/FEX/Lepton run there. This is not the same as source being released.

## 2. Monado Mercury hand tracking

Sources: https://monado.freedesktop.org/handtracking (fetched; CONFIRMED for the listed facts), https://www.collabora.com/news-and-blog/news-and-events/monados-mercury-hand-tracking-now-ready-for-use.html (Feb 24 2023, fetched), https://www.uploadvr.com/monado-hand-tracking-valve-index-reverb-g2/ (28 Feb 2023, fetched), FOSDEM 2026 talk transcript https://ftp.fau.de/fosdem/2026/h1302/T38AKM-monado-and-beyond.vtt

- **Status (2026):** The docs page still says "still in development!" Tracking is "generally good" on Valve Index and Luxonis cameras. I did not find a 2026 changelog confirming a later state. Treat as beta (CONFIRMED as of the docs page version I fetched; date of that page UNVERIFIED).
- **Hardware:** Valve Index (full support, via the front cameras); most Luxonis stereo cameras (North Star builder, "Hand-Tracking Demo" scene in monado-gui); WMR and Rift S (degraded by autoexposure). Other calibrated stereo cameras "should work" per the docs. Intel RealSense and Vive Pro are possible targets that need plumbing.
- **Camera requirements:** calibrated stereo. The docs give no resolution, fps, or IR-vs-color spec. Light matters: the docs recommend front lighting and red light, because skin reflects red best. (CONFIRMED)
- **Model format:** Neural nets run via ONNX Runtime, CPU build (`onnxruntime-linux-x64-<version>.tgz`). The docs say you do not need the GPU build. (CONFIRMED). Note that the docs do not name the models.
- **Model training data:** The FOSDEM 2026 talk says "We trained model on open and on synthetic data" (SECONDARY, transcript at ~12:07). A slide deck (https://ws.engr.illinois.edu/sitemanager/getfile.asp?id=5310) names a greenscreen dataset annotated with MediaPipe, CMU Panoptic, and FreiHand (SECONDARY). Licenses of those datasets: UNVERIFIED.
- **Latency and accuracy:** No numbers published. Qualitative only: "estimates hand pose accurately," "tracks fast hand movements," usable for drawing, typing, UI; "limited" hand-over-hand. (CONFIRMED from the Collabora and UploadVR pages.) Latency: UNVERIFIED.
- **Training repo:** `mercury_train` on freedesktop GitLab, per the docs page. Not fetched; its license is UNVERIFIED.
- **License of Monado:** Debian packaging metadata lists Monado as BSL-1.0 for the main tree, with some CMake helpers under BSD-3/Expat, docs under CC0-1.0, glad under Apache-2.0 and Khronos headers under their own license (SECONDARY: https://tracker.debian.org/media/packages/m/monado/copyright-25.0.0dfsg-2). Upstream LICENSE file: UNVERIFIED (fetch of gitlab.freedesktop.org was blocked by an access-denied page).
- **Mercury model/code license: UNVERIFIED.** Check `mercury_train` and the `src/xrt/tracking/hand/mercury` directory directly.
- **SteamVR side:** Monado hand tracking has a separate Windows SteamVR driver (Collabora / community), which was announced for "the coming weeks" in Feb 2023 (CONFIRMED that it was announced; delivery UNVERIFIED).

## 3. SteamVR driver API for hand tracking on Linux

- The OpenVR driver API exposes skeletal input via `IVRDriverInput`. The OpenVR repo includes a `samples/drivers/handskeletonsimulation` sample that derives hand positions from the HMD (SECONDARY: search summaries of https://github.com/ValveSoftware/openvr). Direct fetch of the sample source was not done, so exact code is UNVERIFIED.
- Skeletal tracking levels are `VRSkeletalTracking_Full` vs `_Partial` (SECONDARY: https://creators.vrchat.com/platforms/pc/steamvr-drivers/).
- License: OpenVR is BSD-3-Clause (SECONDARY, Debian copyright file, Arch metadata). Some bundled files (jsoncpp) are under other licenses. Sample header license: UNVERIFIED.
- Linux example: ALVR's Linux docs describe a skeleton path that maps 26 OpenXR joints to 31 SteamVR bones, exposed at `/input/skeleton/left|right` (SECONDARY: https://deepwiki.com/alvr-org/ALVR/7.1-linux-troubleshooting). This is a usable reference for the joint-mapping problem.
- Gap: no Linux-specific SteamVR driver loading details were found. Would need the Valve Skeletal Input Driver docs (UNVERIFIED URL) or the Monado/ALVR source.

## 4. 2025–2026 Valve open-source news relevant to tracking

- Lepton open-sourced Sep 2026 (see table above). Not tracking.
- SteamOS Manager open-sourced May 2025 (SECONDARY).
- No news found of Valve releasing Frame SLAM, inside-out tracking code, or datasets. UNVERIFIED negative.
- Community: TouchFrame (MIT, GitHub) lets Quest 3 Touch Plus controllers work on Steam Frame, with a second headset doing the tracking (SECONDARY: https://www.digitalcitizen.life/steam-frame-touchframe-quest-3-controller-driver-mod/). Not Valve.

## What we could reuse directly

1. **Monado Mercury (already in Monado).** Use the existing `src/xrt/tracking/hand/mercury` pipeline with a calibrated stereo pair. This is the only open, drop-in hand tracker here. Needs ONNX Runtime CPU build and OpenCV. Check license of the model weights before shipping.
2. **OpenVR `handskeletonsimulation` + ALVR's 26-to-31 joint mapping.** Reference for feeding skeletal input into SteamVR-style drivers. Not needed if we stay in OpenXR (Monado exposes `XR_EXT_hand_tracking` directly).
3. **Lepton/FEX/Proton.** Not relevant to tracking. Only useful if we want to run Frame-targeted software stacks.
4. **Frame-specific SLAM or tracking code.** Not available. Do not plan around it.

## Open questions (UNVERIFIED)

- Steam Frame tracking-camera resolution, fps, shutter type, and whether any tracking runs on-device.
- Mercury model weights license and dataset licenses (CMU Panoptic, FreiHand, the greenscreen set).
- Current Mercury latency and accuracy numbers.
- Whether the Monado upstream LICENSE file matches the Debian summary.
- Steam Frame release status (launch date vs pre-order only).

## Sources

- https://monado.freedesktop.org/handtracking
- https://www.collabora.com/news-and-blog/news-and-events/monados-mercury-hand-tracking-now-ready-for-use.html
- https://www.uploadvr.com/monado-hand-tracking-valve-index-reverb-g2/
- https://ftp.fau.de/fosdem/2026/h1302/T38AKM-monado-and-beyond.vtt
- https://ws.engr.illinois.edu/sitemanager/getfile.asp?id=5310
- https://tracker.debian.org/media/packages/m/monado/copyright-25.0.0dfsg-2
- https://www.gamingonlinux.com/2026/09/lepton-from-valve-to-run-android-games-on-linux-is-now-open-source/
- https://vr.org/steam-frame-specs
- https://www.ifixit.com/Device/Steam_Frame
- https://en.wikipedia.org/wiki/Steam_Frame
- https://creators.vrchat.com/platforms/pc/steamvr-drivers/
- https://deepwiki.com/alvr-org/ALVR/7.1-linux-troubleshooting
- https://www.digitalcitizen.life/steam-frame-touchframe-quest-3-controller-driver-mod/
- https://gamingonlinux.com/2026/05/were-inching-closer-to-the-steam-frame-release-with-a-steamos-manager-update
