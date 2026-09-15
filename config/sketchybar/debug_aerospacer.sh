#!/usr/bin/env bash
# Toggle + inspect debug logging for aerospacer.sh (sketchybar workspace pills).
#
# Usage:
#   debug_aerospacer.sh on|off|tail|count|stats
#
# The script writes one line per invocation to
#   ~/.cache/sketchybar/aerospacer.log
# with columns: epoch ts | wallclock | SENDER | NAME | FOCUSED_WORKSPACE | PREV_WORKSPACE | note

CACHE="${AEROSPACER_DEBUG_DIR:-$HOME/.cache/sketchybar}"
FLAG="$CACHE/debug"
LOG="${AEROSPACER_DEBUG_LOG:-$CACHE/aerospacer.log}"

mkdir -p "$CACHE"

case "${1:-stats}" in
on)
    touch "$FLAG"
    echo "debug on (flag: $FLAG, log: $LOG)"
    ;;
off)
    rm -f "$FLAG"
    echo "debug off"
    ;;
tail)
    tail -f "$LOG"
    ;;
count)
    [ -f "$LOG" ] && wc -l < "$LOG" || echo 0
    ;;
stats)
    [ -f "$LOG" ] || { echo "no log yet — enable with: $0 on"; exit 0; }
    echo "== total invocations: $(wc -l < "$LOG")"
    echo
    echo "== by sender (event that fired the script):"
    awk -F'\t' '{print $3}' "$LOG" | sort | uniq -c | sort -rn
    echo
    echo "== by name (item / phase):"
    awk -F'\t' '{print $4}' "$LOG" | sort | uniq -c | sort -rn
    echo
    echo "== invocation rate (last 10 min vs overall):"
    now=$(date +%s)
    awk -F'\t' -v now="$now" '{ if (now - $1 <= 600) recent++ } END { print recent " in last 10 min" }' "$LOG"
    echo
    echo "== last 15 lines:"
    tail -n 15 "$LOG"
    ;;
*)
    echo "usage: $0 on|off|tail|count|stats"
    exit 1
    ;;
esac