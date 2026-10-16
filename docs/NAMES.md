<!-- names:keep-file (this page is about the old names; tools/apply-names.py must not rewrite it) -->
# Names: old (lab) -> omarchyvr

The product is called **omarchyvr**. Everything general-purpose carries the prefix `omarchyvr-` (not `ovr-`, which clashes
with Oculus LibOVR / OpenVR names). Names that say which **Pimax hardware** a part talks to keep `pimax`.
The lab (`~/pimax-re`, the archive) keeps the old names; `tools/names.tsv` is the machine-readable form of this page.

## Renamed

| Kind | Old | New |
|---|---|---|
| CLI binary (crate pimax-tools) | `pimax` | `omarchyvr` (subcommands unchanged: `omarchyvr vr start`, `omarchyvr desk stop`, `omarchyvr keepalived`) |
| App binary (crate pimax-setup) | `pimax-setup` | `omarchyvr-app` (window title and app id `omarchyvr`) |
| Daemon | `pimaxd` | `omarchyvrd` |
| Crates (dir, package, lib `omarchyvr_x`, binaries) | `pimax-ai`, `-anchor`, `-autod`, `-backgrounds`, `-components`, `-config`, `-desktest`, `-doctor`, `-gui-core`, `-hud`, `-passthrough`, `-perf`, `-record`, `-room`, `-roommap`, `-slam`, `-voice`, `-voicebench`, `-voicehud`, `-xrtest`, `-tools`, `-setup` | `omarchyvr-` + the same word (`-setup` -> `omarchyvr-app`, `-tools` -> `omarchyvr-tools`) |
| Extra binaries | `pimax-autod`, `pimax-health`, `pimax-voice-hud` | `omarchyvr-autod`, `omarchyvr-health`, `omarchyvr-voice-hud` |
| systemd user units | `pimax-keepalived.service`, `pimaxd.service`, `pimax-autod.service`, `pimax-ai.service` | `omarchyvr-keepalived.service`, `omarchyvrd.service`, `omarchyvr-autod.service`, `omarchyvr-ai.service` (`ExecStart=%h/.local/bin/<binary>`) |
| Desktop entries, icon | `linux/pimax.desktop`, `pimax-vr.desktop`, `pimax.svg` | `linux/omarchyvr.desktop`, `omarchyvr-vr.desktop`, `omarchyvr.svg`; installed `~/.local/share/applications/omarchyvr.desktop`, icon `omarchyvr.svg`; template placeholders `@OMARCHYVR_APP@`, `@OMARCHYVR@` |
| Steam copy marker | `X-Pimax-Steam-VR` | `X-Omarchyvr-Steam-VR` (the old marker and `90-pimax-steam-vr.conf` are still recognised, so `steam-setup off` undoes an old install) |
| environment.d file | `90-pimax-steam-vr.conf` | `90-omarchyvr-steam-vr.conf` |
| Autostart | `~/.config/autostart/pimax-setup.desktop` | `omarchyvr-app.desktop` |
| User dirs | `~/.config/pimax`, `~/.local/share/pimax`, `~/.local/state/pimax`, `~/.cache/pimax`, `~/.cache/pimax-ctrl-replay` | `~/.config/omarchyvr`, `~/.local/share/omarchyvr`, `~/.local/state/omarchyvr`, `~/.cache/omarchyvr`, `~/.cache/omarchyvr/ctrl-replay` |
| Root-owned scripts | `/usr/local/lib/pimax` | `/usr/local/lib/omarchyvr` (the old path is still accepted by the safety check and detected) |
| Package data | `/usr/share/pimax` | `/usr/share/omarchyvr` |
| Runtime files (ours only) | `/tmp/pimax-vr-session`, `pimax-headset.lock`, `pimax-cam-broker.sock`, `pimax-screens.state`, `pimax-floor-grid`, `pimax-room.status`, pid files and sockets of the daemons | `omarchyvr-session`, `omarchyvr-headset.lock`, `omarchyvr-cam-broker.sock`, `omarchyvr-screens.state`, ... |
| Report bundles, logs | `pimax-report-*`, `pimax-setup.log` | `omarchyvr-report-*`, `omarchyvr-app.log` |
| Process names (`comm`, 15 characters) | `pimax`, `pimax-passthrou`, `pimax-voice-hud` | `omarchyvr`, `omarchyvr-passt`, `omarchyvr-roomm`, `omarchyvr-recor`; the voice HUD sets its own comm `omarchyvr-vhud` at start (its file name cut at 15 characters would equal `omarchyvr-voice`) |

`~/.config/pimax` etc. are moved once, at the first start of the app, the CLI, `omarchyvrd` or `omarchyvr-autod`
(`omarchyvr_config::migrate_legacy_dirs`, log `~/.local/state/omarchyvr/logs/migration.log`). Symlinks are never
followed, a dir that exists in both places is left alone, and a move across file systems is skipped. The Monado fork reads the
new dirs first and falls back to the old ones.

## Kept (and why)

| Name | Why |
|---|---|
| crates `pimax` (library), `pimax-proto`, `pimax-device`, `pimax-capi`, `pimax-cam`, `pimax-hands`, `pimax-lens`, `pimax-lens-difftest`, `pimax-lenstest`, `pimax-eye`, `pimax-eyecal`, `pimax-ipd`, `pimax-difftest`, `pimax-ctrltrack`, `pimax-selfcal`; `libpimax/` (C) | they name Pimax hardware or its protocol; Monado links `libpimax_capi.a`, `libpimax_cam.a`, `libpimax_hands.a`, `libpimax_lens.a` |
| every `PIMAX_*` environment variable, the Monado markers (`PIMAX_SLAM`, ...) | interface shared with the Monado fork and scripts |
| runtime files `pimax-record.shm`, `pimax-anchor.offset`, `pimax-anchor.head`, `pimax-stage-bounds`, `pimax-rate`, `pimax-ctrl-inhand`, `pimax-ctrl-inhand-mode`, `pimax-haptic-request`, `pimax-hmd.state`, `pimax-basalt-calib.json`, `pimax-slam-info.json`, and the magic lines `pimax-anchor-offset 1`, `pimax-stage-bounds 1` | the Monado fork reads or writes them |
| device serial prefixes `pimax-camera-hand-*`, `pimax-ctrl-left/right` | names inside Monado |
| "Max" wake word (and the spoken word "pimax") | voice vocabulary |
| `70-pimax.rules`, `/etc/pimax/`, `/etc/modprobe.d/nvidia-pimax.conf`, `/etc/mkinitcpio.conf.d/pimax.conf`, `65-nvidia-pimax-patch.hook`, `75-nvidia-pimax-verify.hook`, EDID files `pimax-dp-edid*.bin` | system files of existing installs and hardware (HID allowlist) |
| `pimax-local` (AI model alias), `*.pimax-orig`, `*.pimax-backup`, `*.pimax-steamvr`, `*.pimax-xrizer` backup suffixes | saved in users' configs and next to files other programs own |
| `pimax-re` paths | the lab / archive repo |

## How lab -> omarchyvr syncs use the mapping

Work continues in omarchyvr; the lab is the archive. To bring lab changes over: `tools/lab-sync.sh OLD NEW LAB_DIR` (lab commits; LAB_DIR is the old lab clone).
It exports both lab commits with that commit's own `tools/export-public.sh --no-test`, applies `tools/names.tsv` to both
exports with `tools/apply-names.py` (path renames and text substitutions), diffs them and applies the diff here with
`git apply` (`--check` first; conflicts are left as `.rej`). Rules of `tools/names.tsv`:

* `protect`: regexes whose matches are hidden from every other rule (fork-shared names, hardware, lab, data);
* `regex`: ordered patterns (directories, `pimax <subcommand>`, placeholders, tmp-name prefixes);
* `word`: a hyphenated name, matched on word boundaries and applied longest first, so `pimax-voicehud` and `pimax-voice-hud`
  are handled before `pimax-voice`, and kept names such as `pimax-cam`, `pimax-eye`, `pimax-lens`, `pimax-lenstest` are never
  touched; `ident`: a Rust identifier (`pimax_gui_core` -> `omarchyvr_gui_core`), whole token only; `path`: exact path renames.

Edits made by hand after the rename that the table cannot express (legacy recognition, 15 character comm names, the
migration, window class matching) live only here; if the lab changes the same lines, `lab-sync.sh` reports a conflict.
Code that must keep an old name carries `names:keep` on the line (or `names:keep-file` near the top of a file);
`tools/apply-names.py` leaves those alone.
