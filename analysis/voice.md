# Local voice-to-text for the Dream Air (`omarchyvr-voice`, `omarchyvr-voice-hud`), 2026-10-14

Goal: the headset mic, transcribed continuously by a local model, with the text shown in the headset as a debug
line; commands later. Nothing leaves the machine (local model files, local Unix socket).

## Choice: Parakeet TDT 0.6B v3 int8 on the CPU (sherpa-onnx C API, in-process Rust) + Silero VAD

It is the only option tested here that has English and Spanish in one model (no language setting, 25 European
languages), near-zero errors, and costs no GPU time or VRAM (the VR budget stays with Monado and the app).

Measured on this machine (i9-14900KF, RTX 4090; voicebench Kokoro clips, 10 utterances en+es, 114 words, -54 dBFS
noise added, fed in real time through the live driver: `omarchyvr-voice bench`; raw tables in
`~/src/voice-bench/parakeet/results-*.md`). Latency = last speech sample handed over -> final transcript.

| config | latency median / max | word errors | CPU cores while listening | idle (VAD only) |
|---|---|---|---|---|
| threads 1, partials | 1311 / 2221 ms | 1/114 | 0.36 | 0.01 |
| threads 2, partials (default) | 1223 / 1996 ms | 1/114 | 1.53 | 0.07 |
| threads 2, no partials | 1232 / 1681 ms | 1/114 | 0.26 | 0.05 |
| threads 4, partials | 1174 / 1478 ms | 1/114 | 3.78 | 0.15 |
| `--vad silero-gate` threads 2 | 673 / 866 ms | 28/114 (lost es4) | 1.24 | 0.05 |

Decode itself is 80-400 ms (1-8 s utterances, threads 2, RTF 0.03-0.05); the rest is the end-of-speech decision
(480 ms `--endpoint-ms` hangover) plus Silero's 150-500 ms hysteresis tail. The energy-gated VAD cuts that tail
(0.67 s median) but lost one long utterance in the bench, so it is opt-in until that is understood. Offline the
fixture and all clips transcribe correctly (`omarchyvr-voice transcribe`). GPU: none. Model RAM ~1 GB.

Other options (same clips):

| option | result | why not |
|---|---|---|
| (a) whisper.cpp v1.8.2 (= whisper-rs), 4 P-cores, `-l auto` | es4 (8.7 s): base 2.6 s, small 4.8 s, large-v3-turbo q5 26.7 s on CPU; Vulkan 0.14 / 0.16 / 0.15 s (turbo en1 0.26 s) | CPU far too slow; GPU path takes GPU time + VRAM from VR; auto language runs the encoder twice |
| (b) faster-whisper 1.2.1 (what a Docker/WhisperLive image runs), measured natively | CPU int8: small ~1.0-1.2 s, turbo 4.4-4.8 s per call; CUDA fp16: small 45-96 ms, turbo 110-141 ms, ~1 GB VRAM, GPU util peak 80 %, 6-8 host cores busy; small misdetects short Spanish (es1 -> Malayalam) | Docker is not usable without sudo (user not in `docker` group, no nvidia-container-toolkit); GPU contention; CPU slow |
| (c) sherpa-onnx streaming (Kroko en / es, NeMo fast-conformer en 80 ms) | RTF 0.010-0.026 (Kroko), 0.10-0.15 (NeMo); first partial after 1.0-1.5 s of audio; endpoint 0.6-1.5 s after speech; one model per language; Kroko-en weaker | no single en+es model, endpointing not faster; candidate for cheap partials later |
| (d) WayVR built-in `whisper` feature | whisper-rs 0.16 + Vulkan (GPU on by default), push-to-talk button panel, models base/small/turbo from wayvr.org, final text only into its panel/clipboard/OSC, unloads after 5 min | no continuous mode, no VAD, no API for other apps, GPU; our desk WayVR build leaves the feature out |

Raw data: `~/src/voice-bench/{whisper,fw,streaming,parakeet}/` (whisper.cpp matrix stopped at 795 run files when we
wrapped up; GPU-load block not run).

**Mic finding:** at 14:24 with no VR session the Pimax USB mic delivered exact zeros (`pw-record` too; ALSA capture
switches on, PipeWire volume 100 %). The firmware log powers the audio codec after "Panel ON", so the mic probably
only works while the headset display runs. `omarchyvr-voice` reports 3 s of exact zeros as `status no_device` ("input is
digital silence ...").

## How it works

mic (PipeWire stream, F32 mono 16 kHz, node selected by name substring, default "Pimax", else the default source) ->
32 ms frames -> Silero VAD (sherpa-onnx, 1 thread) -> segmenter (128 ms onset, 320 ms pre-roll, 480 ms hangover,
160 ms tail, 20 s max, partial decode every 640 ms of speech) -> decode thread (finals in order, only the newest
partial, stale partials dropped) -> events -> stdout + Unix socket `$XDG_RUNTIME_DIR/omarchyvr-voice.sock` (JSON lines:
`hello`, `status`, `level`, `speech_start`, `partial`, `final` {text, lang, t0, t1, latency_ms, decode_ms}, `intent`,
`pong`; clients may send `{"cmd":"pause"|"resume"|"ping"}`). Language is guessed from the text (en/es).

HUD: `omarchyvr-voice-hud` is an `XR_EXTX_overlay` client (Vulkan, placement 10 > WayVR's 5) that subscribes to the socket
and draws the last 2 finals + the live partial, a mic dot and a level bar on a head-locked quad 20 deg below the view
centre (2 m, 1.6 m wide, Noto Sans; accents/ñ/¿ render), re-uploads only on change, hides after 12 s, waits for
Monado and the socket. Previews: `omarchyvr-voice-hud preview OUT.png --text ...` (`~/src/voice-bench/hud/`). Its
OpenXR path has NOT run against Monado yet.

## Run

    cd ~/pimax-re/rust && cargo build --release -p omarchyvr-voice -p omarchyvr-voicehud -p omarchyvr-tools
    target/release/omarchyvr-voice info                 # library, models, VAD, socket
    target/release/omarchyvr-voice devices              # * = the source listen picks
    target/release/omarchyvr-voice listen [--levels]    # prints partial/final lines; Ctrl-C stops
    target/release/omarchyvr-voice tail                 # second terminal: what subscribers get
    target/release/omarchyvr-voice transcribe F.wav     # offline
    omarchyvr desk start --voice                        # + listener (voice.log) + HUD (voicehud.log) after WayVR

Models: `~/models/parakeet-tdt-0.6b-v3-int8`, `~/models/vad/silero_vad.onnx`; library `~/src/venv-voice` (sherpa-onnx
1.13.8 C API, dlopen). Options: `--threads`, `--endpoint-ms`, `--partial-ms 0`, `--vad silero|silero-gate|energy`.
`--features whisper` adds `--engine whisper` (whisper-rs/Vulkan) for comparisons; that build was not compiled here.

## Live test (main session)

1. Headset lit (`omarchyvr vr start` or `omarchyvr desk start`), then `target/release/omarchyvr-voice listen --levels`: levels
   must move above -120 dBFS when you speak; say "Open the browser." and "Abre el navegador." -> two finals, en/es.
2. `omarchyvr desk start --voice` (install the new `omarchyvr` first, see linux/DISPLAY.md): the text appears at the bottom of
   the view above WayVR; say "Borra la pantalla" / "Clear the screen" -> `intent hud.clear` clears the HUD.
3. If the HUD is too small or soft: `omarchyvr-voice-hud --font-size 80 --width 1.8` (in voicehud.log on errors).

## Commands later

`intent.rs`: rules = name + patterns ("open {app}", "abre {app}"), matched on normalised whole utterances (lower case,
accents folded, "please"/"por favor" ignored), built-in `hud.clear` and `vr.recenter`, more via `--intents FILE.json`.
omarchyvr-voice only emits `intent` events; a separate subscriber (deskd) executes them with confirmation for risky ones.

## Tests

`cargo test -p omarchyvr-voice`: 41 unit (protocol, VAD, segmenter, queue, live driver with a fake engine, socket server,
intents, language guess, WAV I/O) + 2 end-to-end on `tests/data/en_es.wav` (local Kokoro TTS: "Recenter the view.",
"Hola, esto es una prueba del micrófono.", "Borra la pantalla."; offline pipeline and the CLI live driver + socket;
skipped without the models). `omarchyvr-voicehud`: 111 tests. `omarchyvr-tools`: desk `--voice` plan/stop tests. Clippy clean.
