#!/usr/bin/env bash
# pkg.sh — build a native macOS click-through installer (.pkg).
#
#   ./pkg.sh
#
# Produces workspace-switcher.pkg: double-click it and macOS shows the
# standard Installer wizard (Continue → Install → password → Done). The
# package carries the whole repo; the postinstall runs INSTALL.sh as the
# logged-in user (Homebrew refuses to run as root).
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"   # repo root (this script lives in scripts/)
WORK="$(mktemp -d "${TMPDIR:-/tmp}/ws-pkg.XXXXXX")"
PAYLOAD_ROOT="$WORK/root"
SCRIPTS="$WORK/scripts"
PKG="$ROOT/workspace-switcher.pkg"

say() { printf '\033[1;36m== %s ==\033[0m\n' "$*"; }
die() { printf '\033[31m%s\033[0m\n' "$*" >&2; rm -rf "$WORK"; exit 1; }

say "staging the repo as package payload"
mkdir -p "$PAYLOAD_ROOT/tmp/ws-pkg-payload"
( cd "$ROOT" && git archive --format=tar HEAD ) | tar -x -C "$PAYLOAD_ROOT/tmp/ws-pkg-payload" 2>/dev/null \
    || { say "git archive unavailable — copying the working tree"; \
         rsync -a --exclude workspace-switcher.app --exclude .build \
             --exclude Installer.app --exclude .git --exclude '*.pkg' --exclude '*.dmg' \
             "$ROOT/" "$PAYLOAD_ROOT/tmp/ws-pkg-payload/"; }

say "writing the postinstall script"
mkdir -p "$SCRIPTS"
cat > "$SCRIPTS/postinstall" <<'EOF'
#!/bin/sh
# pkg postinstall — runs as root: hand the repo to the logged-in user and
# run INSTALL.sh AS THAT USER (Homebrew refuses root).
set -e

CONSOLE_USER="$(stat -f '%Su' /dev/console 2>/dev/null || echo danielbaker)"
USER_HOME="$(dscl . -read "/Users/$CONSOLE_USER" NFSHomeDirectory 2>/dev/null | awk '{print $2}')"
USER_HOME="${USER_HOME:-/Users/$CONSOLE_USER}"

SRC="/tmp/ws-pkg-payload/workspace-switcher"
DEST="$USER_HOME/workspace-switcher"

rm -rf "$DEST"
cp -R "$SRC" "$DEST"
chown -R "$CONSOLE_USER:staff" "$DEST"

# run the real installer as the user (its output shows in the Installer log)
sudo -u "$CONSOLE_USER" -H /bin/bash "$DEST/INSTALL.sh" || exit 1
exit 0
EOF
chmod +x "$SCRIPTS/postinstall"

say "building the package"
pkgbuild --root "$PAYLOAD_ROOT" \
    --scripts "$SCRIPTS" \
    --identifier dev.danielbaker.workspace-switcher-installer \
    --version 1.0 \
    "$PKG" >/dev/null || die "pkgbuild failed"

rm -rf "$WORK"
say "package ready: $PKG"
echo "  open \"$PKG\"   → macOS Installer wizard (click through)"