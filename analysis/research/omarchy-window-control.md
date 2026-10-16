# Voice control of Omarchy windows, workspaces and tabs

Date: 2026-10-15. Hyprland 0.56.2 (Lua config; dispatchers are `hl.dsp.*`). Investigation was read-only: only
`hyprctl -j` queries, `ls`/`cat` on `/usr/share/omarchy`, and two harmless dispatches (the already-active workspace 2
and the already-focused window) to confirm the dispatch syntax. Nothing was closed or killed.

## 1. Discovery

| query | what it gives |
|---|---|
| `hyprctl -j clients` | one object per mapped window: `address` ("0x..."), `class`, `initialClass`, `title`, `initialTitle`, `workspace {id,name}`, `monitor`, `pid`, `floating`, `fullscreen`, `hidden`, `grouped` (array of the group members' addresses, `[]` when not grouped), `tags`, `focusHistoryID` (0 = focused now, 1 = the one before, ...), `stableId` |
| `hyprctl -j activewindow` | the same object for the focused window, or `{}` when none |
| `hyprctl -j workspaces` | `id`, `name`, `monitor`, `windows` (count), `lastwindow`, `lastwindowtitle`, `hasfullscreen` |

Seen on this machine: workspace 2 only, 3 windows (two `foot` terminals and the `pimax` GUI), no group, no browser
running. Titles are fully visible to anything that can run `hyprctl`, so they can be private (page titles, file names).

Omarchy scripts in `/usr/share/omarchy/bin` that act on windows or workspaces:
- `omarchy-hyprland-focus-app <name>`: focuses the first window whose class matches the regex (class only).
- `omarchy-launch-or-focus <pattern> [cmd]`, `-tui`, `-webapp`: focus a window matching class or title, else launch.
- `omarchy-hyprland-window-pop`, `-window-width`, `-window-gaps-toggle`, `-window-tiled-fullscreen-toggle`,
  `-window-transparency-toggle`, `-window-single-square-aspect-toggle`, `-workspace-layout-toggle`.
- `omarchy-hyprland-window-close-all` (closes everything; blocked, "never").
- `omarchy-launch-terminal`, `-terminal-tmux`, `-browser`.
Omarchy's own key bindings (`/usr/share/omarchy/default/hypr/bindings`) show the working 0.56 syntax:
`hl.dsp.focus({ workspace = "3" | "e+1" | "e-1" | "previous" })`, `hl.dsp.focus({ window = "address:0x.." })`,
`hl.dsp.window.move({ workspace = "N", follow = false })`, `hl.dsp.window.close({ window = "address:0x.." })`,
`hl.dsp.window.cycle_next()`.

## 2. Hyprland groups ("tabs")

`SUPER+G` toggles a group (`hl.dsp.group.toggle()`); windows grouped together show a group bar and one is visible at a
time. In 0.56 the dispatchers are `hl.dsp.group.next()`, `hl.dsp.group.prev()`, `hl.dsp.group.active({ index = N })`
(Omarchy binds `SUPER+ALT+1..9` to it), `hl.dsp.window.move({ into_group = "l" })`, `{ out_of_group = true }`. The old
`hyprctl dispatch changegroupactive f|b|N` is the legacy name of the same thing. In `clients`, every member has
`grouped = [all member addresses]`, so "is the focused window in a group, and how many tabs" is one query. Non-active
members are `hidden`. This machine has no groups right now, so group dispatches were not run (syntax from Omarchy's
bindings).

## 3. Browser tabs

Installed: Chromium (`/usr/bin/chromium`, Omarchy's default; `omarchy-default-browser` says `chrome`). No Brave, no
Firefox. Chromium starts with `~/.config/chromium-flags.conf` (wayland flags and three `--load-extension` entries); no
remote-debugging port.

| option | verdict |
|---|---|
| Remote debugging (`--remote-debugging-port`) | Gives titles, URLs, activate/close by tab id. Needs the flag, so a browser restart, and the port gives any local process full control of every logged-in session. Rejected. |
| Extension + native messaging | Full tab control, but needs installing and trusting an extension and a native host; Omarchy already ships such hosts (`omarchy-chromium-copy-url-host`), so it is possible later. More moving parts; not needed for the commands asked. |
| `wtype` (installed) | Virtual keyboard; Omarchy's own clipboard binding notes it merges with physically held modifiers. Works for voice (no held keys) but is a second code path. |
| `hl.dsp.send_key_state` to the focused surface | The mechanism Omarchy uses for "universal copy/paste" (`bindings/clipboard.lua`: key down, then up). No restart, no flag, no extra package, works for Chromium and Firefox alike. **Recommended.** |

Chosen: when the focused window's class is a browser, voice sends one chord with a single modifier:
Ctrl+Page_Down (next tab), Ctrl+Page_Up (previous), Ctrl+1..8 (tab N), Ctrl+9 (last tab), Ctrl+T (new tab, safe),
Ctrl+W (close tab, careful). Limits: only the focused window's active tab can be addressed (no "the tab where I read
about X"; that needs the extension option), and a key chord goes to whatever has focus, so the code checks the class
first. These chords were not sent during the investigation (no browser window was open, and sending keys is not a
harmless dispatch); the unit tests assert the exact dispatch strings, and a live check is the first thing to do with a
browser open.

## 4. Terminal tabs

Default terminal: `foot` (`xdg-terminal-exec` → `foot.desktop`; alacritty, kitty and ghostty are not installed).
foot has no tabs. Omarchy has an optional tmux flow (`omarchy-launch-terminal-tmux` attaches/creates session "Work";
tmux 3.x is installed) but tmux is not the default and windows inside it need the tmux prefix key. "Tab" in a terminal
therefore means a Hyprland group; for tmux, voice says "No tabs in foot" instead of guessing.

## 5. Safety classes

| class | commands | rule |
|---|---|---|
| safe | focus/switch to an app, workspace N/next/previous (1 to 10), group tab next/prev/N, browser tab next/prev/N/new, move window to workspace, focus direction, fullscreen, list windows, AI pick | runs at once; reversible |
| careful | close window, close tab | needs a spoken "yes" within `confirm_s`; the loader refuses an `@win close`/`tab_close` intent without `confirm = true`; only the focused window, targeted by its address |
| never | exit/kill Hyprland, `kill`, close-all, shutdown/logout, reload | no intent and no `@win` action exists for them |

## 6. What was built

`rust/pimax-voice/src/windows.rs` (`@win` built-in), 11 intents in `voice-intents.toml` marked
`source = "user request 2026-10-15"` plus extended phrases for rows 11 and 16, an AI switch (`llm::LlmClient`,
`llm::ChatClient` for anthropic / openai / openai-compatible reading `~/.config/pimax/ai.toml`), and ASR aliases
("work space", "tab to/for", "tap", "prev"). The AI switch sends the request plus, for each of the 40 most recent
windows, `{id (address), class, title (80 chars), ws}` and accepts only `focus_window` with an address from that list.
Tests use mocks only; there are no network calls.
