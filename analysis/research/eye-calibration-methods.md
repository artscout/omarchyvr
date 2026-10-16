# Eye-tracker calibration and accuracy testing in VR: research notes

Scope: web research only, no device access. Goal is to design our own calibration and accuracy test for the Tobii eye tracker inside the Pimax Dream Air, driven from Linux. Raw per-eye gaze arrives at ~120 Hz (origin in mm, direction unit vector, pupil in mm). Uncalibrated: ~10 deg constant offset and ~0.6 gain.

Verification legend:
- **[V]** verified by fetching the page or paper text in this session
- **[S]** seen only in a search snippet, not opened
- **[M]** from memory or general knowledge, NOT verified; treat as a hypothesis to check

Official Tobii XR SDK / Stream Engine documentation was not reachable through search. Anything about those APIs is marked accordingly.

---

## 1. How VR eye-tracker calibration is done

### 1.1 Tobii

- **Point count and layout (screen-based Tobii Pro SDK, not the XR/Stream Engine API)** [V]: 2, 5 or 9 points; 5 recommended. More than 9 gives no significant gain. Points use normalized Active Display Area coordinates, span at least the area where gaze-controlled content appears, and must lie inside the trackable area. Source: https://developer.tobiipro.com/commonconcepts/calibration.html
- **Target behaviour** [V]: animated target that shrinks and rests about 0.5 s, then collection starts; the tracker notifies the client when collection finishes. Slow enough for the user to fixate. Missing or low-quality points are recollected. Same source.
- **Compute and apply** [V]: after all points, the client calls compute-and-apply; the tracker fits an eye model for the current person. Recalculation is allowed until calibration is left. Same source.
- **Target size in degrees, sample count per point, total duration** [M]: not stated in what I read. Typical Tobii sessions run roughly 10–30 s total. Not verified.
- **Head-locked vs world-locked** [M]: Tobii VR calibration targets are generally shown in the headset's view space. Not verified against a Tobii XR source.
- **Validation step** [V]: Tobii documents a validation procedure that measures accuracy and precision against a new set of stimuli, with open-source add-ons on GitHub. Same source.
- **Reported accuracy and precision for Tobii in VR** [V, via other papers]:
  - Tobii-based VR accuracy is rarely given by the manufacturer. A workshop paper on a Pico Neo 3 Pro Eye (Tobii XR SDK, custom logging tool) states that precision, accuracy and other metrics are often insufficient or not reported by the manufacturer at all. Source: https://muni.cz/en/research/publications/2462137 [S].
  - Vive Pro Eye (Tobii-based) in VR, independent measurement, 30 deg field: mean accuracy 1.08 deg, median 0.84 deg; SD precision mean 0.36 deg; RMS precision mean 0.20 deg (median 0.08 deg). Per-participant mean accuracy 0.58–1.62 deg. Accuracy worse at periphery (up to 1.72 deg at +15/+15 on the far plane). Source: https://pmc.ncbi.nlm.nih.gov/articles/PMC10136368/ [V]. This is the best measured reference we have. Note these are post-outlier-removal numbers and the study used head-fixed targets.

### 1.2 Vive Pro Eye, SRanipal (Tobii-based, the closest published VR analogue)

- **Points** [V]: five positions, shown in sequence ("follow the dot"). Layout, angles, target size and dwell time NOT reported. Mapping model NOT reported; the binocular combination is described as undocumented. Source: https://pmc.ncbi.nlm.nih.gov/articles/PMC10136368/
- **Pre-step** [V]: the wearer fits the HMD and adjusts the IPD knob before calibration. Same source.
- **Validation layout (used for measurement, not the SRanipal calibration)** [V]: far plane 6 m with 49 targets over ±15 deg; near plane 0.5 m with 25 targets over ±10 deg; 74 trials per session; 90 Hz display, tracker at 120 Hz. Per target: 2 s presentation, 0.2 s colour change, 1 s gap. First 0.5 s (45 samples) skipped, next 1 s (90 samples) analysed. Targets were head-locked. Source: same PMC page.
- **Other VR calibration figures** [S]:
  - Independent study on Vive Pro Eye with a 5x5 grid: mean calibration error > 1 deg, no explicit validation; calibration decay 4.09 deg +/- 1.06 horizontal across a block. Source: https://neuroergonomicsconference.um.ifi.lmu.de/wp-content/uploads/submissions/180.pdf
  - Newer paper (arXiv table, 2026): a 9-point method on a VR HMD at about 1.25 deg. Source found via search: https://arxiv.org/pdf/2608.29739 (ID looks unusual; treat as unverified).

### 1.3 Varjo (Tobii-based headsets, own SDK)

- **Calibration types** [V]: One-dot "Foveation" (XR-3, VR-3, Aero, used for foveated rendering); Five-dot "Fast" (default for `varjo_RequestGazeCalibration`, combines current user's observations with statistical priors from other people's data); Ten-dot "Legacy" (slower, uses only the current user's data; suggested for medical research). Source: https://developer.varjo.com/docs/v3.6.1/native/eye-tracking (docs version 3.6.1; newer may differ).
- **Accuracy/precision in degrees** [V]: none given on v3.6.1 page. Older v2.1.0 docs state 1 deg accuracy and 0.2 deg precision for the 20/20 eye tracker [S]: https://developer.varjo.com/docs/v2.1.0/eye-tracking-with-varjo-headset
- **Storage/application** [V]: in Varjo Base, a mode decides when calibration runs: "Always calibrate" (every time headset is put on), "Remember my calibration" (run once, then saved and reused), "Best estimation without calibration" and "Never calibrate" (no automatic run). Same v3.6.1 page.
- **Status properties** [V]: `GazeCalibrating`, `GazeCalibrated` (bool) and `GazeCalibrationQuality_Left/Right` (double, 0.0–1.0; no threshold documented). Same page.
- **Validation** [V]: none described beyond the quality property.

### 1.4 Meta Quest Pro

- **Calibration** [V]: user-run from Settings > Movement Tracking > Eye tracking > Calibrate eye tracking. Release notes say eye tracking was "improved... to make it more accurate across a broader field of view". No degree figures. Source: https://www.uploadvr.com/meta-improved-quest-pro-eye-tracking-accuracy/
- **Point count, layout, accuracy in degrees** [M]: not verified. Meta's developer docs say accuracy varies by user and calibration and drops toward the field edges. Source for that claim: https://developers.meta.com/horizon/design/eyes-technology/ [S].
- **Independent measurement** [S]: a Texas State University case study measured spatial accuracy, precision and linearity over 78 participants (numbers not extracted): https://digital.library.txstate.edu/items/cc68eb47-0c67-48f4-886c-eaf3060f12a4 . Lancaster University comparative study of Quest Pro, Vive Pro Eye, Tobii Pro and Vive Focus 3 (accuracy in degrees, not extracted): https://research.lancaster-university.uk/en/publications/unveiling-variations-a-comparative-study-of-vr-headsets-regarding/

### 1.5 Apple Vision Pro

- **Calibration** [S]: user re-runs "Redo Eye Setup" (Settings > Eyes & Hands) when gaze seems off. No point count or accuracy published. Source: https://www.macworld.com/article/2252026/how-to-apple-vision-pro-recalibrate-eye-tracking-ipd-adjustment.html
- **Third-party accuracy test** [S]: iTrace click-based test reports a 0–100 score, not degrees: https://arxiv.org/pdf/2508.12268
- Apple's internal calibration method, point count and accuracy: [M] not public.

### 1.6 Pico, Pupil Labs VR add-ons

- **Pico**: no verified public calibration spec. Workshop paper on Pico Neo 3 Pro Eye (Tobii-based, custom logging on Tobii XR SDK) exists [S]: https://muni.cz/en/research/publications/2462137 . Point count not extracted.
- **Pupil Labs VR/AR add-on (Core)**: marker-fixation pairs fitted with a polynomial mapping from 2D pupil position to 2D gaze point [S]; source: Pupil Labs chat archive and papers. Pupil Labs also reported that accuracy is computed by running calibration, then running the same procedure and measuring the offset between gaze and reference points; pixel-to-degree conversion uses HMD intrinsics [S]: https://chat-archive.pupil-labs.com/logs/vr-ar/2018/05/ . Neon VR mount accuracy numbers: not found [M].

### 1.7 Summary table

| System | Points | Layout | Typical reported accuracy | Verified? |
|---|---|---|---|---|
| Tobii screen trackers | 2/5/9, 5 recommended | Normalised ADA, span content | Not given in SDK docs | [V] layout, [M] numbers |
| Vive Pro Eye (SRanipal) | 5 | not reported | Independent: mean 1.08 deg, median 0.84 deg, RMS 0.20 deg, 30 deg field | [V] |
| Varjo | 1 / 5 / 10 | not reported | 1 deg / 0.2 deg (v2.1 docs, old) | [V]/[S] |
| Meta Quest Pro | not public | not public | no degree figure | [V] (no numbers) |
| Apple Vision Pro | not public | not public | none published | [S] |
| Pupil Labs VR add-on | marker-based | markers in scene | not found | [S] |

---

## 2. Mapping models (raw gaze to true gaze)

Brief survey. Items marked [M] are general knowledge in the eye-tracking field, not checked against a fetched source for this run.

1. **Per-eye vs combined (cyclopean)** [M]: most VR SDKs produce a combined gaze ray; some output per-eye. Fitting per eye and then averaging the corrected rays generally handles vergence better than fitting one combined ray. Our raw data is already per-eye, so per-eye fits are the natural choice.
2. **2D polynomial on gaze tangents** [M]: order 1 (affine, 6 coefficients per axis) is the usual starting point; order 2 (quadratic, 12 per axis for two-variable) adds barrel/pincushion-like nonlinearity at the field edges. Our own `pimax-lenstest` gaze pattern already fits affine and quadratic; compare them with the 13/17-point data.
   - Vive Pro Eye paper notes that more complex polynomials need more targets [V indirectly: https://pmc.ncbi.nlm.nih.gov/articles/PMC10136368/ and a HTC Vive paper snippet [S]].
3. **Homography** [M]: appropriate when the gaze signal lives on a flat plane (e.g. screen-based tracking). For VR with angular data, an affine map in angle space is usually equivalent over ±20 deg.
4. **3D eye-model approaches** [M]: fit eyeball centre and visual-axis offset (kappa angle) per eye, then recompute gaze direction. This is the principled model for a constant offset plus gain: a constant offset that is near-constant across the field implies a kappa-like rotation, and a gain of 0.6 is more suspicious of a unit or tangent-vs-angle mismatch, or of the gaze origin being in a different frame. Check this first (see Recommendations).
5. **Smooth-pursuit calibration** [S]: a neural-plus-smooth-pursuit method on a desktop tracker reported 0.40 deg, from a 2026 arXiv table: https://arxiv.org/pdf/2608.29739 (unverified ID). Pursuit targets reduce the fixation-timing issue but add a model of target motion.
6. **Implicit / continuous recalibration** [M]: fit corrections from UI clicks or known fixation targets during normal use. Requires ground-truth assumption; risk of bias from fixation on click targets.
7. **Robust fitting** [M]: RANSAC or Huber/IRLS loss on per-point median samples. Reject per-point outliers before the polynomial fit; the Vive Pro Eye study rejected 2.76% of trials for errors >= 5 deg or > 90% repeated samples [V].
8. **Blink and saccade rejection** [V for the procedure, M for the thresholds]: drop samples with invalid flags, pupil loss, and the first 0.5 s after a target change (the Vive Pro Eye test skipped 0.5 s then analysed 1 s) [V: PMC10136368].
9. **Fixation detection** [S]: I-VT with ~30 deg/s threshold is common; Tobii Pro ships 30 deg/s fixation filter [S]: https://arxiv.org/pdf/2512.23926 . Typical I-VT range for lab studies 5–50 deg/s [S, same source]. I-DT dispersion ~1 deg is a common choice [M]; not verified in this run.
10. **Filtering (One Euro filter)** [V]: defaults mincutoff = 1 Hz, beta = 0, dcutoff = 1 Hz. Tuning: start with beta 0 and mincutoff about 1 Hz; raise beta to reduce lag in fast motion; lower mincutoff to reduce jitter at rest. Sources: https://pypi.org/project/oneeurofilter and https://cristal.univ-lille.fr/~casiez/1euro [S, snippets]. Gaze-specific beta/mincutoff: not found [M].

**Best accuracy with fewest points:** for our setup, the evidence supports 5 points (Tobii recommendation, Varjo fast mode) as a minimum, with 9 points as the sweet spot for a 2D polynomial, and per-eye fits. Accuracy gains beyond 9 points were not shown in the sources I read. No head-to-head comparison of mapping models on Tobii VR data was found.

---

## 3. Accuracy test best practice

1. **Target distribution** [V for Vive Pro Eye layout, M for comfortable-range figure]: the Vive Pro Eye study used a grid up to +/-15 deg (far) and +/-10 deg (near). Comfortable eye rotation without head movement is commonly cited as roughly +/-15 to 20 deg [M]; test inside that range and add a separate head-movement condition if needed.
2. **Target distance and parallax** [V for test layout]: far plane 6 m, near plane 0.5 m (Vive Pro Eye). Gaze origin is per-eye 3D (mm); for near targets, compute the direction from each eye origin to the target, not from the headset origin, otherwise parallax (on the order of IPD / distance) appears as error. The Tobii HMD gaze origin is a 3D point in mm in HMD coordinates with a validity flag [S]: https://developer.tobiipro.com/tobii.research/matlab/reference/1.7.0.8-alpha-g6c0fdf3/Gaze/HMDGazeOrigin.html
3. **Head locking** [V]: Vive Pro Eye targets were head-locked, so the head was fixed relative to targets; head-free was not tested. Same PMC source.
4. **Timing** [V]: 2 s presentation, 0.5 s skipped, 1 s analysed, 90 samples per target at 120 Hz tracker (sample rate 120 Hz, display 90 Hz). Source: PMC10136368.
5. **Metrics** [V/M]:
   - Accuracy: mean (and median) angular offset between measured and true direction; the Holmqvist definition is "average angular error between the measured and the actual location of the intended fixation target" [S: search result summarising the definition].
   - Precision: RMS-S2S (sample-to-sample) is the most common; SD is also reported. RMS-S2S is sensitive to sample rate, so state the rate [S: search result summarising Holmqvist & Blignaut and Blignaut/Beelders].
   - Data loss: fraction of invalid samples [S].
6. **Sample counts** [V]: Vive Pro Eye study used 74 targets x 18 participants x 10 sessions = 13,320 trials. A single-session test of ~9–17 points, 1 s per point, is enough to detect a 0.5 deg change in mean error for a single subject [M, rough power reasoning, not verified].
7. **Reporting** [S]: the Holmqvist minimum reporting guideline (2022) asks for accuracy in degrees to be reported. Source: https://link.springer.com/article/10.3758/s13428-023-02105-5 [S]. Also Holmqvist & Blignaut (2020) argue the newer "resolution" measure's relation to RMS-S2S is unclear [S].
8. **Validation vs calibration split** [V]: keep calibration points and validation points disjoint, and report validation error (not fit residuals) as the accuracy number. Tobii documents validation with new stimuli [V: Tobii commonconcepts page].
9. **Standard tools** [S]: GlassesValidator (printable poster and Python code, 61 participants, six trackers): https://www.ncbi.nlm.nih.gov/pmc/articles/PMC10991001/ . Useful as a protocol reference for wearables, not for VR.

---

## 4. Tobii Stream Engine calibration API

What was checked and what was not.

- **Not verified**: the exact Stream Engine C signatures for `tobii_calibration_start`, `tobii_calibration_collect_data_2d`, `tobii_calibration_collect_data_3d`, `tobii_calibration_compute_and_apply`, `tobii_calibration_retrieve`, `tobii_calibration_apply`, `tobii_calibration_discard_data`, `tobii_calibration_stop`. The search did not return the Stream Engine reference.
- **Confirmed names via bindings** [S]: the Rust `tobii-sys` crate (generated from Stream Engine v1.2.1.305 headers) lists calibration error codes including `TOBII_ERROR_CALIBRATION_ALREADY_STARTED`, `TOBII_ERROR_CALIBRATION_BUSY`, `TOBII_ERROR_CALIBRATION_NOT_STARTED`. Source: https://docs.rs/tobii-sys . Check the crate source or the headers in the SDK package for exact function names and behaviour.
- **Analogous flow (Tobii Pro SDK, not Stream Engine)** [S]: enter calibration mode; collect points; compute-and-apply; if compute fails, the result is not applied; discard and recollect bad points, then compute-and-apply again. Source: https://developer.tobiipro.com/tobii.research/python/reference/1.0.4.109-alpha-g566424be/classtobii__research_1_1ScreenBasedCalibration.html [S].
- **Calibration state is exclusive** [V, Tobii Pro concepts]: only one client can hold the calibration state at a time; collect, discard and compute-and-apply need it; enter/leave notifications go to all clients. Source: https://developer.tobiipro.com/commonconcepts/calibration.html
- **Storage and re-apply** [V for Tobii Pro SDK, not verified for Stream Engine]: the client can retrieve the active calibration, save it locally, and re-apply it later without the full procedure. The docs do not say that firmware stores calibration or auto-applies it on start. The .NET reference says applying calibration data makes it active and that it can be read back and saved to a file [S]: https://developer.tobiipro.com/tobii.research/dotnet/reference/2.1.0.3-alpha-gd8b35e1b/interface_tobii_1_1_research_1_1_i_eye_tracker.html
- **Conclusion for us** [M]: assume calibration is per session unless our own test shows otherwise. Hold our own correction on the host (we already have `eye-correction.toml`), and only use device-side calibration if Stream Engine's retrieve/apply is confirmed.

---

## 5. Recommendations for our case

1. Check the 10 deg offset and 0.6 gain first. A constant offset plus a 0.6 gain across the field points to a units or frame problem before it points to a calibration problem: confirm that the direction vector is a unit vector, that tangent vs angle is handled consistently, and that the gaze origin is in the same frame as the display. Fix this before fitting.
2. Fit per eye, then combine the corrected rays for the cyclopean gaze. Start with an affine map (6 coefficients per axis) in angle space; move to quadratic only if the residuals show field-edge structure, and check the quadratic on held-out points.
3. Use 9 calibration points (5 is the minimum): centre plus +/-10 deg and +/-15 deg on each axis, to match the comfortable-rotation range [M]. Use a validation set of 9 disjoint points, spanning +/-15 to 20 deg.
4. Timing: per point, 0.5 s settle (drop it), 1 s analysed at 120 Hz (about 120 samples; drop invalid/blink samples and fit per-point medians). Match the Vive Pro Eye protocol so our numbers are comparable to its published 1.08 deg mean / 0.84 deg median / 0.20 deg RMS.
5. Robust fit: per-point median, then RANSAC or Huber on the 9 points; report residuals per point so one bad point can be re-collected (same logic as Tobii's recollect rule).
6. Accuracy test: head-locked targets (use a chin rest or check head-pose drift); compute target angles from each eye's 3D origin for near targets (parallax); keep the far plane at about 6 m and the near plane about 0.5 m if we test depth; report mean and median angular error, RMS-S2S and SD precision at 120 Hz, data-loss fraction, and per-point error maps.
7. Filtering: apply One Euro after the correction, not before. Start with beta 0 and mincutoff about 1 Hz, then tune: raise beta if fast motions lag, lower mincutoff if the output jitters at rest. Keep fixation detection (I-VT at 30 deg/s, or I-DT at about 1 deg) separate from the filter so the accuracy numbers are not smoothed.
8. Re-calibration per session: plan on it unless our own test shows Tobii or Stream Engine keeps calibration across starts. Store our correction (JSON or toml, as now) with the date, IPD and headset serial, and show validation error on start so the operator can decide whether to re-run.
9. Expect a residual gain and edge error: the Vive Pro Eye data shows accuracy worsens toward the edges (1.72 deg at +15/+15 on the far plane) and that glasses lower performance. Our test should report per-quadrant errors so edge error is visible and not averaged away.
10. Before trusting any number, run the same test twice on the same headset within one session, and once on a different day, to separate calibration error from calibration decay (the LMU poster reports decay of 4.09 deg over a block for Vive Pro Eye [S]). Report decay as a separate metric.

---

## Open items (not verified in this run)

- Tobii Stream Engine signatures and behaviour (retrieve/apply, storage). Check the SDK package headers.
- Tobii XR SDK accuracy spec, point count, target size in degrees. Check the Tobii XR SDK or Tobii developer contact.
- Holmqvist 2012 and 2022 test specifications (full text not fetched).
- One Euro gaze-specific parameter values (paper paywalled).
- Pico, Apple Vision Pro, Pupil Labs VR add-on accuracy numbers.
- Whether Tobii firmware re-applies calibration on start.
