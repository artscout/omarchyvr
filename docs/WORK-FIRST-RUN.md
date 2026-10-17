# The Work first run (2026-10-17)

User request: once the setup is finished the user clicks Play or Work. The first time Work is pressed, the app asks
whether to install Lara and which AI answers (Claude, a local model, or any other compatible API), and how speech
becomes text. Only for Work, never for Play; asked once, changeable later.

Code: `rust/omarchyvr-gui-core/src/worksetup.rs` (the questions, the answers, the downloads), the page
`rust/omarchyvr-app/src/pages/worksetup.rs`, the record `rust/omarchyvr-config/src/work_setup.rs`, the downloads
`omarchyvr-ai get` (rust/omarchyvr-ai/src/main.rs).

## What the user sees

1. Home: **Play** and **Work**, two big buttons. Play starts games right away (no questions, ever).
2. The first **Work** opens "Set up Work" (the user is still at the desktop, not in the headset):
   - **Voice assistant**: who answers what the built-in commands do not know ("Max, four screens" never needs an AI):
     - **Claude: the Claude Code program on this PC**: runs your installed `claude` with its own sign-in (your Claude
       account; it counts on your plan). The app never reads or copies that login (`~/.claude` is not touched; no
       token is passed). Needs the internet. Test sends one fixed line.
     - **Claude with my Anthropic API key**: your own key, billed to your Anthropic account; model Haiku (default),
       Sonnet or Opus; Test.
     - **A model on this PC (Qwen3.5-4B)**: 2.6 GB download, nothing leaves the PC, the processor by default (no video
       memory, about 1.3 s per request); "Use the graphics card" is an explicit switch (about 0.35 s, about 3 GB of
       video memory while Work runs, never during Play).
     - **Another compatible API**: base URL, key, model (OpenRouter, a hosted model, a server on the network); Test.
     - **None**: the built-in commands only.
     - **Install Lara** (2.7 GB, Apache-2.0, processor only): sorts each command on this PC before the brain acts, so the
       brain sees only the right tools. With Claude Code too: Lara goes first (it answers the commands it knows at
       once), only the requests it does not handle go to Claude Code; without Lara everything goes to Claude Code; with None it has nothing to sort for.
       After its download it tunes itself to the voice commands in the background (about an hour, processor, low
       priority); Laya as published answers meanwhile.
   - **Speech to text**: on this PC, the processor (Parakeet, recommended, no video memory); on the graphics card
     (whisper large-v3-turbo, about 0.9 GB of video memory while Work runs, never during Play; offered only when the
     installer built it and with the model on this PC: its server runs next to the local model, with another brain it
     would silently be the processor); or off (no voice commands, no subtitles).
3. **Next** lists what will be downloaded (only what is missing) and the button says the size: "Download 5.3 GB and
   start Work", or "Start Work" when everything is already on this PC.
4. **Download** shows each file with its progress, the total, and **Cancel** (the part already downloaded stays; the
   next try goes on from there). A failure says why in plain words, with **Try again** and **Back**.
5. Saved, then **Work starts by itself**. The next Work presses start at once.
5b. Already set up: when `~/.config/omarchyvr/ai.toml` exists with a configured brain (a provider, a default or a voice
   entry), the questions count as answered and the first Work starts at once (`worksetup::ai_configured`).
6. Change the answers later: AI page > **Work questions** (the same page, it saves without starting Work).

Screenshots (`omarchyvr-app --screenshots DIR`): `worksetup-assistant.png` (the default: Claude Code),
`worksetup-assistant-local.png` (a local model with Lara), `worksetup-assistant-api.png` (another API, tested),
`worksetup-speech.png` (speech, and what will be downloaded), `worksetup-download.png` (progress, Cancel),
`worksetup-download-error.png` (a failed download, Try again).

## Where the answers go

Everything goes into the shared settings, so voice control, `omarchyvr-ai`, omarchyvrd and the CLI read the same:

| Answer | ai.toml (`~/.config/omarchyvr/ai.toml`, mode 600) | Elsewhere |
|---|---|---|
| Claude Code | `[use] voice` = the Claude CLI provider, `voice_local_models = false` | |
| Claude with a key | `[providers.claude]` kind anthropic, `api_key`, `model`; `[use] voice = "claude"`; `voice_local_models = false`; `voice_lara = true/false` | |
| A model on this PC + Lara | `voice_local_models = true`, `voice_local = "two-stage"`, `[local_two_stage]` preset Low (processor) or Medium (graphics card) | |
| A model on this PC, no Lara | `voice_local_models = true`, `voice_local = "simple"`, `[local_two_stage] gpu_layers = 0` (processor) or `"all"` | |
| Another API | `[providers.api]` kind openai-compatible, `base_url`, `model`, `api_key`; `[use] voice = "api"`; `voice_lara` | |
| None | `voice_ai = "off"` | |
| Speech: processor / graphics card | `asr` absent / `asr = "gpu"` | |
| Speech: off | | the app's Work switch voice off (no `--voice`) |
| answered | | `~/.config/omarchyvr/work-setup.toml` (`done`, the answers; never a key) |

Other providers, their keys, Autofix's choice and every key or section the app does not know stay as they are. A key
is only ever in ai.toml (written atomically with mode 600); the typed field is wiped once saved or when the page
closes; it is never in a log, a report or the app's debug output (tested).

**Lara with an online brain** (`voice_lara = true` with Claude through a key or a compatible API): the router on this
PC sorts each command (port 8081, processor), the online model acts with that category's tools only (the two-stage
path of AI.md section 14 with Lara as stage 1 instead of an online classifier). If Lara is down, the matcher's guess
takes its place and the online model still answers. `omarchyvr-ai up` then starts the router alone (no model slot, no
speech server, no video memory). omarchyvrd's ai.server follows `ai_servers_wanted` (local models on, or Lara with an
online brain), in Work only.

## Downloads

`omarchyvr-ai get ITEM...` (items `runtime`, `lara`, `qwen3.5-4b`) is the one implementation: the same pinned files,
sizes and SHA-256 checks as `omarchyvr-ai install` (a file with the right size and digest is kept, a `.part` file is
resumed with `curl -C -`, a mismatch is deleted and refused). Lara: the pinned Python packages (PyTorch CPU wheel,
`laya==0.4.8`) and the checkpoint at its reviewed revision; each step is skipped when done. One event per line on
stdout (`get plan|progress|note|done|error|cancelled|all done`); SIGTERM cancels (the app sends it to that one child,
by its pid; curl and pip are stopped by their pid / process group; exit code 130).

`./install.sh` no longer downloads the AI models by default (it used to run `omarchyvr-ai install --two-stage --router
lara`): `--ai-models` does it for agents and tests. Models already on disk are never removed and are found by the
first run (it shows "already on this PC").

## For agents (the CLI behind the page)

```sh
omarchyvr-ai get lara qwen3.5-4b      # what the page downloads, with progress lines
omarchyvr-ai up                       # what ai.toml asks for (refused while Play runs; --force overrides)
omarchyvr vr gamemode status          # Play's settings (light, eye in games)
```

## Not done

- The graphics-card speech server is offered only when the installer built it (`./install.sh --gpu-speech`); the page
  cannot download or build it.
- The Claude CLI path does not use Lara (its own state loop sends the whole action list, which the CLI caches).
- Not tried with the real downloads on this PC by the agent that wrote it (demo mode and tests only).
