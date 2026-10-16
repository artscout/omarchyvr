# Pimax AI: "Max ..." with a model and tools

Voice control has an AI agent. You say "Max, ..." and the agent reads the request together with what you are looking at,
your windows and the VR session. Then it acts through tools, under the same safety rules as the spoken command
catalogue. The model can run on this machine (`omarchyvr-ai`, llama.cpp) or be Claude or OpenAI.

- Code: `rust/omarchyvr-ai` (install / serve / status / stop of the local model) and `rust/omarchyvr-voice/src/agent.rs` (the
  agent).
- Settings: `~/.config/omarchyvr/ai.toml`, written by the Pimax app's **AI tab** (`omarchyvr_config::ai`).
- Log: `~/.local/state/omarchyvr/logs/ai.log`.

## 1. The local model

**Recommended while VR runs: `omarchyvr-ai serve --cpu`** (the 4B on 24 CPU threads at nice 19; section 11 has the
measurements). GPU mode is ten times faster but drops about one VR frame per command.

| | `serve --cpu` (recommended in VR) | `serve` (GPU) |
|---|---|---|
| Model | Qwen3.5-4B, Q4_K_M GGUF (unsloth), 2.6 GiB, in RAM | the same, in VRAM |
| Why | Small, current, follows the one-line answer format well; thinking off answers quickly | |
| Runtime | llama.cpp `llama-server` b11630, the official Linux x64 **Vulkan** build, with `--device none`, 3/4 of the CPU threads (24 of 32), nice 19 | the same binary, all layers on the GPU (29 MiB; runs on the NVIDIA Vulkan driver, no CUDA runtime download) |
| VRAM | none | about 3.5 GiB, **budget 4 GiB** (weights 2.6 GiB, 8k context, compute buffers) |
| Latency per command (model) | p50 1.27 s, p95 1.62 s with VR running and the machine busy | about 0.15 s once warm; the first request after a start takes ~10 s (Vulkan pipeline set-up) |
| VR impact | none measured (no skipped frames) | about 1 skipped compositor frame per command |

The 2B (`--model .../Qwen3.5-2B-Q4_K_M.gguf`, 1.2 GiB) answers in ~0.65 s on the CPU but picked a wrong intent for
~20 % of the misheard commands of the bench, so it is no longer the CPU default.

Sources (pinned; each SHA-256 is checked):
- runtime: https://github.com/ggml-org/llama.cpp/releases/tag/b11630, `llama-b11630-bin-ubuntu-vulkan-x64.tar.gz`
  (the GitHub asset digest);
- models: https://huggingface.co/unsloth/Qwen3.5-4B-GGUF and https://huggingface.co/unsloth/Qwen3.5-2B-GGUF, pinned
  revisions (the LFS SHA-256).

A CPU-only runtime (`--runtime cpu`, 17 MiB) is available for machines without Vulkan.

```sh
cd ~/pimax-re/rust && cargo build --release -p omarchyvr-ai
target/release/omarchyvr-ai install            # 4.1 GB into ~/.local/share/omarchyvr/ai; resumes; run again after a failure
target/release/omarchyvr-ai install --dry-run  # what it would fetch
target/release/omarchyvr-ai serve              # GPU, all layers, 127.0.0.1:8080, waits until loaded
target/release/omarchyvr-ai serve --cpu        # no GPU at all (the 4B, 3/4 of the CPU threads): the VR default
target/release/omarchyvr-ai serve --gpu-layers 20   # part on the GPU
target/release/omarchyvr-ai status [--json]
target/release/omarchyvr-ai test               # one tiny request, prints the latency
target/release/omarchyvr-ai stop
```

The server:
- listens on 127.0.0.1 only;
- runs under `nice -n 19`, `ionice -c 2 -n 7` (lowest best-effort I/O; the idle class held a model load for minutes under steady disk load, 2026-10-16) and `--prio -1`, with one slot and an 8k context;
- uses `--jinja` (tool calling through the model's chat template), with `--reasoning off` and
  `enable_thinking: false`.

Files:
- pid file: `~/.local/state/omarchyvr/ai/llama-server.pid` (`stop` checks that the PID is a llama-server before signalling);
- server log: `~/.local/state/omarchyvr/logs/llama-server.log`.

It answers to any model name; `pimax-local` is its alias.

A systemd user unit is in `packaging/systemd/omarchyvr-ai.service`. It is not installed; its header says how to try it.

**GPU budget next to VR.** The RTX 4090 has 24 GiB. A desk session (Monado, WayVR, passthrough) uses a few GiB, and the
agent adds about 3.5 GiB, capped by design at 4 GiB (`omarchyvr-ai status` prints the actual figure from `nvidia-smi`). The
model only computes while a request runs, for a fraction of a second after "Max ...". If frame time suffers anyway,
use `serve --cpu`, or `--gpu-layers N` with a smaller N.

## 2. Settings (`~/.config/omarchyvr/ai.toml`)

Do not create the file by hand unless you want to. The AI tab writes it (mode 0600). No cloud model is configured by default:
with no file or no providers nothing is ever sent to a cloud service (`voice_ai = "fallback"` then means the local omarchyvr-ai
server only). The first-run setup does not offer a cloud model; it is added on the AI page by users who have a key. An example:

```toml
voice_ai = "fallback"        # off | fallback | always
voice_confirm = "none"       # none | dangerous | always: when a command waits for a spoken "yes" (AI page: Confirm)
strip_titles = false         # true: window titles never leave the machine (class + workspace only)
default = "local"

[use]
voice = "local"              # the provider voice uses (else `default`)

[providers.local]            # omarchyvr-ai serve
kind = "openai-compatible"
base_url = "http://127.0.0.1:8080/v1"
model = "pimax-local"

[providers.claude]
kind = "anthropic"
model = "claude-haiku-5-5"   # the default; fast and cheap
api_key_env = "ANTHROPIC_API_KEY"   # or api_key = "sk-ant-..." (the file is 0600)

[providers.openai]
kind = "openai"
model = "gpt-..."
api_key_env = "OPENAI_API_KEY"
```

Extra keys you can add by hand (optional; the AI tab does not show them yet):
- `voice_ai_timeout_s` (default 4 local, 8 remote);
- `voice_ai_gaze = false`;
- `voice_ai_log = false`;
- `voice_ai_debug` (default true): the debug line under the captions (section 12); "Max, debug on/off" overrides it
  until the next login;
- `voice_ai_confidence` (0-1, default 0.6): below it the agent asks "Did you mean ...?" instead of acting;
- `voice_ai_style = "compact" | "tools" | "two-stage"`: how the model is asked (sections 4 and 14). Default:
  compact for a local model, two-stage for a remote one (OpenRouter, Claude, OpenAI).

Tuned values on this machine (the file as it is now):

```toml
voice_ai = "fallback"
voice_ai_timeout_s = 4
voice_ai_debug = true
voice_ai_confidence = 0.6
```

Rules:
- No file means the agent is off, and the old `[llm]` fallback and AI switch behave as before.
- A file with no providers means the local server at 127.0.0.1:8080.
- `omarchyvr-voice act` reads the file at start, so restart it after a change. It prints a line like
  "AI agent: local (openai-compatible pimax-local, local) (fallback, compact, 77 tools, ask below 60% confidence,
  debug on)".

## 3. Routing (the toggle)

The speech-to-text is not exact ("liz windows", "leaks list windows", "Time Max War Three"), so the catalogue phrases
are evidence, not an exact-phrase gate. Every wake-word request goes through these steps (`Actor::hear`):

1. **Exact phrase** (the old matcher, with the alias table): runs at once.
2. **Noise**: only "max"-like sounds and fillers after the wake word ("max ma max max"), or chatter such as "can you
   hear me", "testing", "hello": ignored, no toast, no model.
3. **Fuzzy, near-exact** (`fuzzy.rs`): every phrase is aligned with the heard words (word edit distance; a
   substitution costs 1 - similarity, where similarity combines spelling, a Metaphone-like key and the known ASR
   confusions war=work, liz/leaks=list, mars/marks=max, tree=three, ...). A clear winner (score >= 0.9, 0.1 ahead of
   the next, no free-text slot) runs at once: "liz windows" and "leaks list windows" run `win.list` in ~5 ms.
4. **The model** gets everything else, with the fuzzy top 3 as hints. It answers one intent with a confidence;
   `none` is ignored silently; a confidence under `voice_ai_confidence` asks "Did you mean "list windows"? Say yes".
5. **The model failed** (timeout, error): a fuzzy candidate with score >= 0.7 is asked about ("Did you mean ...?");
   otherwise one toast "Didn't get ... (AI: timed out)". A timeout also re-primes the server's cache in the
   background.

Without the agent (no ai.toml), steps 1-3 still apply, and step 5's question replaces "Didn't get" when a candidate
is close.

- **off**: only the offline catalogue (steps 1-3 and the question of step 5).
- **fallback** (default):
  - The offline matcher answers first (steps 1-3). A plain match runs at once, with no model call.
  - A deictic window command such as "Max close this" or "Max send that to workspace 2" can match while your eyes
    rest (fixation under 3 s old) on a window other than the focused one. Then that window is focused first and the
    match runs, still with no model call.
  - Anything the matcher does not understand goes to the model.
- **always**: every request goes to the model, and the matcher's answer is the fallback.

**Plans.** A request with "and" or "then" may get up to 3 steps from the model, in order ("go to workspace 2 and make
it fullscreen" -> `win.ws.goto n=2 ; omarchy.window.fullscreen`); without those words the grammar allows one step. A
question (low confidence, a yes under `voice_confirm`) or a failure ends the plan; there is a 400 ms pause between
steps so a launched app or a switched workspace settles.

**Words** (user decision 2026-10-15, in both system prompts): a "window" is a workspace = one VR screen with all its
apps (close window = `win.ws.close`); one program is an "app" (`win.close`, and the model's `focus_window`,
`move_window`, `close_window` act on apps); a "tab" is inside an app; "N screens" / "the other screens" are VR presets.
The model's app list is labelled `apps:`.

**Confirmation** (`voice_confirm`, section 2): `none` runs every command at once (the default); `dangerous` asks
"...? Say yes" for `confirm = true` intents (lock, close window / app / tab) and for typing or Enter into a terminal;
`always` asks for everything. The audit runs with `none`.

On a model error, timeout or unusable answer you get a toast ("AI: timed out"). The matcher's result then runs, if
there was one; otherwise you get the usual "Didn't get ...". A plain-text answer from the model (for example "Which
window?") is shown as a toast "Max: ...".

## 4. Context the model gets

### Compact request (default for the local model, `nlu.rs`)

The old request sent 75 JSON tool schemas plus a JSON context, about 5,650 prompt tokens per command. This one is
built for the prompt cache and a small model:

- **System prompt, static** (~1,450 tokens, byte-for-byte the same every time, so llama.cpp keeps it in the slot):
  the rules, the known ASR confusions, one line per intent (`id slot=N: gloss` - the `gloss` key of
  voice-intents.toml, else the first 3 phrases), the three window pseudo-intents (`focus_window window=wN`,
  `move_window window=wN workspace=N`, `close_window window=wN`) and a few examples.
- **User message, per command** (~40-120 tokens):

  ```
  apps: w1 foot "fix-arrow-key-input" ws2; w2 google-chrome "hand - Google Search" ws2   (only when the request
                                                                                           mentions windows/apps)
  focused: w1
  eyes on: w2
  hints: win.list 0.95, win.close 0.72, omarchy.focus.left 0.76
  heard: "leaks list windows"
  ```
- **Answer: one line**, `win.list 9`, `win.ws.goto 9 n=4`, `omarchy.reminder 9 minutes=5 text="stretch"`,
  `none 9`. On the local server a GBNF grammar (`grammar` in the request) allows only the offered ids, each with its
  own slots and value shapes, so the 4B answers in 4-8 tokens. Remote models get the same text without a grammar
  (the parser is lenient).
- The decision goes through the same checks as a tool call (section 5): safety, careful = spoken yes, window ids.
- `cache_prompt` is on, and `omarchyvr-voice act` primes the cache at start and again whenever `/health` turns ready
  (server restart).
- Lessons from the bench: the model copied the literal answer-format placeholder ("INTENT ...") and any id-like text
  in the context ("vr: preset cinema" made it answer `vr.preset.cinema` for "give me three screens"; a "last
  command" line made it repeat the last command). So the format is shown by example, the VR line is worded unlike
  the ids, and there is no last-command line.
- Window lines change rarely, but Qwen3.5 is a hybrid (recurrent) model: llama.cpp restores its state only from
  checkpoints, which in practice sit at the end of the system prompt, so the whole user message is evaluated each
  time. Hence the short message (no window list unless the request is about windows; leading spinner glyphs of
  terminal titles are dropped anyway).

`omarchyvr-voice aiprompt WORDS...` prints the request for some words.

### Tool calling (`voice_ai_style = "tools"`; remote providers now default to two stages, section 14)

The context is one compact JSON object per request, about 1000 tokens at most (windows are dropped from the end first,
then titles):

```json
{"request":"close the one i'm looking at","time":"Thu 14:05",
 "this":{"window":"0xc3","from":"gaze"},
 "gaze":{"window":"0xc3","class":"spotify","title":"Spotify Premium","screen":"center","pos":[0.4,0.6],"fixation_ms":900,"age_ms":100},
 "focused":{"id":"0xa1","class":"firefox","title":"YouTube - cats","ws":1},
 "windows":[{"id":"0xa1","class":"firefox","title":"YouTube - cats","ws":1}, "... up to 24, most recent first"],
 "workspaces":{"used":[1,2,3],"active":1},
 "vr":{"running":true,"preset":"cinema","background":"dimmed","passthrough":"dim","captions":true},
 "recent":[{"heard":"cinema mode","did":"vr.preset.cinema","ok":true}]}
```

- `this` is the gazed window when the gaze is fresh and on a known window; otherwise it is the focused window.
- Gaze comes through the `GazeSource` trait. The default implementation runs `wayvrctl gaze --json` and accepts these
  keys, at the top level or under `target`:
  - `window` / `address`;
  - `screen` / `panel`;
  - `tab`;
  - `x`, `y` or `pos`;
  - `fixation_ms`;
  - `age_ms`.

  With no gaze, there is no `gaze` key.

## 5. Tools and the safety policy

The agent offers these tools:
- **One tool per enabled catalogue intent** (`vr_preset_cinema`, `omarchy_window_to_workspace {n}`,
  `omarchy_reminder {minutes, text}`, ...):
  - generated from `voice-intents.toml`;
  - the slot types become a JSON schema (number = integer 1-999; word, text and name = short strings, with the same
    character checks as speech);
  - about 70 tools.
- **Window tools by id:**
  - `focus_window {window}`;
  - `move_window {window, workspace 1-10}`;
  - `close_window {window}` (careful);
  - `browser_tab {action: next|previous|new|goto|close, n, window}` (close is careful).
- **Read-only queries:** `list_windows`, `get_gaze`, `get_session_state`.

The policy is enforced in code; the model cannot change it:
- **safe** tools run at once and give their normal feedback toast.
- **careful** tools (`confirm = true` intents, lock the screen included, close an app, close a window = workspace, close tab) ask "...? Say "yes" to confirm" and run only
  after a spoken "yes" within 8 s. This is the same confirmation as the catalogue.
  Whether they ask depends on `voice_confirm` (section 3): with `none`, the default, they run at once.
- **never** rows without `confirm` (lock the screen is a "never" row in the research but approved by the user on
  2026-10-15 with `confirm = true`, so it is careful, not refused), disabled intents, the AI switch itself and any made-up tool are refused. The refusal goes back to the model, which can only
  explain.
- Window ids must be in the current window list. Arguments are checked against the slot rules; unknown arguments are
  refused.
- Each request has at most 3 tool calls and 3 model rounds, under one deadline (4 s local, 8 s remote).
- Window titles, tab titles and the request are marked as data in the system prompt. Even so, the worst a hostile
  title can do is make the model pick another allowed tool.

## 6. Privacy

- **Local provider:** nothing leaves the machine.
- **Remote provider, two stages (the default for OpenRouter, Claude, OpenAI; section 14):** sent only after
  "Max ...", and only when the matcher did not understand the command (fallback mode) or for every command except
  exact phrases (always mode):
  - to the intent model (stage 1, `typesafe/jev-router`): the words heard and a one-line summary (the number of VR
    screens, the focused app's class, the side work/play, the preset), with a fixed system prompt (the categories
    and the user's vocabulary). No app list, no titles, no tools;
  - to the action model (stage 2): the category's tools (catalogue ids, phrases, schemas), a fixed system prompt,
    the words, the focused app (id, class, title), the matcher's 3 guesses, and only what those tools need:
    screens/apps: the VR screens (output, left/center/right, workspace) with their apps (id, class, title cut to
    80 characters, tab count), the other workspaces, the gaze target (app id, screen, tab guess, fixation);
    tabs/browser: up to 12 apps with their tab counts; session/passthrough/recording/captions: the VR session
    (running, preset, background, passthrough, captions; from omarchyvrd: overall health, side, components up or down,
    recording) and the time; session (undo): the last 5 commands with what ran and its result; other: all of it;
  - `strip_titles = true` removes every window and tab title;
  - OpenRouter also learns your key (in the Authorization header only) and the models it routes to.
- **Remote provider, single request (`voice_ai_style = "tools"`):** the tool list (catalogue ids, phrases, schemas)
  and the system prompt are sent with every request. The context of section 4 is sent only after "Max ...", when
  the agent is used (always mode: for every command). Its contents:
  - the request words;
  - local time (weekday, hh:mm);
  - gaze: window id, class, title, screen name, position, fixation;
  - the focused window and up to 24 windows (id, class, title cut to 80 characters, workspace, tab count);
  - workspace numbers;
  - VR state (running, preset, background, passthrough mode, captions on/off);
  - the last 4 commands (words heard and the intent that ran).
- **Never sent:** process ids, command lines, URLs, file contents, screenshots, audio, the API key in a body or log.
- `strip_titles = true` removes every window and tab title (class and workspace remain).
- Anthropic requests cache the fixed system prompt and tools (`cache_control`), so repeated requests are cheap. Claude
  Haiku 5.5 (the default) runs with `effort: low`.

## 7. Log

`~/.local/state/omarchyvr/logs/ai.log` gets one JSON line per request: request, provider, mode, context (compact: the user
message as `prompt`, the `answer`, the `decision`), tool calls with their results, outcome or error, and
milliseconds. Every wake-word request also gets a `{"trace": ...}` line: the heard words, the route (match, fuzzy,
noise, ai, yes), the fuzzy candidates, the model's decision, what ran, whether it asked, the result and the latency
(the same as the debug line of section 12).
- Keys are never logged. Anything that looks like a key (`sk-...`, long opaque tokens) is replaced by `[redacted]`.
- The log rotates at 1 MiB into `ai.log.1` to `ai.log.3`.
- `voice_ai_log = false` turns it off.

## 8. Examples

| You say | What happens |
|---|---|
| Max, cinema mode | matcher, no model |
| Max, I want to watch a film | model: `vr_preset_cinema` |
| Max, close this (looking at Spotify, Firefox focused) | matcher `win.close`: closes Spotify, the app you look at |
| Max, close window (looking at the screen with workspace 3) | matcher `win.ws.close`: every app on workspace 3 (asks first with `voice_confirm = "dangerous"`) |
| Max, move this here (Firefox focused, looking at workspace 2) | matcher `win.move.here`: Firefox moves into workspace 2 as a split |
| Max, make the one I'm looking at fullscreen | model: `omarchy_window_fullscreen`; the gazed window is focused first |
| Max, where's my terminal with the pimax repo | model: `focus_window {window: "0xb2"}` |
| Max, put Spotify on workspace 3 | model: `move_window {window, workspace: 3}` |
| Max, get rid of the music app | model: `close_window` (asks for "yes") |
| Max, lock everything | model: `omarchy_lock` (asks for "yes"; locks only after it) |

## 9. Live test

1. Leave VR running. If you do not want the GPU used, run `omarchyvr-ai serve --cpu`. Otherwise run `omarchyvr-ai serve`;
   `omarchyvr-ai status` then shows the VRAM.
2. In the AI tab: provider local, voice AI = fallback. Or write the example above.
3. Restart the voice actor; its log shows "AI agent: ...".
4. Try the examples. Check that "close" and "lock" ask for "yes", and that `ai.log` has the lines.
5. Dry run without touching anything:
   `omarchyvr-voice do "Max I want to watch a film" --dry-run`.
   This sends a model request, but the actions are printed, not run.

## 10. AI tab integration (for the GUI)

**Keys in ai.toml** (all in `omarchyvr_config::ai`):
- `voice_ai` (off | fallback | always): show it as a 3-way toggle.
- `strip_titles` (bool): a privacy checkbox. Show the section 6 list next to it when the voice provider is remote.
- `[use] voice` and `default` pick the provider.
- For the local provider, write `kind = "openai-compatible"`, `base_url = "http://127.0.0.1:8080/v1"` and
  `model = "pimax-local"`.
- Optional, preserved if present: `voice_ai_timeout_s`, `voice_ai_gaze`, `voice_ai_log`. The current parser warns about
  them and drops them on save, so please keep unknown top-level keys.

**Local model controls:** run `rust/target/release/omarchyvr-ai <cmd>` (or `~/.cargo/bin/omarchyvr-ai`). All are user-level
and need no sudo.

| Button | Command | Notes |
|---|---|---|
| Install | `omarchyvr-ai install` | 4.1 GB; progress bars on stderr; text lines on stdout; can be run again to resume |
| Start | `omarchyvr-ai serve` (GPU) / `omarchyvr-ai serve --cpu` | returns after "ready" (or "still loading"); exit 1 + message on error |
| Stop | `omarchyvr-ai stop` | |
| Status | `omarchyvr-ai status --json` | poll every few seconds |
| Test | `omarchyvr-ai test` | prints "<ms> ms: <reply>" |

`omarchyvr-ai status --json` prints one line:

```json
{"installed":true,
 "runtime":"/home/u/.local/share/omarchyvr/ai/runtime/runtime-vulkan-b11630/llama-b11630/llama-server",
 "models":[{"id":"model-gpu","file":"Qwen3.5-4B-Q4_K_M.gguf","installed":true},
           {"id":"model-cpu","file":"Qwen3.5-2B-Q4_K_M.gguf","installed":true}],
 "running":{"pid":1234,"port":8080,"mode":"gpu (all layers)","model":"Qwen3.5-4B-Q4_K_M.gguf","url":"http://127.0.0.1:8080/v1"},
 "ready":true,"gpu_mib":3480,"gpu_budget_mib":4096,"model_alias":"pimax-local"}
```

- `running` is `null` when the server is stopped.
- `ready` means `/health` answered: the model is loaded.
- `gpu_mib` is `null` in CPU mode or without nvidia-smi.

Show these:
- installed or not, with the install size;
- the running state (GPU or CPU), ready or loading;
- VRAM against the budget;
- the Start GPU / Start CPU / Stop buttons;
- a warning when GPU mode is chosen while a VR session runs (that is allowed; it is a 4 GiB budget).

## 11. Speed and accuracy (measured 2026-10-15, VR desk session running)

`omarchyvr-voice aibench` sends the phrases of `rust/omarchyvr-voice/tests/fixtures/aibench.tsv` (45 real and mangled
transcripts: the live ones from voiceact.log plus paraphrases and noise) through the full route against the running
server, executing nothing (hyprctl and gaze are read live). Latency = end of the transcript to the decision.

```sh
omarchyvr-voice aibench --runs 3                      # ai.toml's provider
omarchyvr-voice aibench --url http://127.0.0.1:8081/v1/chat/completions --label "variant"   # another server
omarchyvr-voice aibench --style tools --timeout-s 10  # the old tool-calling request
```

| setup | right | wrong actions | model latency p50 / p95 |
|---|---|---|---|
| before, live: tool calling, 4B CPU 8 threads, 10 s timeout | every unmatched command timed out (~5,650 prompt tokens, 63-77 tok/s) | - | > 10 s |
| before, tool calling, warm cache, 4B CPU 24 threads (18 model phrases) | 56 % | 5 of 18 | 4.6 s / 9.0 s |
| compact, 4B CPU 8 threads nice 19 | 97 % | 0 | 2.6 s / 2.9 s |
| compact, 4B CPU 16 threads nice 19 | 100 % | 0 | 1.43 s / 1.77 s |
| compact, 2B CPU 16 threads nice 19 | 78 % | 4 of 36 | 0.65 s / 0.81 s |
| **compact, 4B CPU 24 threads nice 19 (chosen)** | **100 % (135/135)** | **0** | **1.27 s / 1.62 s** |
| compact, 4B GPU (Vulkan, all layers) | 100 % (6) | 0 | ~0.15 s once warm |

All requests (most never reach the model): p50 4 ms, p95 1.57 s. The load average was 10-20 (other builds running),
so an idle machine is faster. Caveat: the glosses and the few prompt examples were tuned on these phrases.

Findings:
- **The size of the request was the problem**, not the model: 75 tool schemas = ~5,650 tokens, re-read whenever the
  dynamic JSON context moved the cache boundary. The compact request evaluates 60-120 new tokens per command.
- **CPU threads:** 8 -> 16 -> 24 threads at nice 19: 2.6 -> 1.4 -> 1.27 s. nice 0 instead of 19 gained little
  (8 threads: 2.49 vs 2.61 s), so the server stays at nice 19. Generation stays at ~12 tokens/s (memory bound with
  VR running), so the 4-8 answer tokens cost 0.35-0.7 s; prompt evaluation ~100-120 tokens/s.
- **VR frame timing** (`/run/user/1000/omarchyvr-perf.stats`, 8 s windows): idle 0 skipped frames; CPU 16 or 24 threads
  back to back: 0 skipped, late ratio unchanged; GPU back to back (53 requests): 53 skipped frames and late frames
  35 % -> 78 %, i.e. **about one skipped frame per GPU request**.
- **"Vulkan at 8 tokens/s next to Monado" was the first request after a server start**: Vulkan builds its pipelines
  (the earlier log's task 0: 19 tokens in 2.3 s; the same session's next request ran 5,675 tokens at 345 tok/s,
  106 tok/s generation). Warm, the GPU does 800-2,100 tok/s prompt and ~95 tok/s generation even with VR running.
  llama.cpp has no Vulkan queue-priority option, and NVIDIA time-slices between processes, so the compositor still
  loses a frame when a request lands; `--gpu-layers N` (partial offload) would only add CPU-GPU transfers. The GPU
  stays an option (`omarchyvr-ai serve`) when a dropped frame per command is acceptable.

## 12. The debug line under the captions

With `voice_ai_debug = true` (the default) `omarchyvr-voice act` writes one line per command to
`$XDG_RUNTIME_DIR/omarchyvr-voice-debug.line`, and `omarchyvr-voice captions` shows it under the newest transcript line, in
dim cyan (`#5fd0e0`, set with `wayvrctl panel-modify captions cap_l2 set-color`; the panel XML is unchanged). It
clears with the captions after the silence timeout. Format (fitted to the 56-character caption width; the latency
always shows):

| line | meaning |
|---|---|
| `= vr.preset.cinema > Cinema mode 3ms` | exact phrase, the result toast, latency |
| `~win.list .95 > 3 windows: ... 5ms` | near-exact fuzzy hit (no model) |
| `noise > ignored 1ms` | noise or chatter, nothing done |
| `~focus.app .58 > AI vr.preset.work3 9/9 > Work mode, 3 screens 1145ms` | best fuzzy candidate > the model's decision (intent, confidence of 9, slots) > result |
| `~window.wider .62 > AI none 9/9 > ignored 831ms` | the model says: not a command |
| `~media.next .80 > AI timed out > Did you mean "skip song"? ~ 4012ms` | model failure, asked about the candidate |

"Max, debug off" / "Max, debug on" toggle it (also in `~/.local/state/omarchyvr/logs/ai.log` as `trace` lines, always).
The captions panel exists only in sessions started with voice (`omarchyvr desk start --voice` or the assistant preset).

## 13. Checking every command (`omarchyvr-voice audit`, 2026-10-15)

`omarchyvr-voice audit` (the AI page's **Check all voice commands**; docs/VOICE.md) runs each enabled intent's first
phrase (exact), an ASR-mangled copy of it and the natural phrasings of `tests/fixtures/audit.tsv` through the full
route in a dry run, with `voice_confirm = none`, and writes `analysis/voice-audit.html` (the app: `<state>/voice-audit.html`).
It pauses 1 s after each model request (`--pause-ms`), so a run takes ~4 minutes.

| run (4B, CPU, 24 threads) | phrases | pass | exact | mangled | natural | model p50 / p95 |
|---|---|---|---|---|---|---|
| before the voice work (559382e, intent only) | 355 | 75 % (266) | | | | |
| WIP 0d06dbc | 355 | 96 % (340) | 87/87 | 78/87 | 175/181 | 1.31 s |
| 0d06dbc + merge, first run today | 388 | 93 % (362) | | | | |
| after the fixes below | 394 | **99.7 % (393)** | 95/95 | 94/95 | 204/204 | 1.16 s / 1.54 s |

Fixes:
- **The VR state line was copied.** With a session running, the compact message said "vr now: assistant preset,
  ...", and the 4B answered `vr.preset.assistant` for "dark", "necks background", "re center everything" (11 of the
  first run's 25 failures; a run without a session had none of them). The line is gone; no intent needs it.
- The window words in the prompt (section 3): "close the window I'm in" was `win.close`, now `win.ws.close`.
- More known mishearings (winter=window, tight=type, full scream=fullscreen, and do=undo, high=hide), glosses for
  next/previous window (workspace, desktop), screenshot vs recording, a weather question = search; an example
  "open two more screens" -> work3.
- Typing is logged to typed.log only when it really ran (a dry run, the audit and tests logged fake entries).
- The system prompt is ~8,200 characters (~2,050 tokens, cached).

The one failure left: "go to winter where youtube" (mangled `win.ai.switch`) runs `omarchy.focus.app youtube`, which
focuses the same window.

## 14. Remote providers in two stages: jev classifies, a cheaper model acts (2026-10-15)

User decisions (2026-10-15): stage 1 is `typesafe/jev-router` and only classifies, with little context; stage 2 is a
cheaper model with tools, the category's tools only and the minimum state those tools need. Exact catalogue hits stay
local. Code: `rust/omarchyvr-voice/src/remote.rs`, `Agent::two_stage`.

```toml
[providers.openrouter]
kind = "openai-compatible"
base_url = "https://openrouter.ai/api/v1"
model = "typesafe/jev-router"        # the intent model (stage 1)
action_model = "openai/gpt-6-luna"   # the action model (stage 2); default for OpenRouter when not set
```

The AI page shows both ("Intent model", "Action model" with the two benchmarked choices) in the provider editor, and
the voice card shows "intent ..., action ..." with a Test button (latency and the price of one request).

1. **Classify (intent model).** One short request: the words, a one-line summary (`remote::summary`: "3 VR screens,
   focused app foot, work side, preset cinema") and a fixed prompt listing the categories with one line each:
   `screens` (VR screens = windows = workspaces), `apps`, `tabs`, `typing` (keys, dictation), `browser`, `session`
   (presets, undo), `passthrough` (and backgrounds), `recording`, `captions` (and the debug line, help), `system`
   (volume, media, info, reminders, power), `other`. The answer is JSON (`response_format` with a JSON schema;
   the parser takes the last JSON object, since some routed models write reasoning first, else "category 0.9").
2. **Act (action model).** The category's tools only (`remote::subset`: catalogue intents by id family, the window
   tools for screens/apps, `browser_tab` for tabs/browser, undo and help everywhere; `other` = all tools) and the
   state they need (`remote::Needs`, section 6). Up to 3 tool calls under the usual policy (section 5); a tool that
   was not offered is refused. Below 0.7 confidence the classifier's second category is added.

**One round trip of latency.** Stage 2 starts at the same time as stage 1, with the categories the local matcher
guesses (`remote::local_guess`; no guess = all tools). Its answer is used when its tools belong to the classified
category; otherwise stage 2 is sent again with that category. "Other" with no tool call (chatter such as "are you
listening") is ignored without a toast.

**Fast paths stay local.** Exact phrases, near-exact fuzzy hits and noise never reach a provider; in always mode an
exact phrase still runs locally.

**Timeouts and fallback.** Both stages share one deadline, `voice_ai_timeout_s` (8 s here). On a timeout, an HTTP
error or an unusable answer the local model answers instead when its server is up (`/health` within 300 ms; the
compact request, 4 s), else the matcher asks "Did you mean ...?" or says "Didn't get". The debug line shows the path:
`~media.next .80 > AI system .99 > openrouter timed out > local omarchy.media.next 9/9 > Next song 8911ms`.

**State, built in ~16 ms** (measured, VR desk session running): `hyprctl -j clients`, `activewindow`, `monitors`,
`wayvrctl gaze` and omarchyvrd's snapshot (`$XDG_RUNTIME_DIR/omarchyvrd/state.json`, re-read at most once a second, ignored
when older than 30 s). The passthrough socket is not asked (it can take 2 s).

**Cost.** OpenRouter requests carry `usage: {include: true}` and `reasoning: {enabled: false}`; `usage.cost` is added
up per command (ai.log `cost`, the trace, the audit's total).

**Benchmark** (`omarchyvr-voice audit --remote --max 40 --action-model M`: the same 40 natural phrasings the matcher
leaves to the model, spread over the intents, dry run, nice 19, 1 s apart, VR desk session running; latency = end of
the transcript to the decision, both stages):

| stage 2 (action model), jev-router classifies | pass | p50 / p95 | cost per command | notes |
|---|---|---|---|---|
| **openai/gpt-6-luna (default)** | **37/40 (92 %)** | **3.4 s / 5.1 s** | **$0.00036** | fails: "type how to get friends in bali" (a search, ran help), "app on the left/right" (focused an app by id) |
| anthropic/claude-haiku-5.5 | 31/40 (78 %) | 4.0 s / 7.0 s | $0.00077 | answers in words instead of calling the tool for "make this float / fullscreen / maximize / pop out" (no app focused in the dry run), 2 timeouts |
| before the decision: jev-router for both stages, the whole state | 36/40 (90 %) | 4.0 s / 6.2 s | $0.00114 | |

Both stages cost ~$0.00036 per command with gpt-6-luna (~3,000 commands per dollar). The minimum state per category
cut the cost to a third. The router's own latency (2.5-5 s for a 10-token answer) dominates; stage 2 runs alongside
it. The report: `analysis/voice-audit-remote.html`.

**Keys.** The key is only in curl's config on its stdin (never in argv, a body, a file or a log). ai.log and the
voice log go through `agent::redact` (`sk-...`, long opaque tokens), errors too; tests check that the OpenRouter key
never reaches ai.log, a toast or a request body.

```sh
omarchyvr-voice audit --remote --max 40 --pause-ms 1000 --out analysis/voice-audit-remote.html
omarchyvr-voice audit --remote --action-model anthropic/claude-haiku-5.5    # another stage-2 model
omarchyvr-voice audit --remote --log --fixture my.tsv     # only these phrases; ai.log keeps requests and costs
```

## 15. The local model's HTTP 400 (fixed 2026-10-15)

After the VR screen intents (324efff) every request to the local llama-server answered HTTP 400 ("failed to parse
grammar: Undefined rule identifier 'side'"): the new `side` slot type had no GBNF rule, so the AI route never
answered (the installed build's audit: 312/444). Fixes:
- the grammar defines `side ::= "left" | "right" | "up" | "main"`, and a test checks that every rule a grammar uses
  is defined;
- curl keeps the error body (`fail-with-body`), so the debug line and the toast say `HTTP 400: <the server's
  message>` instead of "curl: (22)";
- a size guard refuses a request that cannot fit the local 8k context with "request too large for the local model"
  (the old one-request tool calling with all ~100 tools no longer fits; the compact request does);
- the matcher: "a scape" = escape, "make it louder" = volume up, "go back one song" = previous song, "take me to
  workspace 3" = go to workspace, "close the window i'm in", "merge all the apps on one screen"; a side slot is
  stored as its one word (top / above -> up).

Local dry-run audit after the fix (Qwen3.5-4B on the CPU, VR session running): **440/444 (99.1 %)**, 128 model
requests p50 1.29 s, p95 2.05 s. Two of the four remaining failures got phrases afterwards.

## 16. Two stages on this PC: Lara sorts, the model on the GPU acts (2026-10-16)

User request (2026-10-16): "we already have an OpenRouter model with Jev set up; let's have a toggle that is LOCAL: 'lara',
same as Jev, and the other model on the GPU, all local for the orders; lara for the same filter, and whatever model for
the uses. Add this to WayVR." Later the same day: the GPU memory is adaptive (no fixed cap), and both models are picked
from a list that shows size, video memory, speed and the audit's accuracy, with the recommended one marked.

### What "Lara" is

- `typesafe/jev-router` (Jev) is TypeSafe's hosted "System One" router. No weights are published (Hugging Face: none;
  OpenRouter and TypeSafe's API only), so Jev cannot run on this PC.
- No model called "Lara" exists (OpenRouter, Hugging Face, TypeSafe). The open counterpart of Jev is **Laya**
  (`convaiinnovations/laya`, code github.com/NandhaKishorM/laya, Apache-2.0): LiteLLM lists it next to Jev and Bespoke
  Nimble as the self-hosted decision classifiers, and its `laya-serve` speaks Jev's own `/v1/systemone` protocol.
  "Lara" is almost certainly Laya heard or spelled differently. The app keeps the name **Lara** for the router's role.
- Laya is not a language model: a ModernBERT-large encoder (421M parameters) with decision heads answers a `choice`
  question (our 11 categories with a short description each) with one probability per category in one forward pass.
  No text is generated, so it cannot go off script. It runs on the processor only (PyTorch CPU, ~2 GB of RAM).
- As published it does not know this desk's words ("dim", "necks background", "liz windows"). **Lara** is Laya tuned on
  this PC to the command catalogue (`omarchyvr-ai tune-lara`): every catalogue phrase of every intent, its ASR-mangled
  copy and some chatter (1,975 rows; `omarchyvr-voice routerbench --out`), 3 epochs on the processor (here ~65 minutes on
  16 cores at nice 19, next to VR and these benchmarks; 0.8 GB). The audit's natural phrasings stay out, so the audit tests unseen wording.
  Laya's own calibration slice (in-sample): 41 % -> 90 %.

### The pipeline (`rust/omarchyvr-voice/src/router.rs`, `Agent::lara_filtered`)

Exact phrases, near-exact fuzzy hits and noise stay with the matcher (no model, as in every mode). Everything else:

1. **Lara and the model at the same time.** Lara (port 8081) gets only the words. Meanwhile the model on port 8080
   answers the simple request of section 4 (the cached prompt with every intent, a one-line answer under a GBNF
   grammar). One round trip of latency.
2. **The filter.** An answer whose intent belongs to one of Lara's categories stands. So does an answer the model is
   sure of (8 or 9 of 9, `LARA_TRUST`). Lara's categories are its best one, plus the runner-up below 70 %
   probability, plus the third when the first two are under 80 % together, plus the matcher's first guess.
3. **Otherwise the model is asked again** with a grammar of only those categories' intents (`nlu::grammar_subset`;
   "none", undo and help always allowed). The prompt is the same, so the server's prompt cache still holds.
4. **Fallbacks.** Lara down or slow (1.5 s): the model's own answer (the simple mode). The model down: the matcher's
   "Did you mean ...?" or "Didn't get". `voice_ai_style = "two-stage"` switches stage 2 to tool calling with the
   category's tools, like the online path (section 14); measured below, it is worse on this PC.

The debug line under the captions starts with Lara's verdict: `~x .58 > AI Lara system .93 > omarchy.volume.up 9/9 >
Volume up 412ms`; an overruled answer shows as `Lara system .97 > not vr.preset.cinema > omarchy.volume.up 9/9`, a
missing router as `Lara off > ...`.

### Models

| role | id in ai.toml | what | download | memory | licence |
|---|---|---|---|---|---|
| router | `lara` (recommended) | Laya 421M tuned here on the catalogue | 2.7 GB (Python packages ~0.4 GB, 1.2 GB installed; the Laya repository with its three checkpoints 2.3 GB), then ~1 h of tuning once (0.8 GB more) | ~2 GB RAM, no VRAM | Apache-2.0 |
| router | `laya` | Laya 421M as published (`english` checkpoint, revision 55cf4c4) | 2.7 GB | ~2 GB RAM | Apache-2.0 |
| router | `qwen3.5-0.8b` | Qwen3.5-0.8B Q4_K_M on a second llama-server, JSON schema | 508 MiB | ~0.9 GB RAM | Apache-2.0 |
| action | `qwen3.5-4b` (recommended) | Qwen3.5-4B Q4_K_M (the simple mode's model) | 2.6 GiB | ~3.0 GB VRAM (measured 3,006 MiB) | Apache-2.0 |
| action | `qwen3.5-9b` | Qwen3.5-9B IQ4_XS (Q4_K_M would be 5.3 GiB) | 4.8 GiB | ~4.8 GB VRAM (measured 4,846 MiB) | Apache-2.0 |

On this PC everything above is downloaded already (2026-10-16), and Lara is tuned (`~/.local/share/omarchyvr/ai/laya/lara`).

Downloads are pinned and checked: GGUF files by Hugging Face revision and LFS SHA-256 (`omarchyvr-ai` `Asset`s); Laya by
`laya==0.4.8` with every Python package at a tested version (`LAYA_CONSTRAINTS`, PyTorch's CPU wheel index) and the
checkpoint at the revision Laya lists as reviewed (`LAYA_REVISION=reviewed`). Nothing is bundled in this repository.

### Runtime (`rust/omarchyvr-ai/src/twostage.rs`)

| port | process | pid file / log |
|---|---|---|
| 8080 | llama-server with the action model (in the simple mode: the simple model) | `llama-server.pid`, `llama-server.log` |
| 8081 | `laya-serve` (Lara / Laya, CPU, 8 threads, offline, nice 19) or llama-server (0.8B, CPU) | `router.pid`, `ai-router.log` |
| - | `omarchyvr-ai watch` (VRAM watch) | `vram-watch.pid`, `ai-vram-watch.log` |

```sh
omarchyvr-ai install --two-stage [--router lara|laya|qwen3.5-0.8b] [--action qwen3.5-4b|qwen3.5-9b]
omarchyvr-ai up            # what ai.toml asks for (simple or two stages), fitted to the free VRAM; waits until ready
omarchyvr-ai fit [--json]  # what up would choose now and why
omarchyvr-ai test --two-stage   # "two stages: router 31 ms: system; model 190 ms (...); VRAM 3.0 GB in use, 19.3 GB free"
omarchyvr-ai down          # router, model and watch
omarchyvr-ai tune-lara     # (again) tune Laya to the catalogue
```

omarchyvrd's `ai.server` component (System page: "Local AI (omarchyvr-ai)") now starts `omarchyvr-ai up` and stops with
`omarchyvr-ai down`, so Play and Work bring the AI up in the saved shape; it stays a leaf (VR never waits for it).

**Video memory: Auto by default** (`vram_mib = "auto"`, the AI page's "Video memory for AI": Auto / up to 4, 6, 8, 12 GB).
At each `up` (each Play / Work start, or a change of the setting) `omarchyvr-ai` reads the free VRAM (nvidia-smi; its own
server's memory counts as free) and what runs next to it from the process list:

| session | VRAM kept free for it |
|---|---|
| none | 1 GB |
| the Work desk (wayvr) | 2 GB |
| SteamVR without a game (vrserver) | 8 GB (room for the game to come) |
| a game (a process under `steamapps/common/`, not SteamVR) | 3 GB more than it uses now |

Then (`twostage::fit`): the chosen model whole on the GPU when it fits; else a smaller downloaded action model that
fits whole; else llama.cpp's own fit (`--fit on --fit-target M`: as many layers as leave M MiB free, the rest on the
CPU, slower; checked: the 9B with `--fit-target 18000` took 3.8 GB, left 18.1 GB free and answered); below 1.5 GB of
room, the CPU. A limit ("up to N GB") caps the room. `gpu_layers = N` by hand wins over
all of it. While the model sits on the GPU, `omarchyvr-ai watch` checks every 15 s: when a game squeezes the free memory
under half its reserve, the model is fitted again (fewer layers, the smaller model or the CPU). It never grows back by
itself (the next start does), so it cannot flap.

### Settings (`~/.config/omarchyvr/ai.toml`, via `omarchyvr_config::ai`)

```toml
voice_local = "two-stage"      # simple (default) | two-stage; with [use] voice = a local provider

[local_two_stage]
router = "lara"                # lara | laya | qwen3.5-0.8b
action = "qwen3.5-4b"          # qwen3.5-4b | qwen3.5-9b
gpu_layers = "all"             # or a number (0 = CPU); "all" = the VRAM fit decides
vram_mib = "auto"              # or a limit in MiB
```

"AI runs on" writes only `[use] voice`, `voice_local` and this section; keys and other providers stay as they are
(`AiConfig::set_runs_on`, tested with a key in the file). `omarchyvr-voice act` checks the file every 2 s and loads the new
agent between two commands (no restart; a toast "AI: This PC, 2 stages" and the watch button follow).

### Where the switch is (user decision 2026-10-16)

- **Work only.** The local models belong to Work (the WayVR desk for work and movies). omarchyvrd's `ai.server` is a session
  component whose rule is "Work and Local models On": Play / SteamVR / games never start them, and a Work session that
  switches to Play stops them (`omarchyvr-ai down`). The switch is `voice_local_models` in ai.toml (omarchyvrd reads it each
  round; it wins over omarchyvrd.toml's `ai_server`). Off and no online choice: voice has no AI agent (offline commands).
- **App: Local AI** (navigation, next to Ready): the switch **Local models On / Off**; **How much of the PC the AI may
  use**: Low / Medium (recommended) / High, each with its download, RAM, video memory, speed and the audit's result;
  **Advanced: choose models**: Lara on / off (two stages or the model alone), the router and action model pickers
  (size, memory, result, licence, the recommended one marked), **Video memory for AI** (Auto / up to 4, 6, 8, 12 GB);
  **State**: Download (and tune), Start / Test (each stage's time, the VRAM in use) / Stop, Lara's and the model's
  state; **Online (Jev) instead**: a separate switch. Screenshot `local-ai.png`. The AI page's "AI runs on" card
  shows the current choice and opens this page.
- **WayVR, Work desk: the watch** (left wrist): "Local AI: On" (press: On / Off, `omarchyvr-voice ai-mode local-toggle`)
  and, while on, the preset button "Medium" (press: Low -> Medium -> High, `ai-mode preset-next`). Hidden unless Pimax
  voice shows it; `omarchyvr-voice act` sets the texts over `wayvrctl panel-modify watch ...` at start and on every change;
  `ai-mode` loads or unloads the models (`omarchyvr-ai up`, detached).

| preset | router + action model | download | memory | hard 40 / all | speed |
|---|---|---|---|---|---|
| Low | Lara + Qwen3.5-4B on the processor | 5.3 GB | ~5 GB RAM, no VRAM | 40/40, 452/454 (the 4B on the CPU) | ~1.2-1.5 s |
| **Medium** (default) | Lara + Qwen3.5-4B on the GPU | 5.3 GB | 2 GB RAM, 3.0 GB VRAM | 40/40, 458/460 (with 6 plans) | ~0.35 s |
| High | Lara + Qwen3.5-9B IQ4_XS on the GPU | 7.5 GB | 2 GB RAM, 4.7 GB VRAM | 38/40 (no better than Medium here) | ~0.35 s |

**Commands of several steps** ("make a new window and then open the terminal there", "go to the next tab and then
reload the page"): with "and" / "then" the model may answer a plan of up to 3 steps (section 3), run one after the
other with 400 ms between them. Lara names one category, so a plan of several steps is never filtered (its steps may be
of several categories); with tool calling (`voice_ai_style = "two-stage"`) such a request gets every tool. The audit
has 6 such plans (`tests/fixtures/audit.tsv`, want `a ; b`: every step, in order): 6/6 for the simple 4B on the GPU and
6/6 with Lara + 4B; agent tests check the order and that a single unsure step is still filtered.

**Why no plan ran live on 2026-10-16** (the first Work session with two stages): the plan grammar was offered only when
the words held "and" / "then", and none of that session's requests did ("can you focus youtube in a new window", "can
we have 2 more screens can we have 4 screens"); every request got the one-step grammar, and the missing actions (a
screen count, an app on a new screen) did not exist as intents, so the model picked the nearest single one. Since then:
plans have up to 4 steps (`nlu::PLAN_MAX`); "also", "plus", "afterwards" and two command verbs back to back ("close
this go to the main screen") offer the plan grammar too; a plan of several steps is never filtered by Lara (its steps
may be of several categories); the composite requests got intents of their own (`vr.screens.count/more/add`,
`win.screen.open`, `win.screen.gather`); and one toast lists every step's result. The session's 29 requests as an
audit fixture (`tests/fixtures/live-2026-10-16.tsv`, VOICE.md "Live session 2026-10-16").

### Benchmark (2026-10-16, `omarchyvr-voice audit --hard` and the full audit, dry run)

Setup: VR Work session running at the start (WayVR was stopped by someone else at 12:46, during the two-stage runs),
the machine busy (Lara's tuning on 16 cores at nice 19, other builds), dry run, 1 s between model requests. **Hard 40** =
the natural phrasings the matcher leaves to the model, spread over the intents (the online benchmark's selection,
`audit --hard`); **all** = every audit phrase (454: each intent's first phrase exact and mangled, plus
`tests/fixtures/audit.tsv`; ~133 reach a model). The dry run reads the live desktop, so an action that needs a focused
window fails when none is focused; the table counts **right decisions** (the model's intent; the HTML pages also show
the strict pass rate). Latency: end of the transcript to the decision, model requests only.

| setup | hard 40 | all 454 | p50 / p95 | VRAM | CPU per command |
|---|---|---|---|---|---|
| This PC, simple: Qwen3.5-4B on the CPU (24 threads) | 40/40 | 452/454 | 1.24 / 1.77 s | 0 | 21.9 s |
| This PC, simple: Qwen3.5-4B on the GPU | 40/40 | 452/454 | 0.20 / 0.26 s | 3.0 GB | 0.1 s |
| This PC, simple: Qwen3.5-9B on the GPU | 39/40 | 452/454 | 0.25 / 0.36 s | 4.7 GB | 0.2 s |
| **Two stages: Lara + Qwen3.5-4B GPU (Medium)** | 40/40 | 458/460 | 0.33 / 0.39 s | 3.0 GB | 2.6 s (2.5 s Lara) |
| Two stages: Lara + Qwen3.5-9B GPU (High) | 38/40 | - | 0.33 / 0.36 s | 4.7 GB | 2.7 s (2.6 s Lara) |
| Two stages: Laya (published) + 4B GPU | 40/40 | 452/454 | 0.46 / 0.86 s | 3.0 GB | 3.5 s (3.4 s Lara) |
| Two stages: Laya (published) + 9B GPU | 39/40 | 451/454 | 0.45 / 0.56 s | 4.7 GB | 3.4 s (3.3 s Lara) |
| Two stages: Qwen3.5-0.8B router + 4B GPU (router missed its deadline every time) | 40/40 | - | 1.03 / 1.04 s | 3.0 GB | 0.1 s + router (not measured) |
| Two stages, stage 2 = tool calling (the online design): Laya + 4B GPU | 23/40 | - | 0.86 / 2.12 s | 3.0 GB | 4.0 s (3.4 s Lara) |
| Online (section 14, 2026-10-15): jev-router + gpt-6-luna | 37/40 | - | 3.4 / 5.1 s | 0 | (remote) |

Router alone (`omarchyvr-voice routerbench`, stage 1 only; "offered" = the right category among the ones Lara passes on).
| router | categories right | offered | with the matcher's guess | p50 |
|---|---|---|---|---|
Natural phrasings (242, the audit's own; never in Lara's training rows) and the hard 40:

| router | categories right | offered | with the matcher's guess | p50 (busy CPU) |
|---|---|---|---|---|
| Laya as published | 57 % (hard 45 %) | 64 % (hard 58 %) | 95 % (hard 90 %) | 0.33 s |
| Qwen3.5-0.8B (llama-server, CPU) | 57 % (hard 40 %) | 57 % (hard 40 %) | 94 % (hard 92 %) | 0.82 s |
| **Lara (Laya tuned here, 3 epochs; the installed one)** | **94 %** (hard 92 %) | **95 %** (hard 92 %) | 98 % (hard 98 %) | 0.31 s |
| the matcher's guess alone | 89 % (hard 88 %) | | | 0 |

A first measurement of Lara gave ~72 %: the requests did not name the checkpoint, and laya-serve sends a sentence it
recognises as English to its published `english` checkpoint even when the tuned one is the default. The router now
names it (`"model": "lara"` / `"english"`, `router::laya_body`). Lara's hard 40 include 5 ASR-mangled catalogue
phrases that were in its training rows (35 are natural phrasings it never saw).

**What the data says.**
- The simple request on the GPU is already right for 40/40 hard phrases (4B) and 452/454 of all, in 0.2 s; the 9B is
  no better here (39/40) and needs 4.8 GB instead of 3.0 GB. On the processor the same 4B takes ~1.3 s (section 13).
- Two stages with Lara and the 4B on the GPU decide every hard phrase (40/40) and 458/460 of all (the 6 plans
  included) in 0.33 s: Lara adds ~0.13 s. The model alone is as accurate on this audit (its prompt was tuned on these
  phrases); Lara is the safety net for wording the model is unsure about.
- Stage 2 as tool calling (the online design) is much worse locally: 23/40, 0.86 s (a 4B picks "help" when the right
  tool is missing, and every category change re-reads ~2,000 tokens of tool schemas).
- Laya as published sorts 57 % of the unseen natural phrasings into the right category; tuned here (Lara, 3 epochs,
  ~65 min of 16 cores) **94 %** (hard 45 % -> 92 %), better than the matcher's own guess (89 %), which stage 2 still
  gets as well.
- The 0.8B LLM router is no better than Laya (57 %) and slower; on the busy processor it missed the router deadline
  every time (`Lara timed out`, the model answered alone, still 40/40). The deadline was 1 s then (curl took whole
  seconds); `curl_seconds` now passes 1.5 s.
- VR frames: the GPU model costs skipped compositor frames while it computes (0 to ~6 per command in these runs,
  ~10 for the 9B; it depends on what the compositor does at that moment); Lara on the processor costs none.

**Defaults** (from the data): Local models **On** in Work with the **Medium** preset: Lara (tuned; Laya as published
runs while it tunes) + Qwen3.5-4B on the GPU, video memory **Auto**. High (the 9B) costs 1.7 GB more video memory and
was not more accurate here; Low keeps the GPU free for 1-1.5 s per command. The online mode (jev-router + gpt-6-luna)
was 37/40 at 3.4 s p50 / 5.1 s p95 (section 14); its key has expired.

The report with every run: `analysis/voice-ai-local-two-stage.html`; the raw audit pages are in the run directories of
the bench script (not kept).

## 17. Speech recognition on the GPU (2026-10-16)

User request (2026-10-16): a "Large GPU" speech-recognition choice next to the local AI in Work. `omarchyvr-voice listen`
used Parakeet TDT 0.6B v3 int8 on the processor; it now has a second engine, whisper.cpp's `whisper-server`
(large-v3-turbo, Vulkan), and Parakeet gets hotwords.

### Benchmark (60 Kokoro TTS commands, and the same with noise; scripts and results in the session scratchpad `asr/`)

| engine | WER clean | WER noisy | p50 / p95 | VRAM |
|---|---|---|---|---|
| Parakeet v3 int8 CPU (greedy, before) | 3.6% | 18.5% | 129 / 185 ms | 0 |
| Parakeet + hotwords (modified_beam_search, score 1.5) | 2.7% | 13.3% | 138 / 203 ms | 0 |
| whisper large-v3-turbo q5_0, GPU (Vulkan, whisper-server) | 6.4% | 10.3% | 43 / 69 ms | 835 MB |
| distil-large-v3, GPU | 3.6% | 10.6% | 36 / 52 ms | 1.7 GB |

Through `omarchyvr-voice transcribe` (the whole path: Silero VAD, segments, partials, the engine; WER on the final of each
clip) the numbers are different, because the VAD cuts and the wake word ("Max") sits first:

| `--engine` | clean | noisy | decode p50 / p95 |
|---|---|---|---|
| parakeet --no-hotwords (greedy) | 7.0% | 21.2% | 120 / 196 ms |
| parakeet (hotwords 1.5, the CPU option) | 6.1% | 17.0% | 132 / 198 ms |
| whisper-server (GPU) | 8.2% | 14.5% | 44 / 71 ms |

So the GPU model is clearly faster (3x) and better in noise; on clean speech the hotword Parakeet is as good or better
(Whisper writes "Re-center", "Mac" for "Max"). The server is English only (`-l en`); Parakeet also hears Spanish.
distil-large-v3 is not used: no better than turbo in noise and twice the memory.

### Integration

- **Engine** (`rust/omarchyvr-voice/src/engine.rs`, no cargo feature): `EngineSpec::WhisperServer { port, timeout_ms,
  fallback }`. Per segment (partials and finals both): a WAV (16 kHz, 16-bit, mono, in memory, padded to 1.1 s) goes as
  multipart `file` + `response_format=json` + `temperature=0.0` to `POST http://127.0.0.1:8082/inference`; connect
  timeout 250 ms, answer timeout 4 s. On any failure the segment is decoded by the Parakeet fallback (always loaded)
  and the server is left alone for 3 s, then tried again, so voice never stops and a server that starts later is used
  by itself. voice.log gets one line on each change ("asr: GPU server not usable (...); Parakeet on the processor
  answers" / "asr: the GPU server answers"). Partials use the server too (40 ms instead of 130 ms, no processor load).
  Bracketed sounds ("[BLANK_AUDIO]", "(silence)") are dropped.
- **Server flags**: `-nc -l en -nt --host 127.0.0.1 --port 8082` (without `-nc` the server feeds earlier output into the
  next request and degrades), at nice 19 / ionice. Binary `~/src/whisper.cpp/build-vk/bin/whisper-server`, model
  `~/models/whisper/ggml-large-v3-turbo-q5_0.bin` (not downloaded by the app yet; a missing file is a plain-words
  message and the small model answers).
- **Lifecycle** (same rule as section 16): a third slot of `omarchyvr-ai` (`Slot::Asr`, port 8082, `asr.pid`,
  `ai-asr.log`), started and stopped by omarchyvrd's `ai.server` through `omarchyvr-ai up` / `down`: Work only, with Local
  models On. `up` decides first and counts the server's 0.9 GB in the fit of the action model. `omarchyvr-ai stop --asr`,
  `omarchyvr-ai status` (line "speech") and `status --json` (`asr`, `asr_missing`) show it.
- **Setting** (`ai.toml`, `omarchyvr_config::ai::Asr`): `asr = "auto" | "cpu" | "gpu"`, default auto (key absent). **Superseded by section 19: the default is the processor; `auto` now means `cpu`.**
  Auto: the large model when Local models are On and at least 1.5 GB of video memory is free, else the small one. `cpu`
  never starts the server; `gpu` starts it when 0.9 GB + margin is free and the local models are On. Local models Off:
  no server (Work only rule), the small model answers.
- **`omarchyvr-voice listen`/`transcribe`** read `asr` at start (`--engine auto`, the default): small = Parakeet with
  hotwords; auto/gpu = the server engine with the Parakeet fallback; the engine line goes to voice.log and the `hello`
  event. Other commands (bench, audit) use fixed Parakeet. `--engine parakeet --no-hotwords|whisper-server`,
  `--asr-port`, `--asr-timeout-ms` override. A change of `asr` reaches the server at once (the app runs `omarchyvr-ai up`)
  and the engine at the next voice start.
- **Hotwords** (CPU option): sherpa-onnx C API fields exist (`hotwords_file`, `hotwords_score`, `modeling_unit`,
  `bpe_vocab`); `SttConfig.hotwords` switches to `modified_beam_search`, score 1.5 (2 to 8 were worse). Words: Max,
  passthrough, screens, workspace, YouTube, Chrome, terminal, foot, cinema, Omarchy (`engine::HOTWORDS`); the files
  (`hotwords.txt`, `bpe.vocab` built from tokens.txt) are written to `~/.cache/omarchyvr/voice/`.
- **App**: Local AI page, card "Speech recognition": Auto / Small (processor) / Large (graphics card) with the numbers
  of each, the decision in plain words, a chip for the server and the missing-file message. Help line in the app's
  help. Not done: a WayVR watch button (it would need `act.rs`, edited elsewhere), a Download button for the model and
  server.

## 18. Agent-mode model: Qwen3.6-27B as an option (2026-10-16)

User request: try a quantised ~27B Qwen for agent mode (multi-step goals) next to the current models, and compare.

**The model.** Qwen3.6-27B exists (Qwen, Apache-2.0). GGUF `unsloth/Qwen3.6-27B-GGUF`, revision `82d411a`, file
`Qwen3.6-27B-Q4_K_M.gguf`, 15.66 GiB (16,817,244,384 bytes, SHA-256 `5ed60d0a...392a0`, checked), in
`~/.local/share/omarchyvr/ai/models/Qwen3.6-27B/`. Measured 16.7 GB of VRAM with a 16k context.

**Wiring (opt-in only).** `[local_two_stage] agent = "qwen3.6-27b"` (default: absent = off). `AiConfig::set_agent(Some(id))`
(for the Local AI page's picker) writes that key plus `voice_agent_url = http://127.0.0.1:8083/v1/chat/completions` and
`voice_agent_model`, which voice reads (goal steps only; simple commands stay on Lara + 4B; the usual model is the
fallback while the 27B is not up); `set_agent(None)` removes all three. `omarchyvr-ai up` starts the server (port 8083, slot
`agent`) only when the setting is on, the file is downloaded, the session is none or the Work desk (never SteamVR or a
game; the watch stops it when one appears) and the free VRAM covers 18 GB plus the session's reserve. Presets, Play
and Work never set it. `omarchyvr-ai down` stops it. Download: `omarchyvr-ai install --two-stage` fetches it when the setting is on.
Still to do on the app side (not touched here, another agent's files): a "27B agent model" switch on the Local AI page
calling `set_agent`, with its size, 17 GB VRAM and the result below.

**Benchmark** (`omarchyvr-voice audit --url ... --pause-ms 200`, dry run, one model at a time on port 8083, VR closed, Lara on
the processor, 4B of the user's own server left running; scripts in the session scratchpad). Sets: **hard 40** (the
two-stage set), **agent 7+10** (`agent-2026-10-16.tsv`: 7 goals, 10 single commands), **long 12** (`agent27b-2026-10-16.tsv`:
goals of 3 to 8 steps). Wrong actions = steps that ran but are not in the goal; per-step time = goal time / steps run.

| model | hard 40 | agent set: goals / singles | long set: goals ok | steps per goal | wrong actions / missed | time per goal (mean) | per step | first token (mean) | load | VRAM |
|---|---|---|---|---|---|---|---|---|---|---|
| Qwen3.5-4B Q4_K_M | 39/40 | 5/7, 10/10 | 2/12 | 4.6 | 9 / 2 | 1.83 s | 0.40 s | 81 ms | 2 s | 3.3 GB |
| Qwen3.5-9B IQ4_XS | 36/40 | 5/7, 10/10 | 2/12 | 4.5 | 8 / 2 | 1.95 s | 0.43 s | 101 ms | 8 s | 5.1 GB |
| Qwen3.6-27B Q4_K_M | 33/40 | 5/7, 10/10 | 2/12 | 4.7 | 10 / 2 | 2.34 s | 0.50 s | 284 ms | 5 s (cached file) | 16.7 GB |

(The 4B/9B/27B hard-40 here uses the audit's prompt, which was tuned on the 4B; the 27B answers "none" with 9/9
confidence for plain commands such as "refresh this page" and "copy the selection". Loads are warm-cache; a cold read of
15.7 GB from disk takes much longer. The 4B's 0.35 s per command in section 16 is the single-command path.)

**What the data says.** The goal results are the same for all three models: the same two agent-set goals and ten long
goals fail with the same cause, a stray first step (`win.screen.put app=wrap`, `vr.passthrough.brighter`, `omarchy.screenshot`,
`media.play q=session`, a doubled `vr.preset.work3`) that comes from the goal loop (parts and hints, the "let's /
get ready" header words), not from the model. A bigger model does not fix this; the loop does (report to the voice
agent). The 27B is 4 to 6 times slower per step and takes 16.7 GB of the 24 GB, leaving ~4 GB with the 4B loaded: not
enough for a Work desk with passthrough and the compositor plus headroom.

**Recommendation.** Do not use the 27B for agent mode now: no accuracy gain here, slower, and too much VRAM to keep
loaded in Work. Keep it as an opt-in experiment for sessions without a headset (Session none), loaded on demand and
stopped before Play/Work. Revisit after the goal loop's stray-step problem is fixed, with a prompt written for the bigger
model (its "none" answers) and thinking enabled for the planning step only. The 9B is no better than the 4B either.

## 19. Speech on the processor by default; "Speech on graphics card" switch (2026-10-16)

User decision 2026-10-16: speech recognition runs on the processor by default, with a switch for the graphics card.
Why: in the 18:08 session the GPU work (llama + whisper-server on Vulkan, plus an audit) made Monado skip 8.8% of its
frames at 90 Hz and the VR screens shook (the session's perf.log frame-health timeline). The PC: i9-14900KF (32 threads, no iGPU), RTX 4090.

### Benchmark on the processor (no VR session running; 60 Kokoro TTS commands, clean and noisy, same scorer as section 17)

whisper.cpp `build-cpu` (AVX2 native, no BLAS), `nice -n 19`, 8 threads, one clip per request, server on port 8095;
Parakeet through sherpa-onnx with hotwords (score 1.5). WER includes the wake word "Max". At normal priority the
latency is the same within a few percent on an idle machine (nothing else wants the cores); under load nice 19 would
be slower. A 3 s command is about the average clip here.

| engine | device | WER clean / noisy | p50 / p95 |
|---|---|---|---|
| **Parakeet TDT v3 int8 + hotwords, 2 threads (the default)** | CPU | 6.4% / 15.2% | 134 / 195 ms |
| Parakeet + hotwords, 4 threads | CPU | 6.4% / 15.2% | 108 / 169 ms |
| Parakeet + hotwords, 8 threads | CPU | 6.4% / 15.2% | 116 / 177 ms |
| whisper base.en, `-ac 512` | CPU | 9.7% / 23.0% | 157 / 195 ms |
| whisper base.en (q8, multilingual) | CPU | 10.9% / 29.7% | 513 / 766 ms |
| whisper small.en, `-ac 512` | CPU | 8.2% / 21.8% | 496 / 604 ms |
| whisper small.en | CPU | 9.1% / 22.1% | 2051 / 4440 ms |
| whisper small q8 (multilingual) | CPU | 8.5% / 19.7% | 1541 / 2931 ms |
| whisper large-v3-turbo q5_0, `-ac 512` | CPU | 8.2% / 13.3% | 2372 / 2556 ms |
| whisper large-v3-turbo q5_0 | CPU | 9.7% / 12.1% | 9867 / 12359 ms |
| whisper large-v3-turbo q5_0 (reference) | GPU | 9.4% / 12.1% | 43 / 69 ms |
| distil-large-v3 | CPU | 7.3% / 12.4% | 8467 / 8738 ms |
| distil-large-v3 (reference) | GPU | 7.3% / 12.4% | 36 / 52 ms |

Reading: whisper keeps its 30 s window, so on the processor its cost does not depend on the clip length; only the
audio-context cut (`-ac 512`) makes small and base usable, and they are less accurate than Parakeet. Large-v3-turbo
needs 2.4 to 10 s per command on the processor: out of the question. Parakeet is both the fastest and the most
accurate on the processor (6.4% clean, 15.2% noisy), is a 2-thread in-process engine (no server, no port, no VRAM, English
and Spanish) and is the budget winner under the 600 ms p95 target by a wide margin.

**Default: Parakeet TDT v3 int8 with hotwords, 2 threads** (`omarchyvr-voice --threads`, default 2; 4 threads would give 108 ms
but 2 leaves more of the 32 threads alone). Monado's threads are SCHED_FIFO (cap_sys_nice) and preempt any normal
thread, so this CPU work cannot take their time slices; the part that is shared is memory bandwidth and caches, which
Parakeet barely touches.

### What changed

- **Setting**: `asr = "cpu" | "gpu"` in `~/.config/omarchyvr/ai.toml` (`omarchyvr_config::ai::Asr`, `AiConfig::asr_on_gpu()`). The
  default is the processor and the key is absent. **Migration**: the old default `auto` (no key, or `asr = "auto"`,
  which picked the GPU server whenever 1.5 GB were free) now means the processor; only an explicit `asr = "gpu"`
  (chosen by hand in the old selector) stays on the graphics card. No file is rewritten.
- **App**: Local AI page, card "Speech recognition": one switch "Speech on graphics card" (default Off) with the "?"
  "GPU is faster but can make VR stutter.", the measured numbers of both, the plain-words decision, and the state of the
  graphics-card server. Saved to ai.toml at once, so `omarchyvr-ai`, `omarchyvr-voice` and the CLI share it; each Work start
  and each restart of the session (omarchyvrd's `ai.server`, `omarchyvr-ai up`) re-reads it, so it needs no flag in
  `start-argv.txt` (a restart keeps it because it lives in ai.toml, not in the argument list).
- **Wiring**: `omarchyvr-ai up` (Work start, the `ai.server` health watch, the Local AI buttons, the resume after Game mode)
  starts the whisper-server on port 8082 only when the switch is On (`asr_decision`); Off stops it. Game mode / Play
  never start it (they only bring back what `up` decides). `omarchyvr-voice listen` uses Parakeet unless the switch is On
  and the local models run, in which case it asks the server and falls back to Parakeet as before. The voice client,
  port and API are unchanged.
- Not kept: a whisper CPU server on port 8082. It would be slower and less accurate than Parakeet, so no second
  engine is started on the processor.

