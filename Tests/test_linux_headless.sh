#!/usr/bin/env bash
# Tests/test_linux_headless.sh — Wave 2 Linux headless smoke test.
#
# Builds ws-rs inside the pinned Linux image (rust/docker/linux.Dockerfile)
# and drives the app with no AppKit at all: `kitchen-sink headless` must bind
# the command socket and answer `state` / `do:open:files` over it.
#
# The whole build + run happens in ONE container (the checkout is bind-mounted
# at /repo, so nothing is baked in). Run:
#
#   bin/run-tests.sh linux-headless
#   Tests/test_linux_headless.sh          # same thing
#
# `--in-container` runs the smoke body directly; the script passes that flag
# to itself inside docker (see bin/linux-check.sh for the same image/target).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
IMAGE="kitchen-sink-linux-check"
DOCKERFILE="$ROOT/rust/docker/linux.Dockerfile"
# Native on Apple Silicon (fast); x86_64 hosts override via the env var.
PLATFORM="${LINUX_CHECK_PLATFORM:-linux/arm64}"
TARGET_DIR="/repo/.build/linux-target"

# probe REQUEST — send one newline-terminated line to the daemon socket and
# print its reply. The pinned image ships python3 but not netcat, so the unix
# socket client is python3 (functionally the same as `printf … | nc -U …`).
probe() {
    printf '%s\n' "$1" | python3 -c '
import socket, sys
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.settimeout(5.0)
s.connect(sys.argv[1])
s.sendall(sys.stdin.buffer.read())
out = []
while True:
    b = s.recv(65536)
    if not b:
        break
    out.append(b)
sys.stdout.write(b"".join(out).decode("utf-8", "replace"))
' "$SOCK"
}

in_container() {
    cd "$ROOT/rust"
    echo "== cargo build -p ws-rs =="
    cargo build -p ws-rs

    BIN="${CARGO_TARGET_DIR:-$ROOT/.build/linux-target}/debug/kitchen-sink"
    [ -x "$BIN" ] || { echo "test_linux_headless: no binary at $BIN" >&2; exit 1; }

    WORK="$(mktemp -d)"
    export WS_HOME="$WORK/home"
    export TMPDIR="$WORK/tmp"
    mkdir -p "$WS_HOME" "$TMPDIR"
    # Non-repo bundle: point the helper at this checkout's pylib/vim trees.
    export WS_RS_ASSET_DIR="$ROOT"

    "$BIN" headless >"$WORK/daemon.out" 2>"$WORK/daemon.err" &
    DAEMON=$!
    trap 'kill "$DAEMON" 2>/dev/null || true' EXIT

    SOCK="$TMPDIR/ws-notes.sock"
    for _ in $(seq 1 100); do
        [ -S "$SOCK" ] && break
        kill -0 "$DAEMON" 2>/dev/null \
            || { echo "test_linux_headless: daemon exited early" >&2; cat "$WORK/daemon.err" >&2; exit 1; }
        sleep 0.1
    done
    [ -S "$SOCK" ] \
        || { echo "test_linux_headless: socket $SOCK never appeared" >&2; cat "$WORK/daemon.err" >&2; exit 1; }

    STATE="$(probe state)"
    echo "state: $STATE"
    python3 -c 'import json,sys; d=json.loads(sys.argv[1]); assert "view" in d, "state has no view key"' "$STATE"

    OPENED="$(probe do:open:files)"
    echo "do:open:files: $OPENED"
    python3 -c 'import json,sys; d=json.loads(sys.argv[1]); assert d.get("view")=="files", "view is %r" % d.get("view")' "$OPENED"

    echo "linux-headless: OK"
}

main() {
    if [ "${1:-}" = "--in-container" ]; then
        in_container
        return
    fi

    command -v docker >/dev/null 2>&1 || { echo "linux-headless: docker not found" >&2; exit 1; }
    docker info >/dev/null 2>&1 || { echo "linux-headless: Docker is not running (open -a Docker)" >&2; exit 1; }

    echo "== docker build ($IMAGE) =="
    docker build --platform "$PLATFORM" -t "$IMAGE" -f "$DOCKERFILE" "$ROOT/rust/docker" >/dev/null

    echo "== headless smoke (container) =="
    docker run --rm --platform "$PLATFORM" \
        -v "$ROOT":/repo -w /repo \
        -e CARGO_TARGET_DIR="$TARGET_DIR" \
        "$IMAGE" bash /repo/Tests/test_linux_headless.sh --in-container
}

main "$@"
