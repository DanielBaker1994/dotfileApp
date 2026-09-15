#!/usr/bin/env bash
# uninstall.sh — remove everything the installer/setup.sh creates:
# the daemon, installed configs, launchd agent, TCC grants, and caches.
# Reverses every installer action; safe to re-run.
set -uo pipefail

UID_="$(id -u)"
BACKUP="$HOME/.config/workspace-switcher-uninstall-$(date +%s)"

say() { printf '\033[1;36m== %s ==\033[0m\n' "$*"; }

say "killing the workspace-switcher daemon"
pkill -f "workspace-switcher.app" 2>/dev/null || true

say "removing the jira poll launchd agent"
PLIST="$HOME/Library/LaunchAgents/com.jira.poll.plist"
launchctl bootout "gui/$UID_" "$PLIST" 2>/dev/null || true
rm -f "$PLIST"

say "removing installed configs (backed up to $BACKUP)"
mkdir -p "$BACKUP"
for d in \
    "$HOME/.config/aerospace" \
    "$HOME/.config/sketchybar" \
    "$HOME/.config/borders" \
    "$HOME/.config/workspace-switcher"; do
    if [ -e "$d" ]; then
        if [ -L "$d" ]; then rm "$d"; else mv "$d" "$BACKUP/"; fi
    fi
done

say "removing TCC grants (mic + speech)"
TCC_DB="$HOME/Library/Application Support/com.apple.TCC/TCC.db"
sqlite3 "$TCC_DB" "DELETE FROM access WHERE client LIKE '%workspace-switcher%';" 2>/dev/null
killall tccd 2>/dev/null

say "removing caches + runtime files"
rm -rf "$HOME/.cache/workspace-switcher" "$HOME/.cache/jira"
rm -f "$HOME/.cache/ws-crash.log" "$HOME/.cache/ws-aero.log" \
    "$HOME/.cache/aero-debug" "$HOME/.cache/ws-auth-debug"
rm -f /tmp/ws-notes.sock /tmp/ws-aerospace-focus /tmp/workspace-switcher-focus \
    "${TMPDIR:-/tmp}/ws-launch.log"

echo
echo "Done. Configs backed up in: $BACKUP"