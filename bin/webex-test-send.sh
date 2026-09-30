#!/usr/bin/env bash
# Fire REAL Webex events at your account to test the sketchybar notifications
# pill (notify/). Sends as a Webex BOT (developer.webex.com → My Apps → Bot),
# whose token never expires.
#
#   WEBEX_TOKEN   the bot's token (required; e.g. exported in ~/.bash_profile)
#   WEBEX_TEST_TO the account the pill watches (the one signed in to Webex)
#
#   webex-test-send.sh dm [text]       direct message   → unread count
#   webex-test-send.sh group [text]    group message    → unread count
#   webex-test-send.sh mention [text]  group @mention   → count + "@N"
#   webex-test-send.sh clear           delete the bot's test space
#   webex-test-send.sh whoami          who the token belongs to
#
# The group space ("notifications pill test") is created once and its id kept
# in ~/.cache/notifications/test-room.
set -euo pipefail

API=https://webexapis.com/v1
ROOM_FILE="$HOME/.cache/notifications/test-room"
ROOM_TITLE="notifications pill test"

[ -n "${WEBEX_TOKEN:-}" ] || { echo "WEBEX_TOKEN is not set (the bot's token)" >&2; exit 1; }

api() {  # api METHOD PATH [JSON]
    local out code
    out=$(curl -sS -w '\n%{http_code}' -X "$1" "$API$2" \
        -H "Authorization: Bearer $WEBEX_TOKEN" -H 'Content-Type: application/json' \
        ${3:+-d "$3"})
    code=${out##*$'\n'}
    out=${out%$'\n'*}
    if [ "$code" -ge 300 ]; then
        echo "$1 $2 → $code: $out" >&2
        exit 1
    fi
    printf '%s' "$out"
}

json() {  # json KEY=VALUE… → object (values are strings)
    python3 -c 'import json,sys; print(json.dumps(dict(a.split("=",1) for a in sys.argv[1:])))' "$@"
}

field() { python3 -c "import json,sys; print(json.load(sys.stdin).get('$1',''))"; }

need_to() {
    [ -n "${WEBEX_TEST_TO:-}" ] || { echo "WEBEX_TEST_TO is not set (your Webex email)" >&2; exit 1; }
}

test_room() {  # the group space id, created (with WEBEX_TEST_TO in it) on first use
    local id=""
    [ -f "$ROOM_FILE" ] && id=$(cat "$ROOM_FILE")
    if [ -n "$id" ] && api GET "/rooms/$id" >/dev/null 2>&1; then
        echo "$id"; return
    fi
    id=$(api POST /rooms "$(json title="$ROOM_TITLE")" | field id)
    api POST /memberships "$(json roomId="$id" personEmail="$WEBEX_TEST_TO")" >/dev/null
    mkdir -p "${ROOM_FILE%/*}"
    echo "$id" > "$ROOM_FILE"
    echo "created space \"$ROOM_TITLE\"" >&2
    echo "$id"
}

stamp() { date '+%H:%M:%S'; }

case "${1:-}" in
dm)
    need_to
    api POST /messages "$(json toPersonEmail="$WEBEX_TEST_TO" text="${2:-test DM $(stamp)}")" >/dev/null
    echo "sent a DM to $WEBEX_TEST_TO" ;;
group)
    need_to
    api POST /messages "$(json roomId="$(test_room)" text="${2:-test message $(stamp)}")" >/dev/null
    echo "sent a group message to \"$ROOM_TITLE\"" ;;
mention)
    need_to
    api POST /messages "$(json roomId="$(test_room)" \
        markdown="<@personEmail:$WEBEX_TEST_TO> ${2:-test mention $(stamp)}")" >/dev/null
    echo "@mentioned $WEBEX_TEST_TO in \"$ROOM_TITLE\"" ;;
clear)
    if [ -f "$ROOM_FILE" ]; then
        api DELETE "/rooms/$(cat "$ROOM_FILE")" >/dev/null || true
        rm -f "$ROOM_FILE"
        echo "deleted \"$ROOM_TITLE\""
    else
        echo "no test space"
    fi ;;
whoami)
    api GET /people/me | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["displayName"], d["emails"], d["type"])' ;;
*)
    sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//'
    exit 2 ;;
esac
