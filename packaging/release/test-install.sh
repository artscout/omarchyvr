#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# test-install.sh: install a release tarball in a disposable ROOT container and prove that the installed Monado still
# loads its libraries after the app's root job gave it cap_sys_nice (release 0.1.1 did not: $ORIGIN RUNPATHs are ignored
# for a program with a file capability). Maintainer side: docs/RELEASE.md. Run it before publishing a release.
#
#   packaging/release/test-install.sh TARBALL [--image omarchyvr-release-builder]
#
# Needs docker (run it from a shell that may use docker; the caller wraps it, e.g. `newgrp docker`) and the image of
# packaging/release/Dockerfile. The container has no network and is removed afterwards; nothing of this PC is touched
# except reading TARBALL. The steps are in test-install-container.sh. Exit 0 = every check passed.
# Never runs while a VR session runs (docker jobs share the CPU and the GPU box with the compositor).
set -uo pipefail
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
IMAGE=omarchyvr-release-builder TARBALL=""
while [[ $# -gt 0 ]]; do
	case $1 in
	--image) IMAGE=${2:?}; shift ;;
	-h | --help) sed -n '3,14p' "$0"; exit 0 ;;
	-*) echo "unknown option $1" >&2; exit 2 ;;
	*) TARBALL=$1 ;;
	esac
	shift
done
[[ -f $TARBALL ]] || { echo "usage: test-install.sh TARBALL [--image NAME]" >&2; exit 2; }
TARBALL=$(realpath "$TARBALL")
for p in monado-service vrserver; do
	if pgrep -x "$p" >/dev/null 2>&1; then echo "a VR session runs ($p): not now (stop VR first)" >&2; exit 3; fi
done
command -v docker >/dev/null || { echo "docker not found" >&2; exit 2; }
if [[ -f $TARBALL.sha256 ]]; then
	(cd "$(dirname "$TARBALL")" && sha256sum -c --quiet "$(basename "$TARBALL").sha256") || { echo "the .sha256 file does not match $TARBALL" >&2; exit 2; }
fi
echo "==> test-install: $TARBALL in image $IMAGE"
# --cap-add SYS_NICE: a file capability that the container's bounding set lacks makes exec fail with EPERM, which is not
# the behaviour of a real PC; the capability is what the app's root job grants
exec nice -n 19 docker run --rm --user root --network none --cap-add SYS_NICE \
	-v "$TARBALL":/in/release.tar.zst:ro -v "$HERE/test-install-container.sh":/test-install-container.sh:ro \
	"$IMAGE" bash /test-install-container.sh /in/release.tar.zst
