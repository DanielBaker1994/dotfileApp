#!/usr/bin/env bash
# jira-poll.sh - compatibility wrapper: the poller is jira_poll.py now
# (per-endpoint schedules, status.json, curl.log). See jira_poll.py --help.
exec python3 "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/jira_poll.py" "$@"
