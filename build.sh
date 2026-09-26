#!/usr/bin/env bash
# build.sh — build (and relaunch) the workspace switcher.
#   ./build.sh           build + relaunch the notes window (only if sources changed)
#   ./build.sh --force   force rebuild even if nothing changed
#   ./build.sh --build-only   build + re-grant TCC, but DON'T launch
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# the launcher only builds on a cold start (hotkeys never pay for the
# stale check), so build here: build-app.sh kills the old daemon after a
# new binary, and the launcher below starts the fresh one
if [ "${1:-}" = "--force" ]; then
    "$DIR/bin/build-app.sh" --force || exit 1
    shift
elif [ "${1:-}" != "--build-only" ]; then
    "$DIR/bin/build-app.sh" || exit 1
fi
if [ "${1:-}" = "--build-only" ]; then
    export WS_BUILD_ONLY=1
fi
exec "$DIR/bin/workspace_switcher.sh" notes
