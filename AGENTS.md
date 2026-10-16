# Agents: start here

This repo makes a Pimax Dream Air work on Linux, with one app and one click (`pimax-setup`).

Before any work, read:

1. `.claude/CLAUDE.md`: the binding project rules (GUI first, the definition of done, one whole build, safety rules).
2. `docs/AI-KNOWLEDGE.md`: the reading order, the system overview, the hard rules, the pitfalls we already hit,
   how to add a headset, GPU or compositor, and how to diagnose a user's failure.
3. `docs/LESSONS.md`: the numbered pitfalls. Read all of it before touching the stack.

Short version:

- The user never uses the command line. Do not reply with a command to run; add a switch or button to the app.
- HID bytes come only from Windows captures and the allowlist in `rust/pimax-device/devices/<id>/device.toml`.
- No firmware flashing, no DRM hotplug, never tell the user to unplug the headset.
- No sudo from agents. Root steps go through the app's pkexec prompt.
- Never touch a live VR session. Install only between sessions, kill by PID only.
- After any Monado relink, check `cap_sys_nice=ep` and the feature markers.
- Forks (Monado, WayVR) get local commits only. Commit only your own files here, and push `origin main`.
- No secrets, API keys, email addresses or device serials in the repo.
