# Camera hand tracking (Mercury)

Controller-free hands for the Dream Air: Monado's Mercury hand tracker runs on the two front tracking cameras
(factory cameras 0 `trackingA` and 1 `trackingB`, baseline 111.6 mm). It is **off by default** and **experimental**.
It has been measured on recordings only. No live session has run it yet (see "Live test" below).

* Monado: worktree `~/src/monado-ht`, branch `hands-mercury` (= `desk` cfad375 + one commit), build dir
  `~/src/monado-ht/build-ht`. It is a superset of the desk build (eye, 6DoF, controllers, recorder tap) plus
  Mercury and ONNX Runtime 1.20.1 (CPU, vendored in `~/src/onnxruntime-1.20.1`, found through the binary's RUNPATH).
  The branch is local only. Never push it.
* Driver code: `src/xrt/drivers/pimax/pimax_ht.{c,h}` (thread, rate limits, joints -> OpenXR),
  `pimax_ht_calib.{c,h}` (the factory XML -> Monado stereo calibration), `pimax_camera.c` (`setup_mercury`),
  `pimax_ht_bench.c` (offline bench).
* Launcher: `rust/pimax-tools/src/vr/hands.rs`. pimaxd component `hands`: `rust/pimax-components/src/spec.rs`.
* Numbers and method: `analysis/research/hand-tracking-results.md`. Background: `analysis/research/hand-tracking-research.md`.

## Turning it on

In the app: Tracking > Hands > **Camera hands** and **Hands as controllers** (developer mode; Camera hands is also in Home > Quick settings); Play starts with them. The switch is
greyed out (with the reason) while the build lacks `PIMAX_HT_HZ` or the models are missing. The guided live test is
Tracking > Tests > **Start the hand test** (docs/PLAY.md). The CLI below is the engine behind it.

```sh
pimax desk start --6dof --hands                 # desk mode with camera hands
pimax vr start --6dof --hands --hands-roles     # VR, the hands take the controller roles
```

| flag | effect |
|---|---|
| `--hands` | Needs `--6dof`, otherwise it is a usage error: Mercury reads the stereo pairs from the camera broker, and the hands are world-locked through the 6DoF head pose. Sets `PIMAX_HANDS=mercury PIMAX_HT_STATS=1` on the build the start uses. |
| `--hands-roles` | Also `PIMAX_HT_ROLES=hands`: the camera hands are the left/right hand controllers, ahead of the Pimax controllers. Apps that only read controllers then see pinch as the trigger. Needs `--hands`. |
| `--hands-build DIR` | Use another build for this start (also `PIMAX_HANDS_BUILD`). Not needed normally: the one build has the hands. |

How the build is chosen (the simplest design that cannot break a start):

1. One Monado build carries everything (`.claude/CLAUDE.md` section 3), hands included: `--hands` does **not**
   switch builds any more. It only sets the variables on the build the start uses (`~/src/monado-eye/build`).
   Another build is used only when it is named (`--hands-build DIR` / `PIMAX_HANDS_BUILD`); then the service binary
   and the runtime manifest move together. (The separate `~/src/monado-ht/build-ht` of the first tests is no longer
   switched to by default.)
2. Hands are on only when that `monado-service` contains the Mercury code (the launcher looks for
   the `PIMAX_HT_HZ` option name in the binary) and both greyscale models are installed. Otherwise the start prints
   `WARNING: --hands: ...; starting WITHOUT camera hands` and runs exactly as without `--hands`.
3. An explicit `--monado-build`, `PIMAX_MONADO_BUILD` or `--service-bin` wins and is used as given. Hands are turned
   on only if that binary has Mercury in it.
4. Inside Monado, any failure (models, calibration, ONNX Runtime) only turns the hands off. Monado logs
   `VR continues without camera hands` and keeps running.

**Restarts keep it.** The flags are written to `<session dir>/start-argv.txt` like every start flag. So `pimax desk
restart`, `pimax vr restart`, the watchdog and pimaxd all start again with `--hands`.

**pimaxd.** `hands` is a leaf feature component (deps: monado, broker; nothing depends on it). It is never restarted
on its own: a toggle applies at the next Monado start. It is up when `monado.log` says `hands: Mercury up`. The
`pimaxd.toml` key is `[components] hands = false`. A start argv decides on its own: no `--hands` means off.

## Hands only in Work (WayVR)

User rule (2026-10-15): in Work, **hands by default, no controllers at all, never, unless toggled on**. The pointer is
the bare hand Monado tracks with the cameras; the controller rays and buttons are ignored entirely, with no fallback
when no hand is tracked (the pointer is then simply absent).

* Pointer: a ray from the index knuckle, along the line from an estimated shoulder (17 cm below the head, 16 cm to the side) through the knuckle, turned 35 % towards the hand's own direction (wrist -> knuckle) so it follows where the hand points (WayVR `HAND_DIR_WEIGHT`, 2026-10-16; docs/WAYVR-CONTROLS.md).
  Index pinch (thumb tip to index tip under 2.2 cm, released over 4 cm) = click. Middle-finger pinch = grab.
* WayVR knows a camera hand from a controller-emulated hand through `XR_EXT_hand_tracking_data_source`: its pointer
  hand trackers ask for the unobstructed source only. Without camera hands (no `--hands`) nothing changes.
  The thresholds have not been tuned on live hands yet.
* Switch: Tracking > **Controllers in Work** (also on Home's Tracking card), OFF by default (user decision 2026-10-15: "in Work hands by
  default, no controllers at all, never, unless toggled on"). Off: the controllers are ignored ENTIRELY (rays,
  buttons, triggers, sticks, grabs), also while no hand is tracked; there is no fallback. On: controllers work too, a
  tracked controller wins and a tracked camera hand fills in for a controller that is not tracked. It is saved in
  `~/.config/pimax/wayvr-desk.conf` (`controllers_in_work`; the old `prefer_hands` key is read as its inverse);
  `pimax desk start` writes it into WayVR's conf.d. The switch also applies at once to a running session.
* Voice: "Max, use controllers" / "prioritize controllers" / "turn on controllers" (`vr.input.controllers`,
  `wayvrctl controllers on`) and "Max, hands only" / "prioritize hands" / "turn off controllers" (`vr.input.hands`,
  `wayvrctl controllers off`). The voice command lasts for the running session; the app switch decides the next
  start. `wayvrctl prefer-hands on|off` still works (the inverse alias).
* Safety: without camera hand tracking in the runtime (no `--hands`) the controllers always work, so nobody is locked
  out. Stopping the session is done by the app, not by a controller. WayVR's own menu/watch is a pointer click, so
  in hands-only mode it is reached by pinching.
* WayVR (`~/src/wayvr`, eye-gaze): `wayvr/src/backend/hand_pointer.rs` (ray, pinch, priority; unit tests),
  `wayvr/src/backend/openxr/camera_hands.rs`. `wayvr.log` says `camera hand pointer: ...` when the hands take or give
  back the pointer.

## Environment (Monado, all optional; the caller's values win over the launcher's)

| variable | default | meaning |
|---|---|---|
| `PIMAX_HANDS` | unset | `mercury` turns the hands on (the launcher sets it) |
| `PIMAX_HT_ROLES` | `tracking` | `tracking`: hand-tracker devices only (`XR_EXT_hand_tracking`, `XR_EXT_hand_interaction`). `hands`: they also take the left/right controller roles |
| `PIMAX_HT_HZ` | 25 | most stereo pairs per second given to Mercury. The camera delivers 25 tracking-exposure pairs/s, because the other 25 frames/s are the controller-LED exposure |
| `PIMAX_HT_DUTY` | 0.5 | Mercury may be busy at most this fraction of the wall time. After a slow pair the next one waits |
| `PIMAX_HT_STALE_MS` | 150 | a hand not seen for this long is reported as not tracked |
| `PIMAX_HT_PREDICT_MS` | 0 | extrapolate the joints forward (ms). 0 = report the pose at the camera time |
| `PIMAX_HT_CAMS` | `0,1` | the two factory camera ids used as the stereo pair |
| `PIMAX_HT_ORIENT` | from the calibration (180,0) | camera roll per view, for Mercury. Only the calibrated one gave any detection |
| `PIMAX_HT_HEAD_OFFSET` | as the 6DoF tracker | x,y,z (m) of the head origin from the IMU |
| `PIMAX_HT_NICE` | 10 | nice level of the Mercury thread |
| `PIMAX_HT_STATS` | 0 (launcher: 1) | every 5 s one `hands (running): N pairs/s processed ..., left active X %, right Y %` line in monado.log |

## Numbers (offline, i9-14900KF; details in the results file)

* Rate: **24.7-25.1 pairs/s processed** out of 25 offered, with 0 or 1 dropped per 5 s window, in the service on
  `~/ctrl-test.pxcam` and the SLAM clip (the clip itself loses camera frames). This meets the 25 Hz target.
* CPU, no hand in view: 1.6-2.8 ms wall and 4-6.5 ms CPU per pair, which is **about 10-16 % of one core** at 25 Hz.
  In the service the four `pimax-hands` threads add up to 8-12 %.
* CPU with hands in view: 4-8 ms wall and 11-14 ms CPU per pair, which is **about 28-35 % of one core** at 25 Hz.
  The cost is the same for one hand or two.
* Model load: 45-110 ms. Mercury starts in about 75 ms.
* Detection: the existing recordings hold controllers or no hands. The best is 11 % of pairs with hands, at a wrist
  distance of 0.28-0.37 m. **There is no detection rate for bare, lit hands yet.** Make that recording with the live
  test below.

## Limits

* Mercury was trained on Index and Luxonis cameras. Our fisheye greyscale cameras and their auto-exposure are
  untested live. Mercury needs **plenty of light** on the hands.
* Only cameras 0+1 (front) are used, so hands low or to the side are out of view.
* Hands are placed with the head pose at the frame time. Without good SLAM they move with the head.
* The pinch, poke, aim, grip and grasp values are derived from the joints with fixed thresholds that have not been
  tuned on real hands.
* Pimax controllers that are on also provide emulated hands (the conforming data source). Apps that ask for the
  unobstructed source get the camera hands. With `--hands-roles` the camera hands win the controller roles, so
  switch the controllers off or put them down.
* CPU: up to about a third of a core while hands are in view, in addition to SLAM and the compositor.

## Licence

* Mercury code: BSL-1.0 (Monado).
* **The models in `~/.local/share/monado/hand-tracking-models` have no licence**: the repository has no LICENSE file
  and GitLab reports none. Use them locally only. Do not redistribute them, bundle them, or put them in this repo.

## Install (between sessions only: no Monado running)

The models are already in `~/.local/share/monado/hand-tracking-models` on this machine. On another machine:
`~/src/monado-ht/scripts/get-ht-models.sh` (needs `git-lfs`).

```sh
# 1. Monado hands build (also after every pull or edit of hands-mercury)
nice -n 19 ninja -C ~/src/monado-ht/build-ht -j8
# 2. every relink drops the capability: the compositor needs it for its realtime priority
sudo setcap cap_sys_nice+ep ~/src/monado-ht/build-ht/src/xrt/targets/service/monado-service
getcap ~/src/monado-ht/build-ht/src/xrt/targets/service/monado-service   # must print cap_sys_nice=ep
# 3. launcher (and pimaxd if you use it), installed only while nothing runs (docs/LESSONS.md row 8)
cd ~/pimax-re/rust && nice -n 19 cargo build --release -j8 -p pimax-tools -p pimax-autod
install -m755 target/release/pimax ~/.local/bin/pimax.new && mv -f ~/.local/bin/pimax.new ~/.local/bin/pimax
install -m755 target/release/pimax-autod ~/.local/bin/pimax-autod
pimax desk start --6dof --hands --dry-run   # must say "camera hands on ... Monado from .../build-ht"
```

Configure a fresh hands build like the desk build, plus
`-DONNXRuntime_ROOT_DIR=$HOME/src/onnxruntime-1.20.1 -DXRT_MODULE_MERCURY_HANDTRACKING=ON
-DXRT_BUILD_DRIVER_HANDTRACKING=ON` (RelWithDebInfo).

## Live test (with the headset, by you)

1. Start in VR mode so the test app is the only OpenXR app. Use a bright room and bare hands, and put the
   controllers down or turn them off:
   `pimax vr start --6dof --hands`
2. `grep -E "hands" /tmp/pimax-vr-session/monado.log | tail`: you want `hands: Mercury up`, then a stats line every
   5 s. Hold your hands in front of the headset, 25-50 cm away, and watch `left active` / `right active`.
3. Joints: `XR_RUNTIME_JSON=~/src/monado-ht/build-ht/openxr_monado-dev.json ~/pimax-re/rust/target/release/xr-handtest --source any --no-grip-check --require any --min-valid 0.3 --seconds 30`
4. Lit bare-hands recording, to measure detection offline (it reads the broker and does not open the camera):
   `~/pimax-re/rust/target/release/pimax-cam record ~/hands-lit.pxcam --seconds 40 --source broker`.
   During the recording, show each hand for about 10 s, then both, then open and close and pinch. Then:
   `nice -n 19 ~/src/monado-ht/build-ht/src/xrt/drivers/pimax_ht_bench ~/hands-lit.pxcam --tsv /tmp/hands-lit.tsv`
   The `detection:` line is the rate. Add `--dump /tmp/hands-dump` for images with the keypoints.
5. Pinch as a controller: `pimax vr stop`, then `pimax vr start --6dof --hands --hands-roles`, then
   `XR_RUNTIME_JSON=~/src/monado-ht/build-ht/openxr_monado-dev.json ~/pimax-re/rust/target/release/xr-pinchtest --seconds 20`.
   While it runs, pinch with each hand. The pinch value should rise above 0.
6. Desk mode: `pimax desk start --6dof --hands`. WayVR then sees hands only if it uses hand tracking or the
   controller roles (`--hands-roles`).
7. Stop with `pimax vr stop` or `pimax desk stop` as usual. The next start without `--hands` uses the desk build
   again.

Offline check without the headset (does not touch a live session: its own runtime dir, null compositor, the pimax
USB driver disabled): `OUT=/tmp/pxht SECS=60 CLIENT=none analysis/tools/run_hands_offline.sh`. Keep `OUT` short,
because the IPC socket path must stay under 108 characters.
