#!/usr/bin/env bash

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

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    prettyprint "$@"
fi
