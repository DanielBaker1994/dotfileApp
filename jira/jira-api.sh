#!/usr/bin/env bash
# jira-api.sh - compatibility wrapper: the API tool is jira_api.py now
# (same flags; every request logged to ~/.cache/jira/curl.log).
exec python3 "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/jira_api.py" "$@"
