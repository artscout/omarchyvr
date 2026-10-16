#!/usr/bin/env bash
# Build an AppImage of the Pimax app: pimax-setup, the pimax launcher and the docs the app reads.
# Run it by hand. It installs nothing and needs no root.
#
#   packaging/appimage/build-appimage.sh
#
# Environment, all optional:
#   CARGO_TARGET_DIR  cargo output directory (default: <repo>/rust/target)
#   OUT_DIR           work and output directory (default: packaging/appimage/build)
#   VERSION           version in the file name (default: short git hash)
#   SKIP_BUILD=1      do not run cargo, use the binaries already in the target directory
#   APPIMAGETOOL      appimagetool to use (default: the one on PATH)
#
# Scope: the app only. The helper tools (pimax-cam, pimax-passthrough, ...), the Monado and WayVR builds, Basalt and
# the voice models are not inside. The Arch package (packaging/arch) carries the tools.
#
# Why AppRun sets PIMAX_REPO: the app detects a package install only for /usr/bin/pimax-setup (is_system_install in
# rust/pimax-gui-core/src/paths.rs). Inside an AppImage the executable is under /tmp/.mount_*, so Paths::from_env
# falls back to PIMAX_REPO, else ~/pimax-re. Paths expects <repo>/docs/LESSONS.md, so the docs go to
# $APPDIR/usr/share/pimax/docs and AppRun sets PIMAX_REPO=$APPDIR/usr/share/pimax (a PIMAX_REPO that the caller
# already exports wins).
#
# With PIMAX_REPO set, Paths looks for every release binary in <repo>/rust/target/release. The AppDir therefore has
# usr/share/pimax/rust/target/release as a symlink to usr/bin, so the app finds the bundled pimax-setup and pimax.
# The other tools are not bundled: the app reports them as not built, and the launcher still looks for them in
# ~/pimax-re/rust/target/release.
#
# Limits of the AppImage:
#   * The binaries link the glibc and libgcc of the build machine; build on the oldest system you want to support.
#     GPU, Wayland, X11 and OpenXR libraries are loaded from the host on purpose (dlopen), they are not bundled.
#   * The AppImage mount is a FUSE mount that root cannot read: root fixes that install a file from <repo>/linux (udev
#     rule, pacman hooks) fail from here. Use the Arch package or a checkout for those.
#   * The Setup page's build fixes (cargo build in <repo>/rust) cannot work inside the read-only mount.
set -euo pipefail

die() { echo "build-appimage: $*" >&2; exit 1; }

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo=$(cd "$here/../.." && pwd)
out=$(realpath -m "${OUT_DIR:-$here/build}")
arch=${ARCH:-$(uname -m)}
version=${VERSION:-$(git -C "$repo" rev-parse --short=9 HEAD 2>/dev/null || echo 0.1.0)}
appdir=$out/Pimax.AppDir
appimage=$out/Pimax-$version-$arch.AppImage

cd "$repo/rust"
# a relative CARGO_TARGET_DIR is relative to rust/, where cargo runs
target=$(realpath -m "${CARGO_TARGET_DIR:-$repo/rust/target}")

if [ "${SKIP_BUILD:-0}" != 1 ]; then
  command -v cargo >/dev/null || die "cargo not found"
  # "pimax" is only the library crate; the launcher binary called pimax is built by the package pimax-tools.
  # pimax-setup's build script reads PIMAX_GIT_HASH and SOURCE_DATE_EPOCH if you set them (else it asks git).
  CARGO_TARGET_DIR=$target cargo build --release --locked -p pimax-setup -p pimax-tools
fi
for b in pimax-setup pimax; do
  [ -x "$target/release/$b" ] || die "$target/release/$b is missing (build failed, or SKIP_BUILD=1 without a build)"
done

rm -rf -- "$appdir"
share=$appdir/usr/share/pimax

# binaries
install -Dm755 "$target/release/pimax-setup" "$appdir/usr/bin/pimax-setup"
install -Dm755 "$target/release/pimax" "$appdir/usr/bin/pimax"

# what the app reads from <repo>: docs, the udev rule it compares, the script its Steam runtime tool runs
install -Dm644 -t "$share/docs" "$repo"/docs/*.md
if compgen -G "$repo/docs/gui/*.png" >/dev/null; then
  install -Dm644 -t "$share/docs/gui" "$repo"/docs/gui/*.png
fi
install -Dm644 "$repo/linux/70-pimax.rules" "$share/linux/70-pimax.rules"
install -Dm755 "$repo/analysis/tools/vr_runtime_switch.sh" "$share/analysis/tools/vr_runtime_switch.sh"

# <repo>/rust/target/release -> usr/bin (usr/share/pimax/rust/target/../../../../bin is usr/bin)
mkdir -p "$share/rust/target"
ln -s ../../../../bin "$share/rust/target/release"

# desktop entry (top level for appimagetool, and under usr/share/applications): the template without its comment
# header, Exec by name because AppRun starts the app
sed -e '/^#/d' -e '/./,$!d' -e 's|@PIMAX_SETUP@|pimax-setup|g' "$repo/linux/pimax.desktop" >"$appdir/pimax.desktop"
grep -qx 'Exec=pimax-setup' "$appdir/pimax.desktop" || die "linux/pimax.desktop has no @PIMAX_SETUP@ placeholder"
install -Dm644 "$appdir/pimax.desktop" "$appdir/usr/share/applications/pimax.desktop"

# icon: top level (Icon=pimax), .DirIcon, and the hicolor path
install -Dm644 "$repo/linux/pimax.svg" "$appdir/pimax.svg"
install -Dm644 "$repo/linux/pimax.svg" "$appdir/usr/share/icons/hicolor/scalable/apps/pimax.svg"
ln -s pimax.svg "$appdir/.DirIcon"

cat >"$appdir/AppRun" <<'EOF'
#!/bin/sh
# AppRun of the Pimax AppImage (written by packaging/appimage/build-appimage.sh)
APPDIR=${APPDIR:-$(dirname "$(readlink -f "$0")")}
export APPDIR
export PIMAX_REPO="${PIMAX_REPO:-$APPDIR/usr/share/pimax}"
exec "$APPDIR/usr/bin/pimax-setup" "$@"
EOF
chmod 755 "$appdir/AppRun"

echo "AppDir: $appdir"
tool=${APPIMAGETOOL:-$(command -v appimagetool || true)}
if [ -n "$tool" ]; then
  ARCH=$arch "$tool" "$appdir" "$appimage"
  echo "AppImage: $appimage"
else
  cat <<EOT
appimagetool is not on PATH, so no .AppImage file was written. To finish:
  1. get appimagetool (https://github.com/AppImage/appimagetool/releases, or the AUR package appimagetool-bin)
  2. ARCH=$arch appimagetool "$appdir" "$appimage"
     (no FUSE: appimagetool-$arch.AppImage --appimage-extract-and-run "$appdir" "$appimage")
Try the app straight from the AppDir with: $appdir/AppRun
EOT
fi
