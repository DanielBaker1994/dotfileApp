#!/usr/bin/env bash
# run-tests.sh — compile and run the workspace-switcher tests
#   ./bin/run-tests.sh           run all tests
#   ./bin/run-tests.sh config    run only config tests
#   ./bin/run-tests.sh recent    run only the file browser's Recent list tests
#   ./bin/run-tests.sh fileops   run only the file browser's file operation tests
#   ./bin/run-tests.sh ai        run only the AI view's rule / table / keep-words tests
#   ./bin/run-tests.sh ai-live   run the shipped rules through fm's on-device model (slow; not in "all")
set -e
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$DIR/.." && pwd)"
TESTS="$ROOT/Tests"

run_test() {
    local file="$1"
    local name="$(basename "$file" .swift)"
    echo "Running $name..."
    # "// sources: A.swift B.swift" = a test of the app's own files: built
    # together with them (the test file has an @main type)
    local sources
    sources="$(sed -n 's|^// sources: *||p' "$file" | head -1)"
    if [ -n "$sources" ]; then
        local bin src=()
        bin="$(mktemp -d)/$name"
        for s in $sources; do src+=("$ROOT/$s"); done
        swiftc -O -o "$bin" "${src[@]}" "$file"
        "$bin"
        rm -rf "$(dirname "$bin")"
    else
        swift "$file"
    fi
    echo ""
}

case "${1:-all}" in
    config)
        run_test "$TESTS/test_config.swift"
        ;;
    recent)
        run_test "$TESTS/test_recent_files.swift"
        ;;
    fileops)
        run_test "$TESTS/test_file_ops.swift"
        ;;
    ai)
        run_test "$TESTS/test_ai_format.swift"
        ;;
    ai-live)
        run_test "$TESTS/live_ai_rules.swift"
        ;;
    all|*)
        for f in "$TESTS"/test_*.swift; do
            [ -f "$f" ] && run_test "$f"
        done
        ;;
esac
