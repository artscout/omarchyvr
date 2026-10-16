# Open-source AI devops pipelines: what to copy for pimax-re

Research note, 2026-10-15. Goal: easy install on many machines, users test on whatever headset/GPU/driver they have, an AI agent diagnoses failures and opens PRs, and automatic tests show whether a change is an improvement.

Confidence: many primary pages (Omarchy repo files, Monado GitLab, ALVR wiki, GitHub docs) could not be fetched directly (404 or bot-protection). Findings below come from search snippets and mirrors and are marked **[unverified]** where so. Re-check the primary pages before building on them.

## 1. Omarchy (basecamp/omarchy)

- **What it does.** Arch-based distro by DHH, MIT-licensed. Updates run from the "Update > Omarchy" menu: pull latest code and configs, run pending migrations, then update system packages. Sources: https://learn.omacom.io/2/the-omarchy-manual/68/updates and https://docs.docuwriter.ai/omarchy-user-docs/78797 (third-party).
- **Packages.** The manual names three sources: the Omarchy Arch mirror, the Omarchy package repository, and the AUR (if installed). The stable mirror lags the official Arch repo by up to a month. The AUR is not vetted by Arch. A third-party config reference shows an `[omarchy]` pacman section at `https://pkgs.omarchy.org/$arch` [unverified]. One third-party page claims the mirror tracks Arch core/extra/multilib; this conflicts with the manual, so treat it as unverified.
- **Migrations.** Versioned scripts run once and are recorded in state files (`omarchy-migrate`). This is the key pattern for us: a user's machine gets forward-only, idempotent fixes. Source: https://docs.docuwriter.ai/omarchy-user-docs/78781 [unverified details].
- **Optional software.** The manual has an "Other Packages" page: https://learn.omacom.io/2/the-omarchy-manual/66/other-packages.
- **Repo layout.** Root contains `install/`, `bin/`, `migrations/`, `default/`, `manual/`, `test/`, `.github/`, `AGENTS.md`, `CLAUDE.md` and `agents/skills/`. The agent files are the closest existing precedent for an AI-operated repo. Source: https://github.com/basecamp/omarchy (landing page only; the CONTRIBUTING and CI details were not retrieved).
- **Copy:** the migrations-as-state-file model; the "update = pull + migrate + packages" single command; an in-repo `AGENTS.md`/`CLAUDE.md` plus `agents/skills/`.
- **Pitfalls:** a rolling Arch base means packaged drivers vary by date; AUR packages are unvetted; the "one command" update must not break a running VR session (see section 2 on session safety).
- **Optional VR install.** Offer it as an optional installer script or `omarchy` menu entry, not a default package. Keep it as a separate package (`pimax-*`) that the user opts into, with the same migration mechanism for upgrades.

## 2. Linux VR projects

**Monado (freedesktop GitLab).** Upstream CI runs on GitLab with build and unit tests; hardware-in-the-loop is mostly done by the maintainers and vendor-contributed drivers, not in public CI. Drivers are added as `src/xrt/drivers/<name>` with a CMake option (`XRT_BUILD_DRIVER_*`), which matches our `XRT_BUILD_DRIVER_PIMAX=ON` pattern. My fetch of the GitLab page was blocked by bot protection (https://gitlab.freedesktop.org/monado/monado), so CI details are **[unverified]**.
- Copy: the per-driver CMake option and a driver-module layout; one build with all drivers compiled in (already our rule).
- Pitfall: our "one build" rule means every CI run must build the Pimax driver; keep the build time bounded with ccache.

**Envision (vronlinux).** A graphical orchestrator that sets up a full Monado stack in a few clicks, shipped as an AppImage built from GitLab pipelines or an AUR package. Source: https://vronlinux.org/docs/fossvr/envision/ (search snippet). It no longer builds WiVRn profiles; the WiVRn project has its own installer [unverified].
- Copy: the "profiles" idea (per-headset/per-driver preset) and AppImage-from-CI distribution, which suits "install on many machines".
- Pitfall: NixOS is explicitly discouraged; a single AppImage does not cover every distro.

**WiVRn.** Monado-based streamer for Quest. Source: its own docs. Not fetched.

**ALVR.** Repo has `CONTRIBUTING.md`, `.github/` workflows, a Linux troubleshooting wiki, and a launcher distributed as `alvr_launcher_linux.tar.gz` via `releases/latest/download`. Privacy policy states "ALVR apps do not directly collect any personal data." No telemetry or bug-report bundle was visible in the fetched page. Source: https://github.com/alvr-org/ALVR.
- Copy: the "latest download" launcher URL and an explicit privacy statement.
- Pitfall: relying on wiki troubleshooting pages means users must read them; our app should show the plain-word fix.

**OpenHMD, libsurvive, xrizer, OpenComposite.** Not researched in depth in this pass. Quick note: xrizer and OpenComposite are OpenVR-to-OpenXR shims that Envision selects for SteamVR games; relevant to our "desk vs play" plan. **[unverified]**

**LVRA wiki (vronlinux).** Community compatibility database. Not fetched.

**Valve SteamVR Linux bug tracker.** Not fetched; treat as unknown.

## 3. Hardware compatibility databases and auto-reporting

**linux-hardware.org / hw-probe.** Collects hardware data and logs, cleans logs on the client, and uploads over HTTPS. Stated exclusions: username, hostname, IP, MAC, UUIDs, serial numbers. Stable device identity is a 32-byte prefix of a salted SHA512 of MAC/serial. The database is published as a dump for third-party analysis. Sources: https://en.opensuse.org/Hardware_probe, https://fedoraproject.org/wiki/Hardware_probe, https://wiki.gentoo.org/wiki/Hw-probe.
- Copy: client-side redaction before upload, a documented list of what is excluded, hashed device IDs, and a public, ODbL-style export.
- Pitfall: the `sudo -E` environment passthrough leaks secrets (env vars). Our tool must never pass the full environment.
- Pitfall: a hashed serial is still a stable identifier. Rotate it or make it opt-in.

**ProtonDB.** Public export with personal identifiers removed; the exported summary contains CPU, GPU and GPU driver but not the Steam account. Export is ODbL + DbCL. Source: https://protondb.com/news/report-exports-odbl (2018 announcement, dated). Submission form and current privacy policy not verified.
- Copy: a reduced, structured system summary (cpu/gpu/driver) as the public record, and an open-licensed export.
- Pitfall: reports need an admin to correct; build edit/delete into our report schema from the start.

**Mesa LAVA CI.** Board-level hardware-in-the-loop: a gitlab-runner on a lab host submits jobs tagged for specific hardware to LAVA, which deploys kernel + device tree + ramdisk with the test suite. Pre-merge testing uses these lab devices. Source: https://docs.mesa3d.org/ci/LAVA.html (search snippets).
- Copy: tag-based job routing (`our Pimax headset rig` runner tag), a stock-QEMU sanity check before real hardware, and a lab runner that only builds and tests.
- Pitfall: the LAVA token was visible to anyone who could submit MRs. Our self-hosted runner must never run untrusted PR code with a headset or secret attached.

**fwupd / LVFS.** Vendor firmware with `.cab` metadata; the maintainer asks vendors for test hardware late in onboarding and some devices need manual update mode. Source: https://blogs.gnome.org/hughsie/?p=1474 (Hardware CI Tests in fwupd, search snippet) and the fwupd test-device config (`fwupd.conf` test parameters).
- Copy: a test-device mode with parameters loaded from a config file, so CI can run without the device.
- Pitfall: flashing is out of scope for us (see project rules: no firmware flashing).

**KernelCI.** Not researched; no source retrieved.

## 4. AI-driven development pipelines

**anthropics/claude-code-action.** Runs Claude Code in a GitHub workflow; can edit code, comment and open PRs. Security: a June 2026 flaw let an untrusted issue be treated as trusted, which could expose OIDC tokens through prompt injection; fixed in v1.0.94 (per press reports, to verify against the release notes). Default permissions are broad (read/write code, issues, PRs, workflow files) and there is no network restriction by default. Sources: https://thehackernews.com/2026/06/claude-code-github-action-flaw-let-one.html, https://www.stepsecurity.io/blog/anthropics-claude-code-action-security-how-to-secure-claude-code-in-github-actions-with-harden-runner.
- Copy: pin the action to a version at least v1.0.94; grant only `contents: write` on a bot branch and `pull-requests: write`; never give it secrets or the self-hosted headset runner; gate triggers on maintainer labels, not on any issue author.
- Pitfall: the diagnosis input (a user's bug-report bundle) is untrusted data. Treat the bundle as an attack surface: no instructions from it, no secrets in it.

**GitHub Copilot coding agent.** Agent opens a PR and requests human review; Copilot's own review does not count as an approval and does not block merge. `AGENTS.md` in the repo root is read for context, and it can list sensitive paths that need human review. Sources: https://docs.github.com/en/copilot/how-tos/use-copilot-agents/use-code-review (the docs disagree on whether the head or base branch's instructions apply; check the current page).
- Copy: `AGENTS.md` listing "do not touch without human review" paths (driver protocol, firmware, install scripts); required human approval on every AI PR.

**OpenHands.** Issue-to-PR via `@openhands-agent` comment; sandboxed Docker shell; v1.6.0 (March 2026) adds a planning mode where the agent proposes a plan and waits for approval. Sources: https://openhands.dev/blog/open-source-coding-agents-in-your-github-fixing-your-issues, and aggregator pages. Primary safety docs not retrieved.
- Copy: a plan-approval step before edits for risky categories (protocol bytes, install scripts).

**SWE-agent.** Fully asynchronous issue resolution, CLI-based. Not checked against primary docs. **[unverified]**

**Aider.** Git-first, each change is an auto-committed commit: good traceability. **[unverified]**

**Labels and guardrails (general).** The pattern across these tools: the agent opens a PR, never merges; a human must approve; the agent works in a sandbox with a scoped token. Automatic triage bots generally label and dedupe rather than fix.

## 5. Performance regression tracking

**Bencher (bencher.dev).** CI tool with statistical thresholds (t-test, upper boundary 0.99, sample size 64 in the docs example). Baseline: a push-to-main run stores history; PRs run with `--err` (fail on alert) and `--branch-start-point` to compare against the base. Needs a repo secret, so fork PRs get no token. Source: https://bencher.dev/docs/how-to/github-actions/ (search snippet; flag names not checked against the current English page).
- Copy: a statistical threshold rather than a fixed percentage (noise on a VR rig is high); compare against main; post the result as a PR comment.
- Pitfall: a headset benchmark needs the real hardware, so the bench runs on the self-hosted rig, not a hosted runner. Fork PRs cannot reach the rig.

**github-action-benchmark.** Separate action that stores history on a gh-pages branch and can fail PRs over a threshold. **[unverified]**

**Mesa / Chromium perf dashboards.** Not fetched. Their general pattern (historical dashboard + per-change comparison) is the same as Bencher.

**For VR specifically.** The meaningful metrics are: missed frames, motion-to-photon latency (from our `tests` and the Windows-capture benchmark in `analysis/tracking-vs-windows.md`), tracking loss rate and jitter. Store them as a fixed-scenario recording (replay of the captured IMU/pose log) so the comparison is reproducible without a headset in the loop. The existing `bench-windows` and `bench-basalt` benches are the right starting point.

## 6. Privacy for diagnostics

- **Camera images (home interiors).** The room camera (Dream Air cameras, hand/room tracking) can show private spaces. Never upload raw frames. Upload only derived data: pose logs, feature counts, timing, and a diagnostic thumbnail only after the user previews and approves it in the app.
- **USB captures.** A raw capture may contain the headset serial and the user's device IDs. Strip serials, use the hashed-ID approach from hw-probe, and show the redacted file to the user before sending.
- **Environment.** Never send the full environment (the `sudo -E` lesson from hw-probe). Allowlist fields.
- **Consent and storage.** Opt-in per report; show exactly what will be sent in the app; a delete path; a stated retention period; no account identifier.
- **Public data.** If reports go into a public database, publish only the reduced summary (ProtonDB model) under an open licence.
- **Data as untrusted input.** A bundle sent to an AI agent is hostile input (claude-code-action flaw). Strip free text, cap sizes, and never let it carry instructions.

## 7. Pitfalls summary

1. An AI agent with write tokens and untrusted issue text is the main risk (June 2026 claude-code-action flaw).
2. A self-hosted headset runner must never run code from a fork or untrusted PR.
3. Telemetry and bug bundles that include the environment or serials leak private data.
4. Benchmarks on a shared rig are noisy; use fixed replays and statistical thresholds.
5. A single "one-click" installer breaks across distros (Envision/NixOS, rolling Arch packages).

## What to copy first (ranked)

1. **Human-gated AI PRs with a scoped token.** The agent opens a PR on a bot branch; a maintainer label and a human approval are required; `AGENTS.md` lists protected paths (protocol, install scripts). Use a pinned claude-code-action >= v1.0.94 (verify).
2. **Replay-based regression benches with statistical thresholds.** Use the existing Windows-capture replay plus Bencher-style thresholds (`--err` on PRs, base-branch comparison). Run on the self-hosted rig only for real-hardware checks.
3. **Opt-in redacted diagnostics bundle (hw-probe model).** Allowlisted fields, hashed device IDs, no environment, preview in the app before send, and a public reduced export.
4. **Migrations-as-state for installs (Omarchy model).** Idempotent, forward-only upgrade scripts recorded in a state file, plus an optional `omarchyvr` package/installer that the user opts into.
5. **Per-driver CMake option and profiles (Monado/Envision model).** Keep `XRT_BUILD_DRIVER_PIMAX` in one build; expose presets per headset in the app.

Open items to verify before building: Omarchy CONTRIBUTING and CI (`test/`, `.github/`); Monado GitLab CI config; the exact claude-code-action release notes; Bencher current English flags; KernelCI and Valve SteamVR bug tracker practices.

Sources: the URLs above (search snippets and mirrors, not all primary pages).
