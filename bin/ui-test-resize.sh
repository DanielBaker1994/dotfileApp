#!/usr/bin/env bash
#
# ui-test-resize.sh — drag every edge/corner of the notes window with the
# real mouse and check the resize is exact and stable:
#   * the dragged edge follows the mouse (within TOL points)
#   * the opposite edges never move (no sliding / inverted edges)
#   * frames sampled DURING the drag change monotonically (no jitter)
#   * text never zooms: the vim grid gains columns/lines as the pane grows
#   * the grab zones follow the window (a second drag after a resize works)
# Runs twice: plain (file browser drawer) and with the terminal drawer open
# as well, since layout depends on what is rendered.
#
# Screenshots of every final state go to $OUT (default /tmp/ws-resize-shots).
# Usage: bin/ui-test-resize.sh [--verbose]

set -o pipefail
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${OUT:-/tmp/ws-resize-shots}"
TOL=3            # pt tolerance for "edge follows mouse" / "edge fixed"
D=120            # drag distance (pt)
STEPS=6          # intermediate mouse positions per drag
VERBOSE=0; [[ "${1:-}" == "--verbose" ]] && VERBOSE=1
mkdir -p "$OUT"; rm -f "$OUT"/*.png

command -v cliclick >/dev/null || { echo "cliclick not found — brew install cliclick"; exit 1; }

PASS=0 FAIL=0
pass() { printf 'PASS: %s\n' "$*"; PASS=$((PASS + 1)); }
fail() { printf 'FAIL: %s\n' "$*" >&2; FAIL=$((FAIL + 1)); }
vlog() { (( VERBOSE )) && printf '  · %s\n' "$*" >&2; }

WSPID() { pgrep -x workspace-switcher | head -1; }
SOCK() { echo "$HOME/.cache/workspace-switcher/nvim-notes-$(WSPID).sock"; }
NOTESOCK="${TMPDIR%/}/ws-notes.sock"
ws_send() { printf '%s' "$1" | nc -U -w 1 "$NOTESOCK"; }
vx() { timeout 4 nvim --headless --clean --server "$(SOCK)" --remote-expr "$1" 2>/dev/null; }
front() { lsappinfo info -only name "$(lsappinfo front)" | sed -E 's/.*="(.*)"/\1/'; }

# AX frame of the notes window: x,y (top-left, y down), w,h
frame() { osascript -e 'tell application "System Events" to tell process "workspace-switcher"
  repeat with w in windows
    set s to size of w
    if item 2 of s > 200 then
      set p to position of w
      return (item 1 of p as text) & " " & (item 2 of p as text) & " " & (item 1 of s as text) & " " & (item 2 of s as text)
    end if
  end repeat
end tell' 2>/dev/null; }

abs() { local v=$1; echo $(( v < 0 ? -v : v )); }
near() { (( $(abs $(( $1 - $2 ))) <= TOL )); }

# drag from (sx,sy) by (dx,dy) in STEPS moves, sampling the frame after
# each move; prints the samples, one "x y w h" per line
drag_sample() {
    local sx=$1 sy=$2 dx=$3 dy=$4 i
    cliclick dd:"$sx,$sy"
    for i in $(seq 1 "$STEPS"); do
        cliclick dm:"$((sx + dx * i / STEPS)),$((sy + dy * i / STEPS))"
        sleep 0.05
        frame
    done
    cliclick du:"$((sx + dx)),$((sy + dy))"
    sleep 0.3
}

# one edge test. $1 name, $2 which edges ("L","R","T","B" combos), $3 dx, $4 dy
edge_test() {
    local name=$1 edges=$2 dx=$3 dy=$4 label=$5
    local x0 y0 w0 h0 sx sy
    read -r x0 y0 w0 h0 <<<"$(frame)"
    # grab point: 1pt inside the edge — inside the system's resize band
    # (where the resize cursor shows; ~3pt outside to ~2pt inside)
    sx=$((x0 + w0 / 2)); sy=$((y0 + h0 / 2))
    [[ $edges == *L* ]] && sx=$((x0 + 1))
    [[ $edges == *R* ]] && sx=$((x0 + w0 - 1))
    [[ $edges == *T* ]] && sy=$((y0 + 1))
    [[ $edges == *B* ]] && sy=$((y0 + h0 - 1))
    local cols0; cols0="$(vx '&columns')"; local lines0; lines0="$(vx '&lines')"
    local samples; samples="$(drag_sample "$sx" "$sy" "$dx" "$dy")"
    vlog "$label $name start=($x0 $y0 $w0 $h0) samples:"; (( VERBOSE )) && echo "$samples" | sed 's/^/      /' >&2
    local x1 y1 w1 h1
    read -r x1 y1 w1 h1 <<<"$(frame)"
    # expected final frame (AX coords: y grows downward)
    local ex=$x0 ey=$y0 ew=$w0 eh=$h0
    [[ $edges == *R* ]] && ew=$((w0 + dx))
    [[ $edges == *L* ]] && { ex=$((x0 + dx)); ew=$((w0 - dx)); }
    [[ $edges == *B* ]] && eh=$((h0 + dy))
    [[ $edges == *T* ]] && { ey=$((y0 + dy)); eh=$((h0 - dy)); }
    local ok=1 why=""
    near "$x1" "$ex" || { ok=0; why+=" x=$x1!=$ex"; }
    near "$y1" "$ey" || { ok=0; why+=" y=$y1!=$ey"; }
    near "$w1" "$ew" || { ok=0; why+=" w=$w1!=$ew"; }
    near "$h1" "$eh" || { ok=0; why+=" h=$h1!=$eh"; }
    (( ok )) && pass "$label $name: edge follows the mouse, opposite edges fixed" \
             || fail "$label $name:$why (start $x0,$y0 ${w0}x$h0)"
    # monotonic: every sample's width/height moves in one direction only
    local prevw=$w0 prevh=$h0 jitter=0 sw sh
    while read -r _ _ sw sh; do
        [[ -z "$sw" ]] && continue
        if (( (ew - w0) >= 0 ? sw < prevw - 1 : sw > prevw + 1 )); then jitter=1; fi
        if (( (eh - h0) >= 0 ? sh < prevh - 1 : sh > prevh + 1 )); then jitter=1; fi
        prevw=$sw; prevh=$sh
    done <<<"$samples"
    (( jitter )) && fail "$label $name: size jittered during the drag" \
                 || pass "$label $name: no jitter during the drag"
    # no zoom: a wider pane must hold MORE columns, taller more lines
    local cols1 lines1; cols1="$(vx '&columns')"; lines1="$(vx '&lines')"
    if [[ -n "$cols0" && -n "$cols1" ]]; then
        local okz=1
        (( ew > w0 + 40 )) && (( cols1 <= cols0 )) && okz=0
        (( ew < w0 - 40 )) && (( cols1 >= cols0 )) && okz=0
        (( eh > h0 + 40 )) && (( lines1 <= lines0 )) && okz=0
        (( eh < h0 - 40 )) && (( lines1 >= lines0 )) && okz=0
        (( okz )) && pass "$label $name: text keeps its size (cols $cols0->$cols1, lines $lines0->$lines1)" \
                  || fail "$label $name: text zoomed instead of reflowing (cols $cols0->$cols1, lines $lines0->$lines1)"
    fi
    screencapture -x -R "$x1,$y1,$w1,$h1" "$OUT/${label}-${name}.png" 2>/dev/null
}

run_suite() {
    local label=$1
    echo "== $label =="
    # every edge out and back, then corners out and back
    edge_test right  R  "$D" 0 "$label";   edge_test right-back  R  "-$D" 0 "$label"
    edge_test left   L  "-$D" 0 "$label";  edge_test left-back   L  "$D" 0 "$label"
    edge_test bottom B  0 "$D" "$label";   edge_test bottom-back B  0 "-$D" "$label"
    edge_test top    T  0 "-$D" "$label";  edge_test top-back    T  0 "$D" "$label"
    edge_test corner-br RB "$D" "$D" "$label"; edge_test corner-br-back RB "-$D" "-$D" "$label"
    edge_test corner-tl LT "-$D" "-$D" "$label"; edge_test corner-tl-back LT "$D" "$D" "$label"
}

# --- setup ---------------------------------------------------------------------
if [[ -z "$(WSPID)" ]]; then
    ("$ROOT/bin/workspace_switcher.sh" notes >/dev/null 2>&1 &)
    for _ in $(seq 1 60); do [[ -n "$(WSPID)" ]] && break; sleep 1; done
fi
ws_send notes; sleep 1.5
[[ "$(front)" == "workspace-switcher" ]] || { echo "notes window not frontmost"; exit 1; }
# start from a known size in the middle of the screen
read -r X Y W H <<<"$(frame)"
[[ -n "$W" ]] || { echo "no notes window"; exit 1; }
echo "start frame: $X,$Y ${W}x$H"

# screen limit: dragging the top edge past the menu bar stops AT the
# menu bar — the bottom edge must not move (no sliding window)
screen_limit_test() {
    local label=$1 x0 y0 w0 h0 x1 y1 w1 h1
    read -r x0 y0 w0 h0 <<<"$(frame)"
    local vis_top; vis_top="$(osascript -l JavaScript -e 'ObjC.import("AppKit"); var s=$.NSScreen.mainScreen; (s.frame.size.height - s.visibleFrame.origin.y - s.visibleFrame.size.height)')"
    drag_sample "$((x0 + w0 / 2))" "$((y0 + 1))" 0 "$(( -(y0 - vis_top) - 80 ))" >/dev/null
    read -r x1 y1 w1 h1 <<<"$(frame)"
    if near "$y1" "$vis_top" && near "$((y1 + h1))" "$((y0 + h0))"; then
        pass "$label top past the menu bar: stops at the menu bar, bottom fixed"
    else
        fail "$label top past the menu bar: y=$y1 (want $vis_top) bottom=$((y1 + h1)) (want $((y0 + h0)))"
    fi
    drag_sample "$((x1 + w1 / 2))" "$((y1 + 1))" 0 "$((y0 - y1))" >/dev/null
}

# no drift: repeated shrink-to-tiny / grow-back leaves the exact same layout
drift_test() {
    local label=$1 x0 y0 w0 h0 lines0 i
    read -r x0 y0 w0 h0 <<<"$(frame)"
    lines0="$(vx '&lines')"
    for i in 1 2 3; do
        drag_sample "$((x0 + w0 / 2))" "$((y0 + h0 - 1))" 0 -400 >/dev/null
        read -r _ _ _ hs <<<"$(frame)"
        drag_sample "$((x0 + w0 / 2))" "$((y0 + hs - 1))" 0 "$((h0 - hs))" >/dev/null
    done
    local x1 y1 w1 h1; read -r x1 y1 w1 h1 <<<"$(frame)"
    local lines1; lines1="$(vx '&lines')"
    if near "$h1" "$h0" && [[ "$lines1" == "$lines0" ]]; then
        pass "$label 3x shrink/grow: no drift (h $h0, vim lines $lines0)"
    else
        fail "$label 3x shrink/grow drifted: h $h0->$h1, vim lines $lines0->$lines1"
    fi
    screencapture -x -R "$x1,$y1,$w1,$h1" "$OUT/${label}-after-drift.png" 2>/dev/null
}

ws_send reset-size; sleep 0.8
run_suite plain
screen_limit_test plain

# same drags with the terminal drawer open too (browser + terminal + vim):
# layout depends on what is rendered
read -r _ _ _ HB <<<"$(frame)"
ws_send "toggle-terminal"; sleep 1
read -r _ _ _ HT <<<"$(frame)"
ws_send reset-size; sleep 0.8
if (( HT > HB )); then
    run_suite drawers
    drift_test drawers
    ws_send "toggle-terminal"; sleep 0.8
else
    echo "SKIP: could not open the terminal drawer"
fi

echo
echo "resize: $PASS passed, $FAIL failed (screenshots: $OUT)"
(( FAIL == 0 ))
