#!/usr/bin/env bash
# fzf-free kitchen sink: native AppKit popup (Swift), themed from
# commands.toml [theme]. If the daemon is running (Unix-socket ping succeeds), send a
# toggle message; otherwise build-if-stale and launch it in the background.
#
# Usage:
#   kitchen_sink.sh            toggle the main popup
#   kitchen_sink.sh window     show the shared window on its last view / hide it
#   kitchen_sink.sh notes      open ONLY the notes window (no popup)
#   kitchen_sink.sh jira       open ONLY the jira window (no popup)
#   kitchen_sink.sh voice      open ONLY the voice-to-text window
#   kitchen_sink.sh jira-poll [on|off|toggle|setup]
#                                    THE jira switch ([jira] enabled + the
#                                    launchd poll agent) — NOT the window
#                                    toggle above; needs a running daemon
# (physical paths: an app install reaches this script through the link
# ~/.config/kitchen-sink/bin -> <App>/Contents/Resources/bin)
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
ROOT="$(cd "$DIR/.." && pwd -P)"
. "$ROOT/install.conf"
APP="$ROOT/$APP_NAME.app"
# app install (DMG): this script lives INSIDE the bundle — nothing to build
BUNDLED=0
case "$ROOT" in *.app/Contents/Resources) BUNDLED=1; APP="${ROOT%/Contents/Resources}" ;; esac
BIN="$APP/Contents/MacOS/$APP_NAME"
TMP="${TMPDIR:-/tmp}"
FOCUS_FILE="$TMP/kitchen-sink-focus"
MODE="${1:-}"

# jira-poll: flip the poll feature through the running daemon (same code
# path as the menu-bar "Enable Jira"/"Disable Jira": config check, login test, setup
# window). Kept apart from `jira` (window) so the two never get conflated.
if [ "$MODE" = "jira-poll" ]; then
    exec "$BIN" jira-poll "${2:-toggle}"
fi

# Hotkey tools run shell commands with a MINIMAL PATH, so `aerospace` is not
# found unless we add its location (it would silently fail every IPC call).
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"

# Single-flight: overlapping invocations (rapid keypresses, a rebuild already
# running) exit immediately — only one script drives the flow at a time, so
# two daemons / double toggles can never race. (macOS has no flock; mkdir is
# atomic. A stale lock from a crashed run older than 60s is cleared.)
LOCK="$TMP/kitchen-sink.lockdir"
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
# can hand focus back when dismissed. ONE aerospace call also gives the
# focused workspace (used by the window modes below).
LINE=$(aerospace list-windows --focused --format '%{window-id} %{app-pid} %{workspace}')
read -r WID APID CUR <<<"$LINE"
if [ -n "$WID" ]; then
    echo "$WID $APID" >"$FOCUS_FILE"
fi

# Build-if-stale — ONE build script (bin/build-app.sh) shared with
# INSTALL.sh. Only when the binary is missing, in build-only mode, or when
# no daemon answers: a hotkey press on a running daemon never pays for it
# (`./ws build` rebuilds + relaunches during development). The daemon ships
# as a real .app bundle: TCC (mic/speech) keys grants by the BUNDLE ID,
# stable across rebuilds; build-app.sh re-signs + re-grants after every
# build. A failed build keeps running the previous binary (a hotkey press
# must not go dead), except in build-only mode where the failure is the
# answer.
build() {
    [ "$BUNDLED" = 1 ] && return 0
    if ! "$DIR/build-app.sh"; then
        [ "${WS_BUILD_ONLY:-}" = "1" ] || [ ! -x "$BIN" ] && exit 1
    fi
}
if [ "${WS_BUILD_ONLY:-}" = "1" ] || [ ! -x "$BIN" ]; then
    build
fi

# Build-only mode (installer): rebuild + re-grant TCC, then stop. The window
# is launched by the user afterwards (installer Done screen).
if [ "${WS_BUILD_ONLY:-}" = "1" ]; then
    exit 0
fi

# WS_DEBUG=1 logs the launch steps to $TMPDIR/ws-launch.log
LOG(){ [ "${WS_DEBUG:-}" = "1" ] && echo "$(date '+%H:%M:%S') $*" >>"$TMP/ws-launch.log"; }

case "$MODE" in screenshot)
    # /screenshot (Hyper+X) with no daemon running (a warm daemon gets the
    # binary's own ping): no window to move, just start it command-first
    if WS_PING_ONLY=1 "$BIN" screenshot >/dev/null 2>&1; then
        LOG "daemon ping: delivered (screenshot)"
    else
        build
        pkill -f "$APP/Contents/MacOS" 2>/dev/null || true
        open -n -g "$APP" --args screenshot >/dev/null 2>&1 || LOG "LaunchServices launch FAILED"
    fi
    exit 0
esac

case "$MODE" in window|notes|jira|voice|files|terminal|confluence|ai)
    LOG "== invoke MODE=$MODE focused=[$LINE] =="
    # notes, files, jira and output windows are all views of ONE shared
    # window: move ours (not the Hyper+S palette, titled like the app) onto
    # the CURRENT workspace — only the ones on another workspace, and no
    # wait: aerospace answers once the move is done. (A warm daemon's
    # hotkey path does this itself; see hotkeyPrep.)
    if [ -n "$CUR" ]; then
        aerospace list-windows --all --format '%{window-id}|%{app-name}|%{window-title}|%{workspace}' 2>/dev/null \
            | awk -F'|' -v c="$CUR" '$2=="kitchen-sink" && $3!="kitchen-sink" && $4!=c {print $1}' \
            | while read -r W; do
                LOG "move $W -> $CUR"
                aerospace move-node-to-workspace --window-id "$W" "$CUR" >/dev/null 2>&1
            done
    fi
    # ping the running daemon; launch a command-first daemon if there is none
    if WS_PING_ONLY=1 "$BIN" "$MODE" >/dev/null 2>&1; then
        LOG "daemon ping: delivered"
    else
        build
        # LaunchServices launch: macOS then attributes microphone/speech TCC
        # to the APP BUNDLE. A nohup child of the hotkey tool's shell inherits that
        # shell as its responsible process, so the bundle's mic grant never
        # applies and voice stays dead for the whole session.
        LOG "daemon ping: FAILED -> launching fresh daemon via LaunchServices ($MODE)"
        # drop any zombie/stale daemon (a pre-fix daemon keeps its broken
        # attribution and would answer future pings forever)
        pkill -f "$APP/Contents/MacOS" 2>/dev/null || true
        # cold start: "window" opens the first view, files —
        # main.swift maps it
        if ! open -n -g "$APP" --args "$MODE" >/dev/null 2>&1; then
            LOG "LaunchServices launch FAILED — voice permissions will be broken"
        fi
    fi
    exit 0
esac

# Toggle the daemon if it's running; otherwise launch it with "show" so the
# popup appears immediately (no retry loop needed). Same LaunchServices rule
# as above so a cold start from Hyper+S still gets the mic grant.
if ! "$BIN" toggle >/dev/null 2>&1; then
    build
    pkill -f "$APP/Contents/MacOS" 2>/dev/null || true
    if ! open -n -g "$APP" --args show >/dev/null 2>&1; then
        LOG "LaunchServices launch FAILED — voice permissions will be broken"
    fi
fi
