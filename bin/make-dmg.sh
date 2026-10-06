#!/usr/bin/env bash
# make-dmg.sh — deprecated shim for `./ws dmg`.
#   bin/make-dmg.sh                the distributable .dmg
#   bin/make-dmg.sh --no-notarize  sign only, skip the notary round-trip
set -uo pipefail
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)/lib.sh"
ws_cmd_dmg "$@"
