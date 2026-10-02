#!/usr/bin/env bash
# fake-jira-tab.sh start [N]|stop|status - show N fake issues in the Jira window
# (reproduces a work-sized list: typing lag in the filter box).
#
#   start [N]  write N Faker issues (default 20000, jira/fake_jira_tab.py) to
#              ~/.cache/workspace-switcher/jira_fake/bench-N.json and point
#              `[jira] sources` in commands.toml at that folder (the old value
#              is kept in jira_fake/sources.orig). The real tabs, the poller
#              and config.json are never touched.
#   stop       put `[jira] sources` back.
#   status     which folder the Jira window reads.
# Filter timings: /tmp/ws-debug.log, lines "jira filter: …".
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FAKE_DIR="$HOME/.cache/workspace-switcher/jira_fake"
ORIG="$FAKE_DIR/sources.orig"

conf() {  # conf get | conf set VALUE|"" -> [jira] sources
    python3 - "$@" <<PY
import sys
sys.path.insert(0, "$ROOT/confluence")
import confluence_config as cc
if sys.argv[1] == "get":
    print(cc.section_value("jira", "sources") or "")
else:
    cc.set_section_value("jira", "sources", sys.argv[2] or None)
PY
}

case "${1:-status}" in
start)
    n="${2:-20000}"
    mkdir -p "$FAKE_DIR"
    cur="$(conf get)"
    # remember the real value once (a second start keeps the first one)
    [[ -f $ORIG || $cur == "~/.cache/workspace-switcher/jira_fake" ]] || printf '%s' "$cur" >"$ORIG"
    rm -f "$FAKE_DIR"/bench-*.json
    python3 "$ROOT/jira/fake_jira_tab.py" "$FAKE_DIR/bench-$n.json" --count "$n"
    conf set "~/.cache/workspace-switcher/jira_fake"
    echo "Jira window now reads $FAKE_DIR; 'bin/fake-jira-tab.sh stop' switches back"
    ;;
stop)
    if [[ -f $ORIG ]]; then
        conf set "$(cat "$ORIG")"
        rm -f "$ORIG"
        echo "[jira] sources = $(conf get)"
    else
        echo "fake tab was not active ([jira] sources = $(conf get))"
    fi
    ;;
status)
    echo "[jira] sources = $(conf get)"
    ls -la "$FAKE_DIR" 2>/dev/null || true
    ;;
*)
    sed -n '2,13p' "$0"
    exit 2
    ;;
esac
