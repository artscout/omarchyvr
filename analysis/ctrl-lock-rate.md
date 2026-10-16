# Controller optical lock rate (2026-10-15)

**Problem.** In the 2026-10-15 08:2x desk session, the controllers were optically locked in only about **32 %** of LED
frames (left and right) during the 38 windows of use. The rest of the time the fuser held the pose with the gyro and
the arm model. This page covers:

* where the lost frames go;
* what was changed in `pimax-ctrltrack`;
* the before/after numbers from an offline replay;
* the recording needed to settle the rest.

Tools: `pimax-ctrltrack lockrate FILE.pxcam [--baseline] [--oracle] [--count N] [--csv OUT]` (new). It replays a
recording through the same per-frame code as `live`, as fast as possible. `--baseline` gives the tracker and blob
detection as they were before this change.

## 1. Data available

* **LED frames:** only `~/ctrl-test.pxcam` (2026-10-14, 1492 LED frames, 60 s).
  * It has no controller IMU data.
  * Its timeline:
    * 0-37 s: hands in use in front of the body (930 LED frames, "in-use");
    * 38-48 s: hands at the waist, no blobs;
    * 49-60 s: headset put down on the controllers (close range, not a use case).
* **The three desk sessions** (`~/.local/share/omarchyvr/ctrl-sessions/`) have the fuser logs (`ctrl-imu-*.csv`: gyro, every
  optical fix with LED count and verdict, shown poses) and the tracker/Monado logs, but no LED frames. Today's
  session cannot be replayed frame by frame. Section 6 gives the recording step that fixes this.

## 2. What today's session logs say (fuser side)

Fixes per session, from the `O` lines of `ctrl-imu-{left,right}.csv` (left / right):

| | 10-14 18:13 | 10-14 18:27 | 10-15 08:26 (today) |
|---|---|---|---|
| Fixes with 5-6 LEDs (the minimum accepted is 5) | 17 % / 20 % | 16 % / 20 % | **64 % / 62 %** |
| Fixes with 12+ LEDs | 38 % / 44 % | 51 % / 46 % | 2 % / 3 % |
| Fix height below the head, median | 13 / 14 cm | 24 / 19 cm | 21 / 26 cm |
| Head-relative speed at the last fix before a gap of more than 300 ms (median) | 0.68 / 0.99 m/s | 0.55 / 0.47 m/s | 0.52 / 0.15 m/s |
| Head-relative speed, all fixes (median) | 0.25 / 0.23 m/s | 0.15 / 0.19 m/s | 0.06 / 0.09 m/s |

* **"Dropped by the prediction" is not a cause.** Today the left hand had 36 `pos-only` and 9 `turn` verdicts; the
  right had 74 `pos-only` and 10 `turn`.
  * That is 4-10 % of the fixes.
  * `pos-only` fixes still update the position; only the few-LED orientation is ignored.
  * The 7 body-model rejects (`merged`) are negligible.
* **Today the controllers were barely seen.** Almost two thirds of the fixes that did pass had 5-6 LEDs, just at the
  acceptance limit. So many more frames must have had 3-4 matched LEDs and were thrown away. The hands were held
  lower (21-26 cm below the head), where the cameras see the LED ring at a grazing angle and near the image edge.
* **Gaps start during motion.** On 10-14, the median speed at the last fix before a gap was 2-4 times the overall
  median. A fast move takes the LEDs outside the 12 px prior gate of a prior that was not predicted.
* **Gap lengths today** (fuser view, 85 s logged, left): 3.7 s in gaps under 300 ms, 4.8 s in 0.3-1 s gaps, 10.6 s in
  1-3 s gaps, 22.9 s in gaps over 3 s. The right hand had 33 s of gaps over 3 s. Long gaps are hands out of view or
  turned away. Short gaps are tracker losses.

## 3. Loss breakdown (offline, `ctrl-test.pxcam` in-use part, 930 LED frames)

`lockrate --oracle` classifies every (frame, hand) that is not locked:

| Class | Meaning |
|---|---|
| `oracle` | The brute-force search on this frame finds the controller near its last position: a pure tracker loss. |
| `out-of-view` | Fewer than 3 facing LEDs of the last pose project into any camera. |
| `undetected` | LEDs should be in view, but fewer than 3 blobs lie within 25 px of them. Causes: the hand moved out of view since the last fix, occlusion by the other hand, LEDs turned away, or blur. |
| `few-blobs` | 3+ blobs are where the controller should be, but there is no 5-LED solve. |
| `long-gap` | More than 1 s since the last fix. |
| `never-seen` | Before the first fix. |

| % of LED frames (left / right) | before | after |
|---|---|---|
| **locked** | **65.9 / 63.2** | **81.5 / 70.2** |
| few-blobs (3+ blobs there, no 5-LED solve) | 8.9 / 7.7 | 5.8 / 5.2 |
| undetected (no blobs where the last pose says) | 13.4 / 15.3 | 7.2 / 14.6 |
| oracle (tracker loss; the search would find it) | 0.9 / 0.6 | 0.2 / 0.4 |
| out-of-view (from the last pose) | 0 / 0 | 0 / 0 |
| long gap (more than 1 s) | 4.6 / 8.4 | 0 / 5.2 |
| never seen yet (start) | 6.2 / 4.7 | 5.3 / 4.4 |
| brute force on every frame finds both controllers | 46.8 | 54.5 |

What the table says:

* **The tracker itself loses little.** Under 1 % of frames are ones where a search would have found the controller.
  The searches already run every frame while a hand is missing.
* **The losses are in detection and LED count.** About half are frames with 3-4 usable blobs (no 5-LED solve).
  The other half have no blobs where the controller was last seen: it moved or turned away.
* **Exposure is not the problem.** In LED frames it is fixed at exposure 40 / gain 62, and every frame has peaks at
  255. The blob threshold was the problem (section 4).

## 4. Changes (`rust/pimax-ctrltrack`)

1. **Shared tracker** (`src/tracker.rs`). `live` and the new `lockrate` replay run the same per-frame code, so the
   lock rate can be measured offline.
2. **Velocity-predicted prior.** Position and rotation are extrapolated from the last two fixes (at most 120 ms,
   capped at 3 m/s and 10 rad/s). The unpredicted pose is still tried.
3. **Adaptive gate.** When the hand moves faster than 0.3 m/s, or the prior is older than one frame, a 24 px gate is
   tried after the 12 px one. The match is always tightened to 2.5 px before the pose is accepted.
4. **Constrained 3-4 LED track.** From a prior, if the free 5-LED solve fails:
   * The LM refine adds a rotation prior: the predicted orientation, 300 px per rad (`solve::refine_with`).
   * The match is redone at 3.75 px.
   * The fix is accepted with 3+ LEDs if:
     * the rms is at most 1.2 px (3 LEDs) or 1.8 px (4+ LEDs);
     * the position is within 3 cm + 0.75 m/s × age of the prediction;
     * the turn is at most 20 deg.
   * At most 12 few-LED fixes in a row; then the slot must be found again by a 5-LED solve or the search.
   * Few-LED fixes are published with their real LED count, so the fuser gives them its low gain (0.2) and ignores
     their orientation (`pos-only`).
5. **Blob threshold 80 instead of 60** (`BlobParams::live`; the peak stays 120).
   * At the LED exposure, the 60-level halo merges neighbouring LEDs and pulls the centroids.
   * With 80, the brute force finds both controllers in 54.5 % of frames instead of 46.8 %.
   * Higher thresholds lock slightly more but jump more:

     | Threshold | Lock (left / right) | Jumps |
     |---|---|---|
     | 90 | 83.7 / 71.6 % | 19 |
     | 100 | 86.9 / 72.3 % | 22 |

6. `live --baseline` restores the old tracker and blobs for an A/B test. `live --thr T --peak P` overrides the blob
   detection.

Few-LED fix contribution (ablation, threshold 60):

| Variant | Lock (left / right) |
|---|---|
| Baseline | 65.9 / 63.2 % |
| Prediction and gate only | 67.1 / 63.4 % |
| Plus 3-LED fixes | 73.1 / 65.5 % |
| Plus threshold 80 | 81.5 / 70.2 % |

## 5. Before / after (replay)

| `ctrl-test.pxcam` | before | after |
|---|---|---|
| **Lock, in-use part (930 frames), left / right** | **65.9 / 63.2 %** | **81.5 / 70.2 %** |
| Lock, whole recording (1492 frames) | 43.4 / 42.4 % | 53.8 / 47.8 % |
| Gaps of 1-2 / 3-7 / 8-25 / >25 LED frames, left | 19 / 6 / 6 / 3 | 18 / 8 / 5 / 0 |
| Gaps of 1-2 / 3-7 / 8-25 / >25 LED frames, right | 13 / 6 / 5 / 4 | 13 / 5 / 4 / 3 |
| Frame-to-frame step p50 / p95, left | 9.6 / 45.2 mm | 12.2 / 51.7 mm |
| Frame-to-frame step p50 / p95, right | 11.1 / 42.5 mm | 10.6 / 41.7 mm |
| Jumps over 8 cm between consecutive fixes | 12 | 11 |
| Error of 5-7 LED fixes vs interpolated 8+ LED neighbours, p50 / p90 | 16.9 / 45.7 mm | 12.6 / 34.8 mm |
| Error of the new few-LED fixes, same measure (n = 26), p50 / p90 | n/a | 15.5 / 42.9 mm |
| Brute-force searches | 691 | 462 |
| Frame work mean (blobs + tracker) | 8.9 ms | 6.6 ms |
| Tracker p95 | 21 ms | 22 ms |

* **CPU.** Today's live log shows 5.6-11 ms mean frame work while tracking, and 1.3-1.5 ms with nothing in view.
  The replay was run while the machine was loaded (load average 10-12), so its absolute times are high.
  * The change lowers the mean, because fewer frames need the 10 ms brute-force search.
  * The new per-frame work (a second gate, a second start pose, one constrained refine) is well under 1 ms.
* **Stability.** The p95 step went up for the left hand: few-LED fixes are noisier than 8+ LED fixes. The fuser's
  LED-count gains already account for that.

## 6. Recording step for the user (not run by the agent)

**Goal.** About 60 s of LED frames together with the controller IMU, in the low-arm poses of today's session. With
it, today's 32 % can be replayed and the `undetected` class split into out-of-view, turned-away and blur.

**What it touches.** It uses only the existing read-only paths:
* the camera broker's client socket;
* the fuser log that `omarchyvr desk start --6dof --ctrl-track` already writes (`ctrl-imu-{left,right}.csv`, host
  CLOCK_MONOTONIC like the broker frames).

**Command.** Run it in a desk session that is already running with `--6dof --ctrl-track`:

```bash
~/pimax-re/rust/target/release/pimax-cam record ~/ctrl-lock-$(date +%Y%m%d-%H%M).pxcam --source broker --seconds 60
```

**Checklist:**

- [ ] **Setup:**
  - [ ] The desk session runs with `--6dof --ctrl-track`.
  - [ ] Both controllers show in VR.
  - [ ] Free disk space is 3 GB or more (2.2 GB per minute).
- [ ] Press a button on each controller (both awake).
- [ ] Start the command. During the 60 s:
  - [ ] 0-10 s: hands still, at chest height in front, then at the waist (today's pose).
  - [ ] 10-25 s: normal use (pointing, menus) with the arms low and to the sides, as in today's session.
  - [ ] 25-40 s: fast swings and wrist flicks, in front and to the sides.
  - [ ] 40-50 s: hands crossing and close together; one hand behind the other.
  - [ ] 50-60 s: one hand out of view (behind the back) and back in; repeat with the other hand.
- [ ] **After the command:**
  - [ ] Copy the session dir: `cp -r ~/.local/share/omarchyvr/ctrl-sessions/<latest> ~/ctrl-lock-session/`.
  - [ ] Note the .pxcam name.
- [ ] **Offline, no device:**
  - [ ] `pimax-ctrltrack lockrate ~/ctrl-lock-*.pxcam --oracle --csv /tmp/after.csv`
  - [ ] the same with `--baseline`;
  - [ ] `pimax-ctrltrack fusereplay` on the session's csv files.

## 7. Not done yet / next steps

* **Gyro orientation for the few-LED track.** The rotation prior is extrapolated from the optical fixes. A 3 deg
  orientation error costs about 7 mm (unit test), and fast turns cost more. The gyro orientation is in Monado's
  fuser, not in `pimax-ctrltrack`. Possible next steps:
  * write the fuser's predicted orientation back through a second seqlock file;
  * or do the few-LED solve in the fuser.
* **Joint multi-camera acquisition.** The search needs 5 LEDs in a single camera. A controller seen with 3 LEDs in
  cam 0 and 3 in cam 2 can only be tracked from a prior, never acquired. It needs the new recording to quantify.
* **EKF with the accelerometer to bridge gaps.** This is the fuser side. Only `undetected` and long gaps remain, so
  better bridging is now worth more than more lock. The new recording gives the ground truth (fixes before and after
  gaps) to test it with `fusereplay`.
* **Camera extrinsics refit (cams 2 and 3).** This has not been redone. It needs the new recording.
