#!/usr/bin/env bash
# Tests/test_ks_input.sh — gates for bin/ks-input (the real-input CLI).
#
# This script exercises only read-only commands and the refusal path: every
# command that sends input is run with --pid 1, which is never the frontmost
# application, so this sends no real input to anything. Run it directly:
#
#   bash Tests/test_ks_input.sh
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
KI="$ROOT/bin/ks-input"
SRC="$ROOT/Tests/tools/ks_input.swift"

PY="${WS_PYTHON:-}"
if [ -z "$PY" ] || ! command -v "$PY" >/dev/null 2>&1; then
    PY=""
    for candidate in /opt/homebrew/bin/python3 /usr/local/bin/python3 python3; do
        if command -v "$candidate" >/dev/null 2>&1; then PY="$candidate"; break; fi
    done
fi

pass=0
fail=0
ok()  { pass=$((pass + 1)); printf '  ok   %s\n' "$1"; }
bad() { fail=$((fail + 1)); printf '  FAIL %s\n' "$1"; }
is()  { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got '$2', want '$3')"; fi; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/ks-input-test.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

echo "== ks-input =="

# --- compile (forces a fresh build; capture swiftc output) -------------------
rm -rf "$ROOT/.build/ks-input"
compile_out="$("$KI" help 2>&1 >/dev/null)"
compile_rc=$?
if [ "$compile_rc" = 0 ] && [ -x "$ROOT/.build/ks-input/ks-input" ]; then
    ok "compiles with swiftc -O"
else
    bad "compiles with swiftc -O (rc=$compile_rc)"
    [ -n "$compile_out" ] && printf '%s\n' "$compile_out" | sed 's/^/       /'
fi
if [ -z "$compile_out" ]; then
    ok "swiftc emitted no warnings"
else
    bad "swiftc emitted diagnostics:"
    printf '%s\n' "$compile_out" | sed 's/^/       /'
fi

# --- front: three tab-separated fields ---------------------------------------
front_out="$("$KI" front 2>/dev/null)"
front_rc=$?
is "front exits 0" "$front_rc" "0"
front_fields="$(printf '%s' "$front_out" | awk -F'\t' '{print NF}')"
is "front prints 3 tab-separated fields" "$front_fields" "3"
case "$front_out" in
    [0-9]*$'\t'*) ok "front first field is a numeric pid" ;;
    *) bad "front first field is a numeric pid (got '$front_out')" ;;
esac

# --- windows --pid $$: valid JSON --------------------------------------------
win_out="$("$KI" windows --pid $$ 2>/dev/null)"
win_rc=$?
is "windows exits 0" "$win_rc" "0"
if [ -n "$PY" ] && printf '%s' "$win_out" | "$PY" -c '
import json, sys
rows = json.load(sys.stdin)
assert isinstance(rows, list), "not a list"
keys = {"id", "title", "x", "y", "w", "h"}
for row in rows:
    assert set(row) == keys, row
' 2>/dev/null; then
    ok "windows prints valid JSON {id,title,x,y,w,h}"
else
    bad "windows prints valid JSON (got '$win_out')"
fi

# --- the safety gate: input commands refuse when P is not frontmost ----------
# pid 1 (launchd) is never the frontmost application.
refuse() {
    local desc="$1"; shift
    local out="$WORK/out" err="$WORK/err" rc
    "$KI" "$@" >"$out" 2>"$err"; rc=$?
    is "refused, exit 3: $desc" "$rc" "3"
    if [ -s "$out" ]; then
        bad "refused, sent nothing: $desc (stdout not empty)"
    else
        ok  "refused, sent nothing: $desc (empty stdout)"
    fi
    if grep -qi "refus" "$err"; then
        ok "refused, stderr message: $desc"
    else
        bad "refused, stderr message: $desc ($(head -1 "$err"))"
    fi
}

refuse "key cmd+k"        key --pid 1 cmd+k
refuse "key sequence"     key --pid 1 ctrl+b w
refuse "key cmd+shift+p"  key --pid 1 cmd+shift+p
refuse "key esc"          key --pid 1 esc
refuse "type text"        type --pid 1 hello
refuse "click"            click --pid 1 10 10
refuse "click --right"    click --pid 1 --right 10 10
refuse "click --double"   click --pid 1 --double 10 10
refuse "drag"             drag --pid 1 1 1 2 2
refuse "drag --steps"     drag --pid 1 --steps 3 1 1 2 2
refuse "scroll"           scroll --pid 1 -3

# --- bad usage exits 2 -------------------------------------------------------
usage_err() {
    local desc="$1"; shift
    local rc
    "$KI" "$@" >/dev/null 2>&1; rc=$?
    is "usage, exit 2: $desc" "$rc" "2"
}

usage_err "no command"
usage_err "unknown command"  bogus
usage_err "missing --pid"    windows
usage_err "--pid without value" windows --pid
usage_err "unknown flag"     windows --pid 1 --nope
usage_err "non-numeric --pid" windows --pid abc
usage_err "zero --pid"       windows --pid 0
usage_err "key without combo" key --pid 1
usage_err "key unknown name"  key --pid 1 nosuchkey
usage_err "type without text" type --pid 1
usage_err "click without y"   click --pid 1 5
usage_err "drag without p2"   drag --pid 1 1 1 1
usage_err "scroll without dy" scroll --pid 1
usage_err "shot without --out" shot --pid 1

echo
printf 'ks-input: %d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
