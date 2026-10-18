# Third-party components

omarchyvr's own code (the Rust workspace in `rust/`, `libpimax/`, the scripts in `linux/`, `tools/`, `windows/`,
`packaging/`) is licensed **MIT OR Apache-2.0** (`LICENSE-MIT`, `LICENSE-APACHE`). Documentation and data
(`docs/`, `analysis/`, fixtures and golden files, the app's line art in `rust/omarchyvr-app/assets/art/`, drawn
for this project) are **CC-BY-4.0** (`LICENSE-CC-BY-4.0`).

The stack runs together with these projects. None of them is vendored in this repository unless the table says so;
each keeps its own licence and notices.

| Component | Licence | How it is used |
|---|---|---|
| [Monado](https://gitlab.freedesktop.org/monado/monado) | BSL-1.0 | OpenXR runtime. Our driver and fixes are commits on the `omarchyvr` branch of `github.com/laughtt/monado` (BSL-1.0 like Monado; see `patches/README.md`). |
| [WayVR](https://github.com/wlx-team/wayvr) | GPL-3.0 | VR desktop overlay, separate program. Our changes are commits on the `omarchyvr` branch of `github.com/laughtt/wayvr` (GPL-3.0; see `patches/README.md`). |
| [xrizer](https://github.com/Supreeeme/xrizer) | GPL-3.0-or-later | OpenVR-on-OpenXR layer for SteamVR games; separate program, used unmodified. |
| [Basalt](https://gitlab.freedesktop.org/mateosss/basalt) | BSD-3-Clause, Copyright (c) 2019, Vladyslav Usenko and Nikolaus Demmel | Visual-inertial odometry for 6DoF (build dependency of Monado, unmodified). One data file is vendored unmodified: `data/euroc/euroc_config.json` of the pinned commit, as `rust/omarchyvr-slam/data/basalt-euroc_config.json` (the 6DoF base config when no Basalt source tree is on the PC). |
| Pimax Dream Air reference calibration (`rust/pimax-device/devices/dream-air/reference/`) | data, shipped with the user's permission | The project headset's own Pimax Windows calibration (factory camera/IMU XML, Pimax's controller LED models) and its fitted lens profile, every serial scrubbed (user decision 2026-10-17). The default until a headset's own data is read or imported. |
| [patchelf](https://github.com/NixOS/patchelf) | GPL-3.0-or-later | **Prebuilt release only**: `lib/omarchyvr/tools/patchelf`, the unmodified Arch package binary of the release builder. The installer runs it once to set the library paths of the installed programs (a separate program, not linked). Its licence text and the version are in `lib/omarchyvr/tools/PATCHELF-LICENSE.txt`; the source is at the URL above (the version is in `BUILD-PACKAGES.txt`). |
| [OpenCV](https://opencv.org) | Apache-2.0 (4.5+) | Dependency of Basalt / Monado tracking (system package). |
| [ONNX Runtime](https://github.com/microsoft/onnxruntime) | MIT | Inference for camera hand tracking (system package or pinned download with its notice). |
| Mercury hand-tracking models | **no licence** | **Not included and never redistributed.** The app's "Download hand models" button fetches them from upstream on the user's machine, for personal use. |
| Whisper / Parakeet / Silero VAD / Kokoro / Qwen voice models | their own licences | Downloaded on first use, never bundled. |
| NVIDIA open kernel modules (`patches/nvidia/`) | MIT / GPL-2.0 (dual) | Our patches are offered under the same dual licence. |
| [web-ocean](https://github.com/Noniv/web-ocean) | MIT (c) 2026 Maksymilian Dendura | Inspiration for the animated ocean background only. **No code or shader was copied**; the ocean in WayVR is a clean-room implementation from the papers listed in `docs/research/web-ocean.md` (Tessendorf 2001, Pierson-Moskowitz 1964, JONSWAP 1973, Cox-Munk 1954, Nishita 1993, Walter 2007). |
| Rust crates (see `rust/Cargo.lock`) | various (MIT, Apache-2.0, BSD, ...) | Fetched by Cargo; their licences apply to the built binaries. |
| SteamVR, Steam, Pimax Play, Tobii eye-tracker firmware | proprietary | **Never bundled.** We talk to them over their documented or observed interfaces only. |

Reverse-engineered HID facts in `docs/` and `analysis/` describe how the hardware behaves (for interoperability);
they contain no vendor code, decompiled sources or binaries.
