# Eye tracking in desk mode

The Dream Air's eye tracker runs in the Monado fork (`forks/monado`, branch `desk`, `PIMAX_EYE=1`) with the
host-side per-eye calibration in `~/.config/omarchyvr/eye-calibration.toml` (v1: accuracy 3.08° mean, 0.54° precision).
Monado exposes `XR_EXT_eye_gaze_interaction`; WayVR (`~/src/wayvr`, branch `eye-gaze`) reads it.
Background: `analysis/eye-gaze-full.md`, `analysis/eye-precision.md`, `analysis/wayvr-gaze.md`.

## The base start list and the built-in test calibration (user decision 2026-10-17)

So that other Dream Air owners can try eye tracking, the public tree ships a **base start list**:
`rust/pimax-eye/data/base-frames.txt` (compiled into `pimax-eye`, so every component has it).

**What it is.** The Windows cold start of the eye chip as 24 steps (descriptor/enumeration step 1 is never sent), byte
for byte as captured: hello (0x3e8), two reads (0x640 ...), the platform config (0x852), the post-auth acknowledgement
(0x77b), the read-only gets (0x532, 0x58c, 0x546, 0x547, 0x5d2, 0x5cd, 0x4b0, 0x596, 0x578, 0x686), the 120 Hz and lens
position setters (0x67c, 0x5be), the rate read (0x672), the gaze subscribe (0x4c4) and the calibration upload (0x456).
Four vendor control transfers (bReq 48 write and read, 70 read, 65 write) are in it as well; the live path leaves the bReq 48 write out.

**What it leaves out.** The two authentication steps (0x76c request challenge, 0x777 answer). The answer belongs to a
live challenge and cannot be replayed, so the list does not carry it. Lab result 2026-10-14: the same start without
the auth pair gave 119 Hz gaze on both eyes (the chip may flag the platform config 0x852 as rejected without auth; gaze does not need it).
Nothing that identifies a unit is in the list (no serial).

**The calibration is the developer's.** Step 26 (0x456, 34 KB) is the developer's own eye calibration profile, shipped as
the default "for people to test". It was made on one person's eyes and **tested on one headset**: expect a rough aim
until you calibrate your own. The Guard still pins that blob by length and FNV-1a hash and refuses every other 0x456 and
the calibration opcodes (0x3f2, 0x407, 0x42e, 0x3fc, 0x44c).

**Which list.** The eye start uses the base list. A developer build that linked the private full list
(`tools/lab-data.sh link`) uses that one (`pimax_eye::frames::list()`); the Eyes card says which.

**Which calibration (the switch).** `calibration=` in `~/.config/omarchyvr/eye.conf`, the app's "Built-in test
calibration" switch (Tracking page, Eyes card):

| value | meaning |
|---|---|
| `auto` (default) | your own `eye-calibration.toml` once you have calibrated, until then the built-in one |
| `builtin` | always the built-in one: the start list sends the blob (`PIMAX_EYE_CAL=1`) and no host-side file is applied (`PIMAX_EYE_CALIBRATION=none`) |
| `own` | always your own: nothing extra is sent, the host-side calibration is applied; no file means eye tracking does not run |

The Eyes card says in plain words which list and which calibration are in use, and that the built-in one is someone
else's eyes. Run the app's eye calibration step to make your own; after that it is used automatically.

**Leak scanners.** `tools/leak-scan.py` (pre-commit, pre-push) blocks eye-chip bytes everywhere except this one file,
identified by path **and** SHA-256 (`EYE_BASE_*` in the script). A changed list needs a reviewed new hash. The private
full list and the USB transcripts stay refused. `tools/leak-scan-test.sh` checks both.

## Eye tracking: on, and only a visual point (user decisions 2026-10-16 and 2026-10-17)

Eye tracking is on in every session when the eye build and a calibration exist. If it is not calibrated it simply
does not run (the app says "Not calibrated"); if it cannot start, VR keeps running without it.

**By default the gaze is only a visual point.** You see a small, dim dot where your eyes land on the WayVR screens
(Work). The gaze is never a mouse: it does not hover, click, focus a window, move the cursor, scroll, select, or pick
the window for voice ("this"). Nothing is drawn in Play/SteamVR (no overlay there); Monado just runs the tracker
(`PIMAX_EYE=1`) when the eye build is the one in use.

User, 2026-10-17: "Our eyes should not trigger the click. Only eyes POINT by default, not eyes = mouse in Work."
Then: "Mouse is not gaze, never make it a mouse, it's just a point", and finally: the user may turn gaze-as-mouse on
with ONE explicit option, default off.

**The one switch.** App: Tracking page, Eyes card, **"Eyes as a mouse"** (off by default; help text "Your eyes move
the pointer and can click."). Saved as `gaze_mouse=on|off` in `~/.config/omarchyvr/eye.conf`; it applies at the next
Work start. With it on, the eyes move WayVR's pointer, a controller click that is not aimed at a screen clicks where
you look, and voice "this" may use the window you look at. There is no dwell click (`gaze_dwell_ms: 0` always).
Every other gaze-input switch is gone: the WayVR page's developer "Gaze pointer" and the Tracking page's developer
"Gaze targets windows".

App, Home and Tracking: "Eye tracking" (saved, `--no-eye` in the start arguments when off), a plain status line
(Off / Running / Not calibrated / Not available / Not running) and "Gaze dot" (`dot=` in `eye.conf`, default on).

### Why the gaze clicked before (2026-10-17)
- `rust/omarchyvr-tools/src/vr/wayvr_conf.rs:109`: the app's WayVR settings had **`gaze_pointer: true` as the code
  default** (since the first public snapshot `f375aca`, the same in the lab). The WayVR page showed it as a switch that
  was ON ("Gaze pointer", developer mode), and every `wayvr-desk.conf` the app saved carried `gaze_pointer = true`,
  which `desk start` wrote to `conf.d/50-pimax-app.yaml`.
- The 2026-10-16 "visual only" fix did not change that default. It only **overrode** it: `conf.d/55-eye.yaml` (sorts
  after 50) wrote `gaze_pointer: false`, and only in the silent eye mode. With `--eye` (the old pointer mode, still
  passed by the app for an older launcher and by the Play eye switch) or with no 55 file, the 50 file's
  `gaze_pointer: true` (and a hand-made `20-gaze.yaml` in the old lab WayVR home: `gaze_pointer: true`,
  `gaze_over_controllers: true`) reached WayVR, and the eyes became the mouse: the gaze hovered, and a controller
  trigger clicked where you looked.
- WayVR itself also offers eye pointers of its own: the handsfree modes "Eye tracking" / "Eye only" (dashboard
  setting, keyboard bar, `wayvrctl handsfree`), saved by its dashboard after our conf.d files.

The clean reinstall did not cause it (it resets `wayvr-desk.conf`, so the next save wrote the bad default again).

### How it is closed now
- **One gate in WayVR** (`forks/wayvr`, `wayvr/src/backend/gaze.rs`, `mod gate`): every gaze input path asks
  `gaze_pointer`: hover/click through a free controller slot (`may_point`), gaze over the controllers
  (`over_controllers`), the dwell click (`dwell_ms` = 0 while off), the targets `wayvrctl gaze` reports for voice
  (`reporting`), and WayVR's own eye handsfree modes (fall back to the head pointer of the same kind). Test
  `switch_off_no_gaze_path_can_point_click_or_focus`.
- **One writer in omarchyvr**: `conf.d/55-eye.yaml` is written at EVERY Work start (also with eye tracking off) and is
  the only file that sets `gaze_pointer`, `gaze_target`, `gaze_over_controllers` and `gaze_dwell_ms`; it sorts after
  `20-gaze.yaml` and `50-pimax-app.yaml`. All of them are false/0 unless eye tracking runs AND `gaze_mouse=on`.
  `50-pimax-app.yaml` no longer has a `gaze_pointer` key.
- **Migration** (how a saved setting is decided): a saved `gaze_pointer = true` in `wayvr-desk.conf` and the old
  `targeting=on` in `eye.conf` are **ignored** (read without a warning, never written again). The old app wrote
  `gaze_pointer = true` into every saved file because it was the code default, so a saved value cannot be told apart
  from "the user wanted it"; and the user said it was meant to be off already. Only the new key `gaze_mouse`, written
  by the new switch, turns the gaze mouse on. Tests: `old_targeting_key_migrates_to_off`,
  `a_saved_gaze_pointer_is_migrated_away`, `conf_files_have_unique_keys_and_no_gaze_input_by_default`.

The text below describes the developer details.

### Modes (`omarchyvr desk start`; `omarchyvr vr start` also takes `--no-eye` and sets `PIMAX_EYE=1` the same way)

| flags | when | Monado | WayVR (`conf.d/55-eye.yaml`, always written) |
|---|---|---|---|
| none | the eye build (`~/.local/lib/omarchyvr/monado`) **and** a calibration exist | `PIMAX_EYE=1` | `gaze_laser: true` (false with `dot=off`); gaze input keys = `gaze_mouse` |
| none | otherwise | – | every gaze key false; the start output says why |
| `--eye` | always (forces eye tracking on) | `PIMAX_EYE=1`, eye build unless `--monado-build`/`PIMAX_MONADO_BUILD` | the same as the first row (before 2026-10-17 it meant "gaze pointer") |
| `--no-eye` | always | – | every gaze key false |

WayVR ignores keys it does not know: an older WayVR binary just has no laser and no gaze API.
Nothing is sent to the headset. Code: `rust/omarchyvr-tools/src/vr/eye_desk.rs`.

### WayVR settings

| key | default | meaning |
|---|---|---|
| `gaze_pointer` | false | **the one gate for gaze input** (`backend::gaze::gate`): the eyes move the pointer and a controller click that is not aimed at a screen clicks where you look. While false no gaze path hovers, clicks, focuses, moves the cursor or reports a target, `gaze_over_controllers` and `gaze_dwell_ms` do nothing, and WayVR's handsfree eye modes fall back to the head pointer. omarchyvr writes it from `gaze_mouse` |
| `gaze_over_controllers`, `gaze_dwell_ms` | false, 0 | only with `gaze_pointer`; omarchyvr writes `gaze_over_controllers = gaze_mouse` and never a dwell click |
| `gaze_target` | false | silent tracking: the filtered gaze ray is cast against the overlays every frame, with no hover, click or mouse events |
| `gaze_laser` | false | a marker where the eyes land (turns tracking on). Shown only on a fresh, valid sample that hits an overlay, so it is hidden during blinks and invalid frames |
| `gaze_laser_style` | `dot` | `dot`: a square of about 0.6° (1–4 cm) facing you, 5 mm in front of the hit. `ray`: the last 25 cm of a 1.5 mm line from below the chin to the hit |
| `gaze_laser_alpha` | 0.35 | opacity of the marker (cool white, premultiplied) |
| `gaze_fixation_ms` | 200 | shortest fixation that counts as "looked at" |
| `gaze_smoothing`, `gaze_deadzone_deg` | 0.5, 0.6 | the shared One Euro filter and fixation dead zone (also used by the pointer) |

At run time: `wayvrctl gaze-laser on|off|toggle` switches the marker. It is not saved, and turning it off does not stop
`gaze_target`.

### Fixations

Code: `wayvr/src/backend/gaze_target.rs` (unit tested).

* A fixation is a run of hits on the **same overlay**. It stays inside a 2.0° cone around its running mean direction.
* A blink (invalid frames under 300 ms) does not end it. A longer loss, looking at nothing or a saccade does.
* `confidence` (0–1) is the share of valid frames × (1 − ½·spread/2°) × (½ + ½·min(1, duration / 2·min)).
* Finished fixations of at least `gaze_fixation_ms` are kept for 10 s (32 at most).
* `last_stable` is the current fixation if it is long enough. Otherwise it is the newest kept one that ended in the
  last 2 s.

### `wayvrctl gaze` (JSON)

```json
{"ts_ms":1760000000500,"tracking":true,"laser":true,"valid":true,"min_fixation_ms":200,
 "current":{"overlay":"SUNSHINE","uv":[0.25,0.5],"px":[480.0,540.0],"start_ms":1760000000000,
            "end_ms":1760000000490,"duration_ms":490,"confidence":0.93,"dispersion_deg":0.4,"dist_m":1.3,"samples":44},
 "last_stable":{ ...same fields... } | null,
 "history":[ ...fixations, oldest first... ]}
```

* Times are Unix epoch milliseconds.
* `uv` has its origin at the top left.
* `px` is the desktop (Hyprland logical) pixel. Screen overlays fill it from the same transform the mouse uses; other
  overlays (watch, panels) give `null`.
* `valid` is false while there is no fresh gaze sample (a blink, or lost tracking).
* The command fails (exit code 1) if WayVR does not know the request; an older WayVR then drops the connection.

### `omarchyvr-voice gaze` and the Rust API

Code: `rust/omarchyvr-voice/src/gaze.rs`. It is read-only and goes through the `act::Runner` trait.

```rust
use omarchyvr_voice::gaze;
// live: wayvrctl gaze + hyprctl -j monitors + hyprctl -j clients
let t: Option<gaze::GazeTarget> = gaze::resolve(&mut runner, wayvrctl, Some(utterance_start_unix_ms))?;
// offline: gaze::resolve_from(&report, &monitors, &windows, at_ms)
if let Some(t) = t {
    let w = t.window.as_ref();      // gaze::Window { address, class, title, workspace, monitor, at, size, floating, grouped, .. }
    let p = t.picked;               // fixation + source ("current" | "last_stable" | "history") + age_ms
    let tab = t.tab;                // Some(TabGuess { kind: "group" | "browser", index, address, x_frac })
    let edge = t.edge_px;           // distance of the point to the window border
}
```

* **Picking the fixation:**
  * With `at_ms` (when the utterance started), it is the fixation of at least the minimum length that covers that
    time. Failing that, it is the newest one that ended at most 2 s before it, and failing that, the current one.
  * Without `at_ms`, it is `last_stable` if it is at most 2.5 s old.
  * `Ok(None)` means nothing was looked at long enough. `Err` means tracking is off or a tool failed.
* **UV → pixel:** WayVR's `px` is used. Failing that, the UV is mapped onto the Hyprland monitor that has the overlay's
  name (`SUNSHINE`, 1920×1080). Scale and 90°/270° transforms are handled.
* **Pixel → window:** only visible windows count. Hyprland's `visible` flag is used when present; otherwise the active
  or special workspace of a monitor, or pinned. Stacking order is: special workspace, then fullscreen, then floating
  or pinned, then tiled. Within a tier, the most recently focused window wins.
* **Tabs:** this is best effort.
  * In a Hyprland **group**, the groupbar (24 px above the window) maps exactly to the member under the point.
  * In a **browser** (Chromium or Firefox family), the top 46 px of the window are taken as the tab strip.
  * The browser `index` assumes tabs at their widest (240 px), which only holds with a few tabs. With a known tab
    count N, use `floor(x_frac·N)`. Hyprland knows nothing about browser tabs.
* **Limits:**
  * The calibration error is about 3°. At 1.2 m that is about 6.5 cm (`error_m`), or 50–80 px on a 2 m wide
    1920 px screen.
  * Small targets and points within about 60 px of a border (`edge_px`) are ambiguous. Ask, or prefer the bigger
    window.
  * Only the screen overlays map to windows.

CLI: `omarchyvr-voice gaze [--wayvrctl BIN] [--at UNIX_MS] [--input saved-gaze.json] [--json]`.

### Install and restart

Done by the user, from a stopped desk session:

1. Build WayVR:
   `cd ~/src/wayvr && CARGO_TARGET_DIR=$WAYVR_TARGET cargo build --release -p wayvr -p wayvrctl --no-default-features --features openxr,osc,wayland,x11`.
   It is already built at `$WAYVR_TARGET/release/`.
2. `omarchyvr desk stop`, then copy the binaries:
   `mkdir -p ~/.local/lib/omarchyvr/wayvr/backup && cp ~/.local/lib/omarchyvr/wayvr/wayvr{,ctl} ~/.local/lib/omarchyvr/wayvr/backup/`, then
   `cp $WAYVR_TARGET/release/wayvr{,ctl} ~/.local/lib/omarchyvr/wayvr/`.
3. `cd ~/omarchyvr/rust && cargo build --release -p omarchyvr-tools -p omarchyvr-voice`, then install as usual.
4. `omarchyvr desk start` (with your usual flags, without `--eye`). The output says
   `eye tracking on, visual only: a dim gaze dot ...`.

### Live test checklist

0. **Default:** "Eyes as a mouse" is off in the app (Tracking > Eyes). `conf.d/55-eye.yaml` has `gaze_pointer: false`,
   `gaze_target: false`, `gaze_over_controllers: false`, `gaze_dwell_ms: 0`; `50-pimax-app.yaml` has no gaze key.
1. The start output has the "visual only" line. `conf.d/55-eye.yaml` exists, and the WayVR log shows
   `gaze: N frames, located(orientation valid) N, accepted N ...` with non-zero counts.
2. **No cursor:** look around the screen. The mouse pointer does not move and nothing gets hover highlights.
3. **The dot:** a small, dim dot follows your eyes on the screens and panels. It disappears when you close your eyes
   and when you look into empty space.
4. `wayvrctl gaze-laser off`: the dot disappears. Then run `wayvrctl gaze-laser on`.
Steps 5-8 need "Eyes as a mouse" on (otherwise `wayvrctl gaze` answers `tracking: false` on purpose).

5. Look at a window for 1 s and run `~/.local/lib/omarchyvr/wayvr/wayvrctl gaze --pretty`. `current.overlay` is
   `SUNSHINE`, `px` is inside that window, and `confidence` is 0.7 or more.
6. Look at a window, then look away, and within 2 s run `omarchyvr-voice gaze`. The target is the first window
   (`last_stable`).
7. Check the edges: look at a window border and at the top bar. The target may be the neighbour, and `edge_px`
   is small.
8. Look at the tab strip of a browser: `tab` is `browser`, with an index guess. Look at the groupbar of a Hyprland
   group: `tab` is `group`, with the exact member.
9. Run `omarchyvr desk start --no-eye`: no dot; `55-eye.yaml` sets every gaze key false (even with an old
   `20-gaze.yaml` in conf.d). `wayvrctl gaze` gives `"tracking":false`.
10. Run `omarchyvr desk start --eye`: eye tracking on, still only the dot (no pointer) unless "Eyes as a mouse" is on.

## Learning from clicks, hold-to-refine and the quick check (2026-10-18)

WayVR can learn a small correction of the gaze from your clicks, so the gaze point lands closer to where you really
look. The method is ported from Frametop (MIT, github.com/Frametop/frametop, `gaze/gazecal.py` `LiveCorrection`);
any accuracy numbers Frametop publishes are theirs, not ours.

**Safety rule (unchanged): by default the eyes never click.** Learning only moves the gaze point WayVR shows (the
dot); it never makes input. Lessons also come from clicks of a controller laser (passive: they only teach). The eyes
click only with the one switch "Eyes as a mouse" (`gaze_mouse`).

| what | app control | where it is saved | WayVR key (written by omarchyvr) |
|---|---|---|---|
| Learn from my clicks | Tracking > Eyes > "Learn from my clicks" (default on) | `learn=on\|off` in `eye.conf` | `gaze_learn`: `learn` AND eye tracking on |
| Hold-to-refine gaze clicks | part of "Eyes as a mouse", no switch of its own | `gaze_mouse` in `eye.conf` | `gaze_refine` = `gaze_mouse` (WayVR: effective only with `gaze_pointer`) |
| Calibration fingerprint | automatic | the user's `eye-calibration.toml` | `gaze_learn_key`: first 12 hex of a 64-bit FNV-1a hash of the file, `""` without a file; lessons with another key are discarded |
| Steady clicks | Tracking > Controllers and Desktop > "Steady clicks" (default on) | `click_steady_px` in `wayvr-desk.conf` | `click_steady_px: 32` (0 = off) in `conf.d/50-pimax-app.yaml` |

`conf.d/55-eye.yaml` stays the ONE writer of the gaze keys (`rust/omarchyvr-tools/src/vr/eye_desk.rs`), rewritten at
every start; with eye tracking off, or `gaze_mouse` off, `gaze_pointer`, `gaze_target`, `gaze_over_controllers` and
`gaze_refine` are false in every mode. Changes take effect at the next Work start.

**Steady clicks:** a click press holds its point until the laser has moved more than 32 px, so a shaky click does not
turn into a drag.

**Quick eye check** (Tracking > Eyes, Work must run): a dot appears 1.5 m ahead in the headset and a toast says "Look
at the dot in front of you" (the same in-headset toast as the Exit Work prompt; there is no speech output installed).
WayVR waits up to 8 s for a steady look and answers how far off the gaze was before and after the learned correction
("Your eyes were 2.1 deg off; now 0.4 deg") or why it could not ("no gaze", "not steady", "timed out", "learning off").
**Forget what it learned** clears the lessons.

CLI (the engine behind the app, for agents and tests): `omarchyvr eye learn status [--json]`,
`omarchyvr eye learn forget`, `omarchyvr eye check [--json]`; they call `wayvrctl gaze-learn status --json`,
`wayvrctl gaze-learn forget` and `wayvrctl gaze-check --json`.

Files: lessons in `$XDG_STATE_HOME/omarchyvr/gaze-lessons.json` (fallback `~/.local/state/omarchyvr/`); settings in
`~/.config/omarchyvr/eye.conf` and `wayvr-desk.conf`.
