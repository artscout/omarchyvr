omarchyvr, prebuilt release
===========================

This folder is a finished build: nothing is compiled on your PC.

  1. Open a terminal in this folder and run:   ./install.sh
  2. The app opens. It asks for your password ONCE to finish the system setup
     (headset access rule, NVIDIA patch on a tested driver, the realtime permission).

What ./install.sh does
  - checks every file against MANIFEST.txt (a damaged download stops here, nothing is changed);
  - needs a running Arch Linux or Omarchy, and only the small runtime packages (it tells you which are missing;
    "./install.sh --install-packages" installs them with one password prompt);
  - refuses to run while a VR session is active;
  - copies the set into ~/.local/bin, ~/.local/lib/omarchyvr and ~/.local/share/omarchyvr, sets the library paths of
    the installed files for that folder, writes the OpenXR and SteamVR registration for those folders, and records every
    file in ~/.local/share/omarchyvr/manifest.tsv;
  - downloads the speech model (about 600 MB) at the end; if that fails it warns and goes on (run ./install.sh again).

Needs
  - the NVIDIA driver nvidia-open-dkms 610.57.04 (the version we patch and test; newer drivers show both eyes the left
    half until they are supported);
  - Steam with SteamVR installed in Steam: games run through SteamVR.

Remove it again:   in the app: Ready > Uninstall omarchyvr  (or ./uninstall.sh in this folder)

Files in here
  bin/                 the omarchyvr programs
  lib/omarchyvr/       Monado (OpenXR runtime + SteamVR driver), Basalt (6DoF), ONNX Runtime, speech library, WayVR,
                       deps/ (libraries that change often on Arch, such as OpenCV) and tools/patchelf (GPL-3.0,
                       used once by the installer)
  share/omarchyvr/     the data the app and the setup read
  MANIFEST.txt         version, commit, the Monado/WayVR fork commits, the Monado feature markers, which libraries are
                       bundled, and the sha256 of every file
  BUILD-PACKAGES.txt   the package versions of the clean builder this was compiled in

Build it yourself instead:   git clone --recursive https://github.com/laughtt/omarchyvr.git && cd omarchyvr && ./install.sh
Unofficial. Supports the Pimax Dream Air. Not affiliated with Pimax or Omarchy. Licences: LICENSE-MIT, LICENSE-APACHE,
THIRD-PARTY.md.
