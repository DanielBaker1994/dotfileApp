#!/usr/bin/env bash
# Login item for the quick agent (Hyper+S palette / Hyper+X screenshot).
#   quick-agent.sh install     write ~/Library/LaunchAgents/<label>.plist and load it
#   quick-agent.sh uninstall   unload and remove it
#   quick-agent.sh status      plist + launchd + the agent's own socket
# The agent itself is `kitchen-sink quick` (see AGENTS.md, "Quick agent").
set -uo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
ROOT="$(cd "$DIR/.." && pwd -P)"
. "$ROOT/install.conf"
APP="$ROOT/$APP_NAME.app"
case "$ROOT" in *.app/Contents/Resources) APP="${ROOT%/Contents/Resources}" ;; esac
LABEL="${QUICK_AGENT_LABEL:-dev.danielbaker.kitchen-sink.quick}"
PLIST="${QUICK_AGENT_PLIST:-$HOME/Library/LaunchAgents/$LABEL.plist}"
SRC="$ROOT/config/launchd/$LABEL.plist"
GUI="gui/$(id -u)"

case "${1:-status}" in
install)
    [ -f "$SRC" ] || { echo "missing template $SRC" >&2; exit 1; }
    mkdir -p "$(dirname "$PLIST")"
    sed "s|__APP__|$APP|g" "$SRC" >"$PLIST" || exit 1
    plutil -lint "$PLIST" >/dev/null || { echo "bad plist $PLIST" >&2; exit 1; }
    launchctl bootout "$GUI" "$PLIST" 2>/dev/null
    launchctl bootstrap "$GUI" "$PLIST" 2>/dev/null || launchctl load "$PLIST" || exit 1
    echo "quick agent login item installed: $PLIST"
    ;;
uninstall)
    launchctl bootout "$GUI" "$PLIST" 2>/dev/null || true
    rm -f "$PLIST"
    echo "quick agent login item removed"
    ;;
status)
    [ -f "$PLIST" ] && echo "plist: $PLIST" || echo "plist: not installed"
    launchctl list "$LABEL" >/dev/null 2>&1 && echo "launchd: loaded" || echo "launchd: not loaded"
    if "$APP/Contents/MacOS/$APP_NAME" quick-state 2>/dev/null; then :; else echo "agent: not running"; fi
    ;;
*)
    echo "usage: quick-agent.sh install|uninstall|status" >&2
    exit 2
    ;;
esac
