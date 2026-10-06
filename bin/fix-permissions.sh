#!/usr/bin/env bash
# fix-permissions.sh — deprecated shim for `./ws permissions fix`.
# Run it in a normal Terminal (it may ask for your login password):
#   bin/fix-permissions.sh
set -uo pipefail
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)/lib.sh"
ws_cmd_permissions fix
