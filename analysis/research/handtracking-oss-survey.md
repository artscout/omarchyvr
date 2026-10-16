# Open-source hand tracking survey for Monado / OpenXR XR_EXT_hand_tracking

Date of research: 2026-10-13. Verified = read from the primary page during this session. UNVERIFIED = secondhand, from search snippets only, or not checked. Several dates could not be retrieved (GitHub pages showed commit counts but no dates), so "last activity" is marked UNVERIFIED throughout.

## Comparison table

| Option | Repo / source | License (code / weights) | Commercial use | Input | Output | Real-time | Last activity | Monado / OpenXR status |
|---|---|---|---|---|---|---|---|---|
| Monado Mercury | gitlab.freedesktop.org/monado/monado, src/xrt/tracking/hand/mercury | BSL-1.0 (per header in hg_interface.h, search snippet) | Yes for BSL-1.0 code; model weights UNVERIFIED | Stereo camera pair (Index, WMR, Luxonis per Collabora blog) | 26-joint hand data via Monado OpenXR; ONNX detector (160 px) + keypoint net (128 px) | Yes on consumer GPU/CPU (UNVERIFIED FPS) | UNVERIFIED (Monado 25.1.0 release reported Dec 2025) | Native, in-tree |
| Monado "old_rgb" | UNVERIFIED, could not find the name in any source | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED. Check the Monado tree directly |
| Monado Ultraleap driver (V2 and V5 variants) | Monado docs, drv group | Monado BSL-1.0 | Driver is fine. Ultraleap SDK terms apply (below) | Leap Motion Controller (two NIR cameras) | Hand joints fed to Monado tracking | Yes (hardware) | UNVERIFIED | Native driver; needs leapd running and Leap headers at build time |
| Ultraleap Gemini / LeapC / OpenXR API layer | docs.ultraleap.com; ultraleap/leapc-python-bindings; ultraleap-archlinux/openxr-layer-ultraleap (unofficial) | Proprietary SDK agreement. Personal, non-commercial, consumer game dev, and evaluation are allowed; other uses need to contact Ultraleap (search snippet) | NO without a commercial agreement | Leap Motion Controller stereo IR | XR_EXT_hand_tracking via the layer, plus XR_ULTRALEAP_hand_tracking_forearm | Yes (hardware) | Gemini v5.20.0 dated 26/04/2024 per download page (UNVERIFIED that this is latest) | Not in Monado. Works as an OpenXR API layer on any runtime. The layer intercepts only the hand-tracking extension calls |
| MediaPipe Hands / Hand Landmarker | github.com/google-ai-edge/mediapipe | Apache-2.0 for code (Verified: LICENSE file). Model weights terms UNVERIFIED (secondhand Core ML port says Apache-2.0 terms, UNVERIFIED) | Likely yes for Apache code; confirm weight terms | RGB (single frame) | 21 3D landmarks per hand (palm detector + landmark net) | Yes, phone-class CPU/GPU (Google claim) | UNVERIFIED | Not integrated. Needs a 21 to 26 joint mapping |
| HaMeR | github.com/geopavlakos/hamer | MIT for code (repo page verified: "MIT license"). MANO must be licensed separately | Code yes. MANO license restricts use | RGB, single image | MANO mesh + 3D joints (MANO joints, not XR 26) | UNVERIFIED (ViT-H, likely not real-time on CPU) | UNVERIFIED | Not integrated |
| WiLoR | github.com/rolpotamias/WiLoR (CVPR 2025, arXiv 2409.12259) | Models: CC-BY-NC-ND (Verified on repo page). MANO separate. Ultralytics dependency under its own license | NO. Non-commercial, no derivatives | RGB, single image | 3D hands, MANO-based | UNVERIFIED (has a `--fast` mode with half precision and depth pruning, README March 2026 update) | UNVERIFIED (22 commits) | Not integrated |
| UmeTrack | github.com/facebookresearch/UmeTrack | CC BY-NC 4.0 (Verified: LICENSE) | NO | Multi-view headset-style cameras (UNVERIFIED specifics) | Hand skeleton + mesh (UmeTrack format) | Pretrained inference model included (speed UNVERIFIED) | UNVERIFIED | Not integrated |
| HOT3D (Meta, egocentric) | projectaria.com/datasets/hot3D/license; facebookresearch.github.io/hot3d | Sequence data: CC BY-SA. Hand data: CC BY-NC-SA. 3D object models: CC BY-SA with no-sell (Verified) | NO for hand data. No selling of object models | Aria and Quest 3 multi-view | Hand poses in UmeTrack/MANO format | Dataset, not a model | 2024 paper, 2024-11 v2 | Not a tracker. Data only |
| hand_tracking_toolkit (Meta) | github.com/facebookresearch/hand_tracking_toolkit | Apache-2.0 (Verified). SMPLX/MANO dependencies have their own licenses | Toolkit yes. Dataset and MANO terms apply | n/a | Loaders for UmeTrack and HOT3D | n/a | UNVERIFIED (45 commits) | Not integrated |
| InterHand2.6M | mks0601.github.io/InterHand2.6M (paper arXiv 2008.09309) | UNVERIFIED (license page not retrieved) | UNVERIFIED | RGB, multi-view capture | 3D joints only (no mesh). Paper ECCV 2020 | Baseline InterNet, speed UNVERIFIED | 2020 | Dataset, not integrated |
| MobileHand | github.com/gmntu/mobilehand | UNVERIFIED | UNVERIFIED (needs MANO, separate license) | RGB, single image | 3D shape + pose | Claims >110 Hz on GPU (UNVERIFIED) | UNVERIFIED | Not integrated |
| HandMvNet (VISAPP 2025) | github.com/pyxploiter/handmvnet | UNVERIFIED | UNVERIFIED | Multi-view RGB | 3D pose | Claims real-time (UNVERIFIED) | 2025 | Not integrated |
| FrankMocap, MobRecon, HandOccNet, A2J | NOT RESEARCHED in this pass | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED |
| Quest / Frame-style IR mono trackers | NOT RESEARCHED. Proprietary to Meta and Frame, UNVERIFIED general knowledge | Proprietary | No | Mono IR | Proprietary | Yes | n/a | Not available |
| driver_leap, VMT, SlimeVR, HandsOnVR, webcam OpenXR hand tracking | Search returned nothing relevant | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED | UNVERIFIED. Re-search each by name before relying on them |

## Key findings

1. Monado's own Mercury tracker is the only option here that is native to Monado and already speaks OpenXR hand tracking. Its code is BSL-1.0 (permissive). Its weights and any licence limits are not verified.
2. Ultraleap Gemini is not free for commercial use. Its OpenXR layer is clean for personal use only.
3. The strong-quality 2024-2026 models (WiLoR, HaMeR) have licence problems for a commercial product: WiLoR is CC-BY-NC-ND, and HaMeR's MANO dependency is a separately licensed body model. Both are fine for personal research.
4. Meta's public hand datasets (UmeTrack, HOT3D) are non-commercial for hand data. They are useful for evaluation or training only if the project stays non-commercial.
5. MediaPipe is the most permissive mainstream option (Apache-2.0 code), but it is a 21-landmark 2D/relative-3D model, so it needs a mapping to the 26-joint XR layout.

## Recommendations

(a) Fastest path to working hands in Monado this month:
- Use a Leap Motion Controller through Monado's Ultraleap driver (V5 variant), with leapd running. This is hardware-proven and needs no model training. Personal use only under the Ultraleap licence.
- In parallel, check whether Mercury can take the Pimax Dream Air's SLAM/front cameras. This needs their frames plus a stereo calibration, which we do not have yet.

(b) Best long-term quality path:
- Use Mercury's architecture (detector then keypoint net on crops, stereo or mono), and train or fine-tune a keypoint network on data we can use commercially. MediaPipe weights (Apache-2.0) are a candidate starting point, pending confirmation of the weight terms. Avoid WiLoR and HaMeR in a shipped product because of their licences.

(c) Ideas to borrow:
- Filtering: One Euro filter on joint positions (general technique, UNVERIFIED that Mercury uses it).
- Pose prediction: extrapolate joints with the headset's IMU or head pose, and use Monado's existing prediction timing.
- Keypoint-to-XR mapping: the 26-joint XR_EXT_hand_tracking layout includes palm, wrist, and forearm. Ultraleap's layer adds a forearm extension, which shows one way to add the extra joints. MANO's 16 joints plus fingertips can be mapped to the XR set (UNVERIFIED mapping table).

## Sources
- Monado Mercury API docs: https://monado.pages.freedesktop.org/monado/structxrt_1_1tracking_1_1hand_1_1mercury_1_1_hand_tracking.html
- Collabora Mercury blog: https://www.collabora.com/news-and-blog/news-and-events/monados-mercury-hand-tracking-now-ready-for-use.html
- Monado Ultraleap driver docs: https://monado.pages.freedesktop.org/monado/md_drivers_ultraleap_v2_readme.html
- Ultraleap OpenXR layer install: https://docs.ultraleap.com/openxr/installation/index.html
- MediaPipe LICENSE (Apache-2.0): https://github.com/google-ai-edge/mediapipe/blob/master/LICENSE
- MediaPipe hands doc: https://github.com/google/mediapipe/blob/master/docs/solutions/hands.md
- HaMeR: https://github.com/geopavlakos/hamer
- WiLoR: https://github.com/rolpotamias/WiLoR
- UmeTrack LICENSE (CC BY-NC 4.0): https://github.com/facebookresearch/UmeTrack/blob/main/LICENSE
- HOT3D license: https://projectaria.com/datasets/hot3D/license
- hand_tracking_toolkit: https://github.com/facebookresearch/hand_tracking_toolkit
- InterHand2.6M: https://arxiv.org/abs/2008.09309
- MobileHand: https://github.com/gmntu/mobilehand
- HandMvNet: https://www.scitepress.org/PublishedPapers/2025/131073
