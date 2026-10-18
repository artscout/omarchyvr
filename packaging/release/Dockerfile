# SPDX-License-Identifier: MIT OR Apache-2.0
#
# The clean Arch Linux builder of the prebuilt release (docs/RELEASE.md). The build context is the REPOSITORY ROOT:
#
#   docker build -f packaging/release/Dockerfile -t omarchyvr-release-builder .
#   docker run --rm -v "$PWD":/src:ro -v "$PWD/dist":/out omarchyvr-release-builder \
#       /src/packaging/release/build-release.sh --version 0.1.0 --ref v0.1.0
#
# The package lists are NOT repeated here: they are read from installer/lib.sh (OVR_BUILD_PKGS, the same list the
# from-source install checks, and OVR_RUNTIME_PKGS), so the container has exactly what install.sh needs. Only the
# packaging tools are added below.
#
# Pin the base image by date for a reproducible release: --build-arg ARCH_TAG=base-devel-YYYYMMDD.0.NNNNNN
# (tags: https://hub.docker.com/_/archlinux/tags). The release MANIFEST.txt records the builder's package list.
ARG ARCH_TAG=base-devel
FROM docker.io/library/archlinux:${ARCH_TAG}

COPY installer /opt/ovr-installer

# pacman 7's download sandbox needs kernel features a container lacks
RUN if pacman -V | grep -qE 'v7\.'; then sed -i '/^\[options\]/a DisableSandbox' /etc/pacman.conf; fi \
 && pacman -Syu --noconfirm \
 && bash -c '. /opt/ovr-installer/lib.sh \
      && pacman -S --needed --noconfirm "${OVR_BUILD_PKGS[@]}" "${OVR_RUNTIME_PKGS[@]}"' \
 && pacman -S --needed --noconfirm \
      patchelf zstd procps-ng util-linux binutils curl ca-certificates python \
      libinput dav1d libglvnd libxext libxi libxcursor libxinerama libxss libxxf86vm libxrender libxfixes libxkbcommon-x11 \
 && pacman -Scc --noconfirm \
 && rm -rf /var/cache/pacman/pkg/* /opt/ovr-installer

# an unprivileged user with a generic home: build paths must never carry a real person's name
RUN useradd --create-home --home-dir /home/build --shell /bin/bash build \
 && install -d -o build -g build /opt/ovr-build /out
USER build
ENV HOME=/home/build LANG=C.UTF-8 CARGO_HOME=/home/build/.cargo
WORKDIR /opt/ovr-build
