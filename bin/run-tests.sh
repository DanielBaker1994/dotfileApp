#!/usr/bin/env bash
set -e
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$DIR/.." && pwd)"
TESTS="$ROOT/Tests"

# The app's unit tests live in the Rust workspace (rust/): `rust_test FILTER`
# runs the ws-rs tests whose path matches FILTER (a module, e.g.
# engines::nvim_rpc); no filter runs the whole workspace.
rust_test() {
    echo "Running cargo test ${1:-(workspace)}..."
    command -v cargo >/dev/null 2>&1 || { echo "cargo not found — install Rust" >&2; return 1; }
    if [ -n "${1:-}" ]; then
        (cd "$ROOT/rust" && CARGO_TARGET_DIR="$ROOT/rust/target" cargo test -q -p ws-rs "$1")
    else
        (cd "$ROOT/rust" && CARGO_TARGET_DIR="$ROOT/rust/target" cargo test -q)
    fi
    echo ""
}

case "${1:-all}" in
    config)
        py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
        "$py" "$TESTS/test_config_text.py"
        ;;
    recent)
        rust_test engines::recent_files
        ;;
    fileops)
        py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
        "$py" "$TESTS/test_file_ops.py"
        ;;
    status)
        py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
        "$py" "$TESTS/test_status.py"
        ;;
    ai)
        py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
        "$py" "$TESTS/test_ai_format.py"
        ;;
    nvim)
        rust_test engines::nvim_rpc
        ;;
    filter)
        rust_test engines::list_filter
        ;;
    paths)
        rust_test engines::path_shelf
        ;;
    ignore)
        py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
        "$py" "$TESTS/test_ignore_rules.py"
        ;;
    jsonmgr)
        py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
        "$py" "$TESTS/test_jsonmgr.py"
        ;;
    screenshot)
        rust_test screenshot
        ;;
    ansi)
        rust_test engines::ansi_render
        ;;
    ansi-parse)
        py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
        "$py" "$TESTS/test_ansi.py"
        ;;
    paneshot)
        py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
        "$py" "$TESTS/test_paneshot.py"
        ;;
    prose)
        rust_test engines::ai_format
        ;;
    prose-pdf)
        py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
        "$py" "$TESTS/test_prose_pdf.py"
        ;;
    snippets)
        py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
        "$py" "$TESTS/test_snippet_render.py"
        ;;
    panes)
        rust_test panes::pane_geometry
        ;;
    vim-keys)
        rust_test panes::vim_search
        ;;
    compare)
        py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
        "$py" "$TESTS/test_compare.py"
        rust_test views::compare
        ;;
    settings)
        py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
        "$py" "$TESTS/test_settings_hub.py"
        ;;
    jira-data)
        py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
        "$py" "$TESTS/test_jira_data.py"
        ;;
    jira-pages)
        py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
        "$py" "$TESTS/test_jira_pages.py"
        ;;
    jira-search)
        py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
        "$py" "$TESTS/test_jira_search.py"
        ;;
    jira-directory)
        py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
        "$py" "$TESTS/test_jira_directory.py"
        ;;
    jira-dashboard)
        py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
        "$py" "$TESTS/test_jira_dashboard.py"
        ;;
    jira-boards)
        py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
        "$py" "$TESTS/test_jira_boards.py"
        ;;
    jira-fields)
        py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
        "$py" "$TESTS/test_jira_fields.py"
        ;;
    jira-setup)
        py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
        "$py" "$TESTS/test_jira_setup.py"
        ;;
    confluence-glue)
        py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
        "$py" "$TESTS/test_confluence_glue.py"
        ;;
    confluence-pages)
        py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
        "$py" "$TESTS/test_confluence_pages.py"
        ;;
    setup-checks)
        py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
        "$py" "$TESTS/test_setup_checks.py"
        ;;
    shot-model)
        py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
        "$py" "$TESTS/test_shot_model.py"
        ;;
    shelf)
        py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
        "$py" "$TESTS/test_shelf.py"
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
    doc-templates)
        py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
        "$py" "$TESTS/test_doc_templates.py"
        ;;
    rust)
        rust_test
        ;;
    all|*)
        py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
        # the python suites with no `ws test` case of their own (the rest
        # have cases above), then every Rust test in the workspace
        for t in "$TESTS/test_jira_poll.py" "$TESTS/test_confluence.py" "$TESTS/test_notifications.py" \
                 "$TESTS/test_snippet_render.py"; do
            [ -f "$t" ] && echo "Running $(basename "$t" .py)..." && "$py" "$t"
        done
        rust_test
        ;;
esac
