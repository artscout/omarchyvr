# Forks and patches

omarchyvr runs on two forks and on one set of kernel-module patches. The forks are plain repositories (Monado lives
on GitLab, so they are not GitHub forks). **They are private for now and will be made public together with
omarchyvr.** Our commits sit on top of an untouched upstream history; the default branch is `omarchyvr`, and `main`
is the upstream base commit.

| Component | Repository | Branch | Upstream base | Our commits | Licence |
|---|---|---|---|---|---|
| Monado (OpenXR runtime: Pimax driver, 6DoF, eye, controllers, hands) | `github.com/laughtt/monado` | `omarchyvr` | `e686ba77e5` of `gitlab.freedesktop.org/monado/monado` (main) | 60 | BSL-1.0 |
| WayVR (VR desktop, gaze, screens, controllers) | `github.com/laughtt/wayvr` | `omarchyvr` | `cb3aa42` = tag `v26.8.0` of `github.com/wlx-team/wayvr` | 30 | GPL-3.0 |
| NVIDIA open kernel modules | [nvidia/](nvidia/) (patch files) | | `open-gpu-kernel-modules` 610.57.04 | 3 patches | MIT / GPL-2.0 |

Build:

    git clone -b omarchyvr git@github.com:laughtt/monado.git
    git clone -b omarchyvr git@github.com:laughtt/wayvr.git

Monado needs the CMake options listed in `.claude/CLAUDE.md` (section 3) and the Rust libraries of this repository
(`libpimax`, `rust/pimax-*`); the paths default to `$HOME/omarchyvr/...` and can be set with the `PIMAX_*_DIR` cache
variables. To send something upstream, branch from the upstream base commit and cherry-pick the commits you want.
