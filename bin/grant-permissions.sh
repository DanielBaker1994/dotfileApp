#!/usr/bin/env bash

set -u
WS_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$WS_ROOT/install.conf"
BUNDLE_ID="${1:-$BUNDLE_ID}"
APP_DIR="${2:-$WS_ROOT/$APP_NAME.app}"
APP_BIN="$APP_DIR/Contents/MacOS/$APP_NAME"

if [ -n "${SUDO_USER:-}" ]; then
    REAL_HOME="$(/usr/bin/dscl . -read "/Users/$SUDO_USER" NFSHomeDirectory 2>/dev/null | awk '{print $2}')"
    TCC_DB="${REAL_HOME:-/Users/$SUDO_USER}/Library/Application Support/com.apple.TCC/TCC.db"
else
    TCC_DB="$HOME/Library/Application Support/com.apple.TCC/TCC.db"
fi

die() { echo "grant-permissions: $*" >&2; exit 1; }
[ -d "$APP_DIR" ] || die "app bundle missing: $APP_DIR"
[ -x "$APP_BIN" ] || die "binary missing: $APP_BIN"
[ -f "$TCC_DB" ] || die "TCC database missing: $TCC_DB"
command -v sqlite3 >/dev/null 2>&1 || die "sqlite3 not found"
[ -r "$TCC_DB" ] && [ -w "$TCC_DB" ] || die "cannot write $TCC_DB — give your terminal Full Disk Access (System Settings ▸ Privacy & Security ▸ Full Disk Access)"

CSREQ=NULL
REQ="$(codesign -d -r- "$APP_DIR" 2>&1 | sed -n 's/^designated => //p')"
if [ -n "$REQ" ] && ! printf '%s' "$REQ" | grep -q 'cdhash'; then
    TMP="$(mktemp)"
    if printf '%s' "$REQ" | csreq -r- -b "$TMP" 2>/dev/null; then
        CSREQ="X'$(xxd -p "$TMP" | tr -d '\n')'"
    fi
    rm -f "$TMP"
else
    echo "grant-permissions: WARNING the app is ad-hoc signed — grants use no code requirement" >&2
fi

row() {
    printf "  ('%s', '%s', %s, 2, 2, 1, %s, 'UNUSED', 0, strftime('%%s','now'))" "$1" "$2" "$3" "$4"
}
VALUES="$(row kTCCServiceMicrophone "$BUNDLE_ID" 0 "$CSREQ"),
$(row kTCCServiceSpeechRecognition "$BUNDLE_ID" 0 "$CSREQ"),
$(row kTCCServiceMicrophone "$APP_BIN" 1 NULL),
$(row kTCCServiceSpeechRecognition "$APP_BIN" 1 NULL),
$(row kTCCServiceSystemPolicyDownloadsFolder "$BUNDLE_ID" 0 "$CSREQ"),
$(row kTCCServiceSystemPolicyDesktopFolder "$BUNDLE_ID" 0 "$CSREQ"),
$(row kTCCServiceSystemPolicyDocumentsFolder "$BUNDLE_ID" 0 "$CSREQ")"

sqlite3 "$TCC_DB" <<SQL || die "writing $TCC_DB failed"
BEGIN;
INSERT OR REPLACE INTO access
  (service, client, client_type, auth_value, auth_reason, auth_version,
   csreq, indirect_object_identifier, flags, last_modified)
VALUES
$VALUES;
COMMIT;
SQL

killall tccd 2>/dev/null
sleep 1
echo "grant-permissions: granted microphone, speech recognition, Downloads, Desktop, Documents to $BUNDLE_ID"
