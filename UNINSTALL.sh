#!/usr/bin/env bash
# UNINSTALL.sh — remove the whole workspace-switcher stack.
#
#   ./UNINSTALL.sh
#
# Stops the services, kills the daemon, removes configs/launchd/TCC/caches.
# Your personal files (notes, ~/.config/jira) are left alone.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
UID_="$(id -u)"
BACKUP="$HOME/.config/workspace-switcher-uninstall-$(date +%s)"

GREEN='\033[32m'; RED='\033[31m'; CYAN='\033[1;36m'; DIM='\033[2m'; RESET='\033[0m'
ok()   { printf "${GREEN}  ✔ %s${RESET}\n" "$*"; }
warn() { printf "${RED}  ✘ %s${RESET}\n" "$*"; }
step() { printf "\n${CYAN}== %s ==${RESET}\n" "$*"; }

printf "${CYAN}== Uninstalling the workspace switcher ==${RESET}\n"

step "stopping brew services (sketchybar, borders)"
brew services stop sketchybar >/dev/null 2>&1 || true
brew services stop borders >/dev/null 2>&1 || true
ok "services stopped"

step "killing the app daemon"
pkill -f "workspace-switcher.app" 2>/dev/null || true
ok "daemon stopped"

step "removing the jira poll launchd agent"
PLIST="$HOME/Library/LaunchAgents/com.jira.poll.plist"
launchctl bootout "gui/$UID_" "$PLIST" 2>/dev/null || true
rm -f "$PLIST"
ok "poll agent removed"

step "removing installed configs (backed up to $BACKUP)"
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
ok "configs removed"

step "removing microphone + speech permissions"
TCC_DB="$HOME/Library/Application Support/com.apple.TCC/TCC.db"
sqlite3 "$TCC_DB" "DELETE FROM access WHERE client LIKE '%workspace-switcher%';" 2>/dev/null
killall tccd 2>/dev/null || true
ok "permissions removed"

step "removing caches and runtime files"
rm -rf "$HOME/.cache/workspace-switcher" "$HOME/.cache/jira"
rm -f "$HOME/.cache/ws-crash.log" "$HOME/.cache/ws-aero.log" \
    "$HOME/.cache/aero-debug" "$HOME/.cache/ws-auth-debug" \
    "$HOME/.cache/ws-auth.log"
rm -f /tmp/ws-notes.sock /tmp/ws-aerospace-focus /tmp/workspace-switcher-focus \
    "${TMPDIR:-/tmp}/ws-launch.log"
ok "caches removed"

printf "\n${GREEN}Done. Configs backed up in: %s${RESET}\n" "$BACKUP"