# Contributing

## What this project is

omarchyvr makes the Pimax Dream Air VR headset work on Linux with one app. It is an unofficial community project.
See the [README](README.md).

## Before you start

Read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and [docs/LESSONS.md](docs/LESSONS.md). The lessons list
mistakes that already cost time.

Safety rules for contributors:

- HID bytes sent to the headset come only from captures of the vendor's own Windows software. A command is
  sendable only if it is in the allowlist in `rust/pimax-device/devices/<id>/device.toml`.
- No firmware flashing.
- Never touch a running VR session. Do not start builds, tests or benchmarks while one runs, and stop only your
  own processes.
- No root steps except through the app's root helper (`installer/omarchyvr-root.sh`, started with pkexec). Do not
  add code that runs `sudo`.
- Do not commit serials, keys, e-mail addresses, home paths or captures of private data.

## Build

The workspace is in `rust/`. The minimum Rust version is the `rust-version` in `rust/Cargo.toml` (1.80 at the time
of writing). System packages and the full build of Monado and WayVR are described in
[docs/INSTALL.md](docs/INSTALL.md) and done by `install.sh`. Clone with the forks:

    git clone --recursive https://github.com/laughtt/omarchyvr.git

## Tests

    cd rust && cargo test --workspace

A guard in `.cargo/config.toml` (`tools/vr-guard.sh`, `tools/vr-busy.sh`) makes cargo refuse to build or test
while a VR session runs, and runs everything at low priority. Run tests between sessions.

A fast subset that needs no hardware: `cargo test -p pimax-proto -p pimax -p pimax-device`.

Some tests replay private lab data (a headset calibration, camera recordings). They build with `cfg(lab_data)` when
that data is present (`tools/lab-data.sh`). Without it they are marked ignored with a reason, so a normal checkout
does not need it.

`tools/known-test-failures.txt` lists tests that are known to fail. Do not add to it to hide a new failure.

## Adding a headset

Follow the contributor flow in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) section 2.5. Give every value a
source. A maintainer approves each state-changing command.

## Leak guard hooks

Enable the hooks once per clone:

    git config core.hooksPath tools/git-hooks

The pre-commit hook refuses staged files under private paths and runs `tools/leak-scan.py` on the staged diff.
The pre-push hook scans the commits being pushed and refuses a commit identity that is not a noreply address. The
scanner looks for device serials, API keys, private keys, e-mail addresses, home paths, login names and eye-chip
bytes. It prints the kind of hit and the path, never the value.

## Commits and pull requests

- Keep commits small and focused. Say what changed and how you tested it.
- A feature is done when all of this is true: it has a switch in the app (the setting is saved and shared with the
  CLI); it is built in the one normal build, not a side build; it shows its status and errors in plain words; the
  app's Play button starts and stops it; a failure does not stop VR from running; tests pass; and there is a doc
  page plus a line in the app help.
- Everything is compiled into the one build. Only the user's settings decide what is active.
- The app is the interface. Do not add features that need the command line to turn on.

## Reporting security problems

Do not open a public issue. See [SECURITY.md](SECURITY.md).

## Licence of contributions

By contributing you agree that your code is licensed under MIT OR Apache-2.0 and your documentation and data under
CC-BY-4.0. See `NOTICE`, `LICENSE-MIT`, `LICENSE-APACHE` and `LICENSE-CC-BY-4.0`.
