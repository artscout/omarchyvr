# Voice state tests

The voice AI is measured by the **state it reaches**. Whether it picked the right intent name does not count.
The user wants state X and has state Y: which tools, in which order, get from Y to X, and does anything else change?

`omarchyvr-voice audit --state` is the quality measure. The old `omarchyvr-voice audit` (intent names,
tests/fixtures/*.tsv) is kept as a fast smoke test of the matcher only (section 5).

## 1. The World (src/world.rs)

`world::World` is the desk as state. It is serialisable (JSON) and holds:

| part | what |
|---|---|
| screens 1-6 | present (Screens setting), shown in VR, the workspace each shows, width, distance, place (yaw/pitch), curvature |
| show rule | `$XDG_RUNTIME_DIR/omarchyvr-screens.state`: limit, pinned, hidden, sort |
| workspaces | id, and the screen (monitor) each lives on |
| apps | address, handle (t1, c1, ...), kind (terminal/browser/editor/video/pimax/other), class, title, cwd, workspace, fullscreen, browser tabs + active tab |
| focus, gaze | the focused app and screen; the screen and app the eyes rest on |
| view | passthrough (normal/dim/off), skybox, brightness, captions, fps, badges, controllers |
| media, rest | volume, mute, music, track; recording; screen locked; typed text and keys; events (reminder, screenshot) |
| session | running, mode, preset |

The **screen visibility** rule is the one in omarchyvr-tools `vr/screens.rs::visibility`:
- the main screen always shows;
- another screen shows when it is pinned, busy (has an app) or focused, up to `limit` screens;
- the focused screen shows even over the limit;
- a hidden screen never shows.

`world::describe(&w, blocks)` is the compact text of the state for a prompt.

`world::snapshot(runner, "wayvrctl")` reads the live desk, read-only: `hyprctl -j monitors/clients/activewindow`,
`wayvrctl gaze` and the show-state file. Some things cannot be read live today:
- screen size and place (no wayvrctl command reports them);
- hidden browser tabs (voice only sees the active tab's title);
- the volume.

## 2. The simulator (world::Sim, world::apply)

`Sim` is an `act::Runner` over a World. It answers the reads the voice code makes from the World and applies its
writes to it. Each primitive mirrors its implementation:

| command | mirrors |
|---|---|
| `hyprctl dispatch hl.dsp.focus/window.move/close/fullscreen/float/send_key_state/...` | Hyprland: focusing a window shows its workspace on its monitor; moving to a new workspace creates it on the focused monitor; empty, unshown workspaces vanish |
| `omarchyvr desk screens show/more/add/hide` | screens.rs `parse_show` + `plan_show` (the focused screen counts as shown) |
| `omarchyvr desk preset P` | preset.rs `live_steps`: show rule, passthrough, width/distance/curvature |
| `wayvrctl screen-nudge [--step S] NAME DIR` | WayVR grabmath `nudge` (step 0.02-1, default 0.25; a hidden screen does not move) |
| launchers, `wtype`, volume, media, `omarchyvr record`, `omarchyvr desk stop`, the passthrough socket | their effect on the state |

A command the Sim does not know is listed in `Sim::unmodelled`, and the case fails as a "simulator gap" rather than
passing by accident. These places are approximations and are marked `APPROX:` in world.rs:
- the screens watcher's re-sorting of apps by kind;
- directional focus across monitors;
- the screen places after a reset.

`world::apply(&w, &cat, tool, &inputs)` runs one tool through the real `Actor::execute` against a Sim, so it
matches the real code by construction. A planner's dry run folds `apply` over its steps.

## 3. Cases (tests/state/)

`worlds.toml` holds the starting desks in two tiers (user decision 2026-10-18: no easy tier; a goal with an `easy`
key in `worlds` or `set` makes the loader fail with a clear error, so it cannot come back by accident):
- **medium**: 3-4 screens, mixed apps;
- **hard**: 6 screens, five terminals spread around, Chrome with 10 tabs, a fullscreen video, apps on workspaces
  no screen shows, a hidden screen, the gaze on an unrelated screen.

`goals-*.toml` holds one goal per entry:
- the end state as expectations (`expect`, or several acceptable ones in `any`);
- the starting world per tier;
- the phrasings: `live` as the speech-to-text really heard them, `says` written for the test.

A goal expands into cases:
- the first phrasing runs in every tier (medium and hard), the others in one tier each;
- one generated ASR-noisy copy of the first written phrasing is added (`statetest::noisy`: fillers, "closed" for
  close, "tree" for three, number words, "can you Max, can you ..." restarts);
- all live phrases on record are included: today's sessions, live-2026-10-16*.tsv and the ai.log traces.

482 cases from 100 goals, plus the 154 fresh cases (636 in all).

**Split.** The held-out split takes 4 of every 5 live cases and 1 of every 4 written ones (a stable hash of the
case id). Never tune on it.

**Fresh split.** `held` is no longer clean: the live-session fixes of 2026-10-16 were rules written for the very
phrases that sit in it. `goals-fresh.toml` (split `fresh`, 154 cases; plus `goals-live-1818.toml`, the live
session of 18:05-18:17, added before its fixes) was written on 2026-10-16 before any tuning
run looked at a failure and committed before any code changed:
- the same goals said differently: terse, polite and long, non-native ("please you close the screen of the left"),
  the wake word first, fillers;
- ASR-style errors of the kind seen in voice.log / ai.log: "write" for right, "to"/"for" for two/four, "of" for
  off, "brows her", "you tube", "lift", "tree";
- new starting states: `fm5` (5 screens, an editor, Spotify),
  `fh6` (6 screens, Chrome on the main screen with YouTube in a hidden tab, screen 3 hidden with its terminal, a
  windowed video, the room dim), `fcin` (cinema with a fullscreen film, the room off).

Every phrasing is one case, rotating over the goal's tiers; no noisy copies. Rules, aliases, prompt examples and
catalogue phrases are never written from these phrases. `--split fresh` runs only them; every run reports dev, held
and fresh (the CSV's last column).

**No test answers in the model's context** (user decision 2026-10-16). The prompts' examples are written fresh and
are disjoint from every test set; rules are general grammar. `leak::tests` fails when a test phrase (or a near copy,
word LCS >= 0.8) appears in a prompt, an example, a compound alias, the rules' code or a long catalogue phrase.

**Expectations.** Examples:
- `screen 2 hidden`, `shown 1,2,3`, `shown +2`, `shown =4`;
- `terminals on 3`, `chrome on 5`, `t1 stay`, `all stay`, `focused closed`, `together terminals`, `sorted`, `swapped 1 2`;
- `screen all size x0.5`, `screen gazed right`, `screen 1 front`;
- `focus 3`, `focus empty`, `tab youtube`, `alone youtube`, `tabs c1 -1`, `opened terminal on 3`, `playing dune`;
- `preset cinema`, `passthrough off`, `session stopped`, `volume up`, `music paused`, `typed hello`, `said chrome`, `nothing`.

**Score.** A case passes when all of these hold:
1. its expectations hold;
2. nothing else changed (focus changes are ignored; `allow` lists other exceptions);
3. the Sim modelled every command.

An unasked app close, session stop, screen lock or typing into a terminal counts as **unsafe**. When the pipeline
asks "say yes", the case answers yes unless it sets `yes = false`, so the end state shows what that yes would do.

## 4. Runs, variants, the leaderboard

```
omarchyvr-voice audit --state                       # every case, the pipeline as ai.toml sets it
omarchyvr-voice audit --state --subset 40 --seed 3  # ~40 cases stratified by tier, under a minute
omarchyvr-voice audit --state --split dev           # tune on dev only; confirm on --split held and --split fresh
omarchyvr-voice audit --state --variant lara-off    # an experiment (tests/state/variants/*.toml or a file)
omarchyvr-voice audit --state --no-ai               # matcher and rules only: checks the cases and the simulator
omarchyvr-voice audit --state --snapshot            # the live World as JSON, read-only
```

There is no rules-only scoring (the old `--no-ai` was removed on 2026-10-18): every audit runs the whole pipeline,
Lara and the AI. If the model server does not answer (or `voice_ai` is off), the audit stops with a message saying so
and never scores the rules alone. A cheap check without an AI is a unit test: `cargo test -p omarchyvr-voice statetest`
loads every goal file and parses every expectation, steps included (that is not a score).

Built-in variants: `baseline`, `lara-off`, `state-prompt` (for the planner), and one per local model:
`qwen35-0.8b` (port 8091), `qwen35-2b` (8092), `qwen35-4b` (8080, the user's server), `qwen35-9b` (8093) and
`qwen36-27b` (8094). Every model variant keeps Lara on and the rest at baseline. Each needs its own llama-server
on its port: the same runtime and flags as the 4B on 8080, alias `pimax-local`, started at nice 19, one at a time.

**Variants.** A variant (`variant::Variant`) sets:
- the model (endpoint, alias, temperature, grammar, thinking) and the goal model;
- Lara (on/off, url);
- the prompt (template, state blocks, target line);
- the tool scoping (all, category, category+hints, top-k);
- the run (subset, seed, split).

The runner applies the model, Lara and goal model settings. A planner reads the rest through `variant::active()`
and calls `variant::honour(knob)` for each knob it uses. The report names every knob that was set but never read.

**Output of each run:**
- one row appended to `analysis/voice-experiments.csv`: variant, git, split, score overall/medium/hard (the CSV keeps the retired `easy_retired` column, empty, so old rows line up),
  dev/held, unsafe, simulator gaps, p50/p95 ms, estimated tokens, VRAM;
- `analysis/voice-state-audit.html`: the leaderboard of all runs, the failure causes, and per case the utterance,
  start state, expectation, what the AI chose, the commands, the end state, the verdict, and what the model was
  given (for failing cases).

## 4b. Steps and in-between states

Start and end are not the whole story. A plan can do step 1 and break at step 2, move an app and move it back,
run half of a request that was then cancelled, or meet an app or screen that shows up late.
So the Sim keeps `history`: the world after every write command (reads and `wayvrctl gaze` are not writes).

**Steps.** A goal may say `steps = [["screen 2 hidden"], ["opened terminal on 1"]]`: a list of expectation lists
(same words as `expect`). Pass rule: there are worlds in the history, in this order (a world may hold several
steps), where the steps hold one after the other. Other changes do not matter for a step, only the end state
is checked for "nothing else changed". The report says which step never held: "step 2 of 2 never held: ...".

**Checks on every world on the way** (no goal change needed):
- Every "stay" expectation (`all stay`, `terminals stay`, `t1 stay`) holds in every world, not only at the end.
- A `nothing` goal: no world differs from the start. A cancel must mean that nothing ran, even for a moment.
- No app is placed on a screen that is not there.
- No app is closed on the way unless the goal closes it (closing and relaunching is unsafe).

**Slow or moving desk** (opt-in, per tier in a goal's `set`; the default is the instant desk):
- `launch_delay_reads = N`: a launched app shows in `hyprctl -j clients` after N more client reads.
- `screen_delay_reads = N`: a screen made shown or added shows in `hyprctl -j monitors` after N more reads.
- `drift = ["focus t2", "close s1", "gaze 3", "open chrome 2"]`: applied once, right after the first read the
  voice code makes (the desk changes while the AI thinks). Checks that compare with the start use the
  drifted start, so the drift is not blamed on the voice code. Add `allow` for what the drift changes. Since
  2026-10-18 (G) the end verdict does the same for apps closed or opened by hand: the unwanted-changes diff starts
  from the drifted desk (`check_from`), the expectations still read the real start ("s1 closed").
- `--only a,b,c` picks every case whose id, category or phrase holds any of them (a quick rerun of a failure list).

New failure buckets: "broke mid-way" (the end is right, a check on the way failed) and "step order / step
missing". The HTML report and the JSON file show `steps_ok` and the mid-way reasons; the leaderboard has three
more columns (`steps_pass`, `steps_n`, `midway`).
`--end-only` scores the old way (start and end only), to compare the two scorers on the same code.
The goals for this are in `tests/state/goals-steps.toml`.

## 5. Why the old audit said 99%

`omarchyvr-voice audit` builds most of its cases from the catalogue itself:
- each intent's own first phrase, which the exact matcher hits by construction;
- a one-word ASR swap of that phrase, which the alias table undoes;
- natural phrasings written next to the prompt examples (several are word for word in the prompt).

It scores only whether the intent name (or an equivalent id) matches and whether the slot values are contained in
the answer. A right name whose action failed still passes. The desk is either the live one or none, never a desk
with 4-6 screens. Nothing checks that other things stayed the same.

None of the live failures of 2026-10-16 could show up in it:
- "close screen 2" asked to close window 1;
- the terminals went to the wrong screen;
- "all screens half the size" changed one screen by one step.

The state audit reports the share of requests that reach the state the user wanted.

## 6. The loop of 2026-10-16 (Qwen3.5-4B on 8080, Lara on 8081)

Each try is a code change or a variant, kept only when dev improves, held and fresh do not drop, unsafe does not
rise and p95 stays under ~800 ms. Never tuned on held or fresh. Audits never run while a VR session runs (they
take the GPU): `pgrep -x monado-service` first, abort when a session starts.

| try | dev | held | fresh | unsafe | p50 / p95 ms | kept |
|---|---|---|---|---|---|---|
| baseline (HEAD b20337e + fresh set) | 85 | 87 | 70 | 13 | 18 / 604 | - |
| clauses, take-backs, openers | 88 | 87 | 72 | 13 | 18 / 603 | yes |
| leak cleanup (fresh prompt examples) | 88 | 86 | 72 | 11 | 17 / 562 | yes (user rule) |
| unhide target, limit raise-only, close-named, side-screen size, lost screens | 92 | 89 | 73 | 11 | 16 / 585 | yes |
| + close guard on fuzzy ask-back, screen-nudge rule | 95 | 90 | 76 | 6 | 15 / 588 | yes |
| + consent (a destructive action needs its verb), grounded close slot | 95 | 90 | 76 | 4 | 15 / 580 | yes |
| planner as committed (state-prompt) | 90 | 87 | 75 | 2 | 14 / 663 | no |
| planner + fallback to the single prompt | 93 | 88 | 77 | 4 | 15 / 869 | no |
| planner + fallback, no target line | 94 | 91 | 79 | 4 | 17 / 1030 | no (dev, p95) |
| Lara off | 95 | 90 | 76 | 4 | 15 / 630 | no (equal) |
| + side-named screens (hide, show back, move/put to), high=hide | 95 | 91 | 78 | 4 | 13 / 453 | yes |

- **The planner stays off.** It loses on dev (90-94 against 95) and is slower (p95 0.87-1.0 s): every category set
  has its own system prompt, which misses llama-server's one cached slot, and the fallback adds a second call.
  It wins on fresh (79 against 78) without the target line: worth another look once its prompt is one fixed text.
- **Lara changes nothing** on these cases (the same 566 pass with it off); it stays on (no loss, a later model may
  use it).
- **Most of the gain was grammar, not the model**: the rules and the matcher alone reached 72% (measured with the since removed `--no-ai`, 2026-10-16).
- The remaining unsafe cases: "this app" closes the app looked at (the documented rule) where two fresh cases
  expect the focused one; two held cases close a whole workspace after "window" words.
- Case fixes (not tuning): `hide-others-and-terminals-main` had "all stay" against "terminals on 1"; two live-1818
  goals had phrasings with different end states and were split.

## 7. Long goals (tests/state/goals-long.toml, 2026-10-18)

Medium and hard only, 27 goals (`lg-` ids, cat `long`). Each is one spoken command of 3 to 9 tool calls,
the way people talk in VR: lists ("terminals on 4, chrome on 2 fullscreen, hide screen 5, volume down"), a whole
work setup, cleanups ("close the btop windows and put the rest together on 3"), corrections in the middle ("chrome on 2,
no, on 3"), take-backs ("hide 4, no wait, keep it"), "this" under a drifting gaze. The expected tool calls are in a
comment above each goal.

New worlds (worlds.toml, end): `ml5` and `ml4` (medium, 4 screens, 5 terminals / btop and htop windows), `hl7` (hard:
6 screens, 7 terminals, Chrome with 6 tabs, code, Spotify under the gaze on screen 4, screen 3 hidden, Obsidian and a
terminal off-screen) and `hl6` (hard: screen 5 hidden, a crowded screen 6). Handles are shared across the tiers of a
goal so one expectation list fits both (t3/t4 btop, t5 htop, s1 Spotify, c1 Chrome).

Twists: slow launch (`launch_delay_reads`), late screens (`screen_delay_reads`), the desk moving meanwhile (`drift`:
gaze, a closed Spotify). Safety is built in: `count` and `stay` checks catch duplicates and lost apps, the in-between
checks catch an app closed that was not named, and a hidden screen must keep its apps open.

Registering: `goals-long.toml` is read through the `GOALS` list in `src/statetest.rs` (`("long", include_str!(...))`).

Goals that only had an easy world (`f-type`, `f-reminder` in goals-fresh.toml) got the medium world `fm5`; the easy
worlds (`e1`, `e1c`, `e3`, `e3c`, `e4`, `fe2`) were deleted. `h6` stays (it is the `base` of `h6b` and `h6c`).
