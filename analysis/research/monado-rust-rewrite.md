# Monado in Rust: rewrite, port, or keep? (research, 2026-10-14)

Question: should we rewrite Monado (C/C++ OpenXR runtime, BSL-1.0) in Rust, port it piece by piece, or keep it and write only our own parts in Rust?

Our setup: fork at `~/src/monado` (branch `omarchyvr`, HEAD `81f7c62`). Our Rust workspace `~/pimax-re/rust` (about 53k lines, C ABI into Monado via `drivers/pimax`).

## 1. Size and structure (measured locally)

Counted with `wc -l` over `.c/.cpp/.h/.hpp` under `src/xrt` (tokei/cloc are not installed). Totals: 1,498 source files under `src/`, 313,938 lines under `src/xrt` (headers included). The tree contains **0 `.rs` files**.

| Area (src/xrt/…) | Lines | Notes |
|---|---:|---|
| auxiliary/ | 80,061 | util 25,909; tracking 11,327; vk 10,816; math 6,858; render 6,632; os 3,181 |
| drivers/ | 77,161 | wmr 7,907; rift_s 5,522; vive 4,955; rift 4,783; pimax (ours) 2,559 |
| state_trackers/ | 64,009 | **OpenXR (oxr) 30,401**; openvr 8,710; gui 5,653; prober 3,695; steamvr_drv 1,883 |
| compositor/ | 30,063 | main 11,980; util 7,096; client 6,929; multi 2,421 |
| tracking/ | 15,978 | constellation 8,817; hand 4,706 |
| ipc/ | 15,066 | server 7,758; client 4,969; shared 1,874 |
| include/ | 14,912 | public headers |
| targets/ | 12,072 | service/in-process binaries |
| base/ | 4,616 | |

Grouping by the areas you asked about:

- **OpenXR state tracker:** about 30k lines (oxr), plus about 10k for OpenVR and SteamVR shims.
- **IPC client/server:** about 15k.
- **Compositor (Vulkan, direct mode, distortion/timewarp):** about 30k in `compositor/`, plus about 17k of Vulkan and render helpers in `auxiliary/vk` and `auxiliary/render`.
- **Drivers:** about 77k, the largest block, but each driver is small (1–8k lines). Most are isolated behind `xrt_device`.
- **Tracking (SLAM, hands):** about 16k in `tracking/` plus 11k in `auxiliary/tracking`. The SLAM back end (Basalt) is, to my knowledge, an external dependency; I did not verify that locally.
- **Aux utilities:** about 80k, mostly generic C utilities (containers, threads, math) that a Rust port would replace with std.

Our own Rust is about 53k lines, which is comparable to the Monado state tracker alone.

## 2. Existing Rust XR projects

| Project | What it is | Lang / licence | Stars | Last push (GitHub API) | Relevance |
|---|---|---|---:|---|---|
| [Monado](https://gitlab.freedesktop.org/monado/monado) | OpenXR runtime (reference) | C/C++, BSL-1.0 ([LICENSE](https://gitlab.freedesktop.org/monado/monado/-/blob/main/LICENSE) text; [Debian copyright](https://metadata.ftp-master.debian.org/changelogs/main/m/monado/stable_copyright)) | n/a (GitLab) | active | Baseline |
| [xrizer](https://github.com/Supreeeme/xrizer) | OpenVR API implemented on top of OpenXR (client side, not a runtime) | Rust, GPL-3.0 | 327 | 2026-10-11 | Shows Rust OpenXR *client* work; not a runtime |
| [ALVR](https://github.com/alvr-org/ALVR) | PC-to-standalone VR streamer (SteamVR driver) | Rust, MIT | 7,997 | 2026-10-11 | Large Rust XR project; streams to headset, does not implement OpenXR runtime |
| [WiVRn](https://github.com/WiVRn/WiVRn) | Linux OpenXR streaming runtime for standalone headsets | C++, GPL-3.0 | 1,787 | 2026-10-12 | Closest comparison: a *C++* OpenXR runtime (built on Monado parts) |
| [Stardust XR](https://github.com/StardustXR/server) | Own Rust display server / compositor for spatial apps on Linux; not OpenXR | Rust (`core` lib, [MIT](https://github.com/StardustXR/core)); server GPL-2.0 | 186 (server); 74 (core) | 2026-10-07 (core) | Rust compositor with its own protocol; no OpenXR runtime |
| [openxrs](https://github.com/Ralith/openxrs) (`openxr` crate) | Rust bindings for the **app-side** OpenXR API | Rust, Apache-2.0 | 338 | 2026-10-13 | Use it for apps and tests; it is not a runtime. [35 releases](https://lib.rs/crates/openxr) |

Finding: **no Rust OpenXR runtime exists that is viable for a conformant, multi-app runtime.** The Rust OpenXR work found is client-side (openxrs, xrizer) or a non-OpenXR compositor (Stardust). A Rust runtime search on the Monado GitLab and mailing lists turned up no rewrite plan; the search did not reach the Monado issue tracker directly, so treat "no plan" as "not found" rather than "confirmed absent".

## 3. Building blocks a Rust runtime would need

| Need | Rust options | Assessment |
|---|---|---|
| Vulkan | `ash` (thin, used by most Rust graphics code), `vulkano`, `wgpu` | `ash` is the only one that gives the external-memory and sync control a runtime needs. `wgpu` hides it; I found no evidence it supports DRM direct-display presentation. |
| DRM direct mode / lease | `drm` / `drm-rs` crates; Wayland `wp_drm_lease_device_v1` | Protocol merged to staging in 2021 ([wayland-protocols MR 67](https://gitlab.freedesktop.org/wayland/wayland-protocols/-/merge_requests/67), [Phoronix](https://www.phoronix.com/news/Wayland-DRM-Lease-Merged)). The Rust Wayland crates (`wayland-client`, `wayland-protocols`) can bind it, but the code is not written. NVIDIA reported lease failures in a [forum thread](https://forums.developer.nvidia.com/t/nvidia-proprietary-non-open-modules-completely-unable-to-acquire-a-drm-lease-on-any-display-server-all-known-nvidia-drivers-any-hardware/341244). We already hit a stuck NVIDIA head in our own work (see `a966aa0`). |
| OpenXR loader runtime interface | Implement `xrNegotiateLoaderRuntimeInterface` and `xrGetInstanceProcAddr` as `extern "C"` | Small surface. Spec: [Khronos registry, 1.1](https://registry.khronos.org/OpenXR/specs/1.1/man/html/xrNegotiateLoaderRuntimeInterface.html). Feasible in Rust. |
| IPC (shm, fds) | `memfd`, `nix`, `rustix`, `shared_memory` crates | Mature. Monado's IPC is about 15k lines, so the port is mostly mechanical. |
| Hand tracking / SLAM | No mature Rust SLAM equivalent to Basalt known to me; `opencv` / `ort` bindings | Gap. We keep the C++/ONNX side behind the C ABI, as we already do. |

## 4. Risks

- **OpenXR conformance:** Monado has passed the Khronos CTS, as of a [2020 report](https://gamingonlinux.com/2020/11/open-source-openxr-vr-ar-runtime-monado-now-passes-conformance-tests); a [later merge request](https://gitlab.freedesktop.org/monado/monado/merge_requests/589) also reports passing the CTS. The CTS is Apache-2.0 ([Phoronix](https://www.phoronix.com/news/OpenXR-CTS-Open-Sourced), [usage guide](https://registry.khronos.org/OpenXR/conformance/cts_usage.html)). A rewrite would have to re-earn that from zero. I did not find a current Khronos adopter listing for Monado.
- **Industry dependence:** Collabora says Monado underpins Google AndroidXR, NVIDIA CloudXR, Pico, Qualcomm Snapdragon Spaces and others ([Collabora](https://www.collabora.com/news-and-blog/news-and-events/how-monado-became-the-foundation-for-openxr-runtimes.html), [GamingOnLinux, Mar 2026](https://www.gamingonlinux.com/2026/03/monado-becomes-the-open-source-foundation-for-various-openxr-vr-vendors)). A fork of that scale is hard to keep in sync.
- **Hard parts a rewrite must reproduce:** timewarp and distortion (compositor, Vulkan); frame pacing across the xrWaitFrame/xrBeginFrame state machine (the MR above shows how fragile this is); multi-client compositing over IPC; Vulkan external memory and sync with apps (our Xid 31 bug lived in this area); SteamVR and Proton compatibility via the `steamvr_drv` and `openvr` shims.
- **Licence:** BSL-1.0 is permissive and does not need source disclosure for derivative works. Rust code linking via C ABI is fine. Rewriting does not change the licence.
- **Our own code:** `drivers/pimax` is 2,559 lines of C glue in front of about 53k lines of Rust. Those already cross the boundary cleanly.

## 5. Options and effort

Effort figures are my rough estimates, not measured. "Agent-weeks" assumes Claude agents write code and humans review and test on hardware.

| Option | What it means | Effort (human pm) | Effort (agent-weeks, est.) | Risk | Verdict |
|---|---|---|---|---|---|
| **A. Full rewrite** | Rewrite 314k lines of C/C++ in Rust, reach CTS parity, keep our drivers | 24–48 | 16–32 | Very high: long period with no working runtime; no conformance; no upstream | Not justified |
| **B. Strangler** | Replace one subsystem at a time behind the C ABI (IPC client, then an aux utility, then a compositor helper); each step ships and is tested on hardware | 12–24 spread over time | 6–12 spread over time | Medium: each step is small and reversible; ABI boundaries are the main cost | Viable as a targeted approach, not a whole-runtime goal |
| **C. Keep Monado C** | Keep Monado as is; write drivers, tracking, passthrough and new features in Rust (what we do now) | 0 extra for runtime; ongoing | 0 extra | Low; tracks upstream | **Recommended baseline** |
| **D. Adopt an existing Rust runtime** | Use a Rust OpenXR runtime | n/a | n/a | None exists that is viable (section 2) | Not available |

## 6. Recommendation

**Option C now, with B only where a concrete need appears.** Keep Monado in C. Keep writing drivers, camera passthrough, lens model and hands in Rust behind the C ABI, as already done. Do not start a full rewrite (A): the runtime is about 314k lines, it has a working conformance story, and no Rust runtime exists to start from. Revisit B for a specific subsystem only if a C piece is the measured bottleneck or a repeated source of bugs (the compositor-side Vulkan sync path is the main candidate, but that needs a profile first, not a rewrite decision).

## Sources

- Monado: https://gitlab.freedesktop.org/monado/monado (LICENSE: https://gitlab.freedesktop.org/monado/monado/-/blob/main/LICENSE)
- Monado licence (Debian copyright, BSL-1.0): https://metadata.ftp-master.debian.org/changelogs/main/m/monado/stable_copyright
- Collabora on Monado as foundation: https://www.collabora.com/news-and-blog/news-and-events/how-monado-became-the-foundation-for-openxr-runtimes.html
- GamingOnLinux, Mar 2026: https://www.gamingonlinux.com/2026/03/monado-becomes-the-open-source-foundation-for-various-openxr-vr-vendors
- Monado passes CTS (2020): https://gamingonlinux.com/2020/11/open-source-openxr-vr-ar-runtime-monado-now-passes-conformance-tests
- Monado MR 589 (CTS state checks): https://gitlab.freedesktop.org/monado/monado/merge_requests/589
- OpenXR CTS: https://github.com/KhronosGroup/OpenXR-CTS ; usage: https://registry.khronos.org/OpenXR/conformance/cts_usage.html ; open-sourced: https://www.phoronix.com/news/OpenXR-CTS-Open-Sourced
- xrNegotiateLoaderRuntimeInterface: https://registry.khronos.org/OpenXR/specs/1.1/man/html/xrNegotiateLoaderRuntimeInterface.html
- xrizer: https://github.com/Supreeeme/xrizer
- ALVR: https://github.com/alvr-org/ALVR
- WiVRn: https://github.com/WiVRn/WiVRn
- Stardust XR server: https://github.com/StardustXR/server ; core: https://github.com/StardustXR/core
- openxrs (openxr crate): https://github.com/Ralith/openxrs ; https://lib.rs/crates/openxr
- DRM lease protocol: https://gitlab.freedesktop.org/wayland/wayland-protocols/-/merge_requests/67 ; https://www.phoronix.com/news/Wayland-DRM-Lease-Merged ; https://drewdevault.com/blog/DRM-leasing-and-VR-for-Wayland/
- NVIDIA lease failures: https://forums.developer.nvidia.com/t/nvidia-proprietary-non-open-modules-completely-unable-to-acquire-a-drm-lease-on-any-display-server-all-known-nvidia-drivers-any-hardware/341244

## Caveats

- GitHub stats were read via the public API on 2026-10-14.
- Not verified: wgpu's lack of DRM presentation (no source found; it is my assessment), the Basalt dependency for SLAM, Stardust's OpenXR status beyond its own README, and any Monado Rust discussion on Discord or the mailing lists.
- Line counts are `wc -l` over C/C++ headers and sources; they include tests and comments.
