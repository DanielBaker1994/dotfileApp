#!/usr/bin/env bash
# jira-doctor.sh — one command that asserts the whole jira + popup stack is
# wired up: config paths, Jira API connectivity, the launchd poll agent (with
# when-it-last-ran / when-it-runs-next), the window JSON the jira window
# reads, and the workspace-switcher daemon registration.
#
#   jira-doctor            read-only health report (exit 1 if anything fails)
#   jira-doctor --fix      also repairs what it can: installs/loads the
#                          launchd agent and rebuilds a stale switcher binary
#
# Every check prints PASS/FAIL/WARN with the value it saw, so the report is
# the documentation of "what needs to be true".
set -uo pipefail

FIX=0
[ "${1:-}" = "--fix" ] && FIX=1

WS_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
JIRA_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONF="$HOME/.config/jira/config"
CACHE="$HOME/.cache/jira"
STATE="$CACHE/poll-state"
JSON_DIR="$HOME/.cache/workspace-switcher/jira_json"
PLIST_SRC="$JIRA_DIR/com.jira.poll.plist"
PLIST_DST="$HOME/Library/LaunchAgents/com.jira.poll.plist"
LABEL="com.jira.poll"
WS_DIR="$WS_ROOT"
WS_APP="$WS_ROOT/workspace-switcher.app"
WS_BIN="$WS_APP/Contents/MacOS/workspace-switcher"
TOML="$HOME/.config/aerospace/aerospace.toml"
KARAB="$HOME/.config/karabiner/karabiner.json"

PASS=0; FAIL=0; WARN=0
ok()   { printf '  \033[32mPASS\033[0m  %s\n' "$*"; PASS=$((PASS + 1)); }
bad()  { printf '  \033[31mFAIL\033[0m  %s\n' "$*"; FAIL=$((FAIL + 1)); }
warn() { printf '  \033[33mWARN\033[0m  %s\n' "$*"; WARN=$((WARN + 1)); }
head_() { printf '\n\033[1;36m%s\033[0m\n' "$*"; }

cfg() { grep "^$1=" "$CONF" 2>/dev/null | cut -d\' -f2; }

# ---------------------------------------------------------------- deps
head_ "== dependencies =="
for b in curl jq aerospace swiftc; do
    if command -v "$b" >/dev/null 2>&1; then ok "$b ($(command -v "$b"))"; else bad "$b missing"; fi
done

# ---------------------------------------------------------------- config
head_ "== config =="
if [ -f "$CONF" ]; then
    ok "config: $CONF"
    for k in JIRA_SITE JIRA_EMAIL JIRA_TOKEN; do
        if [ -n "$(cfg "$k")" ]; then ok "$k set"; else bad "$k missing in $CONF"; fi
    done
else
    bad "config missing: $CONF (run: jira-api --init)"
fi

# ---------------------------------------------------------------- api
head_ "== jira api =="
SITE="$(cfg JIRA_SITE)"; EMAIL="$(cfg JIRA_EMAIL)"; TOK="$(cfg JIRA_TOKEN)"
if [ -n "$SITE" ] && [ -n "$EMAIL" ] && [ -n "$TOK" ]; then
    ME="$(curl -s -m 15 -u "$EMAIL:$TOK" "$SITE/rest/api/2/myself" | jq -r '.displayName // empty' 2>/dev/null)"
    if [ -n "$ME" ]; then
        ok "login OK ($ME) — $SITE"
    else
        bad "login failed against $SITE (check token / network)"
    fi
else
    warn "skipped (config incomplete)"
fi

# ---------------------------------------------------------------- cache + json
head_ "== cache & window json =="
if [ -f "$CACHE/jiras.json" ]; then
    N="$(jq 'length' "$CACHE/jiras.json" 2>/dev/null)"
    ok "cache: $CACHE/jiras.json ($N issues)"
else
    bad "cache missing: $CACHE/jiras.json (run: jira-api --sync full)"
fi
if [ -f "$JSON_DIR/all.json" ]; then
    ok "window json: $JSON_DIR/all.json (modified $(stat -f '%Sm' -t '%Y-%m-%d %H:%M' "$JSON_DIR/all.json"))"
else
    bad "window json missing: $JSON_DIR/all.json (run: jira-poll.sh)"
fi

# ---------------------------------------------------------------- launchd
head_ "== poll agent (launchd) =="
INTERVAL=600
if [ -f "$PLIST_DST" ]; then
    ok "plist installed: $PLIST_DST"
    if ! diff -q "$PLIST_SRC" "$PLIST_DST" >/dev/null 2>&1; then
        if [ "$FIX" = 1 ]; then
            cp "$PLIST_SRC" "$PLIST_DST" && ok "plist refreshed from repo (--fix)"
        else
            warn "installed plist differs from repo copy (jira-doctor --fix)"
        fi
    fi
else
    if [ "$FIX" = 1 ]; then
        mkdir -p "$HOME/Library/LaunchAgents" && cp "$PLIST_SRC" "$PLIST_DST" \
            && ok "plist installed (--fix): $PLIST_DST"
    else
        bad "plist not installed (jira-doctor --fix installs it)"
    fi
fi
INTERVAL="$(plutil -extract StartInterval raw "$PLIST_SRC" 2>/dev/null || echo 600)"
if launchctl list "$LABEL" >/dev/null 2>&1; then
    ok "agent loaded: $LABEL"
else
    if [ "$FIX" = 1 ]; then
        if launchctl bootstrap "gui/$(id -u)" "$PLIST_DST" 2>/dev/null \
           || launchctl load "$PLIST_DST" 2>/dev/null; then
            ok "agent loaded (--fix)"
        else
            bad "agent load failed: launchctl bootstrap gui/$(id -u) $PLIST_DST"
        fi
    else
        bad "agent NOT loaded — polling is dead (jira-doctor --fix loads it)"
    fi
fi

# ---------------------------------------------------------------- last / next run
head_ "== poll schedule =="
LAST_POLL=""; PSTATUS=""
if [ -f "$STATE" ]; then
    LAST_POLL="$(grep '^LAST_POLL=' "$STATE" | cut -d= -f2-)"
    PSTATUS="$(grep '^STATUS=' "$STATE" | cut -d= -f2-)"
    ITEMS="$(grep '^ITEMS=' "$STATE" | cut -d= -f2-)"
    if [ -n "$LAST_POLL" ]; then
        ok "last run: $LAST_POLL (status=${PSTATUS:-?}, items=${ITEMS:-?})"
        LAST_EPOCH="$(date -j -f '%Y-%m-%d %H:%M:%S' "$LAST_POLL" '+%s' 2>/dev/null || echo 0)"
        if [ "$LAST_EPOCH" != 0 ]; then
            NEXT_EPOCH=$((LAST_EPOCH + INTERVAL))
            NOW_EPOCH="$(date '+%s')"
            NEXT_HUMAN="$(date -r "$NEXT_EPOCH" '+%Y-%m-%d %H:%M:%S')"
            if [ "$NOW_EPOCH" -le "$NEXT_EPOCH" ]; then
                ok "next run: $NEXT_HUMAN (in $(( (NEXT_EPOCH - NOW_EPOCH) / 60 ))m, every ${INTERVAL}s)"
            else
                OVERDUE=$(( (NOW_EPOCH - NEXT_EPOCH) / 60 ))
                if launchctl list "$LABEL" >/dev/null 2>&1; then
                    warn "next run was $NEXT_HUMAN — ${OVERDUE}m overdue (launchd fires on wake/load)"
                else
                    bad "next run was $NEXT_HUMAN — ${OVERDUE}m overdue and the agent is not loaded"
                fi
            fi
            AGE=$(( (NOW_EPOCH - LAST_EPOCH) / 60 ))
            [ "$AGE" -gt $((INTERVAL * 3 / 60)) ] && warn "last run was ${AGE}m ago (> 3 intervals)"
        fi
    else
        bad "poll-state has no LAST_POLL"
    fi
else
    bad "poll-state missing: $STATE (agent has never run)"
fi

# ---------------------------------------------------------------- switcher daemon
head_ "== workspace-switcher daemon =="
if [ -x "$WS_BIN" ]; then
    STALE=0
    for src in "$WS_DIR/main.swift" "$WS_DIR/workspace_switcher.swift" "$WS_DIR/PopupWindow.swift"; do
        [ "$src" -nt "$WS_BIN" ] && STALE=1
    done
    if [ "$STALE" = 1 ]; then
        if [ "$FIX" = 1 ]; then
            if (mkdir -p "$WS_APP/Contents/MacOS" && cd "$WS_DIR" && swiftc -O -swift-version 5 -Xlinker -sectcreate -Xlinker __TEXT -Xlinker __info_plist -Xlinker Info.plist PopupWindow.swift workspace_switcher.swift main.swift -o "$WS_BIN" >/dev/null 2>&1); then
                codesign --force --sign - --identifier dev.danielbaker.workspace-switcher "$WS_APP" >/dev/null 2>&1
                ok "binary rebuilt (--fix): $WS_BIN"
                # a rebuilt daemon loses its TCC grants — re-grant mic + speech
                # (bundle-id grants persist across rebuilds)
                if "$JIRA_DIR/voice-permissions.sh" >/dev/null 2>&1; then
                    ok "voice permissions re-granted (mic + speech recognition)"
                else
                    warn "voice permissions could not be granted — run $JIRA_DIR/voice-permissions.sh"
                fi
            else
                bad "binary rebuild failed (see swiftc output)"
            fi
        else
            warn "binary older than sources — next hotkey rebuilds it (jira-doctor --fix rebuilds now)"
        fi
    else
        ok "binary fresh: $WS_BIN"
    fi
else
    bad "binary missing: $WS_BIN (jira-doctor --fix builds it)"
fi
if pgrep -f "workspace-switcher" >/dev/null 2>&1; then
    ok "daemon running (pid $(pgrep -f 'workspace-switcher' | head -1))"
else
    warn "daemon not running — the next Hyper+S/J/N keypress starts it"
fi
if grep -q 'copy-fields' "$WS_DIR/commands.conf" 2>/dev/null; then
    ok "commands.conf: copy-fields configured (jira row copy)"
else
    warn "commands.conf: no copy-fields — jira shows no copy checkboxes"
fi
if grep -q 'test %{app-name} = workspace-switcher' "$TOML" 2>/dev/null; then
    ok "aerospace.toml: workspace-switcher windows float"
else
    bad "aerospace.toml: no floating rule for app-name workspace-switcher"
fi
if [ -x "$WS_DIR/focus-bridge.sh" ]; then
    ok "focus-bridge.sh executable (aerospace focus -> daemon self-activate)"
else
    bad "focus-bridge.sh missing or not executable: $WS_DIR/focus-bridge.sh"
fi
if grep -q 'focus-bridge.sh' "$TOML" 2>/dev/null; then
    ok "aerospace.toml: on-focus-changed wires focus-bridge.sh"
else
    bad "aerospace.toml: on-focus-changed does not run focus-bridge.sh"
fi
if grep -q 'workspace_switcher.sh' "$KARAB" 2>/dev/null; then
    ok "karabiner: Hyper+S/J/N bound to workspace_switcher.sh"
else
    warn "karabiner: no workspace_switcher.sh bindings found"
fi
if [ -f "$WS_DIR/jira_icon.png" ]; then
    ok "jira icon asset: $WS_DIR/jira_icon.png"
else
    warn "jira_icon.png missing — jira window falls back to the SF Symbol tile"
fi

# ---------------------------------------------------------------- voice notes
head_ "== voice notes (Apple speech recognition) =="
if grep -q '^\[voice\]' "$WS_DIR/commands.conf" 2>/dev/null; then
    ok "commands.conf: voice-to-text window configured ([voice] section)"
else
    warn "commands.conf: no [voice] section — the voice window is unavailable"
fi
DICT="$(defaults read com.apple.speech.recognition.AppleSpeechRecognition.prefs DictationEnabled 2>/dev/null)"
if [ "$DICT" = "1" ]; then
    ok "Siri & Dictation enabled (on-device transcription available)"
    OFFONLY="$(defaults read com.apple.speech.recognition.AppleSpeechRecognition.prefs DictationIMUseOnlyOfflineDictation 2>/dev/null)"
    if [ "$OFFONLY" = "1" ]; then
        ok "dictation set to offline-only (no network needed)"
    else
        ok "dictation may use Apple's servers (offline-only is OFF)"
    fi
else
    warn "Siri & Dictation DISABLED — voice notes transcribe over the NETWORK only"
    printf '    enable: System Settings > Apple Intelligence & Siri > Siri & Dictation\n'
    printf '    (on-device transcription needs this ON; network dictation still works)\n'
fi

# ---------------------------------------------------------------- summary
head_ "== summary =="
printf '  \033[1m%d passed\033[0m, \033[31m%d failed\033[0m, \033[33m%d warnings\033[0m\n' \
    "$PASS" "$FAIL" "$WARN"
if [ "$FAIL" -gt 0 ]; then
    printf '  fix with: %s --fix\n' "$0"
    exit 1
fi
exit 0
