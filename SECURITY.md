# Security policy

## Supported versions

Only the `main` branch is supported. The project is alpha software.

## How to report a problem

Use GitHub private vulnerability reporting on this repository: open the "Security" tab, then "Report a
vulnerability". Do not open a public issue for a vulnerability.

Please include what you found, how to reproduce it and which commit you used. Do not include device serials, keys
or other private data.

## What is in scope

- The root helper and the setup steps that run through pkexec.
- The udev rule.
- HID writes to the headset (the allowlist and how it is enforced).
- The voice agent and its tools.
- Data stored on disk (settings, logs, maps, recordings) and its permissions.

## Known open issues

Problems we already know about are listed in [docs/SECURITY-STATUS.md](docs/SECURITY-STATUS.md).

## What to expect

This is a best-effort project with a single maintainer. There is no bounty and no guaranteed response time. Reports
are read, and fixes are made in the order of risk.
