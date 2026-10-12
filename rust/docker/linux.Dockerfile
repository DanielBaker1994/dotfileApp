# Linux cross-compile check for kitchen-sink (bin/linux-check.sh).
#
# rustc matches the local toolchain (1.92) so macOS and Linux agree on lints.
# The checkout is bind-mounted at /repo at run time; nothing here bakes it in.
FROM rust:1.92-bookworm

# python3 backs the plain-script test suites (bin/linux-check.sh test). The
# -dev packages are the system libs later waves need (egui/arboard/rfd) plus
# `xcap`'s Linux backends (Wave 6 capture: libxcb for X11, libdbus for the
# Wayland portal); they are harmless to install now.
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      python3 python3-pip \
      libxkbcommon-dev libgtk-3-dev libxcb1-dev libwayland-dev \
      libdbus-1-dev pkg-config \
 && rm -rf /var/lib/apt/lists/*
