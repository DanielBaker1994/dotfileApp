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
    nvim)
        run_test "$TESTS/test_nvim_rpc.swift"
        ;;
    filter)
        run_test "$TESTS/test_list_filter.swift"
        ;;
    paths)
        run_test "$TESTS/test_path_shelf.swift"
        ;;
    screenshot)
        run_test "$TESTS/test_screenshot.swift"
        ;;
    ansi)
        run_test "$TESTS/test_ansi_render.swift"
        ;;
    prose)
        run_test "$TESTS/test_prose_pdf.swift"
        ;;
    snippets)
        run_test "$TESTS/test_snippet_render.swift"
        ;;
    doctemplates)
        run_test "$TESTS/test_doc_templates.swift"
        ;;
    panes)
        run_test "$TESTS/test_pane_geometry.swift"
        ;;
    vim-keys)
        run_test "$TESTS/test_vim_search.swift"
        ;;
    compare)
        run_test "$TESTS/test_compare.swift"
        run_test "$TESTS/test_compare_folder.swift"
        ;;
    settings)
        py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
        "$py" "$TESTS/test_settings_hub.py"
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
