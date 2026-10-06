#!/bin/bash
# aerospace-monitors.sh — which screen owns which workspaces.
#
#   aerospace-monitors.sh main      1-8 + the letters on the MAIN monitor,
#                                   9 on the other one (the default)
#   aerospace-monitors.sh inverse   1-8 + the letters on the SECONDARY
#                                   monitor, 9 on the main one
#   aerospace-monitors.sh toggle    flip between the two
#   aerospace-monitors.sh status    print the current mode
#
# 9 always gets a screen of its own: when a second monitor appears AeroSpace
# shows the workspace force-assigned to it (9) instead of inventing one
# ("10") — see bin/no_stray_workspaces.sh. With one screen everything falls
# back to it. "main" = the display with the menu bar (System Settings ▸
# Displays), "secondary" = the other one of exactly two.
#
# Rewrites the block between the `# >>> monitor-layout` / `# <<< monitor-layout`
# markers of aerospace.toml (written THROUGH the ~/.config link, so the
# repo copy changes), reloads AeroSpace and re-places the visible workspaces.
# Bound in aerospace.toml: service mode (alt-shift-;) then m.
set -u
CONF="${AEROSPACE_CONF:-$HOME/.config/aerospace/aerospace.toml}"
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"

[ -f "$CONF" ] || { echo "aerospace-monitors: no $CONF" >&2; exit 1; }
grep -q '^# >>> monitor-layout' "$CONF" || {
    echo "aerospace-monitors: $CONF has no '# >>> monitor-layout' block" >&2; exit 1; }

current() { sed -n 's/^# mode: *\([a-z]*\).*/\1/p' "$CONF" | head -1; }

mode="${1:-status}"
case "$mode" in
    status) echo "$(current)"; exit 0 ;;
    toggle) [ "$(current)" = inverse ] && mode=main || mode=inverse ;;
    main|inverse) ;;
    *) echo "usage: aerospace-monitors.sh main|inverse|toggle|status" >&2; exit 2 ;;
esac

if [ "$mode" = main ]; then
    rest="['main']"; nine="['secondary', 'main']"
    desc="1-8 + letters on the main monitor, 9 on the secondary"
else
    rest="['secondary', 'main']"; nine="['main']"
    desc="1-8 + letters on the secondary monitor, 9 on the main"
fi

# the workspaces the block assigns today (its keys), 9 handled on its own
names=$(awk '/^# >>> monitor-layout/{on=1;next} /^# <<< monitor-layout/{on=0} on && /^[A-Za-z0-9]+ *=/{print $1}' "$CONF")
[ -n "$names" ] || names="1 2 3 4 5 6 7 8 9"

tmp=$(mktemp) blk=$(mktemp)
trap 'rm -f "$tmp" "$blk"' EXIT
{
    echo "# mode: $mode  ($desc)"
    echo "[workspace-to-monitor-force-assignment]"
    for ws in $names; do
        [ "$ws" = 9 ] && continue
        echo "$ws = $rest"
    done
    echo "9 = $nine"
} >"$blk"

awk -v blk="$blk" '
    /^# >>> monitor-layout/ { print; while ((getline l < blk) > 0) print l; skip=1; next }
    /^# <<< monitor-layout/ { skip=0 }
    !skip
' "$CONF" >"$tmp"
# never write a broken result over the config: both markers + the new mode
grep -q '^# >>> monitor-layout' "$tmp" && grep -q '^# <<< monitor-layout' "$tmp" \
    && grep -q "^# mode: $mode" "$tmp" \
    || { echo "aerospace-monitors: rewrite failed, $CONF left as is" >&2; exit 1; }
# write THROUGH the link (mv would replace the ~/.config symlink with a copy)
cat "$tmp" >"$CONF"

command -v aerospace >/dev/null 2>&1 || { echo "$mode (aerospace not running)"; exit 0; }
aerospace reload-config >/dev/null 2>&1
# re-place what is on screen: a force-assigned workspace moves to its monitor
# when it is focused — 9 first, then the user's workspace (or 1 when that was 9)
if [ "$(aerospace list-monitors --count 2>/dev/null)" -ge 2 ] 2>/dev/null; then
    orig=$(aerospace list-workspaces --focused 2>/dev/null)
    other=$orig; [ "$orig" = 9 ] || [ -z "$orig" ] && other=1
    aerospace workspace 9 && aerospace workspace "$other"
    [ -n "$orig" ] && aerospace workspace "$orig"
fi
echo "$mode: $desc"
