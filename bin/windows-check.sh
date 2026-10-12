#!/usr/bin/env bash
# bin/windows-check.sh — compile kitchen-sink for Windows without a Windows host.
#
# Runs `cargo check --workspace --all-targets` against the
# x86_64-pc-windows-gnu target (mingw-w64 toolchain) from macOS. This is Wave 0
# of the Windows port: with the macOS deps target-gated, `cargo check` surfaces
# exactly the Unix/AppKit leaks the later waves fix.
#
#   bin/windows-check.sh check [args...]   # cargo check --workspace --all-targets
#   bin/windows-check.sh errors [args...]  # check + per-file error count table
#
# The `errors` table (`.build/windows-errors.txt`) is the port's work queue.
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
ROOT="$(cd "$DIR/.." && pwd -P)"

TARGET="x86_64-pc-windows-gnu"
TARGET_DIR="$ROOT/.build/windows-target"
ERRORS_FILE="$ROOT/.build/windows-errors.txt"

# Preflight: rustup's default toolchain must carry the Windows std, and the
# GNU cross compiler/linker must be on PATH.
command -v rustup >/dev/null 2>&1 || {
    echo "windows-check: rustup not found — install Rust via rustup (https://rustup.rs)" >&2; exit 1; }
if ! rustup target list --installed 2>/dev/null | grep -qx "$TARGET"; then
    echo "windows-check: the $TARGET target is not installed for the rustup toolchain." >&2
    echo "windows-check: install it with:  rustup target add $TARGET" >&2
    exit 1
fi
if ! command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
    echo "windows-check: x86_64-w64-mingw32-gcc not found (mingw-w64 linker/compiler)." >&2
    echo "windows-check: install it with:  brew install mingw-w64" >&2
    exit 1
fi

# Homebrew's cargo and rustc (/opt/homebrew/bin) are first in PATH but lack the
# Windows std and fail with E0463. `rustup run stable` pins RUSTUP_TOOLCHAIN but
# leaves PATH alone, so cargo would still spawn Homebrew's rustc; prepend the
# stable toolchain's own bin so cargo, rustc and nested shims are stable's.
TOOLCHAIN_BIN="$(dirname "$(rustup which --toolchain stable rustc)")"
export PATH="$TOOLCHAIN_BIN:$PATH"
CARGO=(rustup run stable cargo)

run_check() {
    ( cd "$ROOT/rust" \
        && CARGO_TARGET_DIR="$TARGET_DIR" "${CARGO[@]}" check \
               --workspace --all-targets --target "$TARGET" "$@" )
}

# Per-file error counts: rustc prints an `error[...]:` header, then a
# `  --> path:line:col` line naming the file. Count only the location lines
# that directly follow an error (ignore warning notes), then sort by count.
error_table() {
    awk '
        /^error/ { want = 1; next }
        want && /^[[:space:]]*--> / {
            f = $0
            sub(/^[[:space:]]*--> /, "", f)   # strip the arrow
            sub(/:[0-9].*$/, "", f)           # strip :line:col
            count[f]++
            want = 0
        }
        END { for (f in count) printf "%d\t%s\n", count[f], f }
    ' "$1" | sort -rn
}

case "${1:-check}" in
    check)
        shift || true
        run_check "$@"
        ;;
    errors)
        shift || true
        raw="$(mktemp "${TMPDIR:-/tmp}/windows-check.XXXXXX")"
        set +e
        run_check "$@" >"$raw" 2>&1
        status=$?
        set -e
        cat "$raw"
        mkdir -p "$(dirname "$ERRORS_FILE")"
        error_table "$raw" >"$ERRORS_FILE"
        rm -f "$raw"
        echo ""
        echo "== per-file Windows error counts (saved to .build/windows-errors.txt) =="
        if [ -s "$ERRORS_FILE" ]; then
            awk -F'\t' '{ printf "%5d  %s\n", $1, $2 }' "$ERRORS_FILE"
        else
            echo "  (no errors)"
        fi
        exit "$status"
        ;;
    *)
        echo "usage: bin/windows-check.sh check|errors [cargo args...]" >&2
        exit 2
        ;;
esac
