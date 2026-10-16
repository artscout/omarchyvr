# Room model, world locking and relocalisation: research notes

Scope: how headsets keep content world-locked, what Monado/Basalt can reuse, what inside-out tracking needs, how to smooth corrections, and what "recognise objects in place" can mean on Linux.
Date of research: 2026-10-14. No device access. No code run.

Tags: **[V]** fetched page (content read this session), **[S]** search-result snippet only (not opened), **[M]** memory / unverified, to check.

Known gaps are listed at the end. Several vendor numbers could not be found and are marked as gaps rather than filled from memory.

---

## 0. Headline numbers (short list)

| Item | Value | Tag | Source |
|---|---|---|---|
| Single spatial anchor useful range (Meta) | 3 m; "points farther... will suffer ever increasing errors" | [V] | https://developers.meta.com/horizon/documentation/unity/unity-spatial-anchors-best-practices/ |
| Content beyond which Meta suggests fade in/out | 10 m | [V] | same |
| Microsoft drift example | walking 10 m between QR codes moves 11 m in virtual space (~10 %) | [V] | https://microsoft.github.io/MixedReality-WorldLockingTools-Unity/DocGen/Documentation/IntroFAQ.html |
| Microsoft: single anchor poor for objects > about 1 m | ~1 m | [V] | same |
| HoloLens 2 surgical registration, median misalignment (x,y,z) | 22 / 36 / 13 mm (first metric); 4–12 mm (perceptual metric) | [S] | https://www.degruyterbrill.com/document/doi/10.1515/cdbme-2021-1024/pdf |
| HoloLens 2 device scale error (Trimble white paper) | about -0.03 % | [S] | https://fieldtech.trimble.com/resources/ebooks/white-paper-precision-and-accuracy-of-holograms-in-trimble-connect-for-hololens |
| Steam Frame tracking update rate (one secondary summary) | ~200–250 Hz | [S] (unverified) | https://sozai.app/transcript/vr-headset-steam-frame-review/index.md |
| Steam Frame tracking cameras | 4 outward-facing monochrome cameras + IR illuminators | [S] | https://partner.steamgames.com/doc/steamframe |
| Pimax Dream Air SLAM cameras (marketing) | 4 cameras, PC-side SLAM | [S] | https://pimax.com/blogs/blogs/pimax-launches-affordable-slam-tracked-micro-oled-vr-headset ; https://www.scan.co.uk/products/pimax-dream-air-slam-vr-headset-2x-3840-x-3552-micro-oled-lenses-2x-6dof-controllers-53-ppd-90hz-dis |
| Pimax Dream Air camera count vs our kit | Marketing says 4; our kit notes say 2 lower tracking cameras | conflict, check | `analysis/research/dreamair-cameras.md` (local) |
| Basalt VIO-factor mapping | no numbers in abstract; "superior performance" on a public benchmark | [V] | https://arxiv.org/abs/1904.06504 |
| Quest / Pico / Vision Pro / Varjo drift and motion-to-photon numbers | not found | gap | see section 9 |

---

## 1. How the major platforms keep content world-locked

### 1.1 Meta Quest (Insight / inside-out)
- Inside-out tracking: embedded sensors and cameras, no base stations. [S] https://developers.meta.com/horizon/design/head-technology/
- World-locked content uses **spatial anchors**. An anchor is a pose the runtime keeps re-estimating against its internal map. Anchors can be saved and reloaded across sessions. [V] https://developers.meta.com/horizon/documentation/unity/unity-spatial-anchors-best-practices/
- Guidance: create or reuse an anchor **within 3 m** of the content; keep content within 3 m of its anchor; create a new anchor per independent object; anchors **cannot be moved** (delete and recreate). [V] same page
- Room scoping: filter anchor queries by room component or room UUID so anchors from other rooms (floors above/below) are excluded. [V] same page
- Persistence: save anchor UUIDs to external storage (JSON, PlayerPrefs). When several anchors match, use the **closest** one, not the most recent. If an anchor is not found, prompt re-placement or auto-reposition. [V] same page
- No maximum anchor count stated. [V] same page
- Shared (colocated) anchors: developers are asked to walk "in a large circle" before use; no distance given. [V] same page
- Colocation history: an UploadVR article predates the feature shipping. [S] https://uploadvr.com/quest-shared-space-colocation-support/
- Community reports of drift in large free-roam spaces: sparse or uneven features, lighting changes and multi-user map mismatch cited as causes. Not official. [S] (Meta community forum, linked from the search results)
- Relocalisation internals (how the headset re-finds its pose in a saved space): **not found**. Setup-flow internals: **not found**. Mark as gap.
- Latency / prediction numbers for Quest: **not found**. [M] Quest uses IMU-based prediction ahead of display scan-out; treat as unverified.

### 1.2 Apple Vision Pro (visionOS)
- Apple's `WorldTrackingProvider` provides `WorldAnchor`s. Community forum reply says world anchors persist across restarts; points to Apple's "Object Placement" sample (`PersistenceManager.swift`). [S] https://developer.apple.com/forums/thread/745888 ; https://developer.apple.com/forums/thread/805565
- Developer reports: after recreating an anchor from saved UUID and transform on relaunch, placement still resolves relative to current device pose, not the original wall position. No official answer in results. [S] https://developer.apple.com/forums/thread/805672
- Limits: WorldAnchors are on-device and not exportable as an ARWorldMap-like cross-device map (feature request); anchors are removed if the creating app is deleted or the device is reset. [S] https://developer.apple.com/forums/thread/831943
- Numbers for drift / relocalisation on Vision Pro: **not found**. Apple's official `WorldTrackingProvider` docs were not fetched. Gap.

### 1.3 HoloLens 2 / WMR (Windows Mixed Reality)
- World-locking via **spatial anchors** plus SLAM. Microsoft World Locking Tools: anchors are needed because "error grows with distance from the anchor". [V] https://microsoft.github.io/MixedReality-WorldLockingTools-Unity/DocGen/Documentation/IntroFAQ.html
- Drift can happen on one-way trips as well as round trips. Returning to a previously seen pose lets the system restore the frame. [V] same
- Correction behaviour: small corrections applied **smoothly** to the camera/spatial frame to "minimize its perception"; larger fixes are only applied when the client requests them (client is notified of a potential fix and updates its objects). [V] same
- Measured accuracy: HoloLens 2 surgical study, median misalignment 22 / 36 / 13 mm (x, y, z) against tracked markers; perceptual metric 4–12 mm. [S] https://www.degruyterbrill.com/document/doi/10.1515/cdbme-2021-1024/pdf
- Scale error ~ -0.03 %. [S] https://fieldtech.trimble.com/resources/ebooks/white-paper-precision-and-accuracy-of-holograms-in-trimble-connect-for-hololens
- Trimble white paper mechanism: SLAM accumulates error as the user moves; holograms shift away from anchor on return to start. [S] same
- Note: the HL2 surgical number is an ~1 m / clinical-setup measurement, not a general spec. Do not use as a headline.

### 1.4 Pico (Pico 4 / Pico OS)
- Searched; nothing usable returned. Gap. [M] Pico uses an inside-out SLAM stack with anchor-based world locking; specifics unverified.

### 1.5 Varjo (XR-4 / XR-3 inside-out)
- Searched; nothing usable returned. Gap. Varjo is commonly documented as using a Lighthouse or external-tracking setup for some models; not verified here. [M]

### 1.6 Valve Steam Frame
- Four outward-facing monochrome cameras, IR illuminators for low light. [S] https://partner.steamgames.com/doc/steamframe
- Tracking update rate "200–250 Hz" in one secondary summary. [S, unverified] https://sozai.app/transcript/vr-headset-steam-frame-review/index.md
- Eye tracking drives foveated streaming; Valve's "almost 10x" detail claim is a vendor claim. [S] https://www.ifixit.com/Device/Steam_Frame (and Steamworks docs)
- Persistence / anchor / relocalisation internals: **not found**. Gap.

### 1.7 Pimax (Dream Air / Dream Air SE; `platMap_orborb`)
- Pimax says it develops its **own SLAM** tracking; no result confirms ORB-SLAM use. [S] https://pimax.com/blogs/blogs/pimax-launches-affordable-slam-tracked-micro-oled-vr-headset
- One review quotes Pimax: headset and controller tracking processed "on the PC-side with our own SLAM algorithms in the runtime." [S] https://simracingcockpit.gg/pimax-dream-air-slam-review/
- `platMap_orborb`: **not found** in any search. Name suggests an ORB-feature map store; this is an inference, tag [M]. Check the Pimax runtime in the local kit (`~/omarchyvr`) before relying on it.
- Camera count: marketing says 4 cameras; our kit notes say 2 lower tracking cameras. **Conflict, verify against `analysis/research/dreamair-cameras.md`.**

### 1.8 Summary table

| Platform | Map / anchor model | Persistence | Relocalisation time | Correction hiding | Numbers found |
|---|---|---|---|---|---|
| Quest | spatial anchors + room scope | save UUID externally, reload closest | not found | not found | 3 m anchor range, 10 m fade |
| Vision Pro | WorldAnchor on-device | survives restart (forum) | not found | not found | none |
| HoloLens 2 | spatial anchors + SLAM | on-device | not found | smooth frame blend, big fixes on request | 22/36/13 mm surgical; 11 m per 10 m walk example |
| Pico | not found | not found | not found | not found | none |
| Varjo | not found | not found | not found | not found | none |
| Steam Frame | not found | not found | not found | not found | 200–250 Hz (unverified) |
| Pimax Dream Air | own SLAM, PC-side | not found | not found | not found | 4 cameras (conflict with kit) |

---

## 2. Open-source options on top of Basalt in Monado

### 2.1 Basalt (VIO + mapping, the current Monado backend)
- Basalt mapping: **VIO factors are reconstructed as non-linear factors** that approximate the information accumulated along the trajectory; these are combined with loop-closure constraints in bundle adjustment. Claims: VIO factors make roll and pitch of the global map observable; "superior performance" on a public benchmark. [V] https://arxiv.org/abs/1904.06504
- Inertial information "typically contains only little useful information" after several seconds, so mapping uses keyframes with large gaps. [V] same
- Basalt VIO stage is fixed-lag smoothing; mapping extracts loop-closure features. [S] https://arxiv.org/pdf/2206.05066 (survey)
- Repo: GitLab Basalt with GitHub mirror; includes camera, IMU and mocap calibration. [S] https://cvg.cit.tum.de/research/vslam/basalt
- Relocalisation in Basalt: **not in the paper's abstract**; not verified. [V-abstract] Gap.
- Basalt is CPU heavy per a FOSDEM 2026 slide deck: "tracking quality is not reaching commercial systems yet." [S] https://fosdem.org/2026/events/attachments/T38AKM-monado-and-beyond/slides/266219/monado_an_tfzl8ak.pdf
- Licence: [M] Basalt is BSD-3-Clause. Verify in the repo LICENSE before relying on it.

### 2.2 ORB-SLAM3 Atlas
- Atlas = set of disconnected maps; one active. On tracking loss, the current frame is relocalised against **all** maps in the Atlas; active map switches if successful; otherwise a new map starts. [S] https://arxiv.org/pdf/2007.11898
- Shared DBoW2 keyframe database used for relocalisation, loop closing and map merging. [S] same
- Save/load: config keys `System.LoadAtlasFromFile` / `System.SaveAtlasToFile`; saved at shutdown. Direct SaveMap/LoadMap marked TODO in `System.h`. [S] https://www.mintlify.com/UZ-SLAMLab/ORB_SLAM3/guides/map-save-load (third-party mirror; not the repo)
- Licence: **GPLv3**; commercial use requires contacting the authors. [S] https://www.mintlify.com/UZ-SLAMLab/ORB_SLAM3/introduction ; https://anaconda.org/fontan/orbslam3 (GPL-3.0-only)
- Implication: GPLv3 is a problem for a Monado-linked binary. Use ORB-SLAM3 as a **benchmark / reference only**, not as linked code.
- Monado has an ORB-SLAM3 fork for euroc / realsense drivers (MR 933). [S] https://gitlab.freedesktop.org/monado/monado/-/merge_requests/933

### 2.3 maplab, OpenVINS, Kimera, VINS-Fusion
- maplab (ETH): multi-session map merging, map save/load, and localisation in a saved map. Licence Apache-2.0 [M]. Not searched this session. Verify.
- OpenVINS (UDel): MSCKF-style filter, no map persistence by default; BSD-3 [M]. Not searched. Verify.
- Kimera-VIO (MIT): Monado integrated Kimera as an external SLAM system in MR 889. [S] https://gitlab.freedesktop.org/monado/monado/-/merge_requests/889 ; licence BSD-2 [M].
- VINS-Fusion loop closure: GPLv3 [M]. Not searched. Verify. (Licence risk as ORB-SLAM3.)

### 2.4 Monado's SLAM tracker (`t_tracker_slam`)
- Code path: `flush_poses` dequeues tracked poses from the SLAM system and updates prediction data; `predict_pose` returns the best estimate for a requested time using all data; `filter_pose` removes noise from the predicted trajectory. [S] https://monado.pages.freedesktop.org/monado/t__tracker__slam_8cpp.html
- MR 889 (Kimera, external SLAM interface, preliminary): "no pose prediction given a timestamp, the device just gets the last predicted pose from a queue." [S] https://gitlab.freedesktop.org/monado/monado/-/merge_requests/889
- MR 941: Basalt added as an external SLAM system (stereo + IMU, euroc / realsense drivers). [S] https://gitlab.freedesktop.org/monado/monado/-/merge_requests/941
- MR 1060: adds timestamps to returned poses and predicts from the **last two** tracked poses, so tracking is not limited to camera fps. Merge state **not confirmed** (page blocked by Anubis when fetched). [S] https://gitlab.freedesktop.org/monado/monado/-/merge_requests/1060 **Check this MR before writing any prediction code.**
- Basalt crash issue: `SIGABRT: did not expect numerical failure during linearization` after a few minutes on an HP Reverb G2. Relevant to robustness. [S] https://gitlab.freedesktop.org/monado/monado/-/issues/256
- Monado 6DoF history: initial 6DoF (camera + IMU) with Kimera-VIO, ORB-SLAM3, and Basalt as backends (Collabora 2022). [S] https://www.collabora.com/news-and-blog/blog/2022/04/05/visual-inertial-tracking-support-for-monado-openxr/
- Monado device table: Valve Index and Rift S listed as 6DoF via Basalt. [S] Monado webpage (search result, not separately opened).
- Relocalisation / map persistence in Monado: **no issue or MR found** in search. Gap. Next search: Monado GitLab issues for "relocalize", "map save", "slam persist".

### 2.5 Licences at a glance

| Option | Licence | Use in Monado |
|---|---|---|
| Basalt | BSD-3 [M] | already the backend |
| Kimera-VIO | BSD-2 [M] | reference / past backend |
| ORB-SLAM3 | GPLv3 [S] | benchmark only |
| VINS-Fusion | GPLv3 [M] | benchmark only |
| OpenVINS | BSD-3 [M] | candidate |
| maplab | Apache-2.0 [M] | candidate for map ideas |

---

## 3. Requirements for good inside-out tracking

- **Frame rate:** not established in search. [M] Consumer VIO usually runs camera at 20–30 Hz with IMU at 200–1000 Hz. 25 Hz is at the low end. Test 30 / 60 Hz if the sensor allows.
- **Camera-IMU time offset:** Ling et al. (ECCV 2018) treat the time offset as an unknown that changes over time and estimate it inside the VIO optimisation. Consumer devices typically have rolling-shutter cameras and imperfect synchronisation. [S] https://www.ecva.net/papers/eccv_2018/papers_ECCV/html/Yonggen_Ling_Modeling_Varying_Camera-IMU_ECCV_2018_paper.php
- **Rolling shutter:** Ctrl-VIO (RA-L 2022) calibrates per-line exposure delay online and uses a continuous-time trajectory. [S] https://arxiv.org/abs/2208.12008v1 — our cameras are global shutter [M]: verify. If they are, rolling-shutter calibration is not needed.
- **Exposure / gain:** Auto-exposure is missing on Linux (per our notes). Exposure must stay short to limit blur; image quality matters more than frame rate for feature tracking. [M]
- **Motion blur:** no quantitative threshold found. Gap.
- **Camera-IMU extrinsics accuracy:** no sensitivity study found. Gap. Our factory calibration should cover this; verify the calibration file and check reprojection residuals in Basalt's calibration tool.
- **IMU noise calibration:** Basalt's calibration includes IMU noise; verify that the noise model (accel/gyro random walk and white noise) matches the datasheet. [M]
- **Lighting and texture:** featureless walls and low light break VIO. Steam Frame uses IR illuminators for low light. [S] https://partner.steamgames.com/doc/steamframe
- **Dynamic objects:** people and moving objects in view degrade VIO. [M]

---

## 4. Smoothing corrections, prediction and anchor re-snapping

- **HoloLens / WMR approach (documented):** small map corrections are applied smoothly to the spatial frame; large corrections are only applied when the app asks, and the app then updates its own objects. [V] https://microsoft.github.io/MixedReality-WorldLockingTools-Unity/DocGen/Documentation/IntroFAQ.html — this is the closest documented model for our case.
- **Meta approach (documented):** keep content close to its anchor (3 m) so the correction is small. [V] https://developers.meta.com/horizon/documentation/unity/unity-spatial-anchors-best-practices/
- **Prediction in Monado:** `predict_pose` plus the proposed "predict from last two poses" in MR 1060. [S] Check MR 1060 state.
- **Filtering:** `filter_pose` in Monado's SLAM tracker. [S] Parameters not read.
- **Smoothing options [M]:** a One-Euro filter is usually suited to the rendered pose (low lag, adaptive smoothing), not to the VIO estimate. Use a slow blend (time constant of a few hundred ms) for anchor-frame offsets, and clamp the blend rate. These are design choices, not sourced numbers.
- **Anchor re-snap after relocalisation [M]:** store anchors in the map frame. On relocalisation, recompute the map-to-world transform, then blend the frame offset over N frames rather than snapping. This matches the HoloLens pattern.
- **Anchor ambiguity:** Meta says use the closest matching anchor when several match. Apply the same rule.
- **Motion-to-photon latency:** no measured numbers found for any platform in this session. Gap. Prediction is the usual mitigation. [M]

---

## 5. Recognising objects in place: room models, planes, object SLAM

- **OpenXR scene extensions:**
  - `XR_MSFT_scene_understanding` is **obsoleted** by `XR_EXT_spatial_entity` (registry status: not ratified, obsoleted). [S] https://registry.khronos.org/OpenXR/specs/1.1/man/html/XR_MSFT_scene_understanding.html
  - `XR_EXT_spatial_entity` introduces scene understanding and spatial reasoning foundations for OpenXR; contributors include Meta and Microsoft. [S] https://developer.android.com/develop/xr/openxr/extensions/XR_EXT_spatial_entity
  - Meta's `XR_FB_spatial_entity` is the base for Meta's spatial extensions; `XR_FB_scene` adds rooms, objects and boundaries; `XR_FB_spatial_entity_query` and `_storage` are companions. [S] Unity Meta OpenXR docs (search result).
  - MSFT plane model: planes are child components of scene objects; alignment types non-orthogonal, horizontal, vertical. [S] https://registry.khronos.org/OpenXR/specs/1.0/man/html/XrScenePlaneMSFT.html
- **Feasibility on Linux with Monado:** no scene-understanding extension is implemented in Monado as far as searches show. [M] Gap. Plane detection would need to be built from Basalt landmarks plus the IMU gravity vector (floor plane first). Object-level SLAM (for example, chairs) is beyond the current stack. [M]
- **Room-level model:** a point cloud or sparse landmark map from Basalt is enough for world locking. A dense mesh needs depth or a second sensor, which the Dream Air does not have per our kit notes [M].

---

## 6. Ten recommendations, in order

1. **Measure before you change anything.** Log Basalt's residuals, IMU-camera time offset, and bias estimates on our Dream Air data. Verify the camera count conflict (2 vs 4) first, because it changes everything downstream. (Sources: dreamair-cameras.md; Ling 2018.)
2. **Fix exposure first.** Add auto-exposure (short exposure, gain-first) before tuning anything else. VIO depends on feature quality more than on frame rate. [M]
3. **Verify time sync and shutter type.** Confirm global shutter and check the time offset estimate. Enable Basalt's online time-offset estimation if it is not on. [M]
4. **Keep the anchors short-range.** Pin content to anchors within about 3 m (Meta guidance), and split large objects across several anchors (Microsoft: single-anchor error grows with distance). Use these as design targets. [V]
5. **Read Monado MR 1060 and the `t_tracker_slam` prediction code** before writing any prediction logic. Confirm the merge state first; the search could not open the page. [S]
6. **Design the anchor store now, even if persistence comes later.** Store anchors as poses in the Basalt map frame. Save with a timestamp and a landmark signature. On reload, match the closest anchor (Meta rule). [V]
7. **Use Basalt's own mapping and loop closure first.** It already does VIO-factor mapping with loop-closure constraints (Usenko 2019). Benchmark it on a 5-minute room walk before adding anything. Expect high CPU; measure. [V][S]
8. **Use ORB-SLAM3 Atlas as a reference only, not as linked code.** Its GPLv3 licence rules it out for a shipped binary. Compare its relocalisation behaviour against Basalt's on the same room data. [S]
9. **Smooth corrections as the HoloLens model does.** Blend small frame corrections over time; surface large corrections to the app as an event rather than a jump. [V]
10. **Build plane detection from landmarks plus gravity, floor first.** Skip OpenXR scene extensions for now; XR_EXT_spatial_entity is the target if Monado implements it later. [S]

---

## 7. Gaps and next searches

- Quest relocalisation time, drift figures, motion-to-photon (search Meta Horizon docs, Meta papers).
- Vision Pro WorldTrackingProvider official docs and any drift numbers (fetch Apple's developer documentation pages).
- Pico and Varjo tracking internals (search Pico developer docs, Varjo SDK docs).
- Monado GitLab issues for relocalisation and map persistence (search gitlab.freedesktop.org issues: "relocalize", "map", "persist").
- Basalt relocalisation capability and licence (fetch Basalt repo LICENSE and README).
- Motion-to-photon and frame-rate study for inside-out VR (academic: e.g. VIO at 30/60 Hz papers).
- Pimax runtime: check `platMap_orborb` and whether ORB-SLAM is used, from the local kit or the Pimax SDK.
- Monado MR 1060 state (open the MR in a browser or via the GitLab API).

Note on blocked pages: the Monado MR 1060 page returned an Anubis access-denied page, so the MR claims above are snippet-only.

---

## 8. Sources (fetched [V] unless noted)

- https://arxiv.org/abs/1904.06504 [V]
- https://microsoft.github.io/MixedReality-WorldLockingTools-Unity/DocGen/Documentation/IntroFAQ.html [V]
- https://developers.meta.com/horizon/documentation/unity/unity-spatial-anchors-best-practices/ [V]
- https://gitlab.freedesktop.org/monado/monado/-/merge_requests/1060 [blocked, S]
- https://monado.pages.freedesktop.org/monado/t__tracker__slam_8cpp.html [S]
- https://gitlab.freedesktop.org/monado/monado/-/merge_requests/889 [S]
- https://gitlab.freedesktop.org/monado/monado/-/merge_requests/933 [S]
- https://gitlab.freedesktop.org/monado/monado/-/merge_requests/941 [S]
- https://gitlab.freedesktop.org/monado/monado/-/issues/256 [S]
- https://www.collabora.com/news-and-blog/blog/2022/04/05/visual-inertial-tracking-support-for-monado-openxr/ [S]
- https://fosdem.org/2026/events/attachments/T38AKM-monado-and-beyond/slides/266219/monado_an_tfzl8ak.pdf [S]
- https://arxiv.org/pdf/2007.11898 [S]
- https://www.mintlify.com/UZ-SLAMLab/ORB_SLAM3/guides/map-save-load [S, third-party]
- https://www.mintlify.com/UZ-SLAMLab/ORB_SLAM3/introduction [S]
- https://anaconda.org/fontan/orbslam3 [S]
- https://www.degruyterbrill.com/document/doi/10.1515/cdbme-2021-1024/pdf [S]
- https://fieldtech.trimble.com/resources/ebooks/white-paper-precision-and-accuracy-of-holograms-in-trimble-connect-for-hololens [S]
- https://developer.apple.com/forums/thread/745888 [S]
- https://developer.apple.com/forums/thread/805565 [S]
- https://developer.apple.com/forums/thread/805672 [S]
- https://developer.apple.com/forums/thread/831943 [S]
- https://uploadvr.com/quest-shared-space-colocation-support/ [S]
- https://partner.steamgames.com/doc/steamframe [S]
- https://sozai.app/transcript/vr-headset-steam-frame-review/index.md [S, unverified]
- https://pimax.com/blogs/blogs/pimax-launches-affordable-slam-tracked-micro-oled-vr-headset [S]
- https://simracingcockpit.gg/pimax-dream-air-slam-review/ [S]
- https://www.ecva.net/papers/eccv_2018/papers_ECCV/html/Yonggen_Ling_Modeling_Varying_Camera-IMU_ECCV_2018_paper.php [S]
- https://arxiv.org/abs/2208.12008v1 [S]
- https://arxiv.org/pdf/2206.05066 [S]
- https://cvg.cit.tum.de/research/vslam/basalt [S]
- https://registry.khronos.org/OpenXR/specs/1.1/man/html/XR_MSFT_scene_understanding.html [S]
- https://developer.android.com/develop/xr/openxr/extensions/XR_EXT_spatial_entity [S]
- https://registry.khronos.org/OpenXR/specs/1.0/man/html/XrScenePlaneMSFT.html [S]
- Local: `analysis/research/dreamair-cameras.md`, `analysis/research/6dof-head-tracking.md` (prior notes; not re-read in this session).
