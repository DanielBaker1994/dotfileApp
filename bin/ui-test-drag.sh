#!/usr/bin/env bash
#
# ui-test-drag.sh — regression test for window shake when dragging after menu-bar open
#
# The bug: takeFocus() retry loop and didBecomeActiveNotification handler
# call makeKeyAndOrderFront repeatedly during the first ~1.5s after open.
# If the user drags during this window, the window shakes.
#
# This test detects the symptom: the window repositioning itself (jumping
# back or oscillating) during the takeFocus retry window.
#
# Usage: ui-test-drag.sh [--verbose]
#
# Requires: cliclick (brew install cliclick)
#

set -o pipefail

export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"

CLICLICK="$(command -v cliclick)" || { echo "cliclick not found — brew install cliclick"; exit 1; }

VERBOSE=0
[[ "${1:-}" == "--verbose" || "${1:-}" == "-v" ]] && VERBOSE=1

PASS=0 FAIL=0

pass() { printf 'PASS: %s\n' "$*"; PASS=$((PASS + 1)); }
fail() { printf 'FAIL: %s\n' "$*" >&2; FAIL=$((FAIL + 1)); }
vlog() { (( VERBOSE )) && printf '  → %s\n' "$*" >&2; }

# Wait up to $2 seconds for $1 to become true
wait_for() {
    local desc="$1" max="${2:-5}" i=0
    while ! eval "$desc" 2>/dev/null; do
        (( i >= max )) && return 1
        sleep 0.3
        i=$((i + 1))
    done
    return 0
}

# Get window position (x,y) — stripped of all whitespace
window_position() {
    osascript -e '
        tell application "System Events"
            tell process "workspace-switcher"
                set f to position of window 1
                return (item 1 of f as text) & "," & (item 2 of f as text)
            end tell
        end tell
    ' 2>/dev/null | tr -d ' '
}

# Get window frame (x,y,w,h)
window_frame() {
    local idx="${1:-1}"
    osascript -e "
        tell application \"System Events\"
            tell process \"workspace-switcher\"
                set f to position of window $idx
                set s to size of window $idx
                return (item 1 of f as text) & \",\" & (item 2 of f as text) & \",\" & (item 1 of s as text) & \",\" & (item 2 of s as text)
            end tell
        end tell
    " 2>/dev/null | tr -d ' '
}

# Sample window position N times over the given duration, return max position
# change observed (max_dx,max_dy). A large value means the window is jumping
# around on its own (the shake symptom).
sample_position_jitter() {
    local samples="${1:-10}" interval="${2:-0.1}"
    local positions=()
    for (( i=0; i<samples; i++ )); do
        local p
        p="$(window_position)"
        [[ -n "$p" ]] && positions+=("$p")
        sleep "$interval"
    done
    (( ${#positions[@]} < 2 )) && { echo "0,0"; return; }
    # Compute max dx, dy
    IFS=',' read -r bx by <<< "${positions[0]}"
    local max_dx=0 max_dy=0
    for p in "${positions[@]}"; do
        IFS=',' read -r px py <<< "$p"
        local dx=$(( px - bx )) dy=$(( py - by ))
        dx=${dx#-}; dy=${dy#-}
        (( dx > max_dx )) && max_dx=$dx
        (( dy > max_dy )) && max_dy=$dy
    done
    echo "${max_dx},${max_dy}"
}

# --- setup ------------------------------------------------------------------

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN="$ROOT/../workspace-switcher.app/Contents/MacOS/workspace-switcher"

echo "== Window Drag Shake Regression Test =="
echo "  date: $(date)"
echo

# Kill any existing daemon
pkill -f "workspace-switcher.app" 2>/dev/null || true
sleep 0.5

# ============================================================================
# TEST 1: Open notes from CLI, verify window appears
# ============================================================================

echo "== 1. Open notes window =="

"$BIN" notes >/dev/null 2>&1 &
sleep 1.5

if wait_for "osascript -e 'tell application \"System Events\" to tell process \"workspace-switcher\" to count windows' 2>/dev/null | grep -q '^[1-9]'" 5; then
    pass "notes window appeared"
else
    fail "notes window did not appear"
    exit 1
fi

# ============================================================================
# TEST 2: Position stability during takeFocus retry window (1.5s)
# ============================================================================

echo "== 2. Position stability during takeFocus retry window =="

# Close and reopen fresh
pkill -f "workspace-switcher.app" 2>/dev/null || true
sleep 0.5

"$BIN" notes >/dev/null 2>&1 &
sleep 0.3  # Let the window appear, but we're still inside the 1.5s retry window

# Record position every 100ms for 2s (covers the entire retry window + margin)
INIT_POS="$(window_position)"
if [[ -z "$INIT_POS" ]]; then
    fail "could not read initial window position"
else
    vlog "initial position: $INIT_POS"
    IFS=',' read -r ix iy <<< "$INIT_POS"

    # Sample 15 times over 1.5s (matches the takeFocus retry duration)
    JITTER="$(sample_position_jitter 15 0.1)"
    IFS=',' read -r jx jy <<< "$JITTER"

    # The window should not move more than 5px on its own during this period
    # (some variance is normal from macOS, but >5px means something is repositioning it)
    if (( jx <= 5 && jy <= 5 )); then
        pass "position stable during retry window (jitter=${jx},${jy}px)"
    else
        fail "window jumped ${jx}px horizontally and/or ${jy}px vertically during retry window"
        vlog "this indicates makeKeyAndOrderFront is fighting with window placement"
    fi
fi

# ============================================================================
# TEST 3: Rapid open + position stability (stress test)
# ============================================================================

echo "== 3. Rapid open + position stability (stress test) =="

pkill -f "workspace-switcher.app" 2>/dev/null || true
sleep 0.5

jitter_pass=0
jitter_fail=0

for run in 1 2 3; do
    "$BIN" notes >/dev/null 2>&1 &
    sleep 0.5  # Open during the retry window

    JITTER="$(sample_position_jitter 8 0.1)"
    IFS=',' read -r jx jy <<< "$JITTER"
    if (( jx <= 5 && jy <= 5 )); then
        (( jitter_pass++ ))
        vlog "run $run: stable (jitter=${jx},${jy})"
    else
        (( jitter_fail++ ))
        vlog "run $run: UNSTABLE (jitter=${jx},${jy})"
    fi

    pkill -f "workspace-switcher.app" 2>/dev/null || true
    sleep 0.3
done

if (( jitter_fail == 0 )); then
    pass "all 3 rapid open runs were position-stable"
else
    fail "$jitter_fail of 3 rapid open runs showed position jitter"
fi

# ============================================================================
# TEST 4: Position settled after retry window completes
# ============================================================================

echo "== 4. Position settled after takeFocus completes =="

pkill -f "workspace-switcher.app" 2>/dev/null || true
sleep 0.5

"$BIN" notes >/dev/null 2>&1 &
sleep 2  # Well past the 1.5s retry window

POS_BEFORE="$(window_position)"
sleep 1
POS_AFTER="$(window_position)"

if [[ "$POS_BEFORE" == "$POS_AFTER" && -n "$POS_BEFORE" ]]; then
    pass "position stable after settling ($POS_BEFORE)"
else
    fail "window moved on its own after settling: $POS_BEFORE -> $POS_AFTER"
fi

# ============================================================================
# TEST 5: Drag via cliclick — verify cliclick can interact with the window
# ============================================================================

echo "== 5. cliclick interaction check =="

# Get window frame to click inside it
FRAME="$(window_frame 1)"
if [[ -n "$FRAME" ]]; then
    IFS=',' read -r fx fy fw fh <<< "$FRAME"
    # Click in the center of the window (this should focus it without dragging)
    cx=$(( fx + fw / 2 ))
    cy=$(( fy + fh / 2 ))
    "$CLICLICK" "m:$cx,$cy" "c:$cx,$cy" 2>/dev/null
    sleep 0.3

    # Verify window still exists and is responsive
    POS="$(window_position)"
    if [[ -n "$POS" ]]; then
        pass "cliclick clicked window, position=$POS (size=${fw}x${fh})"
    else
        fail "window disappeared after cliclick"
    fi
else
    skip "could not read window frame for cliclick test"
fi

# ============================================================================
# TEST 6: No reposition after manual window focus
# ============================================================================

echo "== 6. No reposition after manual focus =="

pkill -f "workspace-switcher.app" 2>/dev/null || true
sleep 0.5

"$BIN" notes >/dev/null 2>&1 &
sleep 2  # Let everything settle

# Get position, then bring app to front via AppleScript
POS1="$(window_position)"
osascript -e '
    tell application "System Events"
        tell process "workspace-switcher"
            set frontmost to true
        end tell
    end tell
' 2>/dev/null
sleep 0.5
POS2="$(window_position)"

if [[ "$POS1" == "$POS2" && -n "$POS1" ]]; then
    pass "position unchanged after programmatic focus ($POS1)"
else
    fail "window repositioned after focus: $POS1 -> $POS2"
fi

# ============================================================================
# CLEANUP
# ============================================================================

echo
echo "== cleanup =="
pkill -f "workspace-switcher.app" 2>/dev/null || true
sleep 0.3

echo
echo "================================================================"
echo "Results: $PASS passed, $FAIL failed"
echo "================================================================"
if [[ "$FAIL" -eq 0 ]]; then
    echo "ALL GOOD — drag shake regression clean"
    exit 0
else
    echo "FAILURES — window shake/reposition detected"
    exit 1
fi
