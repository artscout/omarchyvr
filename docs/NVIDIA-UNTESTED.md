# The NVIDIA patch on untested drivers

The Dream Air needs three small fixes in the NVIDIA open kernel module (`patches/nvidia/`). They were written and tested
on one driver version. This page says what happens on any other version, and how to opt in.

## Default (switch off)

- The tested versions are listed in `patches/nvidia/tested-versions.txt` (today `610.57.04`). The installer and the
  pacman hook read that one file.
- On another version nothing is patched. `nvidia-patch-reapply.sh` (run by the pacman hook before dkms builds) prints a
  banner, writes `untested-refused` to the status file and exits 0: pacman is not blocked, dkms builds the normal NVIDIA
  module, the desktop works. The headset picture does not (the left half shows in both eyes).
- The app says so: the setup screen (shown when the driver is not tested; always in Developer mode) > **NVIDIA driver patch** (and Ready > Setup), with the installed driver,
  "tested" or "NOT a tested version", and what the patch did, in plain words.

## Opt in

The switch **Allow the patch on untested NVIDIA drivers** (off by default) is on the same card. It is saved with the
other app switches (`gui-state.toml`, key `nvidia_allow_untested`) and written into the setup plan
(`OVR_NVIDIA_ALLOW_UNTESTED`). The card then offers **Apply (one password)**: the system setup job runs again (it is
idempotent, a restore snapshot is taken first) and writes `/etc/omarchyvr/nvidia.conf` (`allow_untested=1`), which the
pacman hook reads on every later driver update. For the CLI: `./install.sh --nvidia-allow-untested`, or
`nvidia-patch-reapply.sh --allow-untested`.

With the opt-in the script:

1. prints a loud WARNING (untested driver code: the picture may break or the patched build may fail);
2. tries the patches with `--fuzz=0`; if that fails it tries `--fuzz=2` (some context lines differ) and says so;
3. still requires `merged_head_sync` in the patched source;
4. with `--fallback` (the hook and the setup job use it) a failed patched build restores the stock files and builds
   the stock module, so the machine always ends with a working module.

The warning shown in the app: the patch is untested driver code; the headset picture may break, or the patched build may
fail; the desktop stays working either way because the normal module is built as the fallback; a restore snapshot is
taken first.

## Ported versions

A version listed in `patches/nvidia/ported-versions.txt` (today `615.78.08`) has its own patches in `patches/nvidia/<version>/`.
They apply with `--fuzz=0` and the patched modules were compile-checked, but were never run on the headset, so the version is
still untested: it needs the opt-in. With the opt-in the script uses the port and does not retry with `--fuzz=2` (a failed
`--fuzz=0` there means the source changed again: the stock module is built and the banner says so). The status file gets
`port=ported`. What changed in the port and the hardware test plan: [NVIDIA-615.md](NVIDIA-615.md).

## The status file

`/var/lib/omarchyvr/nvidia-patch-status` (world-readable, written by every run that patches or refuses; `--check` writes
nothing):

```
driver=610.58.01
result=untested-allowed     # tested | untested-allowed | untested-refused | failed
fuzz=2                      # 0, 2, or - (nothing was applied this run; the last value is kept)
time=2026-10-17T08:00:00Z
```

`/etc/omarchyvr/nvidia.conf` and the status file are removed by the uninstall.

## Uninstall: restoring the stock source

The uninstall restores the `*.pimax-orig` backups, then also reverses any patch that is still in a file without a backup
(an older hand install): `patch -R --fuzz=0`, file by file, dry run first, newest patch first. If a pristine copy
`/usr/src/nvidia-<ver>.orig` exists (and has no patch code itself) it is the reference for files that still differ.
Last, no patch marker may remain (`merged_head_sync`, the per-head viewport call in `nvSetViewPortsEvo`,
`nvEvoReadChannelGetPut`, the bounded make-room code); if one does, the job prints a banner naming it and
`NVIDIA_STOCK=leftover:<tree>`, and keeps the `.orig` copy.

Tests: `patches/nvidia/test_nvidia_patch_reapply.sh` (script), `installer/test_root_nvidia.sh` (uninstall restore and plan),
Rust unit tests in `rust/omarchyvr-gui-core/src/nvidia_patch.rs`.
