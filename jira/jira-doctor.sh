#!/usr/bin/env bash
# jira-doctor.sh — ONE command that heartbeats the whole workspace-switcher
# stack: menu bar (sketchybar + borders + karabiner), aerospace, the
# workspace-switcher daemon, permissions (mic/speech/dictation) — and, ONLY
# if jira is enabled in commands.conf, the jira section (config, API, poll
# agent, schedule, window json). Disabling jira must never disable the
# heartbeat: every non-jira check runs regardless.
#
#   jira-doctor            read-only heartbeat (exit 1 if anything fails)
#   jira-doctor --fix      also repairs what it can: starts brew services,
#                          re-grants mic/speech, loads the poll agent, and
#                          rebuilds a stale switcher binary
#
# Every check prints PASS/FAIL/WARN with the value it saw, so the report is
# the documentation of "what needs to be true".
set -uo pipefail

FIX=0
[ "${1:-}" = "--fix" ] && FIX=1

WS_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WS_APP="$WS_ROOT/workspace-switcher.app"
WS_BIN="$WS_APP/Contents/MacOS/workspace-switcher"
CONF="$HOME/.config/jira/config"
CACHE="$HOME/.cache/jira"
STATE="$CACHE/poll-state"
JSON_DIR="$HOME/.cache/workspace-switcher/jira_json"
PLIST_SRC="$WS_ROOT/jira/com.jira.poll.plist"
PLIST_DST="$HOME/Library/LaunchAgents/com.jira.poll.plist"
LABEL="com.jira.poll"
TOML="$HOME/.config/aerospace/aerospace.toml"
KARAB="$HOME/.config/karabiner/karabiner.json"
SKETCH_DIR="$HOME/.config/sketchybar"
WS_SOCKET="${TMPDIR:-/tmp}"
WS_SOCKET="${WS_SOCKET%/}/ws-notes.sock"
TCC_DB="$HOME/Library/Application Support/com.apple.TCC/TCC.db"

PASS=0; FAIL=0; WARN=0
ok()   { printf '  \033[32mPASS\033[0m  %s\n' "$*"; PASS=$((PASS + 1)); }
bad()  { printf '  \033[31mFAIL\033[0m  %s\n' "$*"; FAIL=$((FAIL + 1)); }
warn() { printf '  \033[33mWARN\033[0m  %s\n' "$*"; WARN=$((WARN + 1)); }
head_() { printf '\n\033[1;36m%s\033[0m\n' "$*"; }

running() { pgrep -x "$1" >/dev/null 2>&1; }

cfg() { grep "^$1=" "$CONF" 2>/dev/null | cut -d\' -f2; }

# ---------------------------------------------------------------- deps
head_ "== dependencies =="
for b in curl jq aerospace swiftc brew; do
    if command -v "$b" >/dev/null 2>&1; then ok "$b ($(command -v "$b"))"; else bad "$b missing"; fi
done

# ---------------------------------------------------------------- menu bar
head_ "== menu bar (sketchybar + borders + karabiner) =="
if running sketchybar; then
    ok "sketchybar running (pid $(pgrep -x sketchybar | head -1))"
else
    if [ "$FIX" = 1 ] && brew services start sketchybar >/dev/null 2>&1; then
        ok "sketchybar started (--fix)"
    else
        bad "sketchybar NOT running (jira-doctor --fix starts it)"
    fi
fi
for f in colors.sh plugins/aerospacer.sh; do
    if [ -f "$SKETCH_DIR/$f" ]; then ok "sketchybar config: $f"; else bad "sketchybar config missing: $SKETCH_DIR/$f"; fi
done
if running borders; then
    ok "borders running (pid $(pgrep -x borders | head -1))"
else
    if [ "$FIX" = 1 ] && brew services start borders >/dev/null 2>&1; then
        ok "borders started (--fix)"
    else
        bad "borders NOT running (jira-doctor --fix starts it)"
    fi
fi
for s in sketchybar borders; do
    if brew services list 2>/dev/null | grep -q "^$s[[:space:]]*started"; then
        ok "brew service $s: started"
    else
        warn "brew service $s: not started (brew services start $s)"
    fi
done
if pgrep -f "Karabiner-Core-Service" >/dev/null 2>&1; then
    ok "karabiner running (Hyper key active)"
else
    warn "karabiner NOT running — Hyper shortcuts (switcher/notes/jira) dead"
fi
if [ -f "$KARAB" ]; then
    ok "karabiner config: $KARAB"
else
    bad "karabiner config missing: $KARAB"
fi
if fc-list 2>/dev/null | grep -i "Hack Nerd Font" >/dev/null; then
    ok "Hack Nerd Font installed (terminal drawer glyphs)"
else
    warn "Hack Nerd Font not found — terminal drawer shows fallback glyphs"
fi

# ---------------------------------------------------------------- aerospace
head_ "== aerospace =="
NW="$(aerospace list-workspaces --all 2>/dev/null | wc -l | tr -d ' ')"
if [ "$NW" -gt 0 ]; then
    ok "aerospace IPC OK ($NW workspaces)"
else
    bad "aerospace IPC failed — is AeroSpace running? (list-workspaces returned nothing)"
fi
if [ -f "$TOML" ]; then
    ok "aerospace config: $TOML"
else
    bad "aerospace config missing: $TOML"
fi
if grep -q 'test %{app-name} = workspace-switcher' "$TOML" 2>/dev/null; then
    ok "aerospace.toml: workspace-switcher windows float"
else
    bad "aerospace.toml: no floating rule for app-name workspace-switcher"
fi
if [ -x "$WS_ROOT/bin/focus-bridge.sh" ]; then
    ok "focus-bridge.sh executable (aerospace focus -> daemon self-activate)"
else
    bad "focus-bridge.sh missing or not executable: $WS_ROOT/bin/focus-bridge.sh"
fi
if grep -q 'focus-bridge.sh' "$TOML" 2>/dev/null; then
    ok "aerospace.toml: on-focus-changed wires focus-bridge.sh"
else
    bad "aerospace.toml: on-focus-changed does not run focus-bridge.sh"
fi

# ---------------------------------------------------------------- daemon
head_ "== workspace-switcher daemon =="
if [ -x "$WS_BIN" ]; then
    STALE=0
    for src in "$WS_ROOT/main.swift" "$WS_ROOT/workspace_switcher.swift" "$WS_ROOT/PopupWindow.swift"; do
        [ "$src" -nt "$WS_BIN" ] && STALE=1
    done
    if [ "$STALE" = 1 ]; then
        if [ "$FIX" = 1 ]; then
            # SwiftTerm is precompiled once (like bin/workspace_switcher.sh does) — build
            # it here too if a repair is the first build on this machine.
            if [ ! -f "$WS_ROOT/.build/SwiftTerm/libSwiftTerm.a" ]; then
                mkdir -p "$WS_ROOT/.build/SwiftTerm"
                (cd "$WS_ROOT" && swiftc -O -swift-version 5 -parse-as-library -emit-library -static -module-name SwiftTerm \
                    Vendor/SwiftTerm/Sources/SwiftTerm/*.swift \
                    Vendor/SwiftTerm/Sources/SwiftTerm/Apple/*.swift \
                    Vendor/SwiftTerm/Sources/SwiftTerm/Apple/Metal/*.swift \
                    Vendor/SwiftTerm/Sources/SwiftTerm/Mac/*.swift \
                    Vendor/SwiftTerm/Sources/SwiftTerm/Portable/*.swift \
                    Vendor/SwiftTerm/Generated/*.swift \
                    -emit-module -emit-module-path .build/SwiftTerm/SwiftTerm.swiftmodule \
                    -o .build/SwiftTerm/libSwiftTerm.a >/dev/null 2>&1)
            fi
            if (mkdir -p "$WS_APP/Contents/MacOS" && cd "$WS_ROOT" && swiftc -O -swift-version 5 -Xlinker -sectcreate -Xlinker __TEXT -Xlinker __info_plist -Xlinker Info.plist \
                -I .build/SwiftTerm -Xlinker .build/SwiftTerm/libSwiftTerm.a \
                PopupWindow.swift workspace_switcher.swift main.swift -o "$WS_BIN" >/dev/null 2>&1); then
                # keep the bundle Info.plist (NSServices -> Finder right-click)
                # in sync with the rebuilt binary
                cp "$WS_ROOT/Info.plist" "$WS_APP/Contents/Info.plist"
                codesign --force --sign - --identifier dev.danielbaker.workspace-switcher "$WS_APP" >/dev/null 2>&1
                ok "binary rebuilt (--fix): $WS_BIN"
                # a rebuilt daemon loses its TCC grants — re-grant mic + speech
                # (bundle-id grants persist across rebuilds)
                if "$WS_ROOT/bin/voice-permissions.sh" >/dev/null 2>&1; then
                    ok "voice permissions re-granted (mic + speech recognition)"
                else
                    warn "voice permissions could not be granted — run $WS_ROOT/bin/voice-permissions.sh"
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
if pgrep -f "workspace-switcher.app" >/dev/null 2>&1; then
    ok "daemon running (pid $(pgrep -f 'workspace-switcher.app' | head -1))"
else
    warn "daemon not running — the next Hyper+S/J/N keypress starts it"
fi
if [ -S "$WS_SOCKET" ]; then
    if printf 'ping' | nc -U "$WS_SOCKET" >/dev/null 2>&1; then
        ok "daemon socket alive: $WS_SOCKET"
    else
        bad "daemon socket present but not accepting: $WS_SOCKET (daemon wedged?)"
    fi
else
    warn "daemon socket missing: $WS_SOCKET (daemon never started)"
fi
if [ -f "$WS_ROOT/commands.conf" ]; then
    if grep -q '^\[app\]' "$WS_ROOT/commands.conf"; then
        ok "commands.conf: [app] section present"
    else
        bad "commands.conf: no [app] section"
    fi
else
    bad "commands.conf missing: $WS_ROOT/commands.conf"
fi
for s in workspace_switcher.sh voice-permissions.sh focus-bridge.sh; do
    if [ -x "$WS_ROOT/bin/$s" ]; then ok "bin/$s present"; else bad "bin/$s missing or not executable"; fi
done
if grep -q 'caps_lock' "$KARAB" 2>/dev/null && grep -qi 'Hyper' "$KARAB" 2>/dev/null; then
    ok "karabiner: caps_lock -> Hyper mapping present"
else
    warn "karabiner: no caps_lock -> Hyper mapping found"
fi
if grep -q 'workspace_switcher.sh' "$TOML" 2>/dev/null; then
    ok "aerospace: Hyper+S bound to workspace_switcher.sh"
else
    warn "aerospace: no workspace_switcher.sh keybinding found"
fi

# ---------------------------------------------------------------- permissions
head_ "== permissions (mic + speech) =="
if [ -r "$TCC_DB" ]; then
    for svc in Microphone SpeechRecognition; do
        N="$(sqlite3 "$TCC_DB" "select count(*) from access where service='kTCCService$svc' and client like '%workspace-switcher%' and auth_value=2" 2>/dev/null)"
        if [ "${N:-0}" -ge 1 ]; then
            ok "TCC $svc: granted"
        else
            bad "TCC $svc: NOT granted — run bin/voice-permissions.sh (or System Settings > Privacy & Security)"
        fi
    done
else
    warn "cannot read TCC db ($TCC_DB) — run bin/voice-permissions.sh and check System Settings"
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

# ---------------------------------------------------------------- jira (optional)
head_ "== jira (optional) =="
JIRA_ENABLED="$(awk '/^\[jira\]/{f=1;next} /^\[/{f=0} f&&/^enabled/{print $3}' "$WS_ROOT/commands.conf" 2>/dev/null)"
if [ "$JIRA_ENABLED" = "true" ]; then
    if [ -f "$CONF" ]; then
        ok "config: $CONF"
        for k in JIRA_SITE JIRA_EMAIL JIRA_TOKEN; do
            if [ -n "$(cfg "$k")" ]; then ok "$k set"; else bad "$k missing in $CONF"; fi
        done
    else
        bad "config missing: $CONF (run: jira-api --init)"
    fi
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
    for s in jira-api.sh jira-poll.sh; do
        if [ -x "$WS_ROOT/jira/$s" ]; then ok "jira/$s present"; else bad "jira/$s missing or not executable"; fi
    done
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
    if [ -f "$WS_ROOT/jira_icon.png" ]; then
        ok "jira icon asset: $WS_ROOT/jira_icon.png"
    else
        warn "jira_icon.png missing — jira window falls back to the SF Symbol tile"
    fi
else
    ok "jira disabled in commands.conf ([jira] enabled = false) — jira checks skipped"
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