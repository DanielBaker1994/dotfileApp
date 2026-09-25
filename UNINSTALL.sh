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
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"
# every name/path removed below lives in install.conf
. "$ROOT/install.conf"
BACKUP="$UNINSTALL_BACKUP_PREFIX-$(date +%s)"

GREEN='\033[32m'; RED='\033[31m'; CYAN='\033[1;36m'; DIM='\033[2m'; RESET='\033[0m'
ok()   { printf "${GREEN}  ✔ %s${RESET}\n" "$*"; }
warn() { printf "${RED}  ✘ %s${RESET}\n" "$*"; }
step() { printf "\n${CYAN}== %s ==${RESET}\n" "$*"; }

printf "${CYAN}== Uninstalling the workspace switcher ==${RESET}\n"

step "stopping brew services ($BREW_SERVICES)"
for svc in $BREW_SERVICES; do
    brew services stop "$svc" >/dev/null 2>&1 || true
done
ok "services stopped"

step "killing the app daemon"
pkill -f "$APP_NAME.app/Contents/MacOS" 2>/dev/null || true
ok "daemon stopped"

step "removing the jira poll launchd agent"
PLIST="$JIRA_AGENT_PLIST"
launchctl bootout "gui/$UID_" "$PLIST" 2>/dev/null || true
rm -f "$PLIST"
ok "poll agent removed"

step "removing installed configs (backed up to $BACKUP)"
mkdir -p "$BACKUP"
for d in $CONFIG_DIRS; do
    if [ -e "$HOME/.config/$d" ]; then
        mv "$HOME/.config/$d" "$BACKUP/"
    fi
done
ok "configs removed"

step "removing microphone + speech permissions"
TCC_DB="$HOME/Library/Application Support/com.apple.TCC/TCC.db"
sqlite3 "$TCC_DB" "DELETE FROM access WHERE client = '$BUNDLE_ID' OR client LIKE '%/$APP_NAME.app/%';" 2>/dev/null
killall tccd 2>/dev/null || true
ok "permissions removed"

step "removing caches and runtime files"
# shellcheck disable=SC2086 — space-separated lists from install.conf
rm -rf $CACHE_DIRS
rm -f $RUNTIME_FILES
ok "caches removed"

printf "\n${GREEN}Done. Configs backed up in: %s${RESET}\n" "$BACKUP"