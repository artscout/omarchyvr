# Pimax AI: "Max ..." with a model and tools

Voice control has an AI agent. You say "Max, ..." and the agent reads the request together with what you are looking at,
your windows and the VR session. Then it acts through tools, under the same safety rules as the spoken command
catalogue. The model can run on this machine (`pimax-ai`, llama.cpp) or be Claude or OpenAI.

- Code: `rust/pimax-ai` (install / serve / status / stop of the local model) and `rust/pimax-voice/src/agent.rs` (the
  agent).
- Settings: `~/.config/pimax/ai.toml`, written by the Pimax app's **AI tab** (`pimax_config::ai`).
- Log: `~/.local/state/pimax/logs/ai.log`.

## 1. The local model

**Recommended while VR runs: `pimax-ai serve --cpu`** (the 4B on 24 CPU threads at nice 19; section 11 has the
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
cd ~/pimax-re/rust && cargo build --release -p pimax-ai
target/release/pimax-ai install            # 4.1 GB into ~/.local/share/pimax/ai; resumes; run again after a failure
target/release/pimax-ai install --dry-run  # what it would fetch
target/release/pimax-ai serve              # GPU, all layers, 127.0.0.1:8080, waits until loaded
target/release/pimax-ai serve --cpu        # no GPU at all (the 4B, 3/4 of the CPU threads): the VR default
target/release/pimax-ai serve --gpu-layers 20   # part on the GPU
target/release/pimax-ai status [--json]
target/release/pimax-ai test               # one tiny request, prints the latency
target/release/pimax-ai stop
```

The server:
- listens on 127.0.0.1 only;
- runs under `nice -n 19`, `ionice -c 3` (idle I/O) and `--prio -1`, with one slot and an 8k context;
- uses `--jinja` (tool calling through the model's chat template), with `--reasoning off` and
  `enable_thinking: false`.

Files:
- pid file: `~/.local/state/pimax/ai/llama-server.pid` (`stop` checks that the PID is a llama-server before signalling);
- server log: `~/.local/state/pimax/logs/llama-server.log`.

It answers to any model name; `pimax-local` is its alias.

A systemd user unit is in `packaging/systemd/pimax-ai.service`. It is not installed; its header says how to try it.

**GPU budget next to VR.** The RTX 4090 has 24 GiB. A desk session (Monado, WayVR, passthrough) uses a few GiB, and the
agent adds about 3.5 GiB, capped by design at 4 GiB (`pimax-ai status` prints the actual figure from `nvidia-smi`). The
model only computes while a request runs, for a fraction of a second after "Max ...". If frame time suffers anyway,
use `serve --cpu`, or `--gpu-layers N` with a smaller N.

## 2. Settings (`~/.config/pimax/ai.toml`)

Do not create the file by hand unless you want to. The AI tab writes it (mode 0600). An example:

```toml
voice_ai = "fallback"        # off | fallback | always
voice_confirm = "none"       # none | dangerous | always: when a command waits for a spoken "yes" (AI page: Confirm)
strip_titles = false         # true: window titles never leave the machine (class + workspace only)
default = "local"

[use]
voice = "local"              # the provider voice uses (else `default`)

[providers.local]            # pimax-ai serve
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
- `pimax-voice act` reads the file at start, so restart it after a change. It prints a line like
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
- `cache_prompt` is on, and `pimax-voice act` primes the cache at start and again whenever `/health` turns ready
  (server restart).
- Lessons from the bench: the model copied the literal answer-format placeholder ("INTENT ...") and any id-like text
  in the context ("vr: preset cinema" made it answer `vr.preset.cinema` for "give me three screens"; a "last
  command" line made it repeat the last command). So the format is shown by example, the VR line is worded unlike
  the ids, and there is no last-command line.
- Window lines change rarely, but Qwen3.5 is a hybrid (recurrent) model: llama.cpp restores its state only from
  checkpoints, which in practice sit at the end of the system prompt, so the whole user message is evaluated each
  time. Hence the short message (no window list unless the request is about windows; leading spinner glyphs of
  terminal titles are dropped anyway).

`pimax-voice aiprompt WORDS...` prints the request for some words.

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
    (running, preset, background, passthrough, captions; from pimaxd: overall health, side, components up or down,
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

`~/.local/state/pimax/logs/ai.log` gets one JSON line per request: request, provider, mode, context (compact: the user
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

1. Leave VR running. If you do not want the GPU used, run `pimax-ai serve --cpu`. Otherwise run `pimax-ai serve`;
   `pimax-ai status` then shows the VRAM.
2. In the AI tab: provider local, voice AI = fallback. Or write the example above.
3. Restart the voice actor; its log shows "AI agent: ...".
4. Try the examples. Check that "close" and "lock" ask for "yes", and that `ai.log` has the lines.
5. Dry run without touching anything:
   `pimax-voice do "Max I want to watch a film" --dry-run`.
   This sends a model request, but the actions are printed, not run.

## 10. AI tab integration (for the GUI)

**Keys in ai.toml** (all in `pimax_config::ai`):
- `voice_ai` (off | fallback | always): show it as a 3-way toggle.
- `strip_titles` (bool): a privacy checkbox. Show the section 6 list next to it when the voice provider is remote.
- `[use] voice` and `default` pick the provider.
- For the local provider, write `kind = "openai-compatible"`, `base_url = "http://127.0.0.1:8080/v1"` and
  `model = "pimax-local"`.
- Optional, preserved if present: `voice_ai_timeout_s`, `voice_ai_gaze`, `voice_ai_log`. The current parser warns about
  them and drops them on save, so please keep unknown top-level keys.

**Local model controls:** run `rust/target/release/pimax-ai <cmd>` (or `~/.cargo/bin/pimax-ai`). All are user-level
and need no sudo.

| Button | Command | Notes |
|---|---|---|
| Install | `pimax-ai install` | 4.1 GB; progress bars on stderr; text lines on stdout; can be run again to resume |
| Start | `pimax-ai serve` (GPU) / `pimax-ai serve --cpu` | returns after "ready" (or "still loading"); exit 1 + message on error |
| Stop | `pimax-ai stop` | |
| Status | `pimax-ai status --json` | poll every few seconds |
| Test | `pimax-ai test` | prints "<ms> ms: <reply>" |

`pimax-ai status --json` prints one line:

```json
{"installed":true,
 "runtime":"/home/u/.local/share/pimax/ai/runtime/runtime-vulkan-b11630/llama-b11630/llama-server",
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

`pimax-voice aibench` sends the phrases of `rust/pimax-voice/tests/fixtures/aibench.tsv` (45 real and mangled
transcripts: the live ones from voiceact.log plus paraphrases and noise) through the full route against the running
server, executing nothing (hyprctl and gaze are read live). Latency = end of the transcript to the decision.

```sh
pimax-voice aibench --runs 3                      # ai.toml's provider
pimax-voice aibench --url http://127.0.0.1:8081/v1/chat/completions --label "variant"   # another server
pimax-voice aibench --style tools --timeout-s 10  # the old tool-calling request
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
- **VR frame timing** (`/run/user/1000/pimax-perf.stats`, 8 s windows): idle 0 skipped frames; CPU 16 or 24 threads
  back to back: 0 skipped, late ratio unchanged; GPU back to back (53 requests): 53 skipped frames and late frames
  35 % -> 78 %, i.e. **about one skipped frame per GPU request**.
- **"Vulkan at 8 tokens/s next to Monado" was the first request after a server start**: Vulkan builds its pipelines
  (the earlier log's task 0: 19 tokens in 2.3 s; the same session's next request ran 5,675 tokens at 345 tok/s,
  106 tok/s generation). Warm, the GPU does 800-2,100 tok/s prompt and ~95 tok/s generation even with VR running.
  llama.cpp has no Vulkan queue-priority option, and NVIDIA time-slices between processes, so the compositor still
  loses a frame when a request lands; `--gpu-layers N` (partial offload) would only add CPU-GPU transfers. The GPU
  stays an option (`pimax-ai serve`) when a dropped frame per command is acceptable.

## 12. The debug line under the captions

With `voice_ai_debug = true` (the default) `pimax-voice act` writes one line per command to
`$XDG_RUNTIME_DIR/pimax-voice-debug.line`, and `pimax-voice captions` shows it under the newest transcript line, in
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

"Max, debug off" / "Max, debug on" toggle it (also in `~/.local/state/pimax/logs/ai.log` as `trace` lines, always).
The captions panel exists only in sessions started with voice (`pimax desk start --voice` or the assistant preset).

## 13. Checking every command (`pimax-voice audit`, 2026-10-15)

`pimax-voice audit` (the AI page's **Check all voice commands**; docs/VOICE.md) runs each enabled intent's first
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
local. Code: `rust/pimax-voice/src/remote.rs`, `Agent::two_stage`.

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
`wayvrctl gaze` and pimaxd's snapshot (`$XDG_RUNTIME_DIR/pimaxd/state.json`, re-read at most once a second, ignored
when older than 30 s). The passthrough socket is not asked (it can take 2 s).

**Cost.** OpenRouter requests carry `usage: {include: true}` and `reasoning: {enabled: false}`; `usage.cost` is added
up per command (ai.log `cost`, the trace, the audit's total).

**Benchmark** (`pimax-voice audit --remote --max 40 --action-model M`: the same 40 natural phrasings the matcher
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
pimax-voice audit --remote --max 40 --pause-ms 1000 --out analysis/voice-audit-remote.html
pimax-voice audit --remote --action-model anthropic/claude-haiku-5.5    # another stage-2 model
pimax-voice audit --remote --log --fixture my.tsv     # only these phrases; ai.log keeps requests and costs
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
