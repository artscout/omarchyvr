# Eye tracking on Dream Air (Linux): new ideas and math, 2026

Scope: ideas we have not yet used for the host-side model (`pimax_eye::calib`, One Euro, drift, eye combination).
Baseline from our notes: per-eye affine/quadratic fit, ~6.9 deg combined error (left 4.6, right 10.6), right eye bad
state ~30 % of the time (origin jump ~15 mm), offline 2.5 mm origin-median gate: right 26 -> 3.4 deg.
Already covered, not repeated: One Euro output filter, leave-one-target-out model choice, 9/13-point layout, per-eye
rays followed to 1.5 m, drift via `observe_target`.

Tags: **[L]** = literature result (source given, read or searched this run). **[H]** = our own hypothesis, not tested.
Effort: S = under a day in `rust/pimax-eye`, M = days, L = weeks or a new capture.

Verification status: arXiv 2311.00391 and 2508.13546 were read (abstract/landing page only). Online-EYE (CHI 2025) and
Dunn 2019 were read from abstracts. The Quest Pro paper (2403.07210) gave no numbers in the abstract, so its figures
are missing here. The Monado search returned nothing Monado-specific, so the Monado notes come from the Khronos spec.

---

## 1. Host-side calibration models

| # | Idea | What it is | Why it fits us | Link | Effort |
|---|---|---|---|---|---|
| 1.1 | Ridge-regularised polynomial | Least squares with a ridge penalty toward identity on the affine/quadratic terms. [L] Ridge cut MSE ~20 % vs plain polynomial fit on mobile eye trackers. | Our 9-13 points per eye, LOO picks the order. Ridge keeps the quadratic from overfitting the edges. | https://www.ncbi.nlm.nih.gov/pmc/articles/PMC10966887/ | S |
| 1.2 | Third order helps inside the box, hurts outside | [L] Head-mounted study: 3rd-order polynomial more accurate inside the calibration area, interpolation and extrapolation errors reported separately. | Our validation targets at +/-15-20 deg are at the edge of the box. Report interpolation vs extrapolation error per eye, do not average them. | https://www.ncbi.nlm.nih.gov/pmc/articles/PMC7733312/ | S |
| 1.3 | GP / RBF residual correction on top of affine | Keep the affine map as the mean function, then fit a Gaussian process (or RBF interpolant) to the residuals as a function of gaze tangent. Gives an error bar per output. | Our residual is field-dependent (upper field compressed, lower field right, per notes). A residual field captures that without a high-order polynomial. The GP variance can drive the confidence weight in 3.1. | https://arxiv.org/pdf/1612.06919 (Tripathi & Guenter, WACV 2017) | M |
| 1.4 | Calibration-free VR GP correction | [L] Continuous self-calibration: local GP models mapping corneal/pupil feature to screen position, updated online to follow headset shifts; reported close to explicit calibration. Used in HMD. | Directly addresses the 0.5-1 h decay problem in section 4 without a new calibration. Needs a pupil-feature input, which we do not have (we only get computed gaze), so the input would be the gaze tangent itself. | https://paperswithcode.com/paper/a-statistical-approach-to-continuous-self | M |
| 1.5 | Regression-based pursuit calibration | [L] Drewes, Pfeuffer & Alt (ETRA 2019): regression pursuit that needs a small calibration area, compared with 9-point; reported as fast and accurate over a large area. | Uses the same 120 Hz stream. Gives a 10-20 s calibration and comfortable targets only. | https://www.medien.informatik.uni-muenchen.de/pubdb/publications/pub/drewes2019etra/drewes2019etra.pdf | M |
| 1.6 | Smooth pursuit with a NN | [L] Li et al. (2022, search summary only, not read in full): pursuit calibration cut MAE from 1.0 deg to 0.4 deg in ~45 s. Velocities above ~6 deg/s degraded accuracy. | Pursuit is a good fit for comfortable-range targets. Keep the target speed under 6 deg/s. Source is a search summary, not read in full. | https://perceptualui.org/publications/pfeuffer13_uist/ (original pursuit method) | M |
| 1.7 | Kappa / visual-axis offset via coplanarity | [L] Liu, Chi & Sun (2023): kappa estimated during normal screen browsing from the coplanarity of the two visual axes (both eyes' rays meet the target plane), optimised with differential evolution. Reported 1.30 deg H / 1.34 deg V. Desk-screen, not HMD. | Our per-eye error is opposite-signed (eyes diverge by ~16 deg, about 8 deg each). That pattern is what a per-eye kappa-like rotation gives. Fit a per-eye 2-parameter offset (yaw/pitch) plus an eyeball-centre shift, then check it against the affine fit. | https://www.ncbi.nlm.nih.gov/pmc/articles/PMC10145701/ | M |
| 1.8 | Binocular kappa constraint (caveat) | [L] Older binocular-constraint kappa estimation is noise-sensitive (Kobe University paper). | Warns against the simpler version of 1.7 without a robust loss. Use Huber/IRLS as in the current fit. | https://da.lib.kobe-u.ac.jp/da/kernel/90007632/90007632.pdf | - (caveat) |
| 1.9 | Fixation-based self-calibration in 3D | [L] Uramune et al. (arXiv 2311.00391): calibration offset (optical to visual axis) chosen to minimise dispersion of points-of-regard during fixations, extended I-VDT. Average error 2.1 deg in walking VR, 18 participants; claims <3 deg. | No target list needed, which helps the desk-mode case. Needs scene geometry (the UI plane), which is available in desk mode. | https://arxiv.org/abs/2311.00391 | M |
| 1.10 | Gaussian-process gaze mapping from 5 points | [L] Semi-supervised GP gaze tracker: 5-point calibration, 0.9 deg reported (appearance-based, not VR). | Shows 5 points can be enough if the mapping is smooth. Use as the minimum for re-calibration in the field. | https://hal.archives-ouvertes.fr/hal-01882823 | S |

**[H] 1.11 Per-eye kappa explains the eye divergence.** If the opposite-signed per-eye error is a fixed visual-axis
offset (kappa), then the per-eye affine fit should leave near-zero residual after a 2-parameter offset per eye.
Test: refit with 1.7 on the existing `eyecal` captures; if residual falls below the affine residual, use it. Effort S.

---

## 2. Combining both eyes

| # | Idea | What it is | Why it fits us | Link | Effort |
|---|---|---|---|---|---|
| 2.1 | Per-eye reliability weights | [L] HMD patent: weight each eye by a reliability score (segmentation confidence, jumps in output, pupil shape, SNR), then combine. | We have per-eye validity, pupil diameter and origin. Use them as the weight input (see 2.2). | https://image-ppubs.uspto.gov/dirsearch-public/print/downloadPdf/12160672 (HMD patent, read as abstract only) | S |
| 2.2 | Jump-based outlier weight | [L] Stereo gaze patent: lower an eye's confidence when its gaze jumps abruptly from the previous samples; lean on the other eye while one is low. | Matches the bad state exactly (origin jumps ~15 mm). Our 2.5 mm origin-median gate is already this. Make it soft: w = exp(-(d/s)^2/2) with d = origin distance from a running median, s ~ 2.5 mm, instead of a hard drop. | https://image-ppubs.uspto.gov/dirsearch-public/print/downloadPdf/9503713 | S |
| 2.3 | Certainty factor on embedded tracker | [L] ICCS 2022: certainty factors on the Vive Pro Eye Tobii output improved accuracy by 25 % and precision by 32 % over raw. Trackers in headsets are capped at the scene rate and may low-pass saccades. | Supports building our own certainty score from the chip's fields instead of trusting the stream. | https://www.iccs-meeting.org/archive/iccs2022/papers/133530596.pdf | M |
| 2.4 | Unreal-style per-ray confidence | [L] Unreal's eye tracker API gives a 0-1 confidence per gaze ray. | Model for an API-compatible confidence field if we expose gaze through Monado. | https://docs.unrealengine.com/5.0/en-US/PythonAPI/class/EyeTrackerGazeData.html | S |
| 2.5 | Vergence closest-approach point | Use the point of closest approach of the two corrected rays as a 3D fixation point; use the cyclopean ray only when the vergence angle is small (< ~2 deg). | Vergence noise is large at far distance. Our rays are followed to 1.5 m, and the closest approach gives a depth for free (for the desk UI that is useful). Gate on vergence angle. | [H] (no source for the gate threshold) | S |
| 2.6 | Sample-level median of two eyes | Replace the mean with a median-of-N over the last few samples of both eyes. | Cheap. Takes out one-sample spikes without lag. | [H] | S |

**[H] 2.7 Hysteresis on eye drop.** Re-admit a recovering eye only after N (say 30) consecutive samples inside
the gate, not on the first valid sample. Bad-state recovery in the notes seems to flicker; untested.

---

## 3. Filtering

| # | Idea | What it is | Why it fits us | Link | Effort |
|---|---|---|---|---|---|
| 3.1 | Kalman with a velocity state and saccade switch | [L] Komogortsev & Khan: Kalman filter estimating both position and speed of gaze (2007/2008). Saccade switching: different process noise for fixation vs saccade (I-VT threshold ~30 deg/s). | Our One Euro uses a 3 deg snap for saccades. A Kalman with a switch can predict 1-2 samples ahead, which matters for latency (see 6). | https://userweb.cs.txstate.edu/~ok11/papers_published/2008_ETRA_Ko_Kh.pdf | M |
| 3.2 | Saccade onset check costs one sample | [L] Spakov (ETRA 2012): a single-point outlier check adds one sample delay at saccade onset, tolerable above ~100 Hz. | At 120 Hz that is 8.3 ms, acceptable. Keep the snap check. | https://homepages.tuni.fi/oleg.spakov/publications/Spakov_ETRA_12.pdf | S |
| 3.3 | Filter lag cost | [L] Blignaut (2019): smoothing delays when gaze settles after a saccade; reported up to 158 ms in earlier work. | Explains why the One Euro beta has to stay low for interaction. Keep beta low in desk mode. | https://bop.unibe.ch/JEMR/article/download/JEMR_12.2.3/7837/19309 | S |
| 3.4 | Fixation clustering (I-DT) on corrected rays | Dispersion-based fixation detection on the corrected rays, used only to gate calibration and drift (not as output). | Keep fixation detection separate from the output filter so accuracy numbers are not smoothed. | [L] (method common; threshold from our notes) | S |

No head-to-head latency comparison of Kalman vs One Euro was found. Treat 3.1 as a hypothesis until measured.

---

## 4. Drift correction and implicit recalibration

| # | Idea | What it is | Why it fits us | Link | Effort |
|---|---|---|---|---|---|
| 4.1 | Click-based implicit recalibration | [L] Online-EYE (CHI 2025, Google/Lancaster): pairs gaze with controller interactions on UI elements and refines the calibration matrix in the background; reported as comparable to explicit calibration. | Desk mode: click targets give ground truth. Use only clicks after a dwell (>= 300 ms) with I-DT dispersion below ~1 deg, and fit only a per-eye offset to limit bias (the user looks at the target before clicking). | https://research.google/pubs/online-eye-multimodal-implicit-eye-tracking-calibration-for-xr/ | M |
| 4.2 | Pursuit-based drift correction | Show a slow pursuit target every few minutes (or during idle desk time), correlate gaze with the target, update the offset (1.5, 1.6). | Cheap drift check. Limit speed to ~6 deg/s. | [L] (1.5/1.6) | M |
| 4.3 | Local GP drift model | 1.4 applied on the time axis: GP on residuals over time per session. | Separates per-session drift from per-position error. Needs ground truth (clicks, 4.1). | [L] (1.4) + [H] for the time axis | M |
| 4.4 | Drift vs decay check | Re-run the validation on a different day to separate decay from calibration error (already recommended in our notes). | Tells whether 4.1-4.3 are needed at all. | [H] (from existing notes) | S |

Caveat [L]: implicit calibration from clicks risks bias (people look at the target before clicking). Gate on dwell.

---

## 5. Automatic IPD and headset-slip detection

| # | Idea | What it is | Why it fits us | Link | Effort |
|---|---|---|---|---|---|
| 5.1 | IPD from eye origins | [L] Kim et al. (IEEE Access 2021): IPD from conjugate eye movement/vergence. [L] ETRA 2022: IPD and vergence jointly from a Vive Pro Eye Tobii tracker. | Our per-eye origins are in mm (+/-31 mm, field 34 IPD ~60 mm). Estimate IPD from the origin separation during a vergence sweep, and check it against field 34. | https://doaj.org/article/2d210cd9b50c48b68a0206f23023fd0f | S |
| 5.2 | Slip as a common-mode origin shift | [H] Headset slip moves both origins in the HMD frame by the same vector; IPD (origin separation) should not change. A sustained midpoint shift of > 3 mm with IPD constant = slip; re-estimate the offset, not the model. | Slip is named as a risk for Quest Pro (23 % of users excluded). It is a cheap detector with no extra sensor. Test on the captures. | [H] (background: Quest Pro slippage study, https://arxiv.org/pdf/2403.07210; slip sensor patent, https://image-ppubs.uspto.gov/dirsearch-public/print/downloadPdf/11366519) | S |
| 5.3 | Slip compensation by shift | Shift the origin by the estimated slip vector before the ray is formed. Parallax at 2 m is ~0.9 deg per eye (from notes), so a 3 mm shift is worth ~1-2 deg at near distance. | Matches the scale of our residual. | [H] | S |
| 5.4 | Robustness to slippage in design | [L] A custom VR tracker kept ~1.1 deg accuracy up to 10 mm slip (Chugh, U. Toronto 2020, thesis). Commercial headsets degrade faster (Quest Pro, percentile study). | Sets expectation: slip > 5 mm is a re-calibration event, not a filter problem. | https://www.eecg.utoronto.ca/~jayar/pubs/theses/Chugh/SoumilChugh.pdf | - (context) |

---

## 6. Foveated rendering requirements

| # | Item | Figure | Source |
|---|---|---|---|
| 6.1 | Tolerable total latency | [L] 20-40 ms eye-tracker latency: no significant loss of acceptable foveation. 80-150 ms: significant loss. Total system 50-70 ms reported as tolerable. | Albert et al., ACM TAP 2017, https://research.nvidia.com/publication/2017-09_latency-requirements-foveated-rendering-virtual-reality |
| 6.2 | Accuracy for varifocal (AR, median adult) | [L] Dunn 2019: tolerance ~0.54 deg. A VR survey cites 1.44 deg for VR varifocal from a different work (second-hand, not checked). | https://telepresence.web.unc.edu/wp-content/uploads/sites/11620/2019/03/Dunn_2019_RequiredAccuracyOfGazeTrackingForVarifocalDisplays.pdf ; https://arxiv.org/pdf/2305.14080 |
| 6.3 | Foveation accuracy | [H] Foveal high-res region radius ~10-15 deg means a 2 deg error still keeps the fovea on target; 1 deg is comfortable. Target: below 2 deg per eye, below 1 deg for the high-res region. | [H] (not measured; Dunn 6.2 is for focus, not rendering) |
| 6.4 | Latency budget | Our path: 120 Hz sample (8.3 ms) + filter + service + Monado prediction. Keep the total under ~50 ms. Use XrEyeGazeSampleTimeEXT to compensate with a predicted pose. | [L] Khronos spec on `XR_EXT_eye_gaze_interaction` (XrEyeGazeSampleTimeEXT, position+orientation tracked bits): https://registry.khronos.org/OpenXR/specs/1.1/man/html/XrSystemEyeGazeInteractionPropertiesEXT.html |
| 6.5 | Output shape for OpenXR | [L] Varjo's OpenXR runtime gives a combined gaze pose only (position + rotation). | Matches our combined-ray output. Keep per-eye data internal. https://developer.varjo.com/docs/unreal/ue5/eye-tracking-with-unreal5 |

Not found: a measured accuracy-vs-latency curve for a commercial headset. Quest Pro (arXiv 2403.07210) is the nearest
study but its numbers were not retrievable from the abstract.

---

## 7. Ideas checked and not useful here

- GazeProphet (arXiv 2508.13546): software-only gaze prediction from scene content, median 3.83 deg, no hardware. Not applicable to our per-eye data.
- Learned confidence heads (same paper): needs training data we do not have.
- Monado-specific eye-gaze implementation: no search results. Check `src/xrt/` in the Monado tree directly.

---

## 8. Top-5 ranked actions (aim: below 2 deg per eye)

1. **Soft per-eye reliability weight from origin distance plus validity (2.2, 2.7 hysteresis, 2.6 median).** S. Replaces the hard 2.5 mm gate in the live path; offline the gate already gives right 26 -> 3.4 deg, so this is the largest expected cut to the tail. Verify live on the next worn capture.
2. **Per-eye kappa-like offset (1.7 plus 1.11 check), then ridge-regularised affine with a GP residual (1.1 + 1.3).** M. Targets the opposite-signed per-eye errors (about 8 deg each) and the field-dependent residual. Test on the existing `eyecal` runs before any new capture.
3. **Click-based implicit recalibration with dwell and dispersion gating (4.1), with a pursuit check (4.2) for drift.** M. Keeps accuracy after the first session without a new calibration; gives the drift number that 4.4 needs.
4. **Slip detector from the common-mode origin shift, checked against IPD (5.2, 5.1).** S. Cheap, no new hardware, and it turns an unexplained error into a re-calibration prompt.
5. **Kalman with saccade switch and 1-2 sample prediction (3.1), budgeted against the 50 ms latency target (6.1, 6.4).** M. Needed for foveated rendering latency; keep One Euro for the cursor.

Ranking note: 1 and 2 are the accuracy path; 3 and 4 keep it there; 5 is the foveated-rendering path. If only one
can be done before the next worn session, do 1.

---

## Sources

- https://www.ncbi.nlm.nih.gov/pmc/articles/PMC10966887/ (ridge vs polynomial, mobile tracker)
- https://www.ncbi.nlm.nih.gov/pmc/articles/PMC7733312/ (head-mounted polynomial error distribution)
- https://arxiv.org/pdf/1612.06919 (Tripathi & Guenter, continuous self-calibration, GP)
- https://www.medien.informatik.uni-muenchen.de/pubdb/publications/pub/drewes2019etra/drewes2019etra.pdf (regression pursuit calibration)
- https://perceptualui.org/publications/pfeuffer13_uist/ (pursuit calibration, 2013)
- https://www.ncbi.nlm.nih.gov/pmc/articles/PMC10145701/ (automatic kappa, coplanarity)
- https://arxiv.org/abs/2311.00391 (fixation-based self-calibration in VR)
- https://hal.archives-ouvertes.fr/hal-01882823 (semi-supervised GP gaze)
- https://research.google/pubs/online-eye-multimodal-implicit-eye-tracking-calibration-for-xr/ (Online-EYE, CHI 2025)
- https://image-ppubs.uspto.gov/dirsearch-public/print/downloadPdf/9503713 (stereo gaze patent, outlier)
- https://image-ppubs.uspto.gov/dirsearch-public/print/downloadPdf/12160672 (HMD reliability weighting patent)
- https://www.iccs-meeting.org/archive/iccs2022/papers/133530596.pdf (certainty factors, Vive Pro Eye)
- https://userweb.cs.txstate.edu/~ok11/papers_published/2008_ETRA_Ko_Kh.pdf (Kalman for gaze)
- https://homepages.tuni.fi/oleg.spakov/publications/Spakov_ETRA_12.pdf (filter delay, saccades)
- https://bop.unibe.ch/JEMR/article/download/JEMR_12.2.3/7837/19309 (filter latency, Blignaut 2019)
- https://doaj.org/article/2d210cd9b50c48b68a0206f23023fd0f (IPD from conjugate eye movement)
- https://arxiv.org/pdf/2403.07210 (Quest Pro signal quality; numbers not retrieved)
- https://www.eecg.utoronto.ca/~jayar/pubs/theses/Chugh/SoumilChugh.pdf (slippage-robust VR tracker, thesis)
- https://research.nvidia.com/publication/2017-09_latency-requirements-foveated-rendering-virtual-reality (Albert 2017)
- https://telepresence.web.unc.edu/wp-content/uploads/sites/11620/2019/03/Dunn_2019_RequiredAccuracyOfGazeTrackingForVarifocalDisplays.pdf (Dunn 2019)
- https://arxiv.org/pdf/2305.14080 (VR eye-tracking survey, second-hand 1.44 deg figure)
- https://registry.khronos.org/OpenXR/specs/1.1/man/html/XrSystemEyeGazeInteractionPropertiesEXT.html (XR_EXT_eye_gaze_interaction)
- https://developer.varjo.com/docs/unreal/ue5/eye-tracking-with-unreal5 (Varjo combined pose)
- https://docs.unrealengine.com/5.0/en-US/PythonAPI/class/EyeTrackerGazeData.html (per-ray confidence)
