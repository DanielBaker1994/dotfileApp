#!/usr/bin/env bash
# voice-permissions.sh — kept for the old name (error messages, docs):
# bin/grant-permissions.sh now grants every permission the app uses.
exec "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/grant-permissions.sh" "$@"
