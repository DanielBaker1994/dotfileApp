#!/usr/bin/env bash
# build.sh — build (and relaunch) the workspace switcher.
#   ./build.sh           build + relaunch the notes window (only if sources changed)
#   ./build.sh --force   force rebuild even if nothing changed
#   ./build.sh --build-only   build + re-grant TCC, but DON'T launch
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [ "${1:-}" = "--force" ]; then
    touch "$DIR/workspace_switcher.swift" "$DIR/PopupWindow.swift" "$DIR/main.swift"
    shift
fi
if [ "${1:-}" = "--build-only" ]; then
    export WS_BUILD_ONLY=1
fi
exec "$DIR/bin/workspace_switcher.sh" notes
