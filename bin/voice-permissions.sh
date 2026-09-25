#!/usr/bin/env bash
# voice-permissions.sh — grant microphone + speech recognition to the
# workspace-switcher .app bundle via the user TCC database.
#
# Grants are inserted for BOTH identities macOS uses to attribute the
# process:
#   1. the app's BUNDLE ID (dev.danielbaker.workspace-switcher) — the
#      classic stable key,
#   2. the .app binary's ABSOLUTE PATH with a NULL csreq — matches ANY
#      ad-hoc signature at that path, so it survives every rebuild/re-sign
#      (on newer macOS the speech recognizer attributes by cdhash; a path
#      row bound to an OLD signature makes the status flip back to
#      "not determined" after each rebuild).
#
# Run it (jira-doctor.sh --fix and workspace_switcher.sh run it
# automatically after rebuilding). No sudo needed.
#
# Usage:  bin/voice-permissions.sh [bundle-id]

set -u
WS_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$WS_ROOT/install.conf"   # BUNDLE_ID default
BUNDLE_ID="${1:-$BUNDLE_ID}"
APP_DIR="${2:-$WS_ROOT/$APP_NAME.app}"
APP_BIN="$APP_DIR/Contents/MacOS/$APP_NAME"

# sudo-safe: under sudo $HOME points at /var/root — resolve the REAL user's
# home + TCC database so the grants land where the daemon runs
if [ -n "${SUDO_USER:-}" ]; then
    REAL_USER="$SUDO_USER"
    REAL_HOME="$(/usr/bin/dscl . -read "/Users/$REAL_USER" NFSHomeDirectory 2>/dev/null | awk '{print $2}')"
    REAL_HOME="${REAL_HOME:-/Users/$REAL_USER}"
    TCC_DB="$REAL_HOME/Library/Application Support/com.apple.TCC/TCC.db"
else
    TCC_DB="$HOME/Library/Application Support/com.apple.TCC/TCC.db"
fi

[ -d "$APP_DIR" ] || { echo "voice-permissions: app bundle missing: $APP_DIR" >&2; exit 1; }
[ -x "$APP_BIN" ] || { echo "voice-permissions: binary missing: $APP_BIN" >&2; exit 1; }
[ -f "$TCC_DB" ] || { echo "voice-permissions: TCC database missing: $TCC_DB" >&2; exit 1; }
command -v sqlite3 >/dev/null 2>&1 || { echo "voice-permissions: sqlite3 not found" >&2; exit 1; }

sqlite3 "$TCC_DB" <<SQL
BEGIN;
INSERT OR REPLACE INTO access
  (service, client, client_type, auth_value, auth_reason, auth_version,
   csreq, indirect_object_identifier, flags, last_modified)
VALUES
  ('kTCCServiceMicrophone', '$BUNDLE_ID', 0, 2, 2, 1, NULL, 'UNUSED', 0, strftime('%s','now')),
  ('kTCCServiceSpeechRecognition', '$BUNDLE_ID', 0, 2, 2, 1, NULL, 'UNUSED', 0, strftime('%s','now')),
  ('kTCCServiceMicrophone', '$APP_BIN', 1, 2, 2, 1, NULL, 'UNUSED', 0, strftime('%s','now')),
  ('kTCCServiceSpeechRecognition', '$APP_BIN', 1, 2, 2, 1, NULL, 'UNUSED', 0, strftime('%s','now'));
COMMIT;
SQL

# tccd caches grants in memory — restart it so the new rows take effect
killall tccd 2>/dev/null
sleep 1
echo "voice-permissions: granted microphone + speech recognition to $BUNDLE_ID + $APP_BIN"