#!/usr/bin/env bash
# Notification chips at the LEFT END of the status group (same border, see
# status_bracket in status.sh): one chip per source (`[notifications] sources`
# in commands.toml). Chip = the app's icon + its unread count as a red badge
# on the icon's corner; "@N" = @mentions from the API source; amber dot = the
# API needs attention. Click = popup (unread spaces, mentions, sign in, open,
# refresh); see notify/notify_poll.py.
# Sourced by sketchybarrc AFTER status.sh (so it lands to its left and can
# call status_bracket) → builds
# the items; run as the items' script (sketchybar sets $NAME) → one tick.
# All logic + config reading lives in notify/notify_poll.py.

source "$HOME/.config/sketchybar/colors.sh"

NOTIF_SCRIPT="$HOME/.config/sketchybar/plugins/notifications.sh"
NOTIF_PY="$HOME/.config/workspace-switcher/notify/notify_poll.py"
# (an app install keeps notify/ inside the signed bundle: no __pycache__ there)
export PYTHONDONTWRITEBYTECODE=1
NOTIF_TAG_FONT="SF Pro:Bold:10.0"
# count = an iOS-style badge over the icon's top-right corner
NOTIF_BADGE_FONT="SF Pro:Medium:10.0"
NOTIF_BADGE_LIFT=7       # how far the badge sits above the chip's center
NOTIF_ICON_INSET=7       # left inset of the (narrowed) icon slot

notif_build() {
    sketchybar --remove '/^notif\./' 2>/dev/null
    eval "$(python3 "$NOTIF_PY" --config-sh)"
    [ "$NOTIF_ENABLED" = on ] && [ -n "$NOTIF_SOURCES" ] || return 0

    sketchybar --add event notifications_update

    sketchybar --add item notif.tail right \
        --set notif.tail width=0 padding_left=0 padding_right=0 \
        background.drawing=off icon.drawing=off label.drawing=off \
        updates=on update_freq="$NOTIF_TICK" script="$NOTIF_SCRIPT" \
        --subscribe notif.tail notifications_update system_woke

    local src tag app icon order
    order=$(echo "$NOTIF_SOURCES" | tr ' ' '\n' | tail -r | tr '\n' ' ')
    for src in $order; do
        eval "tag=\$NOTIF_TAG_$src app=\$NOTIF_APP_$src icon=\$NOTIF_ICON_$src"
        # added right-to-left: @N / warn dot, icon (+ its count badge)
        sketchybar --add item "notif.$src.at" right \
            --set "notif.$src.at" drawing=off icon.drawing=off \
            label.padding_left=2 label.padding_right=6
        sketchybar --add item "notif.$src" right
        if [ "$icon" = app ]; then
            # the app's own icon (sketchybar draws app.<bundle-id>)
            sketchybar --set "notif.$src" icon="" icon.width=22 \
                icon.background.drawing=on icon.background.image="app.$app" \
                icon.background.image.scale="$NOTIF_ICON_SCALE" \
                icon.background.color=0x00000000
        else
            sketchybar --set "notif.$src" icon="$tag" \
                icon.font="$NOTIF_TAG_FONT" icon.color="$NOTIF_TAG_COLOR"
        fi
        # label = the count badge (text + drawing set per tick), lifted onto
        # the icon's top-right corner
        sketchybar --set "notif.$src" label.drawing=off \
            padding_left=0 icon.padding_left=$NOTIF_ICON_INSET icon.padding_right=0 \
            label.font="$NOTIF_BADGE_FONT" label.color="$NOTIF_UNREAD_TEXT_COLOR" \
            label.padding_left=4 label.padding_right=4 label.y_offset=$NOTIF_BADGE_LIFT \
            label.background.drawing=on label.background.color="$NOTIF_UNREAD_COLOR" \
            label.background.corner_radius=7 label.background.height=14 \
            label.background.y_offset=$NOTIF_BADGE_LIFT \
            popup.align=right popup.y_offset=6 \
            popup.background.color=0xff1e2030 popup.blur_radius=20 \
            popup.background.border_color=$GROUP_BORDER_COLOR \
            popup.background.border_width=$GROUP_BORDER_WIDTH \
            popup.background.corner_radius=10
        # a click anywhere on the chip opens its popup; leaving closes it
        for it in "notif.$src" "notif.$src.at"; do
            sketchybar --set "$it" script="$NOTIF_SCRIPT" \
                --subscribe "$it" mouse.clicked mouse.exited.global
        done
    done

    sketchybar --add item notif.lead right \
        --set notif.lead width=$GROUP_EDGE \
        background.drawing=off icon.drawing=off label.drawing=off

    status_bracket   # status.sh: the chips join the status group's border

    python3 "$NOTIF_PY" --tick
}

case "$NAME" in
notif.*) python3 "$NOTIF_PY" --event ;;
*)       notif_build ;;
esac
