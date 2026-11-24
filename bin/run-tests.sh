#!/usr/bin/env bash
set -e
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$DIR/.." && pwd)"
TESTS="$ROOT/Tests"
# The interpreter for the plain-script Python suites (Homebrew first).
py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3

# The app's unit tests live in the Rust workspace (rust/): `rust_test FILTER`
# runs the ws-rs tests whose path matches FILTER (a module, e.g.
# engines::list_filter); no filter runs the whole workspace.
rust_test() {
    echo "Running cargo test ${1:-(workspace)}..."
    command -v cargo >/dev/null 2>&1 || { echo "cargo not found — install Rust" >&2; return 1; }
    if [ -n "${1:-}" ]; then
        (cd "$ROOT/rust" && CARGO_TARGET_DIR="$ROOT/rust/target" cargo test -q -p ws-rs -p ks-core "$1")
    else
        (cd "$ROOT/rust" && CARGO_TARGET_DIR="$ROOT/rust/target" cargo test -q)
    fi
    echo ""
}

case "${1:-all}" in
    config)
        "$py" "$TESTS/test_config_text.py"
        ;;
    recent)
        rust_test engines::recent_files
        ;;
    fileops)
        "$py" "$TESTS/test_file_ops.py"
        ;;
    status)
        "$py" "$TESTS/test_status.py"
        ;;
    ai)
        "$py" "$TESTS/test_ai_format.py"
        ;;
    filter)
        rust_test engines::list_filter
        ;;
    paths)
        rust_test engines::path_shelf
        ;;
    ignore)
        "$py" "$TESTS/test_ignore_rules.py"
        ;;
    jsonmgr)
        "$py" "$TESTS/test_jsonmgr.py"
        ;;
    screenshot)
        rust_test screenshot
        ;;
    ansi)
        rust_test engines::ansi_render
        ;;
    prose)
        rust_test engines::ai_format
        ;;
    panes)
        rust_test panes::pane_geometry
        ;;
    vim-keys)
        rust_test panes::vim_search
        ;;
    settings)
        "$py" "$TESTS/test_settings_hub.py"
        ;;
    jira-data)
        "$py" "$TESTS/test_jira_data.py"
        ;;
    jira-pages)
        "$py" "$TESTS/test_jira_pages.py"
        ;;
    jira-search)
        "$py" "$TESTS/test_jira_search.py"
        ;;
    jira-directory)
        "$py" "$TESTS/test_jira_directory.py"
        ;;
    jira-boards)
        "$py" "$TESTS/test_jira_boards.py"
        ;;
    jira-fields)
        "$py" "$TESTS/test_jira_fields.py"
        ;;
    jira-setup)
        "$py" "$TESTS/test_jira_setup.py"
        ;;
    confluence-pages)
        "$py" "$TESTS/test_confluence_pages.py"
        ;;
    shelf)
        "$py" "$TESTS/test_shelf.py"
        ;;
    text-table)
        "$py" "$TESTS/test_text_table.py"
        ;;
    helper)
        py=""
        for c in "${WS_PYTHON:-}" /opt/homebrew/bin/python3 /usr/local/bin/python3 "$(command -v python3 2>/dev/null)"; do
            [ -n "$c" ] && [ -x "$c" ] || continue
            if "$c" -c 'import sys; sys.exit(0 if sys.version_info >= (3, 11) else 1)' 2>/dev/null; then
                py="$c"; break
            fi
        done
        [ -n "$py" ] || { echo "helper tests need python 3.11+ (brew install python, or set WS_PYTHON)" >&2; exit 1; }
        "$py" "$TESTS/test_helper.py"
        ;;
    helper-client)
        rust_test app::python_helper
        ;;
    mcp)
        "$py" -B "$TESTS/test_ks_mcp.py" && "$py" -B "$TESTS/test_mcp_verbs.py"
        ;;
    mcp-verbs)
        "$py" -B "$TESTS/test_mcp_verbs.py"
        ;;
    rust)
        rust_test
        ;;
    all|*)
        # the python suites with no `ws test` case of their own (the rest
        # have cases above), then every Rust test in the workspace
        for t in "$TESTS/test_jira_poll.py" "$TESTS/test_confluence.py" "$TESTS/test_notifications.py"; do
            [ -f "$t" ] && echo "Running $(basename "$t" .py)..." && "$py" "$t"
        done
        rust_test
        ;;
esac
