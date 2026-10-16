# Controllers only when picked up (PIMAX_CTRL_INHAND)

In Work (the WayVR desk) the controllers often lie on the desk. The optical tracker (PIMAX_CTRL_TRACK) then loses
them or misreads them, and their models and rays jump around the scene. The user's request (2026-10-16): "disable the
controllers unless we pick them up, in work vr".

**The switch:** Home page, Controllers card. It has two rows:

| Row | Options | Default |
|---|---|---|
| In Work | When picked up / Always / Off | When picked up |
| In games | When picked up / Always / Off | Always (games expect controllers) |

The choice is saved in the app state (`controllers_work`, `controllers_play`). The next start gets it as
`omarchyvr desk start --controllers when-held|always|off` (for Play: `omarchyvr vr start --controllers ...`). The flag goes
into `start-argv.txt`, so restarts keep it. A flag that is absent means the default of that side.

While VR runs, each controller shows `in hand`, `put down` or `not in use` next to its battery. Changing the switch
during a session also changes the running session.

Voice: "Max, controllers on" (always) and "Max, controllers off". They last until the session ends.

CLI (the engine behind the app):
`omarchyvr controllers use when-held|always|off` (running session) and `omarchyvr controllers status`.

## How it works

The work is done in the Monado Pimax driver (`pimax_ctrl.c`), so every OpenXR app benefits. The decision itself is
made in Rust, in `rust/pimax-hands/src/inhand.rs` (C ABI `include/pimax_inhand.h`).

Monado gets `PIMAX_CTRL_INHAND=auto|always|off`. The driver's own default is `always`, so anything that does not pass
the variable behaves as before. While a controller is "down" in mode auto (or always in mode off), the driver:

* reports no pose (relation flags 0) for grip and aim;
* marks every input inactive (buttons, trigger, stick, poses);
* reports hand tracking from that controller as inactive.

WayVR already hides the pointer ray and the 3D model when the pose is not valid (`pointer.tracked = false`). With
both controllers down, the head/eye pointer takes over as usual.

Status file `$XDG_RUNTIME_DIR/pimax-ctrl-inhand` (pid, mode, `left` / `right` = held | down | shown | off, plus the
reason). It is read by:

* the app (Home card);
* `pimax-ctrltrack live`: a controller that is down is neither tracked nor searched, and fixes assigned to it are
  dropped. With both controllers down, the LED frames are skipped completely, which saves CPU. The tracker resumes
  within 100 ms of a pick-up.

Run-time change: the file `$XDG_RUNTIME_DIR/pimax-ctrl-inhand-mode` (`auto` / `always` / `off`). The driver's attach
thread takes it within 100 ms and deletes it.

Log lines in monado.log: `PIMAX_CTRL_INHAND=auto ...` at start, then
`PIMAX_CTRL_INHAND: left controller shown (held (motion), mode auto)` and similar lines for every change.

## Detection, tuned on the recording 2026-10-16-1818

The data is the left controller's IMU log of that session: 518 s, about 490 samples/s, gyro 0.122 deg/s per LSB,
accel about 1024 LSB/g. The right controller sent no reports at all in that session. Its "fixes" were misreads, and
the no-report rule now hides them.

The IMU samples are averaged over 20 ms blocks, and the block means are compared over 0.5 s windows. A window is
**still** when, on every gyro and accel axis, the standard deviation of the block means is under 2.5 LSB.

Stillness in the recording:

| Where | Gyro std (LSB) | Accel std (LSB) | Still windows |
|---|---|---|---|
| On the desk | p50 0.74, p95 2.6 | p50 0.54, p95 1.4 | 94 % |
| In the hand, even quiet | p5 2.6, p50 8.4 | p5 3.0, p50 6.2 | 2 % |

The rules (each one tested on the recording):

* **Put down:** 8 of the last 10 windows (5 s) still, none of them with real motion (gyro std of 8 LSB or more), and
  no input change for 5 s. In the hand, at most 4 of 10 windows were ever still. It takes about 5.5 to 6.5 s after
  the controller lands.
* **Picked up**, any one of:
  * gyro (block mean minus the bias learned at rest) above 30 LSB (3.7 deg/s) on some axis, for 6 blocks in a row
    (120 ms);
  * tilted more than 10 deg from where it lay, for 3 blocks (60 ms);
  * any input *change* (button, trigger, grip, stick, capacitive touch).
  On the desk the biggest block was 28 LSB, for a single block (a knock at 70.7 s), and the tilt never passed 1.7 deg.
  In the recording the pick-up was detected 130 ms and 220 ms after the first motion (the second one was a slow lift).
* **No reports for 1.5 s** (switched off, asleep): down.
* Inputs count only when they change. A controller lying on its trigger stays down.
* `PIMAX_CTRL_INHAND_DOWN_S` (1 to 60, default 5) changes the stillness time.

Typing: no typing was recorded with the controllers on the desk. A synthetic test adds keystroke knocks to the desk
recording (6 keys/s, 10 ms bursts of up to 15 LSB gyro and 40 LSB accel), and the controller stays down. To check it
for real, type with the controllers on the desk in a Work session and read the log lines above.

Fixtures: `rust/fixtures/inhand_{desk,desk_bump,pickup,held,putdown}.csv.gz`, cut by
`rust/fixtures/make_inhand_fixtures.py`. Tests: `rust/pimax-hands/tests/inhand_fixtures.rs`.

## The yellow hand over the controllers

WayVR drew a hand skeleton (amber strokes, `hands.rs`) from `XR_EXT_hand_tracking`. In Work the only hands Monado
reports are the ones it makes up from the controllers (`pimax_ctl_get_hand_tracking`: trigger/grip, data source
CONTROLLER), so an amber hand sat on every controller model. WayVR now draws camera (unobstructed data source) hands
only. Without `XR_EXT_hand_tracking_data_source` it draws no hands. The controller-driven hand joints are still
offered to games that want them.

## Test in the headset

1. Start Work. Leave both controllers on the desk. After about 6 s they should disappear (no model, no ray, no amber
   hand), and the Home card should show `put down` (or `not in use` for one that is off).
2. Type for a minute with the controllers next to the keyboard. They should stay hidden.
3. Pick one up. It should appear within a fraction of a second, ray and model in the hand. Touch a button on the
   other one without lifting it: it should appear too.
4. Hold one still in the hand, pointing at a screen, for 10 s. It should never disappear.
5. Say "Max, controllers on". Both should show even on the desk. "Max, controllers off" hides them. Set In Work back
   to When picked up in the app.
6. Play (a game): the controllers should always show (In games = Always).
