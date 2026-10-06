#!/usr/bin/env bash
# UNINSTALL.sh — remove the whole kitchen-sink stack.
#
#   ./UNINSTALL.sh
#
# Stops the services, kills the daemon, removes configs/launchd/TCC/caches.
# Your personal files (notes, ~/.config/jira) are left alone.
set -uo pipefail

# Works for both installs: run from the checkout (repo install), or the copy
# inside the app (app install):
#   /Applications/kitchen-sink.app/Contents/Resources/UNINSTALL.sh
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
UID_="$(id -u)"
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"
# every name/path removed below lives in install.conf
. "$ROOT/install.conf"
WS_HOME="${WS_HOME:-$WS_HOME_DEFAULT}"
# app install: the configs the links point at live in the home, and the app
# itself is this script's bundle
APP_BUNDLE=""
LINK_ROOT="$ROOT"
case "$ROOT" in *.app/Contents/Resources)
    APP_BUNDLE="${ROOT%/Contents/Resources}"
    LINK_ROOT="$(cd "$WS_HOME" 2>/dev/null && pwd -P)" ;;
esac

GREEN='\033[32m'; RED='\033[31m'; CYAN='\033[1;36m'; RESET='\033[0m'
ok()   { printf "${GREEN}  ✔ %s${RESET}\n" "$*"; }
warn() { printf "${RED}  ✘ %s${RESET}\n" "$*"; }
step() { printf "\n${CYAN}== %s ==${RESET}\n" "$*"; }

printf "${CYAN}== Uninstalling the kitchen sink ==${RESET}\n"

step "stopping brew services ($BREW_SERVICES)"
for svc in $BREW_SERVICES; do
    brew services stop "$svc" >/dev/null 2>&1 || true
done
ok "services stopped"

step "killing the app daemon"
pkill -f "$APP_NAME.app/Contents/MacOS" 2>/dev/null || true
[ -n "$APP_BUNDLE" ] && pkill -f "$APP_BUNDLE/Contents/MacOS" 2>/dev/null || true
ok "daemon stopped"

step "removing the jira poll launchd agent"
PLIST="$JIRA_AGENT_PLIST"
launchctl bootout "gui/$UID_" "$PLIST" 2>/dev/null || true
rm -f "$PLIST"
ok "poll agent removed"

# only OUR links go; real files / directories are never moved or deleted
step "removing the config links"
for d in $CONFIG_DIRS; do
    dst="$HOME/.config/$d"
    # links into this repo (INSTALL.sh) just go — the repo keeps the files
    if [ -L "$dst" ]; then
        case "$(readlink "$dst")" in "$ROOT"/*|"$LINK_ROOT"/*|"$WS_HOME"/*) rm "$dst"; continue ;; esac
    elif [ -d "$dst" ]; then
        while IFS= read -r l; do
            case "$(readlink "$l")" in "$ROOT"/*|"$LINK_ROOT"/*|"$WS_HOME"/*) rm "$l" ;; esac
        done < <(find "$dst" -type l)
        find "$dst" -depth -type d -empty -delete
    fi
    [ -e "$dst" ] && warn "left in place (not ours): $dst"
done
ok "config links removed"

step "removing microphone + speech permissions"
TCC_DB="$HOME/Library/Application Support/com.apple.TCC/TCC.db"
sqlite3 "$TCC_DB" "DELETE FROM access WHERE client = '$BUNDLE_ID' OR client LIKE '%/$APP_NAME.app/%';" 2>/dev/null
killall tccd 2>/dev/null || true
ok "permissions removed"

step "removing caches and runtime files"
# space-separated lists from install.conf: split on purpose
# shellcheck disable=SC2086
rm -rf $CACHE_DIRS
# shellcheck disable=SC2086
rm -f $RUNTIME_FILES
ok "caches removed"

# app install: the app goes to the Trash; the home (your commands.toml,
# rules, config copies) stays where it is. A checkout is never touched.
if [ -n "$APP_BUNDLE" ]; then
    step "removing the app"
    [ -d "$WS_HOME" ] && ! [ -L "$WS_HOME" ] && ok "your settings are still in $WS_HOME — delete it if you don't want them"
    mv "$APP_BUNDLE" "$HOME/.Trash/$APP_NAME-$(date +%s).app" 2>/dev/null \
        && ok "app moved to the Trash" || warn "could not move $APP_BUNDLE to the Trash — drag it there"
else
    rm -f "$ROOT/.install"
fi

printf "\n${GREEN}Done.${RESET}\n"