# Third-party components

omarchyvr's own code (the Rust workspace in `rust/`, `libpimax/`, the scripts in `linux/`, `tools/`, `windows/`,
`packaging/`) is licensed **MIT OR Apache-2.0** (`LICENSE-MIT`, `LICENSE-APACHE`). Documentation and data
(`docs/`, `analysis/`, `hw/`, fixtures and golden files, the app's line art in `rust/omarchyvr-app/assets/art/`, drawn
for this project) are **CC-BY-4.0** (`LICENSE-CC-BY-4.0`).

The stack runs together with these projects. None of them is vendored in this repository unless the table says so;
each keeps its own licence and notices.

| Component | Licence | How it is used |
|---|---|---|
| [Monado](https://gitlab.freedesktop.org/monado/monado) | BSL-1.0 | OpenXR runtime. Our driver and fixes are commits on the `omarchyvr` branch of `github.com/laughtt/monado` (BSL-1.0 like Monado; see `patches/README.md`). |
| [WayVR](https://github.com/wlx-team/wayvr) | GPL-3.0 | VR desktop overlay, separate program. Our changes are commits on the `omarchyvr` branch of `github.com/laughtt/wayvr` (GPL-3.0; see `patches/README.md`). |
| [xrizer](https://github.com/Supreeeme/xrizer) | GPL-3.0-or-later | OpenVR-on-OpenXR layer for SteamVR games; separate program, used unmodified. |
| [Basalt](https://gitlab.freedesktop.org/mateosss/basalt) | BSD-3-Clause | Visual-inertial odometry for 6DoF (build dependency of Monado, unmodified). |
| [OpenCV](https://opencv.org) | Apache-2.0 (4.5+) | Dependency of Basalt / Monado tracking (system package). |
| [ONNX Runtime](https://github.com/microsoft/onnxruntime) | MIT | Inference for camera hand tracking (system package or pinned download with its notice). |
| Mercury hand-tracking models | **no licence** | **Not included and never redistributed.** The app's "Download hand models" button fetches them from upstream on the user's machine, for personal use. |
| Whisper / Parakeet / Silero VAD / Kokoro / Qwen voice models | their own licences | Downloaded on first use, never bundled. |
| NVIDIA open kernel modules (`patches/nvidia/`) | MIT / GPL-2.0 (dual) | Our patches are offered under the same dual licence. |
| Rust crates (see `rust/Cargo.lock`) | various (MIT, Apache-2.0, BSD, ...) | Fetched by Cargo; their licences apply to the built binaries. |
| SteamVR, Steam, Pimax Play, Tobii eye-tracker firmware | proprietary | **Never bundled.** We talk to them over their documented or observed interfaces only. |

Reverse-engineered HID facts in `docs/` and `analysis/` describe how the hardware behaves (for interoperability);
they contain no vendor code, decompiled sources or binaries.
