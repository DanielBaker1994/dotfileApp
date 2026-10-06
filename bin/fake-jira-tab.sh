#!/usr/bin/env bash
# fake-jira-tab.sh — deprecated shim for `./ws fake jira`.
#   start [N]|stop|status  show N fake issues in the Jira window.
set -uo pipefail
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)/lib.sh"
ws_cmd_fake jira "$@"
