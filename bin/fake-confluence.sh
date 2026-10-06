#!/usr/bin/env bash
# fake-confluence.sh — deprecated shim for `./ws fake confluence`.
#   start|stop|status  run the fake Confluence and point the app at it.
set -uo pipefail
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)/lib.sh"
ws_cmd_fake confluence "$@"
