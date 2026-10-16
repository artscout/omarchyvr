# Agent-mode model: Qwen3.6-27B vs 4B and 9B (2026-10-16)
User request: try a quantised ~27B Qwen for agent mode (multi-step goals) next to the current models, and compare.

**The model.** Qwen3.6-27B exists (Qwen, Apache-2.0). GGUF `unsloth/Qwen3.6-27B-GGUF`, revision `82d411a`, file
`Qwen3.6-27B-Q4_K_M.gguf`, 15.66 GiB (16,817,244,384 bytes, SHA-256 `5ed60d0a...392a0`, checked), in
`~/.local/share/pimax/ai/models/Qwen3.6-27B/`. Measured 16.7 GB of VRAM with a 16k context.

**Wiring (opt-in only).** `[local_two_stage] agent = "qwen3.6-27b"` (default: absent = off). `AiConfig::set_agent(Some(id))`
(for the Local AI page's picker) writes that key plus `voice_agent_url = http://127.0.0.1:8083/v1/chat/completions` and
`voice_agent_model`, which voice reads (goal steps only; simple commands stay on Lara + 4B; the usual model is the
fallback while the 27B is not up); `set_agent(None)` removes all three. `pimax-ai up` starts the server (port 8083, slot
`agent`) only when the setting is on, the file is downloaded, the session is none or the Work desk (never SteamVR or a
game; the watch stops it when one appears) and the free VRAM covers 18 GB plus the session's reserve. Presets, Play
and Work never set it. `pimax-ai down` stops it. Download: `pimax-ai install --two-stage` fetches it when the setting is on.
Still to do on the app side (not touched here, another agent's files): a "27B agent model" switch on the Local AI page
calling `set_agent`, with its size, 17 GB VRAM and the result below.

**Benchmark** (`pimax-voice audit --url ... --pause-ms 200`, dry run, one model at a time on port 8083, VR closed, Lara on
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
