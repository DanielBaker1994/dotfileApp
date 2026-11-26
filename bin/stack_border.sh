#!/bin/bash
# Colour the focused-window border orange when the focused workspace holds more
# than one window (an accordion stack), white when it is alone. Run from
# AeroSpace's on-focus-changed / exec-on-workspace-change.
n=$(/opt/homebrew/bin/aerospace list-windows --workspace focused --count 2>/dev/null) || exit 0
if [ "${n:-0}" -gt 1 ]; then c=0xfff5a97f; else c=0xffcad3f5; fi
/opt/homebrew/bin/borders active_color="$c" >/dev/null 2>&1
