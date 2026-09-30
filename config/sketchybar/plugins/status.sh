#!/usr/bin/env bash
# Right-side status group: notification chips (notifications.sh) · CPU · RAM · battery · YYYY-MM-DD HH:MM, in the same bordered
# group look as the workspace group (GROUP_* in colors.sh).
# Sourced by sketchybarrc → builds the items; run as each item's script
# (sketchybar sets $NAME) → updates that one item.

source "$HOME/.config/sketchybar/colors.sh"

STATUS_SCRIPT="$HOME/.config/sketchybar/plugins/status.sh"
STATUS_TAG_COLOR=0xff939ab7
STATUS_TAG_FONT="SF Pro:Bold:10.0"
STATUS_FREQ=5        # cpu / ram, seconds
CLOCK_FREQ=15        # minute precision: worst case 15s late
BATTERY_FREQ=60      # + instant on power_source_change / system_woke
BATTERY_LOW=20       # at or below (unplugged) → red

status_cpu() {
    local ncpu
    ncpu=$(sysctl -n hw.ncpu)
    ps -A -o %cpu= | awk -v n="$ncpu" '{s+=$1} END {printf "%d%%", s/n}'
}

status_ram() {
    local total
    total=$(sysctl -n hw.memsize)
    vm_stat | awk -v total="$total" '
        /page size of/                  { ps = $8 }
        /^Pages active/                 { gsub(/\./, "", $3); used += $3 }
        /^Pages wired down/             { gsub(/\./, "", $4); used += $4 }
        /^Pages occupied by compressor/ { gsub(/\./, "", $5); used += $5 }
        END { printf "%d%%", used * ps * 100 / total }'
}

# SF Symbols glyphs (render in SF Pro): battery.100/75/50/25/0, battery.100.bolt.
# Prints "GLYPH PCT COLOR"; nothing when there is no battery (desktop Mac).
status_battery() {
    local batt pct glyph color=$STATUS_TAG_COLOR
    batt=$(pmset -g batt)
    pct=$(echo "$batt" | grep -Eo '[0-9]+%' | head -1 | tr -d %)
    [ -z "$pct" ] && return
    if echo "$batt" | grep -q "AC Power"; then
        glyph=􀢋
        color=$GREEN
    else
        if   [ "$pct" -ge 88 ]; then glyph=􀛨
        elif [ "$pct" -ge 63 ]; then glyph=􀺸
        elif [ "$pct" -ge 38 ]; then glyph=􀺶
        elif [ "$pct" -ge 13 ]; then glyph=􀛩
        else                         glyph=􀛪
        fi
        [ "$pct" -le $BATTERY_LOW ] && color=$RED
    fi
    echo "$glyph $pct $color"
}

# The group's border: status items + the notification chips (notif.*, added to
# its left by notifications.sh, which calls this again once they exist).
status_bracket() {
    sketchybar --remove status 2>/dev/null
    sketchybar --add bracket status '/^status\./' '/^notif\./' \
        --set status \
        background.drawing=on \
        background.color=$GROUP_BG_COLOR \
        background.border_color=$GROUP_BORDER_COLOR \
        background.border_width=$GROUP_BORDER_WIDTH \
        background.corner_radius=$GROUP_CORNER_RADIUS \
        background.height=$GROUP_HEIGHT \
        background.padding_left=0 \
        background.padding_right=0
}

status_build() {
    sketchybar --remove '/^status\./' --remove status 2>/dev/null

    # First `right` item added is the rightmost.
    sketchybar --add item status.tail right \
        --set status.tail width=$GROUP_EDGE \
        background.drawing=off icon.drawing=off label.drawing=off

    sketchybar --add item status.clock right \
        --set status.clock icon.drawing=off \
        label.padding_left=8 label.padding_right=8 \
        update_freq=$CLOCK_FREQ script="$STATUS_SCRIPT"

    sketchybar --add item status.battery right \
        --set status.battery \
        icon.font="SF Pro:Regular:15.0" icon.color=$STATUS_TAG_COLOR \
        icon.padding_left=8 icon.padding_right=4 \
        label.padding_right=4 \
        update_freq=$BATTERY_FREQ script="$STATUS_SCRIPT" \
        --subscribe status.battery power_source_change system_woke

    local item tag
    for item in ram cpu; do
        tag=$(echo "$item" | tr a-z A-Z)
        sketchybar --add item "status.$item" right \
            --set "status.$item" \
            icon="$tag" icon.font="$STATUS_TAG_FONT" icon.color=$STATUS_TAG_COLOR \
            icon.padding_left=8 icon.padding_right=2 \
            label.padding_right=4 \
            update_freq=$STATUS_FREQ script="$STATUS_SCRIPT"
    done

    sketchybar --add item status.lead right \
        --set status.lead width=$GROUP_EDGE \
        background.drawing=off icon.drawing=off label.drawing=off

    status_bracket

    status_update status.clock
    status_update status.battery
    status_update status.ram
    status_update status.cpu
}

status_update() {
    case "$1" in
    status.cpu)   sketchybar --set "$1" label="$(status_cpu)" ;;
    status.ram)   sketchybar --set "$1" label="$(status_ram)" ;;
    status.battery)
        read -r glyph pct color <<<"$(status_battery)"
        if [ -z "$pct" ]; then
            sketchybar --set "$1" drawing=off
        else
            sketchybar --set "$1" drawing=on icon="$glyph" icon.color="$color" label="$pct%"
        fi ;;
    status.clock) sketchybar --set "$1" label="$(date '+%Y-%m-%d %H:%M')" ;;
    esac
}

case "$NAME" in
status.*) status_update "$NAME" ;;
*)        status_build ;;
esac
