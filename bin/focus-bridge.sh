#!/usr/bin/env bash
# focus-bridge.sh — aerospace on-focus-changed hook.
#
# macOS refuses to activate an accessory app from another process, so when
# AeroSpace focuses one of the workspace-switcher windows (jira/notes) the
# app never becomes active and keyboard focus stays behind. AeroSpace can't
# activate us, but it CAN tell us: this hook records the newly focused window
# id, and the daemon (which watches the file) activates itself.
#
# Deliberately minimal: one file write per focus change. No aerospace IPC, no
# polling, no sketchybar interaction — the sketchybar trigger in aerospace.toml
# is a separate, already-debounced callback.
WID="${AEROSPACE_WINDOW_ID:-}"
[ -n "$WID" ] || exit 0
printf '%s' "$WID" >"${TMPDIR:-/tmp}/ws-aerospace-focus"
