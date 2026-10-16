# Voice state tests

The voice AI is measured by the **state it reaches**. Whether it picked the right intent name does not count.
The user wants state X and has state Y: which tools, in which order, get from Y to X, and does anything else change?

`pimax-voice audit --state` is the quality measure. The old `pimax-voice audit` (intent names,
tests/fixtures/*.tsv) is kept as a fast smoke test of the matcher only (section 5).

## 1. The World (src/world.rs)

`world::World` is the desk as state. It is serialisable (JSON) and holds:

| part | what |
|---|---|
| screens 1-6 | present (Screens setting), shown in VR, the workspace each shows, width, distance, place (yaw/pitch), curvature |
| show rule | `$XDG_RUNTIME_DIR/pimax-screens.state`: limit, pinned, hidden, sort |
| workspaces | id, and the screen (monitor) each lives on |
| apps | address, handle (t1, c1, ...), kind (terminal/browser/editor/video/pimax/other), class, title, cwd, workspace, fullscreen, browser tabs + active tab |
| focus, gaze | the focused app and screen; the screen and app the eyes rest on |
| view | passthrough (normal/dim/off), skybox, brightness, captions, fps, badges, controllers |
| media, rest | volume, mute, music, track; recording; screen locked; typed text and keys; events (reminder, screenshot) |
| session | running, mode, preset |

The **screen visibility** rule is the one in pimax-tools `vr/screens.rs::visibility`:
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
| `pimax desk screens show/more/add/hide` | screens.rs `parse_show` + `plan_show` (the focused screen counts as shown) |
| `pimax desk preset P` | preset.rs `live_steps`: show rule, passthrough, width/distance/curvature |
| `wayvrctl screen-nudge [--step S] NAME DIR` | WayVR grabmath `nudge` (step 0.02-1, default 0.25; a hidden screen does not move) |
| launchers, `wtype`, volume, media, `pimax record`, `pimax desk stop`, the passthrough socket | their effect on the state |

A command the Sim does not know is listed in `Sim::unmodelled`, and the case fails as a "simulator gap" rather than
passing by accident. These places are approximations and are marked `APPROX:` in world.rs:
- the screens watcher's re-sorting of apps by kind;
- directional focus across monitors;
- the screen places after a reset.

`world::apply(&w, &cat, tool, &inputs)` runs one tool through the real `Actor::execute` against a Sim, so it
matches the real code by construction. A planner's dry run folds `apply` over its steps.

## 3. Cases (tests/state/)

`worlds.toml` holds the starting desks in three tiers:
- **easy**: 1-3 screens, 1-2 apps;
- **medium**: 3-4 screens, mixed apps;
- **hard**: 6 screens, five terminals spread around, Chrome with 10 tabs, a fullscreen video, apps on workspaces
  no screen shows, a hidden screen, the gaze on an unrelated screen.

`goals-*.toml` holds one goal per entry:
- the end state as expectations (`expect`, or several acceptable ones in `any`);
- the starting world per tier;
- the phrasings: `live` as the speech-to-text really heard them, `says` written for the test.

A goal expands into cases:
- the first phrasing runs in every tier, the others in one tier each;
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
- new starting states: `fe2` (2 of 3 screens shown, kitty, music playing), `fm5` (5 screens, an editor, Spotify),
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
pimax-voice audit --state                       # every case, the pipeline as ai.toml sets it
pimax-voice audit --state --subset 40 --seed 3  # ~40 cases stratified by tier, under a minute
pimax-voice audit --state --split dev           # tune on dev only; confirm on --split held and --split fresh
pimax-voice audit --state --variant lara-off    # an experiment (tests/state/variants/*.toml or a file)
pimax-voice audit --state --no-ai               # matcher and rules only: checks the cases and the simulator
pimax-voice audit --state --snapshot            # the live World as JSON, read-only
```

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
- one row appended to `analysis/voice-experiments.csv`: variant, git, split, score overall/easy/medium/hard,
  dev/held, unsafe, simulator gaps, p50/p95 ms, estimated tokens, VRAM;
- `analysis/voice-state-audit.html`: the leaderboard of all runs, the failure causes, and per case the utterance,
  start state, expectation, what the AI chose, the commands, the end state, the verdict, and what the model was
  given (for failing cases).

## 5. Why the old audit said 99%

`pimax-voice audit` builds most of its cases from the catalogue itself:
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
- **Most of the gain is grammar, not the model**: with `--no-ai` the rules and the matcher alone reach 72%.
- The remaining unsafe cases: "this app" closes the app looked at (the documented rule) where two fresh cases
  expect the focused one; two held cases close a whole workspace after "window" words.
- Case fixes (not tuning): `hide-others-and-terminals-main` had "all stay" against "terminals on 1"; two live-1818
  goals had phrasings with different end states and were split.
