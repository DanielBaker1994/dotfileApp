#!/usr/bin/env bash
# build.sh — build (and relaunch) the workspace switcher.
#   ./build.sh           build + relaunch the notes window
#   ./build.sh --build-only   build + re-grant TCC, but DON'T launch
#                             (the installer uses this — it ends on Done)
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
touch "$DIR/workspace_switcher.swift" "$DIR/PopupWindow.swift" "$DIR/main.swift"
if [ "${1:-}" = "--build-only" ]; then
    export WS_BUILD_ONLY=1
fi
exec "$DIR/bin/workspace_switcher.sh" notes