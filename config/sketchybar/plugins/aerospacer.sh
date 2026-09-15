#!/opt/homebrew/bin/bash

# --- debug instrumentation (zero cost when off) ---------------------------------
# Enable:  touch ~/.cache/sketchybar/debug        (or AEROSPACER_DEBUG=1 env)
# Disable: rm ~/.cache/sketchybar/debug
# View:    tail -f ~/.cache/sketchybar/aerospacer.log
# Stats:   ~/.config/sketchybar/debug_aerospacer.sh stats
# One line per invocation + one line per refresh/build with duration and
# process counts (aerospace CLIs + the batched sketchybar call).
DEBUG_FLAG="${AEROSPACER_DEBUG_FLAG:-$HOME/.cache/sketchybar/debug}"
DEBUG_LOG="${AEROSPACER_DEBUG_LOG:-$HOME/.cache/sketchybar/aerospacer.log}"

dbg() {
    [ -n "$AEROSPACER_DEBUG" ] || [ -f "$DEBUG_FLAG" ] || return 0
    mkdir -p "${DEBUG_LOG%/*}" 2>/dev/null
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$(date +%s)" "$(date '+%F %T')" \
        "${SENDER:-<none>}" "${NAME:-<none>}" \
        "${FOCUSED_WORKSPACE:-<none>}" "${PREV_WORKSPACE:-<none>}" "$*" \
        >> "$DEBUG_LOG" 2>/dev/null
}

ms() { perl -MTime::HiRes -e 'printf "%.0f", Time::HiRes::time()*1000'; }

source "$HOME/.config/sketchybar/colors.sh"

RED=0xff8fc4e8
BLUE=0xff9fc8e8
SILVER_BLUE=0xffb8cfe0
SPACE_BG_ACTIVE=0xff4a7180
SPACE_BG_INACTIVE=0xcc1B2736
SPACE_BORDER_COLOR=0xff9fc8e8
SPACE_BORDER_WIDTH=1
GROUP_BORDER_COLOR=0xccb8cfe0
GROUP_BORDER_WIDTH=1
GROUP_CORNER_RADIUS=8
GROUP_HEIGHT=30
GROUP_BG_COLOR=0x883f4a5a
# Symmetric inset between the group border and the pills, on all four sides.
GROUP_EDGE=2

SPACE_CORNER_RADIUS=6
SPACE_WIDTH=70
SPACE_HEIGHT=24
SPACE_GAP=4
SPACE_ICON_PAD_L=4
SPACE_ICON_Y=6
SPACE_LABEL_PAD_R=10
SPACE_LABEL_Y=-4
SPACE_NUMBER_FONT="SF Pro:Bold:9.0"
SPACE_APP_FONT_SIZE=9.0
APP_FONT="sketchybar-app-font:Regular:$SPACE_APP_FONT_SIZE"
MAX_ICONS=3
# Safety net ONLY: the bar updates are event-driven (aerospace_workspace_change
# + aerospace_focus_change from aerospace.toml). This tick exists purely to
# catch changes the events miss (e.g. a window moved while unfocused), so it
# runs rarely instead of hammering aerospace with 2 CLI calls every 2 seconds.
MONITOR_UPDATE_FREQ=30
# Bar order (AeroSpace lists workspaces alphabetically, which we don't want).
SPACE_ORDER=(M Y W 1 2 3 4 5 6 7 8 9)

CONFIG_DIR="${CONFIG_DIR:-$HOME/.config/sketchybar}"
ICON_MAP="$HOME/.config/sketchybar/sketchybar-app-font/dist/icon_map.json"
# Icon map as a bash assoc array, cached so refresh_all never spawns python3
# (~39ms/refresh). Rebuilt only when icon_map.json changes; needs bash 4+
# (assoc arrays) hence the /opt/homebrew/bin/bash shebang.
ICON_CACHE="$HOME/.cache/sketchybar/icon_map.sh"
load_icons() {
    if [ ! -f "$ICON_CACHE" ] || [ "$ICON_MAP" -nt "$ICON_CACHE" ]; then
        mkdir -p "$(dirname "$ICON_CACHE")" 2>/dev/null
        python3 - "$ICON_MAP" >"$ICON_CACHE" <<'PY'
import json, sys
icon_map = json.load(open(sys.argv[1]))
print("declare -gA ICONS")
seen = set()
for e in icon_map:
    glyph = e["iconName"]
    for name in e["appNames"]:
        k = name.lower()
        if k in seen:
            continue
        seen.add(k)
        print(f'ICONS["{k}"]="{glyph}"')
# Apps without their own glyph: approximate with a close lookalike before
# falling back to the generic :default: tile (same as the old python logic).
for k, g in {"webex": ":microsoft_teams:",
             "webex meetings": ":microsoft_teams:",
             "cisco webex": ":microsoft_teams:",
             "workspace-switcher": ":notes:"}.items():
    print(f'ICONS["{k}"]="{g}"')
PY
    fi
    . "$ICON_CACHE"
}

# One aerospace call + one in-bash icon pass for every workspace, then one
# batched sketchybar invocation: the old per-pill scripts spawned ~70
# processes per workspace switch and applied pills one by one.
refresh_all() {
    local t0=$(ms) focused sid glyphs styles="" windows aero_calls=1
    dbg "refresh_all start"
    focused=${FOCUSED_WORKSPACE:-$(aerospace list-workspaces --focused 2>/dev/null)}
    [ -z "${FOCUSED_WORKSPACE:-}" ] && aero_calls=2   # the 30s tick has no env
    windows=$(aerospace list-windows --all --format '%{app-name} %{workspace}' 2>/dev/null)
    load_icons
    # per-workspace glyph strings (order preserved from list-windows), capped
    # at MAX_ICONS with "…" overflow — pure bash, no python3 subprocess
    declare -A GLYPHS COUNTS CAPPED
    while IFS= read -r line; do
        [ -z "$line" ] && continue
        sid="${line##* }"     # workspace is the last field
        case " ${SPACE_ORDER[*]} " in *" $sid "*) ;; *) continue ;; esac
        app="${line% *}"      # app names may contain spaces
        [ -z "$app" ] && continue
        n=${COUNTS[$sid]:-0}
        if [ "$n" -ge "$MAX_ICONS" ]; then
            [ -z "${CAPPED[$sid]:-}" ] && GLYPHS[$sid]="${GLYPHS[$sid]:-}…" && CAPPED[$sid]=1
            continue
        fi
        glyph="${ICONS[${app,,}]:-:default:}"
        GLYPHS[$sid]="${GLYPHS[$sid]:-}${glyph}"
        COUNTS[$sid]=$((n + 1))
    done <<<"$windows"
    for sid in "${SPACE_ORDER[@]}"; do
        if [ "$sid" = "$focused" ]; then
            styles+=" --set space.$sid background.color=$SPACE_BG_ACTIVE background.border_width=$SPACE_BORDER_WIDTH background.border_color=$SPACE_BORDER_COLOR icon.highlight=on"
        else
            styles+=" --set space.$sid background.color=$SPACE_BG_INACTIVE background.border_width=0 background.border_color=$SPACE_BORDER_COLOR icon.highlight=off"
        fi
        glyphs="${GLYPHS[$sid]:-}"
        if [ -n "$glyphs" ]; then
            styles+=" label=$glyphs label.drawing=on"
        else
            styles+=" label= label.drawing=off"
        fi
    done
    # unquoted on purpose: one batched command, every token is space-free
    [ -n "$styles" ] && sketchybar $styles
    dbg "refresh_all end aero=$aero_calls sketchybar=$([ -n "$styles" ] && echo 1 || echo 0) took=$(( $(ms) - t0 ))ms"
}

build_all() {
    local t0=$(ms) sid
    dbg "build_all start"
    sketchybar --add event aerospace_workspace_change
    sketchybar --add event aerospace_focus_change
    sketchybar --remove '/^space\./' --remove spaces_monitor --remove workspaces

    sketchybar --add item space.lead left \
        --set space.lead width=$GROUP_EDGE \
        padding_left=0 padding_right=0 \
        background.drawing=off icon.drawing=off label.drawing=off

    for sid in "${SPACE_ORDER[@]}"; do
        sketchybar --add item "space.$sid" left \
            --set "space.$sid" \
            icon="$sid" \
            icon.font="$SPACE_NUMBER_FONT" \
            icon.color=$LABEL_COLOR \
            icon.highlight_color=$BLUE \
            icon.padding_left=$SPACE_ICON_PAD_L \
            icon.y_offset=$SPACE_ICON_Y \
            label.font="$APP_FONT" \
            label.color=$LABEL_COLOR \
            label.padding_left=0 \
            label.padding_right=$SPACE_LABEL_PAD_R \
            label.y_offset=$SPACE_LABEL_Y \
            width=$SPACE_WIDTH \
            background.corner_radius=$SPACE_CORNER_RADIUS \
            background.height=$SPACE_HEIGHT \
            background.padding_left=$((SPACE_GAP / 2)) \
            background.padding_right=$((SPACE_GAP / 2)) \
            background.shadow.drawing=on \
            background.shadow.color=0x60000000 \
            background.shadow.distance=2 \
            background.drawing=on \
            click_script="aerospace workspace $sid"
    done

    sketchybar --add item space.tail left \
        --set space.tail width=$GROUP_EDGE \
        padding_left=0 padding_right=0 \
        background.drawing=off icon.drawing=off label.drawing=off

    sketchybar --add bracket workspaces '/^space\./' \
        --set workspaces \
        background.drawing=on \
        background.color=$GROUP_BG_COLOR \
        background.border_color=$GROUP_BORDER_COLOR \
        background.border_width=$GROUP_BORDER_WIDTH \
        background.corner_radius=$GROUP_CORNER_RADIUS \
        background.height=$GROUP_HEIGHT \
        background.padding_left=0 \
        background.padding_right=0

    sketchybar --add item spaces_monitor left \
        --subscribe spaces_monitor aerospace_workspace_change aerospace_focus_change \
        --set spaces_monitor \
        drawing=off \
        updates=on \
        update_freq=$MONITOR_UPDATE_FREQ \
        script="$CONFIG_DIR/plugins/aerospacer.sh"

    refresh_all
    dbg "build_all end took=$(( $(ms) - t0 ))ms"
}

# Safety net: the periodic tick re-runs the same single-pass refresh, so a
# highlight lost to a fast workspace toggle is corrected within
# MONITOR_UPDATE_FREQ seconds.
dbg "invoked"
case "$SENDER" in
mouse.clicked)
    case "$NAME" in
    space.*) aerospace workspace "${NAME#space.}" ;;
    esac
    ;;
aerospace_focus_change)
    # Focus changes fire back-to-back (typing, moving between windows) and
    # each refresh spawns 2 aerospace CLIs + python3 + sketchybar (~100-200ms
    # of AeroSpace main-thread work). Debounce to ONE refresh/sec — workspace
    # changes still refresh immediately, and the 30s periodic tick re-runs
    # refresh anyway as the safety net.
    FOCUS_DEBOUNCE_MS=1000
    CACHE="${AEROSPACER_CACHE:-$HOME/.cache/sketchybar}"
    LAST="$CACHE/aerospacer.focus.last"
    mkdir -p "$CACHE" 2>/dev/null
    now=$(ms)
    last=0
    [ -f "$LAST" ] && last=$(cat "$LAST" 2>/dev/null || echo 0)
    if [ $((now - last)) -lt $FOCUS_DEBOUNCE_MS ]; then
        dbg "focus debounced ($((now - last))ms since last refresh)"
        exit 0
    fi
    echo "$now" >"$LAST"
    refresh_all
    ;;
*)
    if [ "$NAME" = "spaces_monitor" ]; then
        refresh_all
    else
        build_all
    fi
    ;;
esac
