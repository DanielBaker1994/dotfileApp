#!/usr/bin/env bash
# oil.sh — the Oil terminal window (Ghostty + nvim + Oil).
#
#   oil.sh          toggle: show (move to current workspace + focus) /
#                   hide (park on the dedicated "oil" workspace, out of view)
#   oil.sh park     launch once if missing, then park it hidden (startup use)
#
# The window is NEVER closed — the nvim/shell session stays alive; ESC in the
# terminal is just nvim's normal mode, nothing kills it.
set -u
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"

WS_PARK="oil"

find_oil() {
    aerospace list-windows --all --format '%{window-id}|%{app-name}|%{window-title}|%{workspace}' 2>/dev/null \
        | awk -F'|' '$2=="Ghostty" && $3 ~ /^Oil/ {print $1"|"$4; exit}'
}

CUR=$(aerospace list-workspaces --focused 2>/dev/null | head -1)
[ -n "$CUR" ] || exit 1

LINE=$(find_oil)
WID="${LINE%%|*}"

if [ -z "$WID" ]; then
    # Launch the ghostty BINARY directly (not `open -a`, which hands the new
    # window off to a running instance that currently falls back to a config
    # error); the OSC title sets the window title before nvim takes over.
    GHOSTTY="/Applications/Ghostty.app/Contents/MacOS/ghostty"
    "$GHOSTTY" -e bash -lc 'printf "\033]0;Oil\007"; source ~/.bashrc && exec nvim -c "set title titlestring=Oil" -c Oil' \
        >/dev/null 2>&1 &
    sleep 1.5
    LINE=$(find_oil)
    WID="${LINE%%|*}"
    [ -n "$WID" ] || { echo "oil: window not found after launch" >&2; exit 1; }
fi

if [ "${1:-}" = "park" ]; then
    # startup use: make sure it exists, then park it out of view
    aerospace move-node-to-workspace --window-id "$WID" "$WS_PARK" >/dev/null 2>&1
    exit 0
fi

WS="${LINE##*|}"
if [ "$WS" = "$CUR" ]; then
    # visible -> hide: park on the "oil" workspace, out of view; session lives on
    aerospace move-node-to-workspace --window-id "$WID" "$WS_PARK" >/dev/null 2>&1
else
    # hidden -> show on the current workspace + focus
    aerospace move-node-to-workspace --window-id "$WID" "$CUR" >/dev/null 2>&1
    sleep 0.3
    aerospace focus --window-id "$WID" >/dev/null 2>&1
fi