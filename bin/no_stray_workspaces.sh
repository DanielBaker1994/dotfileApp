#!/opt/homebrew/bin/bash
# Guard: never let a monitor sit on a workspace outside ALLOWED (e.g. "10").
#
# AeroSpace invents a numbered workspace ("10", "11", ...) when a monitor
# appears and no invisible workspace is force-assigned to it (see
# getStubWorkspace in AeroSpace's tree/Workspace.swift). There is no config
# switch to disable that, so this script undoes it immediately: windows are
# moved to a real workspace and the monitor is switched to an allowed one.
# An emptied, invisible stray is garbage-collected by AeroSpace.
#
# Triggered by: sketchybar display_change/system_woke (monitor plug/unplug,
# wake) and aerospace exec-on-workspace-change. Fast path = one aerospace call.

ALLOWED=(M Y W 1 2 3 4 5 6 7 8 9)
is_allowed() { case " ${ALLOWED[*]} " in *" $1 "*) return 0 ;; esac; return 1; }

# Display events fire before AeroSpace has placed the new monitor's workspace.
case "${SENDER:-}" in display_change|system_woke) sleep 1.5 ;; esac

# --all = every existing workspace (visible or holding windows).
stray=0
while read -r ws; do
    [ -n "$ws" ] && ! is_allowed "$ws" && stray=1
done < <(aerospace list-workspaces --all 2>/dev/null)
[ "$stray" = 0 ] && exit 0

# Serialize: display changes fire in bursts (mkdir = atomic lock on macOS).
LOCK="${TMPDIR:-/tmp}/no_stray_workspaces.lock"
for _ in 1 2 3 4 5 6 7 8 9 10; do mkdir "$LOCK" 2>/dev/null && break; sleep 0.2; done
trap 'rmdir "$LOCK" 2>/dev/null' EXIT

focused_ws=$(aerospace list-workspaces --focused 2>/dev/null)
visible=" $(aerospace list-workspaces --monitor all --visible 2>/dev/null | tr '\n' ' ') "

# First allowed, invisible workspace assigned to monitor $1. None -> phantom
# monitor with nothing of ours assigned: don't steal another screen's workspace.
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
        # phantom screen: evacuate windows to the user's workspace, leave it empty
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

# Hidden strays that still hold windows: send them to the focused workspace.
for ws in $(aerospace list-workspaces --all 2>/dev/null); do
    is_allowed "$ws" && continue
    for wid in $(aerospace list-windows --workspace "$ws" --format '%{window-id}' 2>/dev/null); do
        aerospace move-node-to-workspace --window-id "$wid" "${focused_ws:-1}"
    done
done

# Restore the user's focus if it was on an allowed workspace.
is_allowed "$focused_ws" && aerospace workspace "$focused_ws"
sketchybar --trigger aerospace_workspace_change 2>/dev/null
exit 0
