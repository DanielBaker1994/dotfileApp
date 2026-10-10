#!/usr/bin/env bash
set -uo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$DIR/.." && pwd)"
. "$ROOT/install.conf"

MIN="${1:-$MACOS_MIN}"
HOST="$(sw_vers -productVersion 2>/dev/null || true)"

ver_ge() {
    awk -v a="$1" -v b="$2" 'BEGIN {
        split(a, A, "."); split(b, B, ".")
        for (i = 1; i <= 3; i++) {
            if ((A[i] + 0) > (B[i] + 0)) exit 0
            if ((A[i] + 0) < (B[i] + 0)) exit 1
        }
        exit 0
    }'
}

[ -n "$MIN" ] || { echo "check-macos: no target version (set MACOS_MIN in install.conf)" >&2; exit 2; }
if ver_ge "$HOST" "$MIN"; then
    exit 0
fi
echo "kitchen-sink targets macOS $MIN, but this Mac runs ${HOST:-an unknown macOS}." >&2
echo "Lower MACOS_MIN in install.conf to build for this machine." >&2
exit 1
