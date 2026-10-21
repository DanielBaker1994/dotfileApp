#!/usr/bin/env bash
# Headless kitchen-sink sandbox for agent control (MCP / `nc`).
#
#   bin/ks-mcp-sandbox.sh start [--gui]|stop|status|sock|home|reset
#
# Runs the Rust daemon in `headless` mode against an isolated TMPDIR + WS_HOME,
# so socket `state` / `do:` / raw verbs are safe to drive without touching the
# live app or the repo's commands.toml:
#
#   export KS_SOCKET="$(bin/ks-mcp-sandbox.sh sock)"
#   printf 'state\n'          | nc -U -w3 "$KS_SOCKET" | jq .
#   echo 'do:open:notes'      | nc -U -w3 "$KS_SOCKET" | jq .views.notes.shown
#
# `start --gui` launches the real windowed app instead (isolated the same way,
# via `open -n -g` so LaunchServices applies the bundle's TCC grants) so
# `pane-shot` / `screenshot` have real windows to capture. AeroSpace is turned
# OFF for the run (its on-window-detected rule moves/tiles the shared window and
# breaks capture + geometry checks) and turned back ON by `stop`. This affects
# the owner's whole session while the sandbox GUI is up — stop it when done.
#
# Live-safety rules (AGENTS.md): never `./ws build` (it pkills the live app),
# never `pkill`; the sandbox daemon is killed only by its pidfile or by the pid
# holding its own socket. Sandbox root: $KS_MCP_SANDBOX (default
# $ROOT/.build/mcp-sandbox). Set KS_MCP_SANDBOX_NO_BUILD=1 to skip the cargo
# build (tests only).
set -euo pipefail

# No git needed (a test VM has none): the repo root is this script's parent.
ROOT=$(cd "$(dirname "$0")/.." && pwd -P)
SANDBOX="${KS_MCP_SANDBOX:-$ROOT/.build/mcp-sandbox}"
BIN="$ROOT/.build/rust-target/release/kitchen-sink"
SOCK="$SANDBOX/tmp/ws-notes.sock"
PIDFILE="$SANDBOX/pid"
# `--gui` launches this bundle; point it at a copy holding the fresh release
# binary (signed with the same identity, so TCC grants apply) to GUI-test code
# newer than the live app's last `./ws build`.
APP="${KS_MCP_SANDBOX_APP:-$ROOT/kitchen-sink.app}"
AERO_MARK="$SANDBOX/aerospace-disabled"
# The app's HOME is this short alias of $SANDBOX/home. nvim listens on
# $HOME/.cache/kitchen-sink/nvim-notes-<pid>.sock (notes_vim.rs socket_path),
# and under the repo-relative sandbox HOME that path is ~105 bytes, past the
# 103-byte AF_UNIX limit: nvim RPC then fails every time and the notes editor
# misbehaves in ways the real app doesn't (2026-10-20 hammer, G1).
HOME_ALIAS="/tmp/ksb-$(printf '%s' "$SANDBOX" | shasum | cut -c1-8)"
# Preferences live in NSUserDefaults keyed by bundle id, not under HOME: a GUI
# sandbox (same bundle id) would read and write the LIVE app's settings.
# WS_DEFAULTS_SUITE (platform/settings.rs) gives the sandbox its own domain.
DEFAULTS_SUITE="dev.danielbaker.kitchen-sink.sandbox-${HOME_ALIAS##*/ksb-}"
LOG="$SANDBOX/daemon.log"

die() { printf 'ks-mcp-sandbox: %s\n' "$*" >&2; exit 1; }

# The pid to consider: the pidfile when it is alive, else whoever holds the
# sandbox socket (a crashed daemon leaves the pidfile behind).
find_pid() {
    local pid=""
    if [ -f "$PIDFILE" ]; then
        pid=$(cat "$PIDFILE" 2>/dev/null || true)
    fi
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
        printf '%s\n' "$pid"
        return 0
    fi
    if [ -S "$SOCK" ]; then
        pid=$(lsof -t "$SOCK" 2>/dev/null | head -1 || true)
        if [ -n "$pid" ]; then
            printf '%s\n' "$pid"
            return 0
        fi
    fi
    return 1
}

# Safety: only ever touch a pid whose command line is the sandbox binary.
is_ours() {
    local cmd
    cmd=$(ps -p "$1" -o command= 2>/dev/null || true)
    case "$cmd" in
        "$BIN"*|"$APP/Contents/MacOS/"*) return 0 ;;
    esac
    return 1
}

socket_answers() {
    local out
    [ -S "$SOCK" ] || return 1
    out=$(printf 'state\n' | nc -U -w1 "$SOCK" 2>/dev/null || true)
    case "$out" in
        *'"pid"'*) return 0 ;;
    esac
    return 1
}

aerospace_off() {
    command -v aerospace >/dev/null 2>&1 || return 0
    if aerospace enable off >/dev/null 2>&1; then
        : > "$AERO_MARK"
        echo "AeroSpace disabled for the sandbox GUI (restored by 'stop')"
    fi
}

aerospace_restore() {
    [ -f "$AERO_MARK" ] || return 0
    aerospace enable on >/dev/null 2>&1 || true
    rm -f "$AERO_MARK"
    echo "AeroSpace re-enabled"
}

cmd_start() {
    local gui=0
    [ "${1:-}" = "--gui" ] && gui=1
    # Live-safe build: raw cargo into .build/rust-target (never `./ws build`).
    if [ "${KS_MCP_SANDBOX_NO_BUILD:-}" != "1" ]; then
        ( cd "$ROOT/rust" && CARGO_TARGET_DIR="$ROOT/.build/rust-target" \
            cargo build --release -p ws-rs -p ws-helpers )
    fi
    # --gui launches $APP (checked below); only headless runs the cargo binary.
    if [ "$gui" != 1 ]; then
        [ -x "$BIN" ] || die "missing $BIN (unset KS_MCP_SANDBOX_NO_BUILD to build)"
    fi

    # The Rust socket layer hard-fails at sun_path >= 104 bytes.
    if [ "${#SOCK}" -ge 104 ]; then
        die "sandbox socket path is ${#SOCK} bytes (must be < 104): $SOCK"
    fi
    local nvim_sock="$HOME_ALIAS/.cache/kitchen-sink/nvim-notes-99999.sock"
    if [ "${#nvim_sock}" -ge 104 ]; then
        die "nvim socket path would be ${#nvim_sock} bytes (must be < 104): $nvim_sock"
    fi

    local pid=""
    if pid=$(find_pid) && is_ours "$pid" && socket_answers; then
        echo "already running (pid $pid, sock $SOCK)"
        return 0
    fi

    mkdir -p "$SANDBOX/tmp" "$SANDBOX/home" \
        "$SANDBOX/home/.cache" "$SANDBOX/home/.config" "$SANDBOX/home/.local/share"
    if [ ! -f "$SANDBOX/home/commands.toml" ]; then
        cp "$ROOT/commands.toml" "$SANDBOX/home/commands.toml"
    fi
    # No Jira credentials in the sandbox: seed a fake board (catalog, columns
    # spec, one board's issues) so `do:board:*` has something to open. Only
    # writes files that are absent, so a live home is never touched.
    python3 -B "$ROOT/bin/ks-jira-board-fixture.py" "$SANDBOX/home" >/dev/null 2>&1 || true
    rm -f "$SOCK" "$SOCK.lock" "$LOG" "$PIDFILE"
    ln -sfn "$SANDBOX/home" "$HOME_ALIAS"

    # `headless` exists from the current build; the grep probe keeps an older
    # binary from silently launching a windowed daemon (still isolated).
    local mode="headless"
    if [ "$gui" = 1 ]; then
        mode="show"   # palette only; a bare start opens + activates the notes window
    elif ! grep -aq "headless daemon" "$BIN"; then
        mode="show"
        echo "warning: $BIN has no headless mode — launching arg 'show' instead (GUI daemon, hidden window)" >&2
    fi

    local envs=(
        "TMPDIR=$SANDBOX/tmp"
        "HOME=$HOME_ALIAS"
        "XDG_CACHE_HOME=$HOME_ALIAS/.cache"
        "XDG_CONFIG_HOME=$HOME_ALIAS/.config"
        "XDG_DATA_HOME=$HOME_ALIAS/.local/share"
        "WS_HOME=$SANDBOX/home"
        "WS_COMMANDS_CONF=$SANDBOX/home/commands.toml"
        "WS_RS_ASSET_DIR=$ROOT"
        "WS_SKIP_LAUNCH_AGENT=1"
        "PYTHONDONTWRITEBYTECODE=1"
        "JIRA_CONFIG_JSON=$SANDBOX/home/jira/config.json"
        "JIRA_CONFIG_FILE=$SANDBOX/home/jira/config-legacy.json"
        "JIRA_TEAM_JSON=$SANDBOX/home/jira/team.json"
        "JIRA_CACHE_DIR=$SANDBOX/home/jira/cache"
        "NOTIFY_CACHE_DIR=$SANDBOX/home/notifications"
        "WS_SETTINGS_CACHE=$SANDBOX/home/settings-cache"
        "WS_LOG_DIR=$SANDBOX/home/logs"
        "WS_DEFAULTS_SUITE=$DEFAULTS_SUITE"
    )
    # `open` hands the app launchd's PATH, so a Python outside it (the test VM's)
    # must be named explicitly.
    [ -n "${WS_PYTHON:-}" ] && envs+=("WS_PYTHON=$WS_PYTHON")
    if [ "$gui" = 1 ]; then
        [ -d "$APP" ] || die "missing $APP (run ./ws build once)"
        aerospace_off
        local oargs=() e
        for e in "${envs[@]}"; do oargs+=(--env "$e"); done
        open -n -g --stdout "$LOG" --stderr "$LOG" "${oargs[@]}" "$APP" ${mode:+--args "$mode"}
    else
        env "${envs[@]}" WS_HEADLESS=1 nohup "$BIN" "$mode" >"$LOG" 2>&1 &
        echo $! > "$PIDFILE"
    fi

    local i
    for i in $(seq 1 100); do
        if socket_answers; then
            # `open` gives no pid: take whoever holds the sandbox socket.
            [ "$gui" = 1 ] && lsof -t "$SOCK" 2>/dev/null | head -1 > "$PIDFILE"
            echo "running (pid $(cat "$PIDFILE"), sock $SOCK)"
            return 0
        fi
        if [ "$gui" = 0 ] && ! kill -0 "$(cat "$PIDFILE")" 2>/dev/null; then
            break
        fi
        sleep 0.1
    done
    {
        echo "--- $LOG (tail)"
        tail -20 "$LOG" 2>/dev/null || true
    } >&2
    pid=$(cat "$PIDFILE" 2>/dev/null || true)
    [ -z "$pid" ] && pid=$(lsof -t "$SOCK" 2>/dev/null | head -1 || true)
    if [ -n "$pid" ] && is_ours "$pid"; then
        kill -TERM "$pid" 2>/dev/null || true
    fi
    aerospace_restore
    die "sandbox daemon did not come up"
}

cmd_stop() {
    local pid=""
    if pid=$(find_pid); then
        if ! is_ours "$pid"; then
            die "pid $pid holds $SOCK but is not the sandbox binary — refusing to kill it"
        fi
        kill -TERM "$pid" 2>/dev/null || true
        local i
        for i in $(seq 1 50); do
            if ! kill -0 "$pid" 2>/dev/null; then
                break
            fi
            sleep 0.1
        done
        if kill -0 "$pid" 2>/dev/null; then
            kill -KILL "$pid" 2>/dev/null || true
        fi
    fi
    rm -f "$PIDFILE" "$SOCK" "$SOCK.lock"
    aerospace_restore
    if [ -n "$pid" ]; then
        echo "stopped (pid $pid)"
    else
        echo "stopped"
    fi
}

cmd_status() {
    local pid=""
    if pid=$(find_pid) && is_ours "$pid" && socket_answers; then
        echo "running (pid $pid, sock $SOCK)"
    else
        echo "stopped"
    fi
}

cmd_reset() {
    cmd_stop >/dev/null
    rm -rf "$SANDBOX/home" "$SANDBOX/tmp"
    defaults delete "$DEFAULTS_SUITE" >/dev/null 2>&1 || true
    echo "reset (home/ reseeds from the repo commands.toml on next start)"
}

case "${1:-}" in
    start)  shift; cmd_start "$@" ;;
    stop)   cmd_stop ;;
    status) cmd_status ;;
    sock)   printf '%s\n' "$SOCK" ;;
    home)   printf '%s\n' "$HOME_ALIAS" ;;
    reset)  cmd_reset ;;
    *)      die "usage: bin/ks-mcp-sandbox.sh start [--gui]|stop|status|sock|home|reset" ;;
esac
