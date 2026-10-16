# Eye tracking in desk mode

The Dream Air's eye tracker runs in the Monado fork (`~/src/monado-eye`, branch `desk`, `PIMAX_EYE=1`) with the
host-side per-eye calibration in `~/.config/pimax/eye-calibration.toml` (v1: accuracy 3.08° mean, 0.54° precision).
Monado exposes `XR_EXT_eye_gaze_interaction`; WayVR (`~/src/wayvr`, branch `eye-gaze`) reads it.
Background: `analysis/eye-gaze-full.md`, `analysis/eye-precision.md`, `analysis/wayvr-gaze.md`.

## Eye tracking: always on, visual only (user decision 2026-10-16)

Eye tracking is on in every session when the eye build and `~/.config/pimax/eye-calibration.toml` exist. If it is not
calibrated it simply does not run (the app says "Not calibrated"); if it cannot start, VR keeps running without it.

**It is only a visual effect.** You see a small, dim dot where your eyes land on the WayVR screens (Work). The gaze is
never a pointer: it does not hover, click, select, scroll, move the mouse or target a window for voice. Nothing is drawn
in Play/SteamVR (no overlay there); Monado just runs the tracker (`PIMAX_EYE=1`) when the eye build is the one in use.

App: the Pimax app Home, row "Eye tracking (visual only)": the switch (saved, `--no-eye` in the start arguments when
off), a plain status line (Off / Running / Not calibrated / Not available / Not running) and "Show the gaze dot in Work"
(`dot=` in `~/.config/pimax/eye.conf`, default on). Developer mode adds "gaze may target windows for voice"
(`targeting=on`, default off: then `gaze_target: true` is written and `wayvrctl gaze` / `pimax-voice gaze` work as below)
and the old gaze pointer toggle (only with `pimax desk start --eye`).
In WayVR, `wayvrctl gaze` answers `tracking: false` unless `gaze_target` or `gaze_pointer` is on, so the dot alone never
feeds voice targeting.

The text below describes the developer path (targeting on).

### Modes (`pimax desk start`; `pimax vr start` also takes `--no-eye` and sets `PIMAX_EYE=1` the same way)

| flags | when | Monado | WayVR (`conf.d/55-eye.yaml`) |
|---|---|---|---|
| none | the eye build (`~/src/monado-eye/build`, the default Monado build when it is built) **and** the calibration file exist | `PIMAX_EYE=1` | `gaze_pointer: false`, `gaze_target: false` (true with `targeting=on`), `gaze_laser: true` (false with `dot=off`; dot, alpha 0.35) |
| none | otherwise | – | file removed; the start output says why |
| `--eye` | always (the old meaning) | `PIMAX_EYE=1`, eye build unless `--monado-build`/`PIMAX_MONADO_BUILD` | `gaze_target: true` only; the pointer stays as `20-gaze.yaml` / the app set it |
| `--no-eye` | always | – | file removed |

`55-eye.yaml` sorts after the app's `50-pimax-app.yaml`, so in silent mode it wins over the app's `gaze_pointer`.
WayVR ignores keys it does not know: an older WayVR binary just has no laser and no gaze API.
Nothing is sent to the headset. The device-side calibration (`PIMAX_EYE_CAL`) is never set.
Code: `rust/pimax-tools/src/vr/eye_desk.rs`.

### WayVR settings

| key | default | meaning |
|---|---|---|
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

### `pimax-voice gaze` and the Rust API

Code: `rust/pimax-voice/src/gaze.rs`. It is read-only and goes through the `act::Runner` trait.

```rust
use pimax_voice::gaze;
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

CLI: `pimax-voice gaze [--wayvrctl BIN] [--at UNIX_MS] [--input saved-gaze.json] [--json]`.

### Install and restart

Done by the user, from a stopped desk session:

1. Build WayVR:
   `cd ~/src/wayvr && CARGO_TARGET_DIR=~/src/target-desk-test cargo build --release -p wayvr -p wayvrctl --no-default-features --features openxr,osc,wayland,x11`.
   It is already built at `~/src/target-desk-test/release/`.
2. `pimax desk stop`, then copy the binaries:
   `mkdir -p ~/src/target-desk/release/backup && cp ~/src/target-desk/release/wayvr{,ctl} ~/src/target-desk/release/backup/`, then
   `cp ~/src/target-desk-test/release/wayvr{,ctl} ~/src/target-desk/release/`.
3. `cd ~/pimax-re/rust && cargo build --release -p pimax-tools -p pimax-voice`, then install as usual.
4. `pimax desk start` (with your usual flags, without `--eye`). The output says
   `eye tracking silently on: a dim gaze dot ...`.

### Live test checklist

1. The start output has the "silently on" line. `conf.d/55-eye.yaml` exists, and the WayVR log shows
   `gaze: N frames, located(orientation valid) N, accepted N ...` with non-zero counts.
2. **No cursor:** look around the screen. The mouse pointer does not move and nothing gets hover highlights.
3. **The dot:** a small, dim dot follows your eyes on the screens and panels. It disappears when you close your eyes
   and when you look into empty space.
4. `wayvrctl gaze-laser off`: the dot disappears. Then run `wayvrctl gaze-laser on`.
5. Look at a window for 1 s and run `~/src/target-desk/release/wayvrctl gaze --pretty`. `current.overlay` is
   `SUNSHINE`, `px` is inside that window, and `confidence` is 0.7 or more.
6. Look at a window, then look away, and within 2 s run `pimax-voice gaze`. The target is the first window
   (`last_stable`).
7. Check the edges: look at a window border and at the top bar. The target may be the neighbour, and `edge_px`
   is small.
8. Look at the tab strip of a browser: `tab` is `browser`, with an index guess. Look at the groupbar of a Hyprland
   group: `tab` is `group`, with the exact member.
9. Run `pimax desk start --no-eye`: no dot and `55-eye.yaml` is gone. `wayvrctl gaze` gives `"valid":false` and no
   `current`; `tracking` stays true only if a `gaze_pointer: true` from `20-gaze.yaml` is still in conf.d.
10. Run `pimax desk start --eye`: the old gaze pointer behaviour comes back, and `wayvrctl gaze` still works.
