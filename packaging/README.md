# Packaging

Two ways to install the Pimax app (`omarchyvr-app`) and its tools. Both build from this repository.
Neither one contains the Monado and WayVR builds (`~/.local/lib/omarchyvr/monado`, `~/.local/lib/omarchyvr/wayvr`), Basalt, the voice models or the
sherpa-onnx library. Those stay user builds in the home directory, and the app checks them there.
Root steps (udev rule reload, NVIDIA patches, pacman hooks, setcap) stay with the app's Setup page.

## Arch package: `arch/PKGBUILD` (omarchyvr-git)

    cd packaging/arch && makepkg -si

makepkg clones this repository, so it builds the committed state: commit first. Edit `source=` for another URL or
branch. Build output (`src/`, `pkg/`, `*.pkg.tar.zst`) is ignored by git (`packaging/.gitignore`).

| Path | Content |
|---|---|
| `/usr/bin` | omarchyvr-app (the app), omarchyvr (launcher), omarchyvr-passthrough, omarchyvr-perf, pimax-cam, omarchyvr-slam, pimax-ctrltrack, pimax-eye, pimax-eyecal, pimax-lenstest, omarchyvr-voice, omarchyvr-voice-hud, xr-handtest, omarchyvr-roommap, omarchyvr-anchor, omarchyvr-autod |
| `/usr/lib/systemd/user` | omarchyvr-autod.service (auto mode; installed, not enabled; dry run until `~/.config/omarchyvr/auto.toml` says otherwise) |
| `/usr/share/omarchyvr/docs` | `docs/*.md`, `docs/gui/*.png`. The app detects a package install by `docs/LESSONS.md` |
| `/usr/share/omarchyvr/analysis/tools` | vr_runtime_switch.sh, run_slam_view.sh, run_slam.sh |
| `/usr/share/omarchyvr/linux` | udev rule copy, NVIDIA patches and reapply script, pacman hook templates, EDID files, DISPLAY.md, desktop template and icon, omarchyvr-keepalived.service (home path inside: data only) |
| `/usr/lib/udev/rules.d` | 70-pimax.rules (pacman reloads udev; replug the headset or run `sudo udevadm trigger`) |
| `/usr/share/applications`, `/usr/share/icons/hicolor/scalable/apps` | omarchyvr.desktop (`Exec=/usr/bin/omarchyvr-app`), omarchyvr.svg |

The packaged launcher finds its tools next to itself in `/usr/bin`; the app reads its docs, scripts and the files of
the root fixes from `/usr/share/omarchyvr` and accepts the udev rule in `/usr/lib/udev/rules.d`.

## AppImage: `appimage/build-appimage.sh`

    packaging/appimage/build-appimage.sh

Builds `omarchyvr-app` and `omarchyvr` (cargo packages `omarchyvr-app` and `omarchyvr-tools`), assembles
`appimage/build/Pimax.AppDir` and runs `appimagetool` if it is on PATH, else prints the command to finish.
It is the app only, without the helper tools. AppRun sets `PIMAX_REPO` to the docs inside the AppImage.
Root fixes that install a file from the AppImage fail (root cannot read the FUSE mount): use the package for those.
