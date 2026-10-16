# Packaging

Two ways to install the Pimax app (`pimax-setup`) and its tools. Both build from this repository.
Neither one contains the Monado and WayVR builds (`~/src/monado-eye`, `~/src/wayvr`), Basalt, the voice models or the
sherpa-onnx library. Those stay user builds in the home directory, and the app checks them there.
Root steps (udev rule reload, NVIDIA patches, pacman hooks, setcap) stay with the app's Setup page.

## Arch package: `arch/PKGBUILD` (pimax-linux-git)

    cd packaging/arch && makepkg -si

makepkg clones this repository, so it builds the committed state: commit first. Edit `source=` for another URL or
branch. Build output (`src/`, `pkg/`, `*.pkg.tar.zst`) is ignored by git (`packaging/.gitignore`).

| Path | Content |
|---|---|
| `/usr/bin` | pimax-setup (the app), pimax (launcher), pimax-passthrough, pimax-perf, pimax-cam, pimax-slam, pimax-ctrltrack, pimax-eye, pimax-eyecal, pimax-lenstest, pimax-voice, pimax-voice-hud, xr-handtest, pimax-roommap, pimax-anchor, pimax-autod |
| `/usr/lib/systemd/user` | pimax-autod.service (auto mode; installed, not enabled; dry run until `~/.config/pimax/auto.toml` says otherwise) |
| `/usr/share/pimax/docs` | `docs/*.md`, `docs/gui/*.png`. The app detects a package install by `docs/LESSONS.md` |
| `/usr/share/pimax/analysis/tools` | vr_runtime_switch.sh, run_slam_view.sh, run_slam.sh |
| `/usr/share/pimax/linux` | udev rule copy, NVIDIA patches and reapply script, pacman hook templates, EDID files, DISPLAY.md, desktop template and icon, pimax-keepalived.service (home path inside: data only) |
| `/usr/lib/udev/rules.d` | 70-pimax.rules (pacman reloads udev; replug the headset or run `sudo udevadm trigger`) |
| `/usr/share/applications`, `/usr/share/icons/hicolor/scalable/apps` | pimax.desktop (`Exec=/usr/bin/pimax-setup`), pimax.svg |

The packaged launcher finds its tools next to itself in `/usr/bin`; the app reads its docs, scripts and the files of
the root fixes from `/usr/share/pimax` and accepts the udev rule in `/usr/lib/udev/rules.d`.

## AppImage: `appimage/build-appimage.sh`

    packaging/appimage/build-appimage.sh

Builds `pimax-setup` and `pimax` (cargo packages `pimax-setup` and `pimax-tools`), assembles
`appimage/build/Pimax.AppDir` and runs `appimagetool` if it is on PATH, else prints the command to finish.
It is the app only, without the helper tools. AppRun sets `PIMAX_REPO` to the docs inside the AppImage.
Root fixes that install a file from the AppImage fail (root cannot read the FUSE mount): use the package for those.
