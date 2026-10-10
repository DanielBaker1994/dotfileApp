#!/opt/homebrew/bin/bash
set -uo pipefail

ALLOWED=(G M Y W N 1 2 3 4 5 6 7 8 9)
is_allowed() { case " ${ALLOWED[*]} " in *" $1 "*) return 0 ;; esac; return 1; }

case "${SENDER:-}" in display_change|system_woke) sleep 1.5 ;; esac

stray=0
while read -r ws; do
    [ -n "$ws" ] && ! is_allowed "$ws" && stray=1
done < <(aerospace list-workspaces --all 2>/dev/null)
[ "$stray" = 0 ] && exit 0

LOCK="${TMPDIR:-/tmp}/no_stray_workspaces.lock"
for _ in 1 2 3 4 5 6 7 8 9 10; do mkdir "$LOCK" 2>/dev/null && break; sleep 0.2; done
trap 'rmdir "$LOCK" 2>/dev/null' EXIT

focused_ws=$(aerospace list-workspaces --focused 2>/dev/null)
visible=" $(aerospace list-workspaces --monitor all --visible 2>/dev/null | tr '\n' ' ') "

pick_target() {
    local mon=$1 ws
    for ws in $(aerospace list-workspaces --monitor "$mon" 2>/dev/null); do
        is_allowed "$ws" || continue
        case "$visible" in *" $ws "*) continue ;; esac
        echo "$ws"; return
    done
}

for mon in $(aerospace list-monitors --format '%{monitor-id}' 2>/dev/null); do
    ws=$(aerospace list-workspaces --monitor "$mon" --visible 2>/dev/null)
    [ -z "$ws" ] || is_allowed "$ws" && continue
    target=$(pick_target "$mon")
    if [ -z "$target" ]; then
        for wid in $(aerospace list-windows --workspace "$ws" --format '%{window-id}' 2>/dev/null); do
            aerospace move-node-to-workspace --window-id "$wid" "${focused_ws:-1}"
        done
        logger -t no_stray_workspaces "monitor $mon: '$ws' has no assigned workspace (phantom?), evacuated"
        continue
    fi
    for wid in $(aerospace list-windows --workspace "$ws" --format '%{window-id}' 2>/dev/null); do
        aerospace move-node-to-workspace --window-id "$wid" "$target"
    done
    aerospace focus-monitor "$mon" && aerospace workspace "$target"
    visible+="$target "
    logger -t no_stray_workspaces "monitor $mon: '$ws' -> '$target'"
done

for ws in $(aerospace list-workspaces --all 2>/dev/null); do
    is_allowed "$ws" && continue
    for wid in $(aerospace list-windows --workspace "$ws" --format '%{window-id}' 2>/dev/null); do
        aerospace move-node-to-workspace --window-id "$wid" "${focused_ws:-1}"
    done
done

is_allowed "$focused_ws" && aerospace workspace "$focused_ws"
exit 0
