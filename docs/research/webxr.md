# Research: web pages in VR (WebXR, WebGPU) on this PC (2026-10-17)

Request (user): "We have this page https://oceano-piratas.vercel.app. Why does it not open in VR automatically when I
enter? Should Monado and all the services be up to get this? If so, the normal services should always be up in the
background while the app is open (it is open by default). Double-check that, and make sure WebGPU is always up and all
is working correctly."

Short answer: the page cannot open in VR on any browser, on this PC or elsewhere, because it has no VR code. Even a
page with VR code could not enter the headset from a Linux browser today: no browser packaged for Arch connects WebXR
to OpenXR. Keeping Monado up in the background would change neither. WebGPU already works in the installed Chrome with
no flags. What works today is the page as a flat screen in Work. The app now has a "Web pages in VR" card for that
(section 6).

Everything below was read on this PC without changing it: the browser binaries' strings, `pacman -Q`, the flag
files, the WebGPU shader cache's timestamp, and a headless Chrome on a throw-away profile in the scratch folder. The
page's HTML was fetched with curl into a scratch folder and only searched with grep, never run.

## 1. What the page needs

`https://oceano-piratas.vercel.app` is one HTML file of 96 KB with an inline module script and no external scripts.
Its title is "Ocean — WebGPU". It is a desktop ocean renderer in the style of `Noniv/web-ocean`
(see [web-ocean.md](web-ocean.md)) with a pirate-ship scene.

| API | In the page? | Evidence (grep of the HTML) |
|---|---|---|
| WebGPU | **yes** | `navigator.gpu.requestAdapter({powerPreference:"high-performance"})`, `getContext("webgpu")`, WGSL shaders, the error "WebGPU is not available in this browser." |
| WebXR (`navigator.xr`, `requestSession`, `immersive-vr`, `XRWebGLLayer`, `XRGPUBinding`) | **no** | 0 matches for each |
| WebGL | no | no `webgl`/`webgl2` context |
| Input | mouse and keyboard | `requestPointerLock` on click, "click: mouse look · WASD move · Space/E up · Q/C down · H panel · G rogue wave · T scene" |

So the page is a flat (2D) WebGPU page. It never asks for a VR session, and no browser, runtime or service can turn it
into one. To be in VR, the page itself must add WebXR (an "Enter VR" button that calls
`navigator.xr.requestSession("immersive-vr")`) and render each eye. With WebGPU it would render through the
WebXR/WebGPU binding (`XRGPUBinding`), which Chrome still keeps behind two flags ("WebXR/WebGPU Binding" plus "WebXR
Projection Layers"; both strings are in the installed Chrome, section 2).

### "Automatic when I enter" is not allowed by the spec

Even a page with WebXR cannot enter VR as it loads. The WebXR Device API (W3C, `requestSession()`, "immersive session
request is allowed") rejects an `immersive-vr` request unless it is made with **transient user activation** (inside a
click or key handler) or while the browser is "launching a web application". The one exception in practice is
`sessiongranted`, from the immersive-web *Navigation* proposal: a browser that is **already in VR** (Meta Quest
Browser) may grant the next page a session without a click. No desktop browser implements it. So the best any page can
do is one click on its "Enter VR" button.

References: W3C WebXR Device API, section 4 ("XRSystem", `requestSession`) and section 13 ("Feature policy" / user
intention); immersive-web/navigation explainer (`sessiongranted`); MDN `XRSystem.requestSession()` ("must be called
from within a user event handler").

## 2. Browsers on this PC and WebXR on Linux

Installed: **google-chrome 155.0.8059.39** (the default browser: `xdg-settings` says `google-chrome.desktop`),
**chromium 152.0.7977.82**, `webkit2gtk-4.1 2.52.6` (a library, not a browser), and `openxr 1.1.60` (the loader).
Firefox, Epiphany, Brave, Vivaldi and Wolvic are not installed.

| Browser | WebXR to a PC headset on Linux | Evidence |
|---|---|---|
| Chrome 155 / Chromium 152 | **no** | Chromium's `device/vr/README.md` lists OpenXR as enabled only on Windows (and Android). On Linux it is "not verified to build", with no loader. The installed `chrome` binary has `--force-webxr-runtime` and the value `openxr` in its switch list, but no OpenXR code: 0 `XR_KHR_*` / `xrCreateInstance` / `XR_RUNTIME_JSON` strings and no `libopenxr_loader` linked (same for chromium). A headless Chrome 155 has `navigator.xr`, but `isSessionSupported("immersive-vr")` never resolved. |
| Firefox | no | WebXR on desktop only existed on Windows (OpenVR/Oculus) and was never built for Linux. Not installed. |
| WebKitGTK / Epiphany | no (on Arch) | WebKit has an OpenXR backend behind `ENABLE_WEBXR`, but Arch's `webkitgtk-6.0` does not depend on `openxr` and `libwebkit2gtk-4.1.so` has no OpenXR strings. |
| Wolvic | no | Android headsets only. |
| Servo | no (in practice) | Its WebXR OpenXR backend targets Windows; not packaged. |

How a browser *would* find the runtime: the OpenXR loader reads `XR_RUNTIME_JSON` from its environment, then
`~/.config/openxr/1/active_runtime.json`. On this PC that file points at **SteamVR**
(`~/.local/share/Steam/steamapps/common/SteamVR/steamxr_linux64.json`; Games run with: SteamVR). Work hands Monado
only to its own children (`XR_RUNTIME_JSON` = the build's `openxr_monado-dev.json`). A browser started from the desktop
would therefore get SteamVR, and starting SteamVR outside Play is unsafe on this machine (LESSONS 34, 35). Today this
does not matter, because no installed browser loads OpenXR at all.

Chrome flags that would be needed on a platform where it works (Windows): `chrome://flags/#webxr-runtime` = OpenXR
(the default there), plus `#webxr-projection-layers` and `#webxr-webgpu-binding` for a WebGPU page. On Linux none of
them can help.

## 3. WebGPU in the installed Chrome

* **The rule:** gpuweb's Implementation Status lists Chrome on Linux as "NVIDIA (driver 535.183.01+) Wayland: 147" and
  "Intel Gen12+: 144". Other GPUs need `--enable-unsafe-webgpu --enable-features=Vulkan`.
* **This PC:** Chrome 155 on Wayland (`~/.config/chrome-flags.conf` from Omarchy has `--ozone-platform=wayland`),
  NVIDIA 610.57.04. WebGPU is **on by default; no flags needed.** `Local State` has no `chrome://flags` set.
* **Evidence it ran:** the profile's WebGPU shader cache `~/.config/google-chrome/Default/DawnWebGPUCache` (95
  entries) was written today at 13:40, when the ocean page was opened. A headless Chrome gets no adapter (headless has
  no Wayland surface: no adapter by default, SwiftShader with flags), so headless runs are not a valid test of WebGPU.
* **Not done on purpose:** no `--enable-unsafe-webgpu`. It turns off Chrome's GPU blocklist for every site and is not
  needed here. No flag files or profiles were changed.
* "Always up": WebGPU is not a service. It is part of the browser and is there whenever the browser runs. The app's
  card shows the verdict and the cache evidence (section 6).

## 4. Monado, SteamVR and a browser (for when a browser can do it)

* **Monado with WayVR (Work):** Monado takes several OpenXR clients, but only one main (visible, focused) app at a
  time; helpers must be `XR_EXTX_overlay` sessions (LESSONS 4). A browser's immersive session would be a second main
  app and would take the headset from the WayVR desk while it runs. The client would need Monado's
  `XR_RUNTIME_JSON` from the session (Work passes it only to its own children).
* **Play with SteamVR:** SteamVR's OpenXR runtime serves Chrome's WebXR on Windows. On Linux there is no browser that
  uses it, and SteamVR must only be started by the app's Play (LESSONS 34, 35).
* **Play with Monado (xrizer):** the same as Work, without WayVR.

## 5. Should the base services always be up while the app is open?

What runs in the background today (checked live): the app (`omarchyvr-app`, autostarts at login),
`omarchyvrd run` (the stack owner, observe-only by default), and `omarchyvr keepalived` (the USB keepalive that keeps the
headset awake). **Monado, WayVR, SteamVR and the AI do not run.** GPU memory in use: 812 MiB of 24 GB (the desktop).
Monado starts only with Play or Work (or "Start VR when I log in", off by default). That is by design and stays.

**Decision: no always-on Monado or VR runtime.**

| Cost of keeping Monado up | |
|---|---|
| The headset display | Monado takes the DRM lease of DP-1 and lights the panel at 72/90 Hz the whole time: heat in the headset, panel and backlight wear, power. |
| GPU | The compositor runs a frame loop at 90 Hz with distortion, plus its VRAM. Play's rule is that the game gets the whole graphics card (docs/GAME-MODE.md). |
| CPU and devices | Cameras, SLAM and controller tracking run; the eye chip is held (only one process can open it, LESSONS 3). |
| Play's runtime | SteamVR and Monado cannot share the headset. With "Games run with: SteamVR", Play would first have to stop the background Monado. |
| Benefit | **None for web pages:** no installed browser can use an OpenXR runtime (section 2). |

There is nothing for an "always on" switch to do, so none was added. The app also writes no desktop-entry override and
no flags. WebGPU needs none, and no WebXR flag works on Linux. If a browser with Linux OpenXR appears later, the plan
is in section 8.

## 6. What was built

Desktop page (the Work desk) > **Web pages in VR** (`rust/omarchyvr-app/src/pages/desktop.rs`, engine
`rust/omarchyvr-gui-core/src/webvr.rs`):

* **Open in VR:** type an address and press the button. During Work the app runs `omarchy-launch-browser <url>`
  (Omarchy's own launcher: the default browser with its flags; a running browser gets a new tab). Work's main VR
  screen is the main desktop and the other VR screens are Hyprland outputs, so the page is a flat screen in the
  headset, used with the controller pointer. Outside Work the button is greyed out with the reason ("Press Work first",
  or in Play "switch to Work"). Addresses are checked first: `https://` is added; only http(s) is allowed (no
  `javascript:`, `file:` or `data:`); spaces are refused.
* **What the browser can do,** read in the background when the page opens ("Check again" repeats it): browser and
  version; WebGPU (on by default / what is missing, with "a WebGPU page ran in it N ago" from the shader cache);
  "Page takes over the headset (WebXR): not possible", with the reason for that browser; and the OpenXR runtime that
  apps started outside the app see.
* Help: "Web pages in VR" in the app's help.
* Tests (`cargo test -p omarchyvr-gui-core webvr`, temp dirs, no real HOME written): desktop entry to browser, version
  parsing, Wayland from the flags file, the WebGPU rule (this PC = yes; old Chrome, X11, old driver, Firefox = what is
  missing), no Linux browser enters VR, runtime naming, address checks, Open only in Work, the report from files, the
  demo report. `probe_this_pc` (ignored; read-only) prints the real report. On 2026-10-17 it gave: Google Chrome
  155, WebGPU on by default, a WebGPU page ran 17 minutes ago, WebXR not possible, runtime SteamVR.

Not a omarchyvrd component: nothing keeps running (one detached browser launch), so VR is never affected.

## 7. Pitfalls found (for whoever adds browser flags later)

* `omarchy-launch-browser` runs **only the first word** of the desktop entry's `Exec=`. An override like the Steam
  one (`Exec=env VAR=1 google-chrome-stable %U`) would make it run `env <url>` and break every link. Use a wrapper
  program as the first word.
* Chrome honours only the **last** `--enable-features=`. Omarchy's `chrome-flags.conf` already has
  `--enable-features=TouchpadOverscrollHistoryNavigation`, and `/usr/bin/google-chrome-stable` puts the conf flags
  before its arguments. A second `--enable-features` silently drops Omarchy's. Merge them into one list.
* Omarchy's launcher starts the browser with `systemd-run --user`, so it gets the **systemd user manager's**
  environment, not the app's. An `XR_RUNTIME_JSON` must go into the wrapper (or `systemctl --user set-environment`),
  and a running browser keeps its old flags and environment until it is quit.

## 8. Open decisions for the user

1. **The ocean in VR today:** the same kind of sea is already a native Work background, "Ocean (animated)"
   (Backgrounds page; docs/BACKGROUNDS.md, [web-ocean.md](web-ocean.md)). It renders in the headset at frame rate, not
   in a browser. Is that what you wanted from this page?
2. **A browser that enters VR on Linux** needs one of these: Chromium patched with a Linux OpenXR backend (Vulkan
   swapchains; a large job, not upstream), WebKitGTK built with `ENABLE_WEBXR` and OpenXR, or waiting for upstream.
   When one exists, the app would launch it with the session's `XR_RUNTIME_JSON` (a wrapper, section 7). The page
   would still need its own "Enter VR" button and one click.
3. **The page itself** (if it is yours): add WebXR with an "Enter VR" button. WebGPU rendering needs the WebXR/WebGPU
   binding, which is still behind flags in Chrome. A WebGL fallback for the XR path is the portable choice.
