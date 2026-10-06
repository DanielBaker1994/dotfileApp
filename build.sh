#!/usr/bin/env bash
# build.sh — deprecated shim for `./ws build` (kept for old docs / scripts).
#   ./build.sh            build + relaunch
#   ./build.sh --force    force rebuild
#   ./build.sh --build-only   build + re-grant TCC, do NOT launch
set -uo pipefail
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)/bin/lib.sh"
ws_cmd_build "$@"
