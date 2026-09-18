#!/usr/bin/env bash
# utils.sh — shared bash utilities for the workspace switcher (and the shell).
# Source this file (e.g. from ~/.bashrc) to make `prettyprint` available in
# the embedded terminal drawer and any interactive bash.

# prettyprint — reformat JSON or XML into an indented, human-readable form.
#
# Reads stdin (or a file argument), sniffs the first non-whitespace byte, and
# pipes through the matching formatter:
#   { or [  -> jq       (JSON)
#   <       -> xmllint  (XML)
#
# Anything else — or a missing formatter — is echoed back unchanged, so the
# function is safe to run on arbitrary text. Parse errors from jq/xmllint are
# left on stderr so malformed input is obvious.
#
# Examples:
#   curl -s ... | prettyprint
#   prettyprint ~/some/file.json
#   pbpaste | prettyprint | pbcopy      # format the current clipboard
prettyprint() {
    local data first
    if [[ $# -gt 0 && -f $1 ]]; then
        data=$(<"$1")
    else
        data=$(cat)
    fi
    first=$(printf '%s' "$data" | sed -E 's/^[[:space:]]+//' | head -c 1)
    case "$first" in
        '{' | '[')
            if command -v jq >/dev/null 2>&1; then
                printf '%s' "$data" | jq .
                return
            fi
            ;;
        '<')
            if command -v xmllint >/dev/null 2>&1; then
                printf '%s' "$data" | xmllint --format -
                return
            fi
            ;;
    esac
    printf '%s\n' "$data"
}

# When run as a script (not sourced), feed stdin/file args straight through.
# This lets `utils.sh` stand in as an executable:  cat x.json | utils.sh
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    prettyprint "$@"
fi