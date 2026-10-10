#!/bin/bash
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
grep -q '^# >>> monitor-layout' "$tmp" && grep -q '^# <<< monitor-layout' "$tmp" \
    && grep -q "^# mode: $mode" "$tmp" \
    || { echo "aerospace-monitors: rewrite failed, $CONF left as is" >&2; exit 1; }
cat "$tmp" >"$CONF"

command -v aerospace >/dev/null 2>&1 || { echo "$mode (aerospace not running)"; exit 0; }
aerospace reload-config >/dev/null 2>&1
if [ "$(aerospace list-monitors --count 2>/dev/null)" -ge 2 ] 2>/dev/null; then
    orig=$(aerospace list-workspaces --focused 2>/dev/null)
    other=$orig; [ "$orig" = 9 ] || [ -z "$orig" ] && other=1
    aerospace workspace 9 && aerospace workspace "$other"
    [ -n "$orig" ] && aerospace workspace "$orig"
fi
echo "$mode: $desc"
