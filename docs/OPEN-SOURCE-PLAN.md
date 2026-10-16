# Open-source plan: publish, install anywhere, test, and let Claude fix it

Date: 2026-10-15. Status: **a plan for the user to approve. Nothing was published, deleted or rewritten.**

**Update 2026-10-16 (P0 + first private export):** name **omarchyvr**, repo `github.com/laughtt/omarchyvr` (PRIVATE),
identity `laughtt` (GitHub noreply). Done here: per-unit/personal values out of the code (`~/.config/pimax/paths.toml`:
`src_dir`, `shared_drive_uuid` without default; controller models without serials; fake test serials and keys;
controller serials in the redactor), LICENSE-MIT/-APACHE/-CC-BY-4.0, THIRD-PARTY.md, NOTICE, `public-manifest.txt` +
`tools/export-public.sh` (allow-list, scrub, scanners, `cargo test --workspace` on the copy, known failures in
`tools/known-test-failures.txt`), `cfg(lab_data)` for the tests that replay private data (synthetic calibration and an
empty eye frame list stand in for them in the export). Open: crate/binary rename, eye frame list loaded at runtime,
forks (P1), CI, gitleaks in CI.
**Sync log (lab commit -> omarchyvr):** `8d09d9b` -> omarchyvr `f375aca` (2026-10-16, first export). Next: the 2026-10-16
evening export (ai/ folder, patches/nvidia, forks `github.com/laughtt/monado` and `github.com/laughtt/wayvr`, both
PRIVATE, branch `omarchyvr`, `main` = upstream base: Monado `e686ba77e5` + 60 commits, WayVR tag v26.8.0 `cb3aa42` + 30
commits, our commits rewritten to the laughtt noreply identity and scrubbed of home paths and the login name). Next
export: recording plus speech on the CPU.
**Update 2026-10-16 night: work continues in ~/omarchyvr; pimax-re is the archive/lab backup.** `0167cf8` -> omarchyvr
`a650ce1`; omarchyvr `678ce37` adds the private (git-ignored) `.claude/` with the lab rules, memory and lab data, the
leak-guard hooks and the forks as submodules (`forks/monado`, `forks/wayvr`). Last sync: lab `5174fc8`, `91bac42`,
`3e8a569` (speech on the CPU by default, distil-large-v3 note) carried over by the export diff (old/new export of the
lab, `diff -ruN`, applied here); the lab is frozen after `3e8a569`.
The audit in section 1 was read-only (pattern scans over the tracked tree and `git log -p --all`); no secret value is
written here, only the file, the commit count and the kind of hit.

Goal (user): open-source the stack. A user installs it easily on their machine, tests it with whatever headset they
have, and when something does not work, Claude finds the cause and makes a pull request that adds support for that
system, GPU, driver or headset. As automatic as possible, and built so it scales.

Related files (written by other agents, referenced here, not duplicated):
- [AI-KNOWLEDGE.md](AI-KNOWLEDGE.md): what we learned, pitfalls, where to look first, how to add hardware, how to
  diagnose a user's failure. This is the "AI file" every agent (ours or a user's) reads first. [../AGENTS.md](../AGENTS.md) points at it.
- [../README.md](../README.md) and [diagram/architecture.html](diagram/architecture.html): status, tested PC, architecture diagram.
- [ARCHITECTURE.md](ARCHITECTURE.md): device descriptors, HID allowlist + Gate, checks, Fix catalogue, Autofix reply contract (the base this plan builds on).
- [ROADMAP-OMARCHY.md](ROADMAP-OMARCHY.md): packaging blockers and Omarchy route. [../analysis/oss-readiness.md](../analysis/oss-readiness.md): the first audit (2026-10-14), refreshed below.
- [../analysis/research/opensource-ai-devops.md](../analysis/research/opensource-ai-devops.md): how other projects do AI PRs, hardware CI, diagnostics, perf tracking (items marked [unverified] there stay so here).

---

## 0. The plan in one page

1. **Publish a fresh public repo with a clean history**, built from an allow-list export of a scrubbed tree. This private
   repo stays private as the "lab" (captures, raw logs, eye-auth notes, calibration). Never flip this repo to public.
2. **Hardware support is data.** Headsets, GPUs/drivers, compositors and distros each get a small TOML manifest. A
   new machine is "supported" when its combination is covered by manifests plus test reports. Code is only added where
   data cannot say it (a Monado driver, a protocol encoder, a compositor adapter).
3. **One probe, one report.** `pimax-doctor --report` (exists in part: doctor + gui-core bundle + redactor) produces an
   allow-listed, redacted TOML that names the exact combination and what failed. It is the input for everything else:
   the compatibility matrix, GitHub issues, and Claude.
4. **Install = packages + a wizard.** Own signed pacman repo first (then AUR, Omarchy optional entry); the app's
   first-run wizard does the rest; all root steps go through one polkit helper with a fixed action list.
5. **The loop:** guided in-headset test in the app → test report + redacted bundle the user previews → either the
   user's own Claude Code works locally in their clone (reads AGENTS.md + AI-KNOWLEDGE.md, proposes a diff, the user
   clicks "Open pull request"), or the user opens a GitHub issue from a prefilled form and a maintainer label lets the
   Claude GitHub Action draft a PR. **A human always reviews and merges.** Protected paths (HID allowlist, root
   helper, packaging hooks, kernel patches, CI) need a maintainer plus evidence.
6. **Proof that a change helps:** frozen golden recordings replayed in CI (tracking, frame timing, still gate,
   corrections, eye, voice), statistical thresholds against `main`, an env switch to the old behaviour for every
   behaviour change, and opt-in hardware-in-the-loop rigs that never run fork code.

```
 user installs ──> wizard ──> "Test my setup" (in headset) ──> report.toml + bundle (preview, consent)
                                                                  │
                     ┌────────────────────────────────────────────┼─────────────────────────────┐
                     ▼                                            ▼                             ▼
         local Claude Code in the user's clone       GitHub issue form (summary only)     keep it (no network)
         (AGENTS.md, AI-KNOWLEDGE.md, replay tests)    maintainer label "ai-triage"
                     │                                            │
                     ▼                                            ▼
            draft PR from the user's account          claude-code-action drafts PR on bot branch
                     └────────────────────┬───────────────────────┘
                                          ▼
                   CI: tests, goldens, lints, secret/serial scan, replay benches (+ HIL on main only)
                                          ▼
                       human review (CODEOWNERS on protected paths) ──> merge ──> signed release
                                          ▼
                     compat matrix updated from the report (tier goes up for that combination)
```

---

## 1. Open-source readiness audit (read-only, 2026-10-15)

Scope: 1385 tracked files, 369 commits, all 1596 paths that ever existed in history (`git rev-list --objects --all`),
the full patch text of history, and the local forks (`~/src/monado-eye`, `~/src/wayvr`, `~/src/basalt`). Serials are
written as `P511****` / `H301****`. Scanner: a pattern script (counts and paths only), patterns for OpenRouter,
Anthropic, OpenAI-style, GitHub, AWS, Google, Hugging Face, Slack keys, private keys, bearer tokens, JWTs,
`key/secret/token/password = ...` assignments, Claude credential file names, headset and controller serials, e-mail
addresses, home paths, Windows user paths, the NTFS volume UUID.

### 1.1 Secrets (API keys, tokens, passwords)

| Finding | Where | Verdict |
|---|---|---|
| **No real API key, token or password in the tree or in history.** | all | The user's real keys live in `~/.config/pimax/ai.toml` (and a `.bak-1550` copy), both mode 0600, outside the repo. Checked: neither value nor its first 20 characters appears anywhere in `git log -p --all`. |
| Key-shaped **test strings** | `rust/pimax-voice/src/agent.rs` (test `OR_KEY`: an `sk-or-v1-` + 48 hex string), `rust/pimax-gui-core/src/{ai,autofix,redact}.rs`, `rust/pimax-config/src/ai.rs` (`sk-ant-...` test values), one `Bearer` in an agent.rs test | Fake (not equal to the real keys), but the `OR_KEY` one has a realistic format: GitHub push protection or partner secret scanning may flag it. Replace with obviously fake values (`sk-or-v1-TEST-NOT-A-KEY...`) before publishing. |
| Credential-handling scripts | `tools/make-transfer-stick.ps1`, `tools/restore-claude-omarchy.sh` (copy/restore the Claude login credential file) | No credential values inside (patterns did not match), but personal helpers that handle login files: **drop from the public tree.** |
| **Redactor gap (must fix before any report leaves a machine)** | `rust/pimax-gui-core/src/redact.rs:37` catches only `sk-ant-...` | An OpenRouter (`sk-or-...`), OpenAI or other key in a log would pass into the problem report. `pimax-voice`'s `redact()` (agent.rs:1082) is broader (any `sk-`/`sk_` token and long mixed tokens). Unify: one redactor crate used by the bundle, the voice agent and the doctor report, with the broad rules plus `Bearer`, `ghp_`/`github_pat_`, `hf_`, `AKIA`, JWTs and PEM blocks; test it with each kind. |

### 1.2 Personal data

| Kind | Tracked now | In history | Notes |
|---|---|---|---|
| Headset serial `P511****` | 67 hits, 44 files | 69 hits, 23 commits, 45 files | docs (HISTORY, STATUS, NEXT-STEPS, vm-test-plan), `analysis/*` (init-sequence, windows-6dof-params, cold-start-fwlog, device-map, eye-*), **code**: `rust/pimax-config/src/store.rs`, `rust/pimax-lens/src/lut.rs`; file name `calib/device_calibration_P511****.xml` |
| Controller serials `H301****` | 21 hits, 6 files | same | `analysis/windows-controller-info.md`, `controllers-*.md`, `init-sequence.md`, `windows-capture-20261015.md`, **code**: `rust/pimax-ctrltrack/src/model.rs` (default path under `pimax-data/.../H301****/`); file names in `calib/` |
| Serials inside binary fixtures | `rust/fixtures/if0_replies_coldstart.rec.gz` (4), `rust/pimax-eye/fixtures/eye_coldstart_init.usbpcap.gz` (1 ASCII + 8 UTF-16, the USB string descriptors), `rust/pimax-cam/tests/fixtures/headset-calibration.xml` (1) | same | Rewrite with same-length masks, re-run the replay tests |
| E-mail addresses | 24 files (19 Monado `From:` patch headers, `analysis/overnight/srgb-target.patch`, research notes) | **every commit's author/committer metadata** (two personal Gmail addresses; 368 + 1 commits); 57 Monado-fork commits | Fixed only by a fresh history with a public identity (GitHub `noreply` address) |
| Home path `/home/<user>` | 77 hits, 40 files | 25 commits | logs (`analysis/incidents/wayvr-exit-spin-20261015/*.prev`, 2.5 MB), `analysis/selfcal/lens.txt`, Monado patches, ROADMAP, PKGBUILD comment; 5 in Rust sources (most in tests of the redactor itself, fine) |
| Machine paths `~/src/...` | 63 hits in 41 Rust files | | `pimax-gui-core/src/paths.rs` etc. Not secret, but a portability blocker (ROADMAP 3.3) |
| Windows user paths `C:\Users\<name>` | 24 hits, 22 files | 26 hits, 23 files | `analysis/a*.py`, `analysis/scripts/imu_*.py`, `tools/*.ps1`, notes |
| NTFS volume UUID of the user's shared drive | 16 hits, 11 files, **incl. code** (`pimax-tools/src/mount_shared.rs`, `pimax-gui-core/src/{checklist,safety}.rs`, `pimax-tools/tests/vr_cli.rs`) | 17 hits | The "mount shared drive" feature is personal: make it a setting with no default, or keep it private |
| Room imagery (camera frames of the user's home) | `analysis/frames/*.pgm/png`, `analysis/frames/monado/*.png`, `analysis/overnight/cam/**.png`, `analysis/passthrough/**`, `analysis/hands-desk/*.jpg`, `analysis/controller-6dof/*.jpg`, `rust/pimax-cam/tests/fixtures/evening2.pxcam`, `rust/pimax-passthrough/tests/fixtures/*.png` | same | Not secret, but private. Drop from the public tree; tests use a consented, neutral recording (a blank wall or a lab) or synthetic frames |
| Chat / agent logs | none tracked (no `.jsonl` transcripts) | none | `.claude/CLAUDE.md` is project rules, fine to publish (generalised, section 5). `analysis/eye-coldstart/*-tobii-transcript.txt` are USB transcripts, not chats (see 1.3) |
| Keyboard input | raw pcaps are git-ignored (`*.pcap`, `captures*/`, `pimax-data/`); tracked `*.rec.gz` fixtures are per-report HID records of the headset only | none tracked | **The USB keyboard shares hub2 with the headset**: every Windows capture records key presses (`analysis/windows-capture-20261015.md` note 4). Any capture that leaves the user's machine must first be filtered to the headset's device addresses (VID 34a4 / 2104) by a tool, never by hand |

### 1.3 Material that must not be published

| Material | Where | Action |
|---|---|---|
| Pimax per-unit factory calibration | `calib/device_calibration_P511****.xml`, `calib/controller_params_H301****.json` (2), `calib/pimax_runtime_profile.json`, `analysis/headset-calibration.xml`, `analysis/headset-calibration-linux.xml`, `rust/pimax-cam/tests/fixtures/headset-calibration.xml` | Remove. Code that reads them (`pimax-tools/src/calib_cmd.rs`, `imu.rs`) reads the headset (`pimax calib`, USB READ_DOC) or `~/.config` instead; tests use a synthetic or masked fixture |
| Pimax Play install files, DLL dumps, crash dumps, firmware updater logs, SteamVR driver resources | `pimax-data/` (2.9 GB, git-ignored; never in history: no dll/exe/sys/dmp/firmware blob ever tracked) | Keep out. Add a CI check that refuses these extensions |
| Firmware | none in the repo | Keep it that way; "never flash" stays a hard rule |
| Tobii eye-chip protocol transcripts and the eye-auth analysis | `analysis/eye-coldstart/*-tobii-transcript.txt`, `*.msgs`, `analysis/eye-auth-feasibility.md`, eye-auth parts of `analysis/eye-coldstart.md` | **Keep private.** Anti-circumvention is the highest legal risk in the repo. The public eye code says only what it sends and why no authentication step is needed |
| Mercury hand models (no licence) | not in the repo (fetched into `~/src`) | Never bundle. The app's "Download hand models" button fetches them on the user's machine from upstream, with a "personal use, no licence" note |
| Opaque pickles | `analysis/p82.pkl`, `analysis/p83.pkl` (5.9 MB; loading a pickle runs code) | Drop; the `.rec.gz` replays cover them |
| Raw kernel/firmware logs | `analysis/logs-hang-*/{fw,kern}.log`, `analysis/overnight/polish-data/*/fwlog.txt`, `analysis/logs-xid31-*.txt`, the incident logs | Drop; keep the findings in the `.md` files |
| Personal helper features | `pimax mount-shared`, the transfer-stick and restore scripts | Private, or a setting without the user's values |

### 1.4 Trademark

"Pimax", "Dream Air", "Crystal" are Pimax trademarks. Use them only to say what the software works with ("for Pimax
headsets"). No logos, no "Pimax" in the project, package or app name, a "not made or endorsed by Pimax" line in the
README, the app's About page and the package description. Today the app is `pimax-setup` and the packages are
`pimax-linux*`: rename the user-facing names before the first public release (decision D2). Binary names can keep a
neutral prefix; config paths (`~/.config/pimax`) can migrate later with a one-time move (a migration, section 3.4).

### 1.5 Licences

| Component | Licence | What it means for us |
|---|---|---|
| Our Rust workspace, app, tools, Python helpers, docs | `MIT OR Apache-2.0` declared in `rust/Cargo.toml`, **no LICENSE file in the repo** | Keep it (Rust norm, links fine into BSL Monado through the C ABI). Add `LICENSE-MIT`, `LICENSE-APACHE`, `THIRD-PARTY.md`, SPDX headers on new files. Docs and data: `CC-BY-4.0` |
| Monado fork (`~/src/monado-eye`, branch `desk`, 57 commits, 56 files, +9083 lines over upstream main) | BSL-1.0 (REUSE layout, `LICENSES/`) | Publish as a fork; keep notices; our driver files carry BSL-1.0 SPDX headers like the rest of Monado |
| WayVR fork (`~/src/wayvr`, branch `eye-gaze`, 22 commits) | GPL-3.0 | Our changes are GPL-3.0; the fork's source must be public for any binary we ship |
| xrizer | GPL-3.0-or-later | Use upstream (AUR/own build); separate program, no licence mixing |
| Basalt (`gitlab.freedesktop.org/mateosss/basalt`, unmodified) | BSD-3-Clause | Build dependency; keep notice |
| OpenCV (Basalt and Monado tracking deps) | Apache-2.0 (4.5+) | System package |
| ONNX Runtime 1.20.1 (vendored by hand) | MIT | Use the distro/AUR package or a pinned download with its notice |
| NVIDIA open kernel module patches (`linux/nvidia-*.patch`) | the module is dual MIT/GPL-2.0 | Our patches under the same dual licence; offered upstream |
| CustomHeadsetOpenVR fork (local, unused) | GPL-2.0 | Do not ship or mix |
| Mercury hand models | none | Never redistributed (1.3) |
| Whisper / Qwen / sherpa models | their own | Downloaded on first use, never bundled |
| SteamVR, Steam, Pimax Play, Tobii firmware | proprietary | Never bundled; we talk to them or read their outputs only |
| Reverse-engineered HID facts | facts, not code | Interoperability; publish our own code and the facts, never vendor code, decompiled sources or binaries; keep the eye-auth analysis private |

### 1.6 What to split out, and the public repo layout

Recommendation: **one public monorepo for our code + public forks for Monado and WayVR, pinned together.** This keeps
the project rule "one whole build" (one version of everything) without vendoring other projects' history.

```
<project>/                          (public, MIT OR Apache-2.0)
  AGENTS.md  CLAUDE.md  CONTRIBUTING.md  SECURITY.md  LICENSE-*  THIRD-PARTY.md
  docs/                    user + developer docs, AI-KNOWLEDGE.md, LESSONS.md
  rust/                    app, tools, libpimax (C ABI), device modules, doctor, benches
  hw/                      manifests: headsets/<id>/device.toml, gpus/*.toml, compositors/*.toml, distros/*.toml (section 2)
  reports/                 accepted test reports (TOML, redacted) -> compat matrix
  components.lock          pinned commits of the Monado fork, WayVR fork, xrizer, Basalt, ONNX Runtime
  packaging/arch/          PKGBUILDs for every package, pacman hooks, polkit policy
  linux/                   udev rules, EDID tools (generated EDIDs only), NVIDIA patches (until upstream)
  windows/                 capture tools (read-only collectors; filter to headset devices before export)
  bench/corpus.toml        list + hashes of the golden recordings (stored as release assets / LFS, CC-BY-4.0)
  .github/                 workflows, issue forms, CODEOWNERS, rulesets
<project>-monado            public fork of Monado, branch `pimax` rebased on upstream as a clean patch series
<project>-wayvr             public fork of WayVR, branch with the gaze/screens patches
private lab repo (this one) captures, raw logs, eye-auth notes, calibration, analysis drafts
```

Split the Monado series into (a) **upstreamable generic fixes** (compositor gamma/gain env, deferred swapchain image
free, rttime preload for SteamVR, `driver_monado` haptic type init, Touch profile paths) sent as merge requests to
Monado and (b) the **Pimax driver** (`drivers/pimax`, behind `XRT_BUILD_DRIVER_PIMAX`), which can also be proposed
upstream once it builds without our Rust library or with it as an optional dependency. Same for WayVR: gaze target,
desk screens, bounded shutdown as PRs to `wlx-team/wayvr`. Every patch that lands upstream is one less to rebase.

### 1.7 Fresh public repo or flip this one: **fresh, with a clean history**

Flipping is not possible without leaking: serials, calibration, e-mails and room images are in 369 commits of history,
and `git filter-repo` on this repo would still be a risky one-shot. A fresh repo from an **allow-list export** is
simpler and safer, and the private repo keeps the full history.

How: a script `tools/export-public.sh` (to write in phase P0) copies only the paths listed in `public-manifest.txt`
(allow-list, not deny-list), runs the scrubber (same-length masks), runs the scanners and the tests on the copy, and
makes one initial commit with the public identity. After the publish, **the public repo becomes the place where code
is developed**; this repo shrinks to the private lab (decision D1). No two-way sync.

### 1.8 Checklist before publishing (for the user to approve, item by item)

| # | Check | Who |
|---|---|---|
| 1 | Name, public identity (GitHub org/user, `noreply` e-mail) and licence decided (D1-D4) | user |
| 2 | `public-manifest.txt` reviewed: every top-level path is either in or out; `analysis/` only the protocol/display/tracking findings, no eye-auth, no raw logs, no frames | user + agent |
| 3 | Code de-personalised: serial paths in `pimax-ctrltrack/src/model.rs`, `pimax-lens/src/lut.rs`, `pimax-config/src/store.rs`; NTFS UUID code; `~/src` paths read from one config | agent |
| 4 | Binary fixtures re-masked (`if0_replies_coldstart.rec.gz`, `eye_coldstart_init.usbpcap.gz`, cam calibration fixture); `cargo test --workspace` green on the export | agent |
| 5 | Calibration files, pickles, raw logs, room images, transfer/restore scripts absent from the export | agent, user looks at the file list |
| 6 | Test key strings replaced with obviously fake ones; redactor unified and tested (1.1) | agent |
| 7 | Scanners clean on the export: gitleaks (or trufflehog) + our serial/home/UUID/e-mail patterns + an extension deny-list (dll, exe, sys, dmp, pcap, pkl, firmware) | CI on the export |
| 8 | LICENSE files, THIRD-PARTY.md, SPDX headers, "unofficial / not affiliated" lines, no-warranty and "can brick your headset" notice in README, app About and the wizard's first page | agent |
| 9 | Monado and WayVR forks: history rebuilt as a clean patch series under the public identity, notices kept; CLAUDE.md rule "never push" changed for these two repos only, by the user (D5) | user decides, agent prepares |
| 10 | GitHub settings: private vulnerability reporting on, secret scanning + push protection on, branch ruleset on `main` (PR + 1 review, CODEOWNERS for protected paths, signed commits optional), Actions pinned by SHA, default `GITHUB_TOKEN` read-only | user clicks, agent lists |
| 11 | The user reads the final file list and the README once, then creates the repo as **private**, checks it on GitHub, and only then flips it public | user |

---

## 2. Scalable hardware support: the matrix as data

### 2.1 What "support" means, and where code is unavoidable

A working setup is a combination: **headset × GPU/driver × kernel × compositor × distro × game runtime**. Most of the
combination is data (IDs, versions, env vars, checks, known problems). Code is only needed for:

| Needs code | Example | Plugin point |
|---|---|---|
| Talking to a new headset (protocol, display bring-up, tracking) | Pimax: `libpimax` + Monado `drivers/pimax` | Monado driver; our `HeadsetModule` trait (`rust/pimax-device/src/module.rs`) for encoders that data cannot describe |
| A new compositor's lease / output handling, window control for voice | Hyprland adapter today | `Compositor` trait (new, section 2.4) |
| A GPU quirk that needs logic, not just env | NVIDIA 2Heads1OR mode detection | `Check` + `Fix` (exists) |

Headsets that **upstream Monado already drives** (Valve Index/Vive via the vive/survive drivers, WMR headsets,
Rift S, North Star; Quest through WiVRn) need no protocol work from us: a manifest with `driver = "monado:<name>"`
lets the app, doctor, wizard, desk mode and voice work on them. This is how the project scales past Pimax without
reverse engineering each headset. Pimax models that are not yet captured stay `status = "mock"` until someone runs
the Windows capture session (the only honest way to add their commands).

### 2.2 Manifests

Existing: `rust/pimax-device/devices/<id>/device.toml` (schema in ARCHITECTURE.md 2.2; Dream Air real, SE / Crystal
Light / Crystal Super / 8KX mocks with unverified PID candidates). Move to `hw/headsets/` and add three siblings. All
are plain TOML parsed by `toml_lite`, compiled in with `include_str!`, validated by tests, every value with a `source`.

```toml
# hw/headsets/<id>/device.toml   (exists; additions in bold in the text below)
[device]   id, name, vendor, status = real|mock|upstream, usb_vid, usb_pids, usb_pid_candidates
           driver = "pimax" | "monado:vive" | "monado:wmr" | "wivrn"     # new: who drives it
[display]  mode, refresh_hz, edid_vendor, edid_product, needs = ["drm_lease", "edid_override_non_desktop", "nvidia_2heads1or_patches"]
[tracking] [controllers] [eye_tracking] [ipd] [cameras] [audio] [calibration] [onboarding]
[[hid]] / [[hid_denied]]                                    # allowlist, unchanged; only for driver = "pimax"
[[quirk]]  id, applies_when = "gpu.vendor=nvidia", env = {...}, why, source   # new

# hw/gpus/nvidia.toml   (new)
[vendor]  id = "nvidia", pci_vendor = 0x10de, kernel_modules = ["nvidia-open", "nvidia"]
[[driver]] range = ">=610 <620", status = "works", needs_patches = ["viewport", "flip-fix", "bounded-wait"],
           params = ["nvidia-drm.modeset=1", "nvidia_modeset.merged_head_sync=1"], tested_reports = 1
[[driver]] range = "<580", status = "unknown"
[[known_problem]] id = "flip-hang", symptom = "headset black, flip never completes", check = "gpu.merged_head_sync", fix = "nvidia.patches", doc = "analysis/nvidia-flip-hang.md"
[[env]]   key = "XRT_COMPOSITOR_FORCE_NVIDIA", value = "0", why = "...", source = "LESSONS 5"

# hw/gpus/amd.toml, hw/gpus/intel.toml   status = "unknown" until a report says otherwise

# hw/compositors/hyprland.toml   (new)
[compositor] id = "hyprland", detect = "env:HYPRLAND_INSTANCE_SIGNATURE", min_version = "0.50"
lease = "wp_drm_lease_device_v1", non_desktop = "honoured", desk_outputs = "headless", window_control = "hyprctl"
status = "works"
# hw/compositors/kwin.toml, mutter.toml, sway.toml, gamescope.toml: lease support known from upstream docs, status "unknown"

# hw/distros/arch.toml   (new)
[distro] id = "arch", detect = "os-release:ID=arch|ID_LIKE=arch", packages = "pacman", repo = "<project> pacman repo", polkit = true
```

Support levels, computed, not hand-written: **0 untested** (manifest only), **1 reported** (≥1 accepted test report),
**2 replay-covered** (golden recordings from that combination run in CI), **3 rig** (a hardware-in-the-loop runner
with that combination). The compatibility page and the app's "Your setup" card are generated from `hw/` + `reports/`.

### 2.3 Capability probing: `pimax-doctor --report`

Builds on what exists: `pimax-doctor` (13 read-only checks, Host/FakeHost), `pimax-device` (detect, checks, Fix
catalogue), `pimax-gui-core` `facts.rs`, `bundle.rs`, `redact.rs`. Output is one TOML with **allow-listed fields
only** (never the environment, never free-form file dumps):

| Section | Fields |
|---|---|
| `[report]` | schema version, random report id (not a hash of a serial: hashed serials are still stable IDs), app version, `components.lock` commits, date |
| `[host]` | distro + version, kernel, CPU model, RAM (GB, rounded), Secure Boot on/off |
| `[gpu]` | vendor, PCI device id, name, VRAM, driver + version, module type (open/closed), patches present, module params of interest, Xid lines this boot (codes only) |
| `[display]` | connector name, EDID vendor/product (no EDID serial bytes), modes offered, non-desktop flag, lease protocol seen |
| `[compositor]` | name + version, lease support, outputs count |
| `[usb]` | VID:PID list **filtered to known XR vendors** + speed + same-hub flag; never other devices (the keyboard!) |
| `[headset]` | matched manifest + status, firmware versions (compared with `firmware_seen`), no serials |
| `[runtime]` | active OpenXR runtime (which build), openvrpaths owner, SteamVR / Proton versions, pressure-vessel version |
| `[findings]` | one line per check (`Finding`: id, severity, title) |
| `[[result]]` | from the guided test (section 4.1): feature, pass/fail, metrics |

The redactor still runs over the result (belt and braces), CI rejects any report matching the serial, e-mail,
home-path or key patterns, and the app shows the exact file before anything is shared.

### 2.4 Plugin points (so a new X is data + a small file)

| New thing | Data | Code (if any) | Tests |
|---|---|---|---|
| Headset driven by upstream Monado | `hw/headsets/<id>/device.toml` with `driver = "monado:<name>"` | none | descriptor lint; doctor FakeHost scenario |
| Pimax-family headset | descriptor + `[[hid]]` from captures | `rust/pimax-device/src/devices/<id>.rs` only if encoders differ; Monado driver changes if the display/tracking path differs | allowlist replay test on the captures; gate refuses non-listed bytes |
| GPU / driver version | `hw/gpus/<vendor>.toml` `[[driver]]` range, `[[known_problem]]`, `[[env]]` | a `Check` in `pimax-device/src/checks/gpu.rs` if it needs logic | FakeHost scenario with that driver's sysfs/modinfo |
| Compositor | `hw/compositors/<id>.toml` | `Compositor` trait impl: `detect`, `lease_ready`, `prepare_headset_output`, `desk_outputs`, `window_ops` (voice `win.*`) | fake Wayland registry fixture |
| Distro | `hw/distros/<id>.toml` | packaging recipe | install test in a container |
| Fix | `Fix` catalogue entry (why, root, reboot, commands, undo) | none (catalogue is reviewed code) | `FixRef::plan()` tests |

The rule from ARCHITECTURE.md stays: nothing outside `hw/` and `devices/` assumes the vendor is Pimax.

---

## 3. Easy install

### 3.1 Packages (Arch first)

| Package | Content | Root step it carries |
|---|---|---|
| `<project>` (today `pimax-linux-git`) | app + all Rust tools + udev rules + polkit policy + root helper + autostart + docs | udev reload (hook) |
| `<project>-monado` | the Monado fork, one build with every marker; `check()` greps the markers; provides the OpenXR runtime json and `driver_monado` | `cap_sys_nice` on `monado-service` in `post_install`/`post_upgrade` (survives every upgrade; the app's pkexec Fix stays as a safety net) |
| `<project>-wayvr` | the WayVR fork (`conflicts/provides` with AUR `wayvr`) | none |
| `xrizer` | upstream (AUR) or our build of the pinned commit | none |
| `<project>-nvidia-patches` (opt-in) | DKMS patch set + pacman hooks (`65-...-patch`, `75-...-verify`) | one pkexec prompt, reboot; refuses to leave an unpatched module silently |
| hand models | **never a package**; app download button | none |

Build all packages in CI in a clean chroot (`makechrootpkg`) from tagged sources, sign them with the project key,
publish a pacman repo as GitHub Release assets (or Pages) with a signed database. AUR packages follow once stable.
Omarchy: no default install; ask the maintainers for an optional "Install > VR" entry later (ROADMAP-OMARCHY 4.5).
Updates follow Omarchy's pattern: **forward-only, idempotent migrations** recorded in a state file
(`~/.local/state/<project>/migrations`), run by the app between sessions only (never during VR).

### 3.2 Other distros (feasibility)

| Route | Verdict |
|---|---|
| Flatpak | **Not for the core.** It needs udev rules, file capabilities, a kernel module, the DRM lease from the host compositor, hidraw/usbfs/video access and host OpenXR runtime registration; a sandbox cannot provide them. The app alone in Flatpak gains nothing |
| AppImage (exists as a script) | Fine for the app + Rust tools; Monado must still come from a package or a local build |
| Fedora COPR, Ubuntu/Debian `.deb`, openSUSE OBS | Feasible with the same recipe set once paths come from config; one per distro family, built in CI containers |
| Envision-style local build | The app builds the pinned Monado/WayVR commits on the user's machine (profiles per headset). Good fallback for distros we do not package; slower, needs build deps |
| NixOS | Community flake later; do not promise |

Phase 1 is Arch/Omarchy only, said plainly.

### 3.3 First-run wizard (app)

Extends the existing Onboarding stepper (`onboarding.rs`, steps from the manifest): **detect → checks → fixes (one
grouped root prompt) → udev → reboot if needed → display test → IPD → pairing → eye calibration (if any) → room →
"Test my setup" (4.1) → done.** Each step: Run, Skip, Back, status in plain words, resume after reboot.

Root steps: one small helper binary (`<project>-root-helper`) with a polkit policy that lists **fixed actions**
(`install-udev`, `setcap-monado`, `install-edid-override`, `install-nvidia-patches`, `set-kernel-param`), each
validated (paths and words only, the `safe_arg` rule) and logged with an undo. The app asks once per group of fixes
("These 3 changes need your password"), never runs shell text. This replaces ad-hoc `pkexec sh -c` lines.

### 3.4 The NVIDIA module and EDID problem

| Problem | Today | Make it optional / upstream |
|---|---|---|
| Flip hang on the 2Heads1OR mode (7104x3840) | three DKMS patches + `merged_head_sync=1`, pacman hooks | (1) Post the NVIDIA PR (draft `analysis/nvidia-pr1262-comment-draft.md`; user posts it). (2) Until merged: the opt-in patches package, detected and offered by the wizard. (3) Investigate a reduced mode that works on a stock driver (lower refresh or a single-head mode), so the first picture works without patches; label it "compatibility mode" (unknown today, needs a test) |
| Panel not marked non-desktop (Hyprland would use it as a monitor) | EDID override in initramfs (`linux/pimax-dp-edid-msvsdb.bin` via `/etc/pimax`), machine-specific connector | (1) Propose a kernel `drm_edid` non-desktop quirk for EDID `PVR 0x1019` (check first whether current kernels already list it); then no override is needed on new kernels. (2) Meanwhile the root helper installs the override with the detected connector, with undo |
| Secure Boot | DKMS modules must be signed | the wizard detects Secure Boot and explains the MOK enrolment in plain words before offering the patches |
| AMD / Intel | untested | manifests say `unknown`; the first AMD report decides; no promises |

---

## 4. Automatic test and the "Claude fixes it" loop

### 4.1 "Test my setup" (guided, in the headset)

A button in the app (and the wizard's last step). It talks to the user **in the headset** (voice + in-VR text; desktop
notifications are invisible while wearing it). Each step records pass/fail plus metrics into `test-report.toml`:

| Step | Measured automatically | User action |
|---|---|---|
| Picture | time to picture, mode, refresh, missed frames over 30 s, late % | "Do you see the grid sharp in both eyes?" (controller or voice yes/no) |
| Head tracking 3DoF/6DoF | IMU rate, 6DoF poses vs lost, still-gate re-zeros, corrections ≥ 20 mm | look around, take a step |
| Controllers | link, battery, every button/stick/touch seen, haptic pulse acknowledged | press what the headset shows |
| Hands / eye (if present) | detection rate; eye validation error (deg) | look at targets |
| Audio / mic | sink found, mic level | say "Max, test" |
| Games path | `pimax vr game-check` result | none |

Recording during the test is **IMU + poses + logs + perf stats only**; camera frames are never recorded by the test.

### 4.2 Diagnostics bundle levels (opt-in, previewed)

| Level | Content | Default destination |
|---|---|---|
| L0 summary | `report.toml` (2.3) + `test-report.toml` | may go in a public GitHub issue |
| L1 logs | redacted log tails (Monado, WayVR, SteamVR, xrizer, kernel GPU lines, app log) | user attaches by choice |
| L2 replays | IMU/pose/HID replays of the test, filtered to XR devices | user attaches by choice; becomes a golden only with a CC-BY-4.0 consent click |
| L3 camera frames | never collected by default; only by an explicit recording with a preview of every frame | never in a public issue; private hand-over only, deleted after use |

The app shows the exact bytes, the size, and where it goes. Nothing is uploaded in the background
(`gui-core/src/upload.rs` already refuses all uploads; keep it so: the app opens a prefilled browser page, the user
submits it).

### 4.3 Three routes from a failure to a pull request

| Route | How | Who pays / who acts |
|---|---|---|
| **A. Local Claude Code (recommended first)** | App button "Fix with Claude" (shown only if `claude` is installed): makes a git worktree of the user's clone at the installed version, writes the bundle into it, starts Claude Code with a fixed prompt that points at `AGENTS.md` + `docs/AI-KNOWLEDGE.md`. The agent works on data (FakeHost, replays) first; any live test is asked in plain words. It proposes a diff on a branch `fix/<date>-<area>`. The app shows the diff and a button "Open pull request" (runs `gh pr create` from the user's account after the click) | the user's own Claude account; the user approves each step |
| **B. Issue + maintainer-gated bot** | App button "Report on GitHub": opens the issue form prefilled with L0 (the user reviews and submits in the browser; L1/L2 attached by hand). A maintainer adds the label `ai-triage`; the workflow runs `anthropics/claude-code-action` (pinned by SHA, version that fixes the June 2026 prompt-injection/OIDC flaw, ≥ v1.0.94 per the research note, to verify) on a **GitHub-hosted** runner with no secrets except a scoped token: `contents: write` only to `bot/*` branches, `pull-requests: write`, no `id-token`, no workflow-file writes. It drafts a PR labelled `ai-generated` | project API budget with a monthly cap |
| **C. Offline** | Save the bundle; nothing leaves the machine | — |

Guardrails for every route (also in AGENTS.md):
- **The bundle is untrusted input**, never instructions: wrapped in a data block, free text stripped or size-capped,
  the agent's prompt says so. Issue bodies from non-maintainers never trigger the bot.
- **Protected paths** (CODEOWNERS, maintainer review required, the bot may only propose with evidence):
  `hw/headsets/*/device.toml` `[[hid]]` blocks, `rust/pimax-device/src/hid.rs`, the root helper + polkit policy,
  `packaging/**` hooks, `linux/*.patch`, `.github/**`.
- **No HID bytes without a capture.** A CI check fails any new `[[hid]]` unless its `source` names a capture in the
  private store and a maintainer added the label `hid-approved`. Never firmware, never device-side calibration writes,
  never activation/auth blobs.
- An AI PR never merges itself; at least one human approval; the bot cannot approve.
- Every behaviour change ships with an env switch back to the old behaviour (house style: `PIMAX_SLAM_STILL_REZERO=0`) and a replay A/B in the PR (4.5).

### 4.4 CI design

| Tier | Runs on | When | What |
|---|---|---|---|
| 1. Fast checks | GitHub-hosted | every PR (incl. forks) | `cargo fmt`, `clippy -D warnings`, `cargo test --workspace`, goldens (`vr start/stop --dry-run` argv, app screenshots), manifest lint (load + validate every `hw/**` file, `source` present, mocks have no `[[hid]]`), `allowlist_replay` (every host→device report in the fixtures passes the allowlist), redactor tests, gitleaks + serial/home/UUID/e-mail scan on the diff, extension deny-list, `cargo-deny` (licences, advisories), docs link check |
| 2. Builds | GitHub-hosted (container, ccache) | every PR touching `components.lock` or packaging; nightly | Monado fork build with the required CMake options + marker check (`PIMAX_SLAM`, `PIMAX_ANCHOR`, `PIMAX_EYE`, `PIMAX_CTRL_TRACK`, `held from the tracker`, ...), WayVR build, all PKGBUILDs in a clean chroot, `namcap` |
| 3. Replay benches | GitHub-hosted (CPU) | every PR touching tracking/timing code; nightly on `main` | golden corpus through `pimax-slam bench-windows`, `bench-basalt`, `stillgate`, `corrections`, `ate`, `timing`; eye replays (`pimax-eye` goldens); `pimax-voicebench`. Results posted as a PR comment against `main` with thresholds (4.5) |
| 4. Hardware in the loop | self-hosted rigs (opt-in volunteers, a headset on a stand) | `main` after merge, nightly, or `workflow_dispatch` by a maintainer; **never fork PRs, never `pull_request_target` with checkout of PR code** | the headless HIL script: start session, 60 s of frames (missed, late, GPU ms), IMU/camera/controller rates, static 6DoF drift, controller link, stop clean (no leftover `monado-service`, cap still set). Runner is ephemeral (JIT), has no secrets, runs as a separate user, refuses if a user session is active, and uses the same allowlist Gate (it cannot send anything the app cannot) |

Results of tiers 3-4 go to a time series (Bencher or `github-action-benchmark` on a `gh-pages` branch), keyed by
the combination (headset, GPU, driver, kernel, compositor).

### 4.5 How we know an improvement is real

| Area | Metric | Reference |
|---|---|---|
| Head tracking | ATE/RPE vs the Pimax pose log, lag, jitter, 6DoF losses per minute, still-gate firings, correction steps ≥ 20 mm | Windows capture takes (`pimax-slam bench-windows`, `analysis/tracking-vs-windows.md`) and our recordings (`stillgate`, `corrections`) |
| Frame timing | missed frames per minute (`comp_skipped`), late %, GPU ms vs budget, time to picture | HIL rigs + perf files (`PIMAX_PERF_FILE`) |
| Controllers | optical tracking error vs IMU/pose log, haptic latency | recorded takes |
| Eye | validation error (deg) per eye | `pimax-eyecal --refit` runs |
| Voice | intent accuracy, WER, latency | `pimax-voicebench`, `pimax-voice audit` |
| Install | wizard completion, steps failed per report | accepted test reports |

Rules:
1. **Golden corpus is frozen and versioned** (`bench/corpus.toml` with hashes); adding a recording is a reviewed PR; no camera frames of private homes (consented lab recordings only).
2. **Noise floor first**: run `main` against itself; a change counts only if it beats the noise with a statistical test (bootstrap CI or Mann-Whitney over several recordings), not a single number.
3. **A/B through env switches**: the PR comment shows old vs new on the same replays; live A/B in the headset uses the same switch (the app's developer mode toggles it; the user does not know which is which for subjective checks).
4. **No live default changes from a replay alone** for anything the user feels (smoothing, prediction): replay first, then a short in-headset A/B, then the default (the house rule already followed in `d33316e`, `32b9386`).

---

## 5. Safety and trust

### 5.1 What an automated agent must never do on a user's machine

Generalised from `.claude/CLAUDE.md` and LESSONS; goes into `AGENTS.md` and the prompt of route A:

1. No root: no `sudo`; root steps only as catalogue actions through the root helper, after the user's click and password.
2. No device writes outside the allowlist; **never flash firmware**; never device-side calibration, activation or authentication commands; never a byte not seen in a vendor capture.
3. Never force a DRM hotplug, never edit bootloader, kernel parameters, systemd system units, compositor or desktop configs except through a reviewed Fix with undo.
4. Never touch a running VR session; install only between sessions; stop by PID, never by name pattern; never SIGKILL Monado.
5. Never tell the user to unplug the headset; never ask them to type commands (the app has the button).
6. Read only what the diagnosis needs: the allow-listed logs and state dirs. Never browser profiles, keyrings, SSH/GPG, mail, shell history, the full environment, other repos.
7. No network sends without a per-action click (issue, PR, upload, online AI). No telemetry.
8. Never commit or push in the user's name without the "Open pull request" click; never push to forks the user did not create.
9. Camera frames never leave the machine; recordings are local, listed in the app with size and a delete button, and expire after N days by default.
10. Keep every change undoable (fixes.log + Undo), and say in plain words what changed.

### 5.2 Consent and privacy

- Every share is opt-in, previewed, per report; L3 (camera frames) needs its own consent and never goes public.
- Recordings show a visible "recording" indicator in the headset and in the app.
- A privacy statement in the README and app: what is collected (nothing by default), what a report contains, how to delete a submitted report (close the issue, ask to delete attachments).
- Reports in `reports/` are the L0 summary only (ProtonDB model: CPU/GPU/driver, no account ids), CC-BY-4.0.

### 5.3 Signing and supply chain

- Packages and the pacman repo database signed with a project key (offline primary key, a CI subkey in a protected environment that only release tags can use).
- Release tags signed; GitHub artifact attestations (build provenance) for every package; checksums published.
- Actions pinned by commit SHA; Dependabot for actions and cargo; `cargo-deny`; branch protection; 2FA required for maintainers.
- The self-hosted rigs never receive release keys.
- `SECURITY.md` with private vulnerability reporting; a "headset safety" section (allowlist, no flashing).

---

## 6. Phased roadmap

Effort is agent working days (A) plus user time (U). Phases P0-P2 are the path to a first public release.

| Phase | Work | Done when | Effort |
|---|---|---|---|
| **P0 Clean and decide** | decisions D1-D8; unify the redactor; fake test keys; de-personalise code paths (serials, UUID, `~/src` from one config); synthetic/masked fixtures; LICENSE + THIRD-PARTY; `public-manifest.txt` + `export-public.sh` + scanners; fix the 4 failing tests (ROADMAP 3.4) | export passes scanners and `cargo test --workspace` | A 4-6 d, U 2 h |
| **P1 Publish** | fresh repo (private first), forks as public repos with clean patch series, rulesets, CODEOWNERS, issue forms, AGENTS.md/CONTRIBUTING/SECURITY, CI tiers 1-2 | user flips public after a look | A 3-4 d, U 1 h |
| **P2 Packages + wizard** | all PKGBUILDs, clean-chroot CI, signed pacman repo, root helper + polkit, migrations, wizard extended | a fresh Omarchy install reaches a green Play after one reboot (VM or second disk) | A 8-12 d, U 2 h (test) |
| **P3 Hardware as data** | `hw/` manifests (gpus, compositors, distros), `driver = "monado:<name>"` path, `pimax-doctor --report`, compat matrix page, support tiers | a report from this PC generates the matrix row; a Vive/Index manifest passes lint | A 5-7 d |
| **P4 Test my setup** | guided in-headset test, test-report.toml, bundle levels L0-L2, issue form prefill | the user runs it once and the report needs no editing | A 6-8 d, U 1 h |
| **P5 Claude loop** | route A button + worktree + prompt; route B workflow (label-gated, pinned, scoped); `hid-approved` check; protected paths | a staged failure (e.g. missing udev rule on a FakeHost) becomes a correct draft PR via both routes | A 4-6 d |
| **P6 Benches + dashboard** | frozen corpus (consented, no private rooms), tier-3 CI with thresholds, time series | a deliberate regression in a replay fails a PR | A 6-9 d, U 1 h (consent, corpus pick) |
| **P7 HIL** | one rig (a second machine, D9), ephemeral runner, HIL script | nightly HIL row on the dashboard | A 3-5 d, U hardware |
| **P8 Upstream** | NVIDIA PR, kernel EDID quirk, Monado MRs (generic fixes first, then the driver), WayVR PRs | each accepted patch removed from our series | ongoing, weeks to months (U posts where an account is needed) |
| **P9 Reach** | other distros (COPR/deb/Envision-style build), AMD reports, other headsets via upstream Monado drivers, Pimax captures from community | tier ≥ 1 for a second GPU vendor or headset | ongoing |

## 7. Decisions for the user (with the recommended answer)

| # | Decision | Recommended |
|---|---|---|
| D1 | Fresh public repo vs flipping this one; where development continues | **Fresh repo, clean history**; afterwards develop in the public repo, this one becomes the private lab |
| D2 | Project and package name (trademark) | **A neutral name without "Pimax"** (vendor-neutral fits the multi-headset goal); "for Pimax headsets, unofficial" in the text |
| D3 | Public identity for commits | **GitHub `noreply` address** under a project org or your account; not the personal Gmail addresses |
| D4 | Licence | **MIT OR Apache-2.0** for our code (already declared), CC-BY-4.0 for docs/data; forks keep BSL-1.0 / GPL-3.0 |
| D5 | Publish the Monado and WayVR forks (changes the "never push" rule for those two) | **Yes**, as public forks with rebased patch series; agents still push only when you say so |
| D6 | What of `analysis/` goes public | **Protocol, display, tracking findings and decoders: yes. Eye-auth notes, Tobii transcripts, raw logs, room images, calibration: no** |
| D7 | Scope of v1 | **Dream Air on Arch/Omarchy + NVIDIA**, architecture generic; other headsets via upstream Monado drivers as tier 0 "community" |
| D8 | Diagnostics delivery | **No server, no background upload**: local Claude Code (route A) first, GitHub issue form (route B) second |
| D9 | AI bot on the public repo and who pays | **Maintainer-label-gated only**, pinned action, scoped token, monthly API budget cap from a project key (not your personal key) |
| D10 | Hardware-in-the-loop rig | **Not on your daily PC.** Until a second machine exists, "manual HIL" = you press "Test my setup" and attach the report |
| D11 | NVIDIA patches | **Post the upstream PR**; keep the patches as an opt-in package; investigate a no-patch compatibility mode |
| D12 | Hand models | **Download button + ask the Mercury author for a licence** |
| D13 | Camera recordings as goldens | **Only consented, neutral recordings** (a lab corner or blank wall), never your home frames |

## 8. Audit method (for re-running it)

- Tree: every `git ls-files` path, decoded as UTF-8 with replacement, pattern counts per file.
- History: `git log -p --all --text -U0`, added lines only, counts per (commit, file) and pattern.
- Binary fixtures: `zcat` then ASCII and UTF-16LE serial patterns.
- Real keys: compared by value (never printed) with the keys in `~/.config/pimax/ai.toml*`.
- Re-run before publishing on the **export**, plus gitleaks; the CI scan keeps it clean afterwards.
