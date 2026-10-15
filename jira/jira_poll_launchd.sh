#!/usr/bin/env bash
# launchd wrapper for the Jira poller (com.jira.poll).
#
# launchd cannot compute a date for StandardOutPath/StandardErrorPath, so this
# wrapper owns the date-based rolling instead: it runs jira_poll.py and appends
# stdout/stderr to today's log directory (logs/YYYY_MM_DD/), the same place
# pylib/log_hub.py puts every other log.
set -u
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
DIR="${WS_LOG_DIR:-$ROOT/logs/$(date +%Y_%m_%d)}"
mkdir -p "$DIR" 2>/dev/null || true
exec python3 "$HERE/jira_poll.py" --quiet \
    >>"$DIR/jira-poll-stdout.log" 2>>"$DIR/jira-poll-stderr.log"
