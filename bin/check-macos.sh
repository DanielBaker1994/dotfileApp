#!/usr/bin/env bash
# check-macos.sh — the ONE macOS-version gate.
#
# The app and the embedded SwiftTerm library are both built for install.conf's
# MACOS_MIN. swiftc would happily emit a binary whose minos is newer than the
# machine (its own SDK default, or MACOS_MIN on a stale checkout), and
# LaunchServices then refuses to launch it (error -10825). So every door that
# starts a build asks here first and gets a readable reason instead.
#
#   bin/check-macos.sh            exit 0 when this Mac can run the build
#   bin/check-macos.sh 27.0       explicit target (default: $MACOS_MIN)
set -uo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$DIR/.." && pwd)"
# shellcheck source=../install.conf
. "$ROOT/install.conf"

MIN="${1:-$MACOS_MIN}"
HOST="$(sw_vers -productVersion 2>/dev/null || true)"

# is "$1" >= "$2"? (up to three dotted components)
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
echo "workspace-switcher targets macOS $MIN, but this Mac runs ${HOST:-an unknown macOS}." >&2
echo "Lower MACOS_MIN in install.conf to build for this machine." >&2
exit 1
