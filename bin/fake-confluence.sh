#!/usr/bin/env bash
# fake-confluence.sh start|stop|status - run the fake Confluence
# (confluence/fake_confluence.py) on 127.0.0.1 and point the app at it.
#
#   start   serve http://127.0.0.1:${FAKE_CONF_PORT:-8765}/wiki in the background,
#           write ~/.config/confluence/fake.json (token "fake-token", spaces
#           ENG + OPS in scope) and set `[confluence] config` in commands.conf
#           to it. Your real config.json is never touched.
#   stop    stop the server and remove `[confluence] config` again (back to
#           ~/.config/confluence/config.json).
#   status  is it running, and which config the app uses.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PORT="${FAKE_CONF_PORT:-8765}"
RUN_DIR="$HOME/.cache/confluence"
PID_FILE="$RUN_DIR/fake.pid"
LOG_FILE="$RUN_DIR/fake.log"
FAKE_CFG="$HOME/.config/confluence/fake.json"

running() { [[ -f $PID_FILE ]] && kill -0 "$(cat "$PID_FILE")" 2>/dev/null; }

set_conf() {  # set_conf VALUE|"" -> [confluence] config
    python3 - "$1" <<PY
import sys
sys.path.insert(0, "$ROOT/confluence")
import confluence_config as cc
cc.set_section_value("confluence", "config", sys.argv[1] or None)
PY
}

case "${1:-status}" in
start)
    mkdir -p "$RUN_DIR" "$(dirname "$FAKE_CFG")"
    if running; then
        echo "fake Confluence already running (pid $(cat "$PID_FILE"))"
    else
        nohup python3 "$ROOT/confluence/fake_confluence.py" serve --port "$PORT" --context /wiki \
            >"$LOG_FILE" 2>&1 &
        echo $! >"$PID_FILE"
        for _ in $(seq 1 30); do
            curl -s -o /dev/null "http://127.0.0.1:$PORT/wiki/" && break
            sleep 0.1
        done
        running || { echo "fake Confluence failed to start - see $LOG_FILE" >&2; exit 1; }
        echo "fake Confluence: http://127.0.0.1:$PORT/wiki (pid $(cat "$PID_FILE"), log $LOG_FILE)"
    fi
    python3 - "$FAKE_CFG" "$PORT" <<PY
import json, os, sys
sys.path.insert(0, "$ROOT/jira")
import jira_config
path, port = sys.argv[1], sys.argv[2]
old = {}
if os.path.exists(path):
    old = json.load(open(path))
cfg = {**old, "site": f"http://127.0.0.1:{port}/wiki", "auth": "bearer", "email": "", "token": "fake-token"}
cfg.setdefault("spaces", [{"key": "ENG", "name": "Engineering"}, {"key": "OPS", "name": "Operations"}])
jira_config.write_json_600(path, cfg)
PY
    set_conf "~/.config/confluence/fake.json"
    echo "app now uses $FAKE_CFG ([confluence] config); 'bin/fake-confluence.sh stop' switches back"
    ;;
stop)
    if running; then
        kill "$(cat "$PID_FILE")" && echo "fake Confluence stopped"
    else
        echo "fake Confluence was not running"
    fi
    rm -f "$PID_FILE"
    set_conf ""
    echo "app uses ~/.config/confluence/config.json again"
    ;;
status)
    if running; then echo "running: http://127.0.0.1:$PORT/wiki (pid $(cat "$PID_FILE"))"; else echo "not running"; fi
    python3 "$ROOT/confluence/confluence_api.py" --check
    ;;
*)
    sed -n '2,12p' "$0"
    exit 2
    ;;
esac
