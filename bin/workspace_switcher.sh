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
APP="$ROOT/workspace-switcher.app"
BIN="$APP/Contents/MacOS/workspace-switcher"
MAIN="$ROOT/main.swift"
SRC="$ROOT/workspace_switcher.swift"
FRAMEWORK="$ROOT/PopupWindow.swift"
# SwiftTerm is precompiled ONCE into a static library + module (it is ~230
# files); the daemon links it. Rebuilt only when a SwiftTerm source changes.
TERM_LIB="$ROOT/.build/SwiftTerm/libSwiftTerm.a"
TERM_MOD_DIR="$ROOT/.build/SwiftTerm"
TERM_SENTINEL="$ROOT/.build/.termbuilt"
build_term_lib() {
    # skip if already built AND no SwiftTerm source changed since
    [ -f "$TERM_LIB" ] && [ -f "$TERM_SENTINEL" ] && \
        [ -z "$(find "$ROOT"/Vendor/SwiftTerm/Sources "$ROOT"/Vendor/SwiftTerm/Generated \
            -name '*.swift' -newer "$TERM_SENTINEL" 2>/dev/null | head -1)" ] && return 0
    mkdir -p "$TERM_MOD_DIR"
    swiftc -O -swift-version 5 -parse-as-library -emit-library -static -module-name SwiftTerm \
        "$ROOT"/Vendor/SwiftTerm/Sources/SwiftTerm/*.swift \
        "$ROOT"/Vendor/SwiftTerm/Sources/SwiftTerm/Apple/*.swift \
        "$ROOT"/Vendor/SwiftTerm/Sources/SwiftTerm/Apple/Metal/*.swift \
        "$ROOT"/Vendor/SwiftTerm/Sources/SwiftTerm/Mac/*.swift \
        "$ROOT"/Vendor/SwiftTerm/Sources/SwiftTerm/Portable/*.swift \
        "$ROOT"/Vendor/SwiftTerm/Generated/*.swift \
        -emit-module -emit-module-path "$TERM_MOD_DIR/SwiftTerm.swiftmodule" \
        -o "$TERM_LIB" >/dev/null 2>&1
    touch "$TERM_SENTINEL"
}
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

# Build if the binary is missing or a source file is newer. The daemon ships
# as a real .app bundle: TCC (mic/speech permissions) keys grants by the
# BUNDLE ID, which is stable across rebuilds — a bare binary loses its grant
# every time the ad-hoc signature changes.
if [ ! -x "$BIN" ] || [ "$MAIN" -nt "$BIN" ] || [ "$SRC" -nt "$BIN" ] || [ "$FRAMEWORK" -nt "$BIN" ] \
    || [ ! -f "$TERM_LIB" ]; then
    build_term_lib
    # a REBUILD means any RUNNING daemon is the OLD binary — kill it or the
    # socket ping keeps talking to the stale, grant-less process
    pkill -f "workspace-switcher" 2>/dev/null || true
    mkdir -p "$(dirname "$BIN")"
    BUILD_TMP="$(mktemp "$TMP/ws-build.XXXXXX")" || exit 1
    if swiftc -O -swift-version 5 -Xlinker -sectcreate -Xlinker __TEXT -Xlinker __info_plist -Xlinker "$ROOT/Info.plist" \
        -I "$TERM_MOD_DIR" -Xlinker "$TERM_LIB" \
        "$FRAMEWORK" "$SRC" "$MAIN" -o "$BUILD_TMP" >/dev/null 2>&1 ||
       swiftc -Xlinker -sectcreate -Xlinker __TEXT -Xlinker __info_plist -Xlinker "$ROOT/Info.plist" \
        -I "$TERM_MOD_DIR" -Xlinker "$TERM_LIB" \
        "$FRAMEWORK" "$SRC" "$MAIN" -o "$BUILD_TMP"; then
        mv "$BUILD_TMP" "$BIN"
        # the bundle's on-disk Info.plist is what LaunchServices reads for
        # Finder right-click services (NSServices) — keep it in sync or the
        # "Copy Path" / "Open in Notes" context items never register
        cp "$ROOT/Info.plist" "$APP/Contents/Info.plist"
        # sign with the stable self-signed cert (login keychain) so TCC
        # grants (Desktop/Documents folders, …) survive rebuilds — an ad-hoc
        # signature's designated requirement is its cdhash, which changes
        # every build and re-prompts. Falls back to ad-hoc if the cert is gone.
        SIGN_ID="workspace-switcher codesign"
        if security find-certificate -c "$SIGN_ID" >/dev/null 2>&1; then
            # perl alarm = portable timeout: an unapproved key ACL pops a
            # keychain dialog and would hang the build forever
            perl -e 'alarm 15; exec @ARGV' codesign --force --sign "$SIGN_ID" --identifier dev.danielbaker.workspace-switcher "$APP" >/dev/null 2>&1 \
                || codesign --force --sign - --identifier dev.danielbaker.workspace-switcher "$APP" >/dev/null 2>&1
        else
            codesign --force --sign - --identifier dev.danielbaker.workspace-switcher "$APP" >/dev/null 2>&1
        fi
        # fresh build = fresh signature — re-grant mic + speech silently so
        # voice notes keep working (bundle-id grants persist across rebuilds)
        "$DIR/voice-permissions.sh" >/dev/null 2>&1 || true
    else
        rm -f "$BUILD_TMP"
    fi
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
