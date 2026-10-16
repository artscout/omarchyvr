# Sound and microphone in the headset

App: Home > Sound: "Headset sound" (switch) + "Test sound", "Headset mic" (switch) + "Test microphone", "Input now:
headset / other". Saved in `~/.config/pimax/audio.toml` (`headset_sound`, `headset_mic`, both default true).
Engine: `pimax vr audio status|on|off|mic-on|mic-off|mic-test|speaker-test` (rust/pimax-tools/src/vr/audio.rs). Only
`pactl`, `pw-record` and `pw-play` are used. Nothing is root-owned and nothing needs a password.

## Speakers: why they were silent, and the fix (2026-10-15)

The Dream Air has no USB speaker: its USB audio (34a4:0044 interfaces 4/5) has one IN endpoint (the microphone) and no OUT.
The speakers get sound over DisplayPort audio, as on Windows.

Evidence:
- Our EDID (the live one in `/sys/class/drm/card1-DP-1/edid`, CTA block identical to the native one) says the sink has
  audio: CTA byte 3 = 0xc1 (basic audio bit 6 set), Audio Data Block `23 09 07 07` = LPCM 2 ch, 32/44.1/48 kHz,
  16/20/24 bit, Speaker Allocation `83 01 00 00` = FL/FR, HDMI VSDB, plus the Microsoft HMD VSDB (`ca125c`, non-desktop).
  DPCD says the sink is audio capable (analysis/headset-dpcd.md).
- NVIDIA does create the ELD for the leased (non-desktop) connector: while Monado drives DP-1,
  `/proc/asound/card1/eld#0.4` = monitor "Pimax", DisplayPort, LPCM 2 ch 32-48 kHz, `aplay -l` shows
  `HDMI 1 [Pimax]` and PipeWire's NVIDIA card gets port `hdmi-output-1` with `device.product.name = "Pimax"`, available
  (analysis/overnight/desk.md section 7). With the headset dark only the EPSON projector's ELD is valid (that is what
  was seen before and taken for "no ELD").
- Windows: the endpoint is "Pimax m (NVIDIA High Definition Audio)" (pimax-data/windows-20261015/info/audio-endpoints.txt;
  the EDID name is "Pimax\nm"). Pimax's service (PSRV log) makes it the default output and "Microphone (Pimax Dream Air)"
  the default input when VR starts ("play_match: Pimax record_match: Pimax", "set default audio 0/1 device"), and
  restores the old ones afterwards. No USB audio command exists (analysis/windows-capture-20261015.md, take 7).

Root cause: not the EDID, not NVIDIA. The NVIDIA HDA card shows one HDMI/DP port per profile, and WirePlumber keeps
its profile on `output:hdmi-stereo` (the projector port; `api.acp.auto-profile = false`, and the projector port has the
higher priority). So the Pimax port never gets a sink, and even with one nothing made it the default output.

Fix (user level, what Pimax Play does on Windows): at Play / Work / SteamVR start the app switches the NVIDIA card to the
profile of the "Pimax" port (`output:hdmi-stereo-extra1` on this PC; found by name, not hard-coded), makes its sink the
default output and saves the old profile and output in the session dir. The port appears a few seconds after the headset
lights up (SteamVR: after its compositor starts), so a small helper (`pimax vr audio follow DIR`, log `audio.log` in the
session dir) waits up to 60 s for it. Stop puts the old profile and default output back. While VR runs the projector's
sound is unavailable (one HDMI/DP profile at a time on this card); Sunshine is never touched. No EDID change, no NVIDIA
parameter, no root step: nothing to approve.

SteamVR: on Linux SteamVR has no audio device switch of its own; games and SteamVR use the PipeWire defaults set above,
so `steamvr.vrsettings` is not changed.

## Microphone

USB audio class, ALSA card "Pimax Dream Air", PipeWire source
`alsa_input.usb-Pimax_Corporation_Pimax_Dream_Air_<serial>-04.analog-stereo` (2 ch, 16 bit, 48 kHz, both channels the
same). While VR runs it is the default input; Stop restores the previous one. Monitors and Sunshine are never chosen.
It sends only zeros while the headset display is off (docs/LESSONS.md row 19), so the test needs VR running.
`pimax-voice listen` captures the source matching "Pimax" by default (falls back to the default input with a note) and
asks PipeWire for 1 ch 16 kHz, so PipeWire does the 48 kHz stereo -> 16 kHz mono conversion.

## Test plan (one session, all from the app)

1. Home > Sound: "Headset sound" and "Headset mic" on. Press Play (or Work), put the headset on.
2. Within about 10 s the session log shows `sound: the headset speaker is on (...)`. Press "Test headset sound":
   two soft notes in the headset; the line next to the button says "played on the headset speaker".
   If it says "not selected" or "no headset speaker", open Logs > session `audio.log`.
3. Play any desktop sound or a game: it comes out of the headset.
4. "Microphone: headset" is shown. Press "Test microphone", speak for 3 s: the level line says "good" and you hear
   yourself back in the headset.
5. Press Stop: sound returns to the old output (Sunshine/projector), the input to the old one.
Rollback: switch "Sound in headset" / "Headset microphone" off (nothing else was changed).
