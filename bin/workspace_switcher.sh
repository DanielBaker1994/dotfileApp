#!/usr/bin/env bash
# fzf-free workspace switcher: native AppKit popup (Swift), themed like
# sketchybar. If the daemon is running (Unix-socket ping succeeds), send a
# toggle message; otherwise build-if-stale and launch it in the background.
#
# Usage:
#   workspace_switcher.sh            toggle the main popup
#   workspace_switcher.sh notes      open ONLY the notes window (no popup)
#   workspace_switcher.sh jira       open ONLY the jira window (no popup)
#   workspace_switcher.sh voice      open ONLY the voice-to-text window
#   workspace_switcher.sh jira-poll [on|off|toggle|setup]
#                                    THE jira switch ([jira] enabled + the
#                                    launchd poll agent) — NOT the window
#                                    toggle above; needs a running daemon
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$DIR/.." && pwd)"
. "$ROOT/install.conf"
APP="$ROOT/$APP_NAME.app"
BIN="$APP/Contents/MacOS/$APP_NAME"
TMP="${TMPDIR:-/tmp}"
FOCUS_FILE="$TMP/workspace-switcher-focus"
MODE="${1:-}"

# jira-poll: flip the poll feature through the running daemon (same code
# path as the menu-bar "Enable Jira"/"Disable Jira": config check, login test, setup
# window). Kept apart from `jira` (window) so the two never get conflated.
if [ "$MODE" = "jira-poll" ]; then
    exec "$BIN" jira-poll "${2:-toggle}"
fi

# Karabiner runs shell_commands with a MINIMAL PATH, so `aerospace` is not
# found unless we add its location (it would silently fail every IPC call).
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"

# Single-flight: overlapping invocations (rapid keypresses, a rebuild already
# running) exit immediately — only one script drives the flow at a time, so
# two daemons / double toggles can never race. (macOS has no flock; mkdir is
# atomic. A stale lock from a crashed run older than 60s is cleared.)
LOCK="$TMP/workspace-switcher.lockdir"
if ! mkdir "$LOCK" 2>/dev/null; then
    if [ -d "$LOCK" ] && [ "$(find "$LOCK" -mmin +1 2>/dev/null)" = "$LOCK" ]; then
        rm -rf "$LOCK"
        mkdir "$LOCK" 2>/dev/null || exit 0
    else
        exit 0
    fi
fi
trap 'rmdir "$LOCK" 2>/dev/null' EXIT

# Record the focused window (wid + app-pid) at keypress time so the switcher
# can hand focus back when dismissed.
LINE=$(aerospace list-windows --focused --format '%{window-id} %{app-pid}')
WID=${LINE%% *}
APID=${LINE##* }
if [ -n "$WID" ]; then
    echo "$WID $APID" >"$FOCUS_FILE"
fi

# Build if the binary is missing or a source file is newer — ONE build
# script (bin/build-app.sh) shared with INSTALL.sh. The daemon ships as a
# real .app bundle: TCC (mic/speech) keys grants by the BUNDLE ID, stable
# across rebuilds; build-app.sh re-signs + re-grants after every build.
# A failed build keeps running the previous binary (a hotkey press must not
# go dead), except in build-only mode where the failure is the answer.
if ! "$DIR/build-app.sh"; then
    [ "${WS_BUILD_ONLY:-}" = "1" ] || [ ! -x "$BIN" ] && exit 1
fi

# Build-only mode (installer): rebuild + re-grant TCC, then stop. The window
# is launched by the user afterwards (installer Done screen).
if [ "${WS_BUILD_ONLY:-}" = "1" ]; then
    exit 0
fi

if [ "$MODE" = "notes" ] || [ "$MODE" = "jira" ] || [ "$MODE" = "voice" ] || [ "$MODE" = "files" ]; then
    DEBUG_LOG="${TMP}/ws-launch.log"
    LOG(){ echo "$(date '+%H:%M:%S.%3N') $*" >>"$DEBUG_LOG"; }
    LOG "== invoke MODE=$MODE =="
    LOG "focused-window line: [$LINE]"
    # If the window is already open (possibly on ANOTHER workspace), move it
    # onto the CURRENT workspace FIRST — done here in bash, not in the daemon,
    # so the daemon's main thread never blocks on aerospace IPC while focusing.
    WS_LIST=$(aerospace list-windows --all --format '%{window-id}|%{app-name}|%{window-title}' 2>/dev/null)
    LOG "list-windows (switcher):"
    echo "$WS_LIST" | grep 'workspace-switcher' | while IFS= read -r l; do LOG "  $l"; done
    EXISTING=$(echo "$WS_LIST" \
        | awk -F'|' -v t="$MODE" '$2=="workspace-switcher" && $3==t {print $1; exit}')
    LOG "existing id for '$MODE': [${EXISTING:-NONE}]"
    if [ -n "$EXISTING" ]; then
        CUR=$(aerospace list-workspaces --focused 2>/dev/null)
        LOG "current workspace: [${CUR:-EMPTY}]"
        if [ -n "$CUR" ]; then
            BEFORE=$(aerospace list-windows --all --format '%{window-id}|%{workspace}' 2>/dev/null \
                | awk -F'|' -v w="$EXISTING" '$1==w {print $2; exit}')
            LOG "window '$EXISTING' workspace BEFORE move: [${BEFORE:-UNKNOWN}]"
            MOVEOUT=$(aerospace move-node-to-workspace --window-id "$EXISTING" "$CUR" 2>&1)
            MOVERC=$?
            LOG "move-node-to-workspace rc=$MOVERC out=[$MOVEOUT]"
            sleep 0.5
            AFTER=$(aerospace list-windows --all --format '%{window-id}|%{workspace}' 2>/dev/null \
                | awk -F'|' -v w="$EXISTING" '$1==w {print $2; exit}')
            LOG "window '$EXISTING' workspace AFTER move: [${AFTER:-UNKNOWN}]"
        fi
    fi
    # ping the running daemon; launch a command-first daemon if there is none
    if WS_PING_ONLY=1 "$BIN" "$MODE" >/dev/null 2>&1; then
        LOG "daemon ping: delivered (daemon will focus)"
    else
        # LaunchServices launch: macOS then attributes microphone/speech TCC
        # to the APP BUNDLE. A nohup child of karabiner's shell inherits that
        # shell as its responsible process, so the bundle's mic grant never
        # applies and voice stays dead for the whole session.
        LOG "daemon ping: FAILED -> launching fresh daemon via LaunchServices ($MODE)"
        # drop any zombie/stale daemon (a pre-fix daemon keeps its broken
        # Karabiner attribution and would answer future pings forever)
        pkill -f "workspace-switcher.app/Contents/MacOS" 2>/dev/null || true
        if ! open -n -g "$APP" --args "$MODE" >/dev/null 2>&1; then
            LOG "LaunchServices launch FAILED — voice permissions will be broken"
        fi
    fi
    exit 0
fi

# Toggle the daemon if it's running; otherwise launch it with "show" so the
# popup appears immediately (no retry loop needed). Same LaunchServices rule
# as above so a cold start from Hyper+S still gets the mic grant.
if ! "$BIN" toggle >/dev/null 2>&1; then
    pkill -f "workspace-switcher.app/Contents/MacOS" 2>/dev/null || true
    if ! open -n -g "$APP" --args show >/dev/null 2>&1; then
        LOG "LaunchServices launch FAILED — voice permissions will be broken"
    fi
fi
