#!/usr/bin/env bash
# bin/linux-check.sh — compile/test kitchen-sink for Linux without a Linux host.
#
# Compiles the Rust workspace inside a pinned Debian image (rustc matches the
# local 1.92 toolchain) with the checkout bind-mounted at /repo. This is Wave 0
# of PLAN-cross-platform.md: with the macOS deps target-gated, `cargo check` on
# Linux surfaces exactly the AppKit leaks the later waves fix.
#
#   bin/linux-check.sh check          # cargo check --workspace --all-targets
#   bin/linux-check.sh test [filter]  # cargo test -p ws-rs -- [filter] + python suites
#   bin/linux-check.sh shell          # interactive shell in the image
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
ROOT="$(cd "$DIR/.." && pwd -P)"

IMAGE="kitchen-sink-linux-check"
DOCKERFILE="$ROOT/rust/docker/linux.Dockerfile"
# Native on Apple Silicon (fast); x86_64 hosts override via the env var.
PLATFORM="${LINUX_CHECK_PLATFORM:-linux/arm64}"
TARGET_DIR="/repo/.build/linux-target"

command -v docker >/dev/null 2>&1 || { echo "linux-check: docker not found" >&2; exit 1; }
docker info >/dev/null 2>&1 || { echo "linux-check: Docker is not running (open -a Docker)" >&2; exit 1; }

echo "== docker build ($IMAGE) =="
docker build --platform "$PLATFORM" -t "$IMAGE" -f "$DOCKERFILE" "$ROOT/rust/docker"

# Mount the whole checkout; keep cargo's target under .build/ (already ignored).
_run() {
    local wd="$1"; shift
    docker run --rm --platform "$PLATFORM" \
        -v "$ROOT":/repo -w "$wd" \
        -e CARGO_TARGET_DIR="$TARGET_DIR" \
        "$IMAGE" "$@"
}

case "${1:-check}" in
    check)
        _run /repo/rust cargo check --workspace --all-targets
        ;;
    test)
        shift || true
        _run /repo/rust cargo test -p ws-rs -- "$@"
        # The plain-script suites with no `cargo test` case of their own.
        for t in Tests/test_config_text.py Tests/test_file_ops.py \
                 Tests/test_jsonmgr.py Tests/test_snippet_render.py; do
            echo "== python3 $t =="
            _run /repo python3 "$t"
        done
        ;;
    shell)
        docker run --rm -it --platform "$PLATFORM" \
            -v "$ROOT":/repo -w /repo/rust \
            -e CARGO_TARGET_DIR="$TARGET_DIR" "$IMAGE" bash
        ;;
    *)
        echo "usage: bin/linux-check.sh check|test [filter]|shell" >&2
        exit 2
        ;;
esac
