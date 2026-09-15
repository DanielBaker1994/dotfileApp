#!/usr/bin/env bash
# run-tests.sh — compile and run the workspace-switcher tests
#   ./bin/run-tests.sh           run all tests
#   ./bin/run-tests.sh config    run only config tests
set -e
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$DIR/.." && pwd)"
TESTS="$ROOT/Tests"

run_test() {
    local file="$1"
    local name="$(basename "$file" .swift)"
    echo "Running $name..."
    swift "$file"
    echo ""
}

case "${1:-all}" in
    config)
        run_test "$TESTS/test_config.swift"
        ;;
    all|*)
        for f in "$TESTS"/test_*.swift; do
            [ -f "$f" ] && run_test "$f"
        done
        ;;
esac
