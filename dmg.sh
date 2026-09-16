#!/usr/bin/env bash
# dmg.sh — build a double-click installer DMG.
#
#   ./dmg.sh
#
# Produces workspace-switcher.dmg in the repo root. The user mounts the DMG
# and double-clicks Installer.app (a GUI wizard) — or runs INSTALL.sh from
# inside it. Everything the installer needs ships IN the DMG.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
STAGE="$(mktemp -d "${TMPDIR:-/tmp}/ws-dmg.XXXXXX")"
DMG="$ROOT/workspace-switcher.dmg"
VOLNAME="Workspace Switcher Installer"

say() { printf '\033[1;36m== %s ==\033[0m\n' "$*"; }
die() { printf '\033[31m%s\033[0m\n' "$*" >&2; rm -rf "$STAGE"; exit 1; }

say "1/2 building Installer.app (GUI wizard)"
"$ROOT/installer/build.sh" >/dev/null || die "Installer.app build failed"

say "2/2 assembling the DMG"
mkdir -p "$STAGE"
# the full repo rides along so Installer.app + INSTALL.sh find everything
cp -R "$ROOT/Installer.app" "$STAGE/"
cp "$ROOT/INSTALL.sh" "$ROOT/UNINSTALL.sh" "$ROOT/README.md" "$STAGE/"
# the repo itself (source + configs + jira tools) — excludes build artifacts
( cd "$ROOT" && git archive --format=tar HEAD ) | tar -x -C "$STAGE" 2>/dev/null \
    || { say "git archive unavailable — copying the working tree instead"; \
         rsync -a --exclude workspace-switcher.app --exclude .build \
             --exclude Installer.app --exclude .git --exclude '*.dmg' \
             "$ROOT/" "$STAGE/"; }

# a visible "double-click me" hint on the DMG
cat > "$STAGE/README.txt" <<'EOF'
=================================================================
  WORKSWITCHER — INSTALL
=================================================================
  Double-click Installer.app → click through the wizard.
  (or open Terminal here and run:  ./INSTALL.sh )

  After installing: Hyper+S opens the switcher, and the menu-bar
  wrench icon opens the notes / jira / voice / health windows.
=================================================================
EOF

rm -f "$DMG"
hdiutil create -volname "$VOLNAME" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null \
    || die "hdiutil create failed"
rm -rf "$STAGE"
say "DMG ready: $DMG"
echo "  open \"$DMG\"   (or: hdiutil attach $DMG)"