#!/usr/bin/env bash
set -uo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
ROOT="$(cd "$DIR/.." && pwd -P)"
. "$ROOT/install.conf"
APP="$ROOT/$APP_NAME.app"
BUNDLED=0
case "$ROOT" in *.app/Contents/Resources) BUNDLED=1; APP="${ROOT%/Contents/Resources}" ;; esac
BIN="${WS_BIN:-$APP/Contents/MacOS/$APP_NAME}"
# WS_BIN (dev/gate) points at another bundle's executable: follow it so the
# LaunchServices `open` calls below launch the same app the ping uses.
case "$BIN" in *.app/Contents/MacOS/*) APP="${BIN%/Contents/MacOS/*}" ;; esac
TMP="${TMPDIR:-/tmp}"
FOCUS_FILE="$TMP/kitchen-sink-focus"
MODE="${1:-}"

if [ "$MODE" = "jira-poll" ]; then
    exec "$BIN" jira-poll "${2:-toggle}"
fi

# WS_DEBUG=1 logs the launch steps to logs/YYYY_MM_DD/launch.log
LOG(){
  if [ "${WS_DEBUG:-}" = "1" ]; then
    _log_date=$(date '+%Y_%m_%d')
    _log_dir="$ROOT/logs/$_log_date"
    mkdir -p "$_log_dir" 2>/dev/null || true
    echo "$(date '+%H:%M:%S') $*" >>"$_log_dir/launch.log"
  fi
}

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
    [ -n "${WS_BIN:-}" ] && return 0
    [ "$BUNDLED" = 1 ] && return 0
    if ! "$DIR/build-app.sh"; then
        [ "${WS_BUILD_ONLY:-}" = "1" ] || [ ! -x "$BIN" ] && exit 1
    fi
}

# Hyper+S (palette) and Hyper+X (screenshot) belong to the QUICK AGENT
# (`kitchen-sink quick`, its own socket $TMPDIR/ks-quick.sock): a small
# resident process of the same signed bundle that works with the main app not
# running. On a running agent a press is one socket write. With no agent
# answering, start it through LaunchServices (TCC) with the press as its first
# action — no lock (the agent's own lock makes a second `open -n` hand its
# action over and exit), no aerospace query. The main app is not involved.
if [ "${WS_BUILD_ONLY:-}" != "1" ]; then
    case "$MODE" in
    "")
        "$BIN" quick-toggle >/dev/null 2>&1 && exit 0
        start=show ;;
    screenshot)
        "$BIN" quick-screenshot "${@:2}" >/dev/null 2>&1 && exit 0
        start=screenshot ;;
    *) start="" ;;
    esac
    if [ -n "$start" ]; then
        [ -x "$BIN" ] || build
        LOG "quick agent not answering -> starting it ($start)"
        open -n -g "$APP" --args quick "$start" >/dev/null 2>&1 || LOG "LaunchServices launch FAILED — TCC grants will be broken"
        exit 0
    fi
fi

# Stop stray MAIN-app processes but spare the quick agent (argv `quick`),
# which must outlive the main app being restarted.
kill_main() {
    local p cmd
    for p in $(pgrep -f "$APP/Contents/MacOS" 2>/dev/null); do
        cmd=$(ps -o command= -p "$p" 2>/dev/null)
        case "$cmd" in *"MacOS/$APP_NAME quick"*) continue ;; esac
        kill "$p" 2>/dev/null
    done
    return 0
}

export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"

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

LINE=$(aerospace list-windows --focused --format '%{window-id} %{app-pid} %{workspace}')
read -r WID APID CUR <<<"$LINE"
if [ -n "$WID" ]; then
    echo "$WID $APID" >"$FOCUS_FILE"
fi

if [ "${WS_BUILD_ONLY:-}" = "1" ] || [ ! -x "$BIN" ]; then
    build
fi

# Build-only mode (installer): rebuild + re-grant TCC, then stop. The window
# is launched by the user afterwards (installer Done screen).
if [ "${WS_BUILD_ONLY:-}" = "1" ]; then
    exit 0
fi

# `kitchen_sink.sh quick [show|screenshot]`: just (re)start the quick agent.
case "$MODE" in quick)
    if ! "$BIN" quick-state >/dev/null 2>&1; then
        build
        open -n -g "$APP" --args quick "${2:-}" >/dev/null 2>&1 || LOG "LaunchServices launch FAILED"
    fi
    exit 0
esac

case "$MODE" in window|jira|files|confluence|ai)
    LOG "== invoke MODE=$MODE focused=[$LINE] =="
    # files, jira and output windows are all views of ONE shared
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
        # attribution and would answer future pings forever); the quick
        # agent is spared
        kill_main
        # cold start: "window" opens the first view, files —
        # the CLI client (rust/ws-rs/src/app/cli.rs) maps it
        if ! open -n -g "$APP" --args "$MODE" >/dev/null 2>&1; then
            LOG "LaunchServices launch FAILED — voice permissions will be broken"
        fi
    fi
    exit 0
esac

echo "kitchen_sink.sh: unknown mode '$MODE'" >&2
LOG "unknown mode '$MODE'"
exit 2
