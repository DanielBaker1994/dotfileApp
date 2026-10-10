#!/usr/bin/env bash
set -e
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$DIR/.." && pwd)"
TESTS="$ROOT/Tests"

run_test() {
    local file="$1"
    local name="$(basename "$file" .swift)"
    echo "Running $name..."
    local sources
    sources="$(sed -n 's|^// sources: *||p' "$file" | head -1)"
    if [ -n "$sources" ]; then
        local bin src=() dir
        dir="$(mktemp -d)" || { echo "mktemp failed" >&2; return 1; }
        bin="$dir/$name"
        for s in $sources; do src+=("$ROOT/$s"); done
        swiftc -O -o "$bin" "${src[@]}" "$file" || { rm -rf "$dir"; return 1; }
        "$bin"
        rm -rf "$dir"
    else
        swift "$file"
    fi
    echo ""
}

case "${1:-all}" in
    config)
        py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
        "$py" "$TESTS/test_config_text.py"
        ;;
    recent)
        run_test "$TESTS/test_recent_files.swift"
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
        run_test "$TESTS/test_nvim_rpc.swift"
        ;;
    filter)
        run_test "$TESTS/test_list_filter.swift"
        ;;
    paths)
        run_test "$TESTS/test_path_shelf.swift"
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
        run_test "$TESTS/test_screenshot.swift"
        ;;
    ansi)
        run_test "$TESTS/test_ansi_render.swift"
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
        run_test "$TESTS/test_prose_pdf.swift"
        ;;
    prose-pdf)
        py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
        "$py" "$TESTS/test_prose_pdf.py"
        ;;
    snippets)
        run_test "$TESTS/test_snippet_render.swift"
        ;;
    panes)
        run_test "$TESTS/test_pane_geometry.swift"
        ;;
    vim-keys)
        run_test "$TESTS/test_vim_search.swift"
        ;;
    compare)
        py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
        "$py" "$TESTS/test_compare.py"
        run_test "$TESTS/test_compare_folder.swift"
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
        run_test "$TESTS/test_python_helper.swift"
        ;;
    doc-templates)
        py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
        "$py" "$TESTS/test_doc_templates.py"
        ;;
    ai-live)
        run_test "$TESTS/live_ai_rules.swift"
        ;;
    all|*)
        py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
        # the three ported python suites with no `ws test` case of their own
        # (the rest have cases above); the Swift sweep stays unchanged
        for t in "$TESTS/test_jira_poll.py" "$TESTS/test_confluence.py" "$TESTS/test_notifications.py"; do
            [ -f "$t" ] && echo "Running $(basename "$t" .py)..." && "$py" "$t"
        done
        for f in "$TESTS"/test_*.swift; do
            [ -f "$f" ] && run_test "$f"
        done
        ;;
esac
