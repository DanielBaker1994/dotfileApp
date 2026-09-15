#!/usr/bin/env bash
# build.sh — build and relaunch the workspace switcher. That's it.
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
touch "$DIR/workspace_switcher.swift" "$DIR/PopupWindow.swift" "$DIR/main.swift"
exec "$DIR/bin/workspace_switcher.sh" notes