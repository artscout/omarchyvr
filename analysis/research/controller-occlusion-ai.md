# Controller occlusion: can a small learned model fill the gaps?

Date: 2026-10-15. Research only, no code changed. Numbers from papers are marked (paper); our estimates are marked (est.) and must be measured before anyone relies on them.

## Verdict in short

- **Viable: yes, but the learned part is the second step, not the first.** A classic fallback (IMU with bias estimate, plus a body/arm prior) gets most of the gain for 0.5-1 s gaps. A learned predictor can then add a modest improvement on top, mainly by learning the arm's motion prior and the rumble noise.
- **Expected error at 1 s occlusion (est.):** classic IMU-only double integration about 10-30 cm; classic plus arm prior about 5-15 cm; a small learned model on top of that about 3-10 cm. These are guesses until we measure them on our own recordings (section 5).
- **Data we need:** our own recordings of optical + IMU, with occlusion simulated by masking the optical data while the controller is still visible. This is self-supervised and needs no external labels.
- **Recommended first step:** build the offline evaluation harness and measure the current Monado arm-model fallback (`PIMAX_CTRL_TRACK_LOST_S`, default 4 s) against optical ground truth on masked segments. That number decides whether a learned model is worth the effort.

## 1. State of the art

### Commercial headsets

- **Meta Quest.** Meta's developer guidance says controllers are camera-tracked and have a "fundamental limitation based on the tracking volume". It states that the IMU "can estimate position and orientation for a short period when the controller is outside the FOV of the tracking cameras", and that "this estimate drifts over time, suffering a rapid degradation in accuracy" ([Meta controller best practices](https://developers.meta.com/horizon/design/controllers-best-practices/)). Meta has not published the exact fallback model. The closest public document is a Meta patent application, US 2024/0370099, "Motion-model based tracking of artificial reality input devices". It switches to an IMU motion model when the video position or velocity confidence drops (for example, when the controller is occluded from the HMD cameras). A Kalman filter then estimates accelerometer bias, velocity and position from the last good video pose plus the IMU. Confidence is tested with Mahalanobis distance. **No learned model is mentioned in that patent** ([patent summary](https://patent.nweon.com/38647)). A second Meta-related application, US 12380658, "Deep inertial prediction system and methods", describes the same problem; its assignee is not confirmed in the search result ([USPTO](https://image-ppubs.uspto.gov/dirsearch-public/print/downloadPdf/12380658)). It is not known whether it is shipped.
- **Windows Mixed Reality.** Microsoft's documentation says Windows "will continue to infer controller positions" when a controller leaves the view, and that positions fall to "approximate accuracy" after a long untracked period ([Microsoft motion controllers](https://developer.microsoft.com/windows/mixed-reality/motion_controllers)). Road to VR's hands-on review says the system uses only the controller IMU for brief out-of-view periods and becomes unreliable after a second or two ([Road to VR](https://roadtovr.com/?p=166699)). Windows Central describes a "no position" state after a long loss ([Windows Central](https://windowscentral.com/how-fix-motion-controller-tracking-loss-windows-mixed-reality)). Microsoft Research has a learning-based out-of-FOV work that uses a learned model plus a Kalman filter, with an ultrasound sensor as the extra signal ([Microsoft Research](https://www.microsoft.com/en-us/research/video/sensor-fusion-for-learning-based-motion-estimation-in-vr/)). It is research, not a product.
- **Pico, PSVR2.** I found no public document describing the occlusion fallback. PSVR2 sources only describe the IR LED tracking and the frequent "can't track surroundings" failures ([Digital Trends / Galaxus pages](https://www.galaxus.nl/en/page/these-tips-will-help-you-improve-your-playstation-vr2s-tracking-26728)). Nothing on IMU fallback or learned models was found. Treat it as unknown.

Summary: the shipped products use **IMU dead reckoning for about 1-2 s**, plus a physics or kinematic model (Kalman filter and arm/body prior). The public material shows no learned gap-filler in shipped controller tracking. The learned work is in the research literature below.

### Learned inertial odometry (IMU-only, handheld or head)

| Work | What it does | Input | Model | Latency / hardware | Accuracy |
|---|---|---|---|---|---|
| **RoNIN** ([arXiv 1905.12853](https://arxiv.org/abs/1905.12853), [ar5iv](https://ar5iv.labs.arxiv.org/html/1905.12853), ICRA 2020) | Pedestrian / phone IMU velocity from a window, then integrate | 200 Hz IMU, 1 s window (200x6, gyro + acc in a gravity-aligned frame) | 1D ResNet-18 (ResNet, LSTM and TCN variants). Parameter counts not stated in the page I read | Training 10 h on a 1080Ti. Inference time not stated | ATE 3.54 m (seen), 5.14 m (unseen) over long trajectories (paper). Not comparable to a 1 s occlusion. |
| **TLIO** ([arXiv 2007.01867](https://arxiv.org/abs/2007.01867), RA-L 2020, Facebook Reality Labs) | Headset-mounted learned displacement + uncertainty, fused in an EKF | 200 Hz IMU, 1 s input (200x6), displacement over 1 s | 1D ResNet-18 with two heads (displacement, 3 log-std values for uncertainty). EKF measurement update at 20 Hz | Inference time not stated | 27-33% less position drift and 27% less yaw drift than the best RoNIN baseline (paper). Absolute numbers only in figures. The paper gives 33% in one place and 31% in another. |
| **AI-IMU Dead-Reckoning** ([arXiv 1904.06064](https://ar5iv.labs.arxiv.org/html/1904.06064), code: [mbrossar/ai-imu-dr](https://github.com/mbrossar/ai-imu-dr)) | Vehicle IMU-only; a network sets the Kalman filter noise and covariance | Raw IMU | Small network, not specified in what I read | Not stated | KITTI about 1.1% translational error (paper). |
| **Denoising gyro with DL** ([arXiv 2002.10718](https://arxiv.org/abs/2002.10718)) | Learned gyro denoising, then dead reckoning for attitude | Gyro | Network, not specified in what I read | Not stated | Beats visual-inertial systems on attitude without vision (paper). Relevant to the gyro-only orientation fallback. |

Note: RoNIN and TLIO are about walking humans with a phone or a headset, over seconds to minutes. The key number for us is the 1 s window and the 200 Hz rate, not the long-run drift.

### Learned body / hand pose from sparse sensors (headset + controllers)

| Work | What it does | Input | Model | Speed | Accuracy |
|---|---|---|---|---|---|
| **AvatarPoser** ([arXiv 2207.13784](https://arxiv.org/html/2207.13784v1), ECCV 2022, ETH + Meta Reality Labs) | Full-body pose from head + two controllers | Position, velocity, 6D rotation, angular velocity from 3 trackers (54 features), 60 Hz | Transformer, 256-dim, 8 heads, 3 layers, then MLPs and inverse kinematics. Parameter count not stated | Network up to 662 fps (about 1.5 ms per frame, derived); IK about 6 ms per iteration (paper) | MPJPE 4.18 cm (3 inputs), 2.61 cm (4 inputs) on AMASS (paper) |
| **EgoPoser** ([arXiv 2308.06493](https://arxiv.org/html/2308.06493v3), ECCV 2024, ETH) | Full-body pose from head + hands, built for intermittent hand tracking | Head + hand tracking, 80 past frames (40 after SlowFast fusion) | Transformer, **4.12 M parameters**. Hands outside the FoV are input as zero | "over 600 fps"; hardware not stated | MPJPE 4.14 cm (full visibility). 5.31 / 6.07 / 6.60 cm when hands are partly out of view at 180 / 120 / 90 degree FoV (paper). |
| **QuestSim** ([arXiv 2209.09391](https://arxiv.org/abs/2209.09391), SIGGRAPH Asia 2022, Meta) | Physics-simulated avatar from HMD + controllers, reinforcement learning | HMD + 2 controllers | RL policy outputting torques | Not checked | Qualitative in what I read |
| **IMUPoser** ([arXiv 2304.12518](https://arxiv.org/abs/2304.12518), CHI 2023, CMU) / **MobilePoser** ([arXiv 2504.12492](https://arxiv.org/html/2504.12492v1)) | Full body from phone / watch / earbud IMUs | Consumer IMUs, sparse | Learned, sizes not checked | Not checked | MobilePoser notes weak temporal consistency and drift on low-end IMUs (paper) |

Key point: EgoPoser's zero-masking of joints outside the view is the same idea as what we need, but its output is a body pose at 60 Hz, not a controller position over a 1-2 s gap. It shows a small model (4 M parameters) handles intermittent input at high speed.

## 2. Viability for us

### What the controller provides (local notes)

From `analysis/controller-6dof.md`:
- Controller IMU on EP 0x82, about **250 reports/s per controller**, with one or two samples each. That is roughly **250-500 Hz** of IMU samples per controller. Accel is about 1024 LSB/g, verified at rest. **Gyro scale is unknown** (Monado guesses 16.4 LSB/(deg/s), `PIMAX_CTRL_GYRO_LSB_DPS`). Axes vs the LED model are in `ImuToDevice`.
- Time stamp: the report `timestamp_us` is the headset MCU microsecond clock, the same clock as the camera and the headset IMU. Good for alignment.
- Optical: LED frames at **25 Hz** (`analysis/overnight/cam.md`, `windows-capture-20261015.md`). Controller optical poses are therefore 25 Hz at best.
- Headset IMU: 500 packets/s, 2 samples each.
- Head pose: up to 50 Hz in the shown-pose stream (`P`).
- Monado fallback today: when the controller is out of view for up to `PIMAX_CTRL_TRACK_LOST_S` (default **4 s**), it keeps its offset in the head-yaw frame (an arm model) and uses the gyro for orientation (`analysis/controller-6dof.md`, around line 271).

The Windows capture notes (`analysis/windows-capture-20261015.md`) show that take 13 had **no controller poses** (status 0), so the Windows pose logs are not a usable source of controller ground truth. Its controller IMU and USB streams are fine (about 250/s per controller).

### Proposed model

- **Inputs (1 s window at 250 Hz, plus the latest optical pose):** controller accel + gyro in the head frame (6 channels, resampled to a fixed rate), head 6DoF pose and velocity, the last optical pose and its age, the haptic command state (slot, amplitude, duration, see below), and the controller's arm-model prior.
- **Output:** displacement over the gap (3 values) and a diagonal covariance (3 log-std values), as in TLIO. Also optionally orientation.
- **Blend:** feed the output to a Kalman filter or complementary blend with the arm model. Gate the optical return with the Mahalanobis test from the Meta patent.
- **Size:** a 1D CNN or a GRU of 50k-500k parameters. TLIO-like ResNet-18 is about 1-2 M (est.). EgoPoser is 4 M, which is above our budget.

### Latency

- Predict at **90 Hz** (render rate), not at IMU rate. The network runs once per frame (11 ms budget) and the IMU is integrated in between.
- A 1-2 M parameter 1D CNN on the 200-sample window: a few ms on CPU (est.); the GPU is much faster (RTX 4090 available). A GRU of 100k parameters: well below 1 ms on CPU through ONNX Runtime (est.).
- ONNX Runtime is already in our build for hand tracking, so the runtime is in place. The pipeline must be a leaf pimaxd component that can be disabled without losing frames (CLAUDE.md section 2, item 5).

### Haptics and rumble noise

- Haptic command 0x07 (slot, amplitude %, duration ms) sends pulses every 8-10 ms during games (per the windows capture notes and your message). Vibration adds broadband accel and gyro noise while a pulse is on, and the pulses are periodic.
- **The learned model must know when rumble is on.** The host sends the command, so the model can get the rumble state (on/off, amplitude) as an input. Training data must include rumble periods. The cheap fix first: while a pulse is active, down-weight the IMU in the Kalman filter, or hold the last IMU-derived velocity.
- EgoPoser-style masking is not needed for rumble. The noise is known in time, so it can be handled with a notch or a gate.

## 3. Training data

### What we have

- **Own optical + IMU recordings:** the Record tracking feature (`docs/RECORD.md`, commit 56ddff0) gives cameras, controller logs and Monado logs per take. While the controller is visible, the optical pose (25 Hz) is the ground truth.
- **Self-supervised occlusion:** take a visible segment, hide the optical data for 0.5-2 s (mask it), train the model to predict the hidden optical pose from the IMU, head pose and the pose before the mask. The mask schedule should cover real occlusions too: low arm, behind the back, looking at the floor.
- **Haptic logs:** the 0x07 stream is recorded in the Windows capture (game-doe2, take 15); useful for rumble noise, but the controller poses for that take are incomplete.
- **Windows poses:** not usable for controller truth (no controller rows in take 13; take 15 pose log covers only 28 s of 180 s).

### What we could record

- A dedicated take set: 20-30 min, controllers worn and held, free arm motion, gestures, handovers, low arm, behind the back, with the rumble on in some takes. Shooting for 10,000 s of visible segments is far more than the model needs; a few thousand masked windows is a start.
- Record the head pose at 50 Hz so the head-frame transform is exact.

### Public datasets (for pre-training, not for the final model)

- **RoNIN** (40 h, 100 subjects, phone IMU, 200 Hz). Pedestrian, so it gives the general "IMU window to motion" mapping, not hand motion. Check the licence before use; the paper says code and data will be shared.
- **TLIO data** (headset IMU, pedestrian). Same caveat.
- **AMASS** (motion capture) with virtual IMU generation: lets us train arm motion priors at scale. Synthetic IMU needs a noise model (bias, noise density, rumble), which is a separate task. I did not find a ready-made tool in this search.
- **Aria Digital Twin** ([arXiv 2306.06362](https://arxiv.org/html/2306.06362v2)): IMU plus 6DoF and human poses, 200+ sequences. Its licence is **personal, non-commercial research only**, and access requires accepting Meta's licence. Our use (personal) fits, but do not redistribute.
- **IMUPoser dataset** (phone / watch / earbud IMUs): sparse-sensor pose, no controllers.

## 4. Classic approach and expected error

The classic approach is what Monado already does, plus improvements:

1. **IMU dead reckoning with a bias estimate:** integrate accel (rotated by the gyro orientation, minus gravity), with the accel bias estimated while the controller is still (zero-velocity update). Without bias estimation, the error grows as 0.5 x bias x t^2.
2. **Arm / body prior:** the controller stays on the arm model around the shoulder, with the head yaw. This is the big win, because a hand does not drift away from the body in 1 s.
3. **Kalman filter** (as in the Meta patent) for velocity, position and bias, with a Mahalanobis gate on the optical return.

Expected errors (est., to be measured):

| Gap | IMU-only dead reckoning, bias-corrected (est.) | IMU + arm/body prior (est.) | Learned gap-filler on top (est.) |
|---|---|---|---|
| 0.5 s | 2-6 cm | 2-5 cm | 1-3 cm |
| 1 s | 10-30 cm (without the prior, double integration of a 0.02-0.1 m/s^2 bias is 1-5 cm at 1 s before noise; the noise and the hand's own acceleration dominate) | 5-15 cm | 3-10 cm |
| 2 s | 0.4-1 m | 15-40 cm | 8-25 cm |

The 0.5 s and 1 s rows are close to what the literature suggests for 1 s IMU windows (TLIO, RoNIN) but are not measured on controllers. The ADT, EgoPoser and AvatarPoser numbers (4-6 cm MPJPE) are for full-body pose with visible joints, not for a controller gap, so they set the floor rather than the target.

## 5. Evaluation plan

- **Ground truth:** optical controller poses while visible (25 Hz); masked for the test.
- **Metric:** position RMSE at 0.5, 1 and 2 s of masked occlusion, the 95th percentile, and the **jump at reacquisition** (the step when the optical pose returns; must be near zero for a good blend, otherwise it is visible in VR).
- **Also:** orientation error (gyro-only fallback), time to the first wrong gate decision, and CPU time per 90 Hz frame.
- **Baseline:** the current Monado fallback (arm model, `PIMAX_CTRL_TRACK_LOST_S = 4`). The learned model must beat it by at least 30% at 1 s and must not produce a visible jump.

## 6. Recommendation and staged plan

**Verdict:** worth doing, but in stages. Expected gain at 1 s over a good classic fallback (est.): about 30-50% less position error, with a visible-jump cost if the blend is wrong. Effort (est.): classic stage 1-2 weeks; learned stage 3-6 weeks including data.

1. **Measure first (1-2 days).** Build an offline harness: replay recorded takes with masked optical data, run the current Monado arm model and the bias-corrected IMU variant, report RMSE at 0.5/1/2 s. Use the existing Record tracking takes.
2. **Classic improvements (1-2 weeks).** Bias estimation by zero-velocity updates, gyro scale from the optical fit (`controller-6dof` phase 2), the Kalman filter with the Mahalanobis gate, rumble gating from the haptic command, device-clock timestamps. These are needed anyway for the 6DoF path.
3. **Data (1-2 weeks, mostly the user's time).** A dedicated take set (20-30 min, worn, with rumble on in some takes, low arm, behind the back, handovers). Keep it in `pimax-data`.
4. **Learned model (2-4 weeks).** A 1D CNN or GRU of 50k-500k parameters, TLIO-style output (displacement + covariance), trained on self-supervised masked windows, with the rumble state and the arm prior as inputs. Export to ONNX, run in a leaf pimaxd component, default off, a switch in the app ("Occlusion prediction"), and ship only if it beats the classic path on held-out takes.
5. **Gate:** ship only with a switch, a status line in the app, and no change when the component fails (CLAUDE.md section 2).

## Sources

- RoNIN: arXiv 1905.12853 — https://arxiv.org/abs/1905.12853 ; ar5iv https://ar5iv.labs.arxiv.org/html/1905.12853 ; project https://ronin.cs.sfu.ca/
- TLIO: arXiv 2007.01867 — https://arxiv.org/abs/2007.01867 ; PDF https://arxiv.org/pdf/2007.01867
- AI-IMU Dead-Reckoning: arXiv 1904.06064 — https://ar5iv.labs.arxiv.org/html/1904.06064 ; code https://github.com/mbrossar/ai-imu-dr
- Denoising IMU gyroscopes: arXiv 2002.10718 — https://arxiv.org/abs/2002.10718
- EgoPoser: arXiv 2308.06493 — https://arxiv.org/html/2308.06493v3 ; ECCV 2024 https://www.ecva.net/papers/eccv_2024/papers_ECCV/papers/00248.pdf
- AvatarPoser: arXiv 2207.13784 — https://arxiv.org/html/2207.13784v1
- QuestSim: arXiv 2209.09391 — https://arxiv.org/pdf/2209.09391.pdf
- IMUPoser: arXiv 2304.12518 — https://arxiv.org/abs/2304.12518 ; MobilePoser arXiv 2504.12492 — https://arxiv.org/html/2504.12492v1
- Aria Digital Twin: arXiv 2306.06362 — https://arxiv.org/html/2306.06362v2
- Meta controller best practices: https://developers.meta.com/horizon/design/controllers-best-practices/
- Meta patent US 2024/0370099 (summary): https://patent.nweon.com/38647
- Meta patent US 12380658 (assignee not confirmed): https://image-ppubs.uspto.gov/dirsearch-public/print/downloadPdf/12380658
- Microsoft motion controllers: https://developer.microsoft.com/windows/mixed-reality/motion_controllers
- Road to VR hands-on: https://roadtovr.com/?p=166699
- Windows Central tracking loss: https://windowscentral.com/how-fix-motion-controller-tracking-loss-windows-mixed-reality
- Microsoft Research learning-based out-of-FOV: https://www.microsoft.com/en-us/research/video/sensor-fusion-for-learning-based-motion-estimation-in-vr/

Not found: a Quest, Pico or PSVR2 document on the IMU fallback model or any learned controller gap-filler. The PDF text of the TLIO, RoNIN, AvatarPoser and EgoPoser papers could not be extracted (binary); figures came from the HTML versions and abstracts. Check the parameter counts for RoNIN/TLIO in the PDF before quoting them.

Local notes used: `analysis/controller-6dof.md`, `analysis/windows-capture-20261015.md`, `analysis/ctrl-lock-rate.md`, `analysis/controller-activation.md`, `analysis/overnight/cam.md`, `docs/RECORD.md`.
