# Prebuilt release

Builds the tarball users install without compiling. How and why: [docs/RELEASE.md](../../docs/RELEASE.md).

| File | What it does |
|---|---|
| `Dockerfile` | the clean Arch builder (package lists come from `installer/lib.sh`; user `build`, HOME `/home/build`) |
| `build-release.sh` | runs in the container: clean clone, `installer/install.sh --stage-only`, keep runtime files, strip, bundle, manifest, tarball, verify |
| `bundle-libs.py` | decides core/tree/bundled per library, sets `$ORIGIN` RUNPATHs with patchelf, `check` = ldd of every ELF |
| `manifest.sh` | writes `MANIFEST.txt` (read by `verify_release` in `installer/lib.sh`) |
| `verify-release.sh` | all checks on the finished tarball; a failure deletes it |
| `scan-tree.py` | `tools/leak-scan.py` over an unpacked tree, plus the builder-path and model-file checks |
| `README-INSTALL.txt` | the plain-text note that sits in the tarball |

Tests (no docker, no root): `installer/test_release.sh`.
