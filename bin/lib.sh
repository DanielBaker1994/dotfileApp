#!/usr/bin/env bash
# lib.sh — the helper functions behind the ./ws entry point.
#
# Sourced by ./ws and by the deprecated wrapper scripts (build.sh,
# bin/make-dmg.sh, bin/fix-permissions.sh, bin/fake-*.sh) so the old names keep
# working. The fixed-path scripts the app / aerospace / install.conf invoke
# (kitchen_sink.sh, setup-home.sh, preflight.sh, build-app.sh, …) do NOT source
# this: they are the internal contract and stay independent.
set -uo pipefail

_ws_lib_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
WS_ROOT="$(cd "$_ws_lib_dir/.." && pwd -P)"
# shellcheck source=../install.conf
. "$WS_ROOT/install.conf"
# hotkey tools / stripped environments run with a minimal PATH
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"

# ------------------------------------------------------------------ output
ws_ok()   { printf '\033[32m  \342\234\224 %s\033[0m\n' "$*"; }
ws_fail() { printf '\033[31m  \342\234\230 %s\033[0m\n' "$*"; }
ws_warn() { printf '\033[33m  ! %s\033[0m\n' "$*"; }
ws_step() { printf '\n\033[1;36m== %s ==\033[0m\n' "$*"; }
ws_info() { printf '\033[2m    %s\033[0m\n' "$*"; }
ws_die()  { printf '\033[31mws: %s\033[0m\n' "$*" >&2; exit 1; }

# =============================================================== build
# compile + relaunch (was build.sh). The launcher only builds on a cold start
# (hotkeys never pay for the stale check), so build here: build-app.sh kills the
# old daemon after a new binary, kitchen_sink.sh then starts the fresh one.
ws_cmd_build() {
    if [ "${1:-}" = "--force" ]; then
        "$WS_ROOT/bin/build-app.sh" --force || exit 1
        shift
    elif [ "${1:-}" != "--build-only" ]; then
        "$WS_ROOT/bin/build-app.sh" || exit 1
    fi
    if [ "${1:-}" = "--build-only" ]; then
        export WS_BUILD_ONLY=1
    fi
    exec "$WS_ROOT/bin/kitchen_sink.sh" window
}

# =============================================================== dmg
# the distributable disk image (was bin/make-dmg.sh)
ws_cmd_dmg() {
    local notarize=1
    [ "${1:-}" = "--no-notarize" ] && notarize=0
    local OUT="$WS_ROOT/.build/dist"
    local APP="$OUT/$APP_NAME.app"
    local DMG="$OUT/$DMG_NAME-$APP_VERSION.dmg"
    local STAGE="$OUT/dmg-stage"

    if [ -n "$DEVELOPER_ID" ]; then
        security find-identity -v -p codesigning 2>/dev/null | grep -qF "$DEVELOPER_ID" \
            || ws_die "DEVELOPER_ID '$DEVELOPER_ID' is not in the keychain (security find-identity -v -p codesigning)"
    fi

    ws_step "building the app bundle (bin/build-app.sh --dist)"
    "$WS_ROOT/bin/build-app.sh" --dist || ws_die "build failed"
    codesign --verify --deep --strict "$APP" 2>&1 || ws_die "the bundle's signature does not verify"

    ws_step "disk image"
    rm -rf "$STAGE" "$DMG"
    mkdir -p "$STAGE"
    cp -Rp "$APP" "$STAGE/"
    ln -s /Applications "$STAGE/Applications"
    hdiutil create -quiet -volname "$APP_NAME $APP_VERSION" -srcfolder "$STAGE" \
        -fs APFS -format UDZO -ov "$DMG" || ws_die "hdiutil failed"
    rm -rf "$STAGE"

    if [ -n "$DEVELOPER_ID" ]; then
        codesign --force --timestamp --sign "$DEVELOPER_ID" "$DMG" || ws_die "signing the disk image failed"
        if [ "$notarize" = 1 ]; then
            [ -n "$NOTARY_PROFILE" ] || ws_die "NOTARY_PROFILE is empty (xcrun notarytool store-credentials NAME …, then set it in install.conf)"
            ws_step "notarizing (Apple's notary service — a few minutes)"
            xcrun notarytool submit "$DMG" --keychain-profile "$NOTARY_PROFILE" --wait \
                || ws_die "notarization failed (xcrun notarytool log <id> --keychain-profile $NOTARY_PROFILE)"
            xcrun stapler staple "$DMG" || ws_die "stapling failed"
            spctl -a -t open --context context:primary-signature -v "$DMG" 2>&1 || ws_die "Gatekeeper rejects the disk image"
        fi
        printf '\033[32m  \342\234\224 %s (Developer ID%s)\033[0m\n' "$DMG" "$([ "$notarize" = 1 ] && echo ', notarized')"
    else
        local SIG HOW
        SIG="$(codesign -dvv "$APP" 2>&1)"
        case "$SIG" in
            *"Authority=$SIGN_ID"*) HOW="self-signed ($SIGN_ID)" ;;
            *) HOW="ad-hoc" ;;
        esac
        printf '\033[32m  \342\234\224 %s\033[0m\n' "$DMG"
        printf '\033[33m  ! %s, not notarized: on another Mac the first open is blocked —\n' "$HOW"
        printf '    System Settings ▸ Privacy & Security ▸ Open Anyway. Set DEVELOPER_ID +\n'
        printf '    NOTARY_PROFILE in install.conf for a build that opens with a double-click.\033[0m\n'
    fi
}

# =============================================================== permissions
ws_cmd_permissions() {
    case "${1:-grant}" in
        grant|"") exec "$WS_ROOT/bin/grant-permissions.sh" ;;
        fix)      ws_permissions_fix ;;
        *)        ws_die "usage: ws permissions grant|fix" ;;
    esac
}

# was bin/fix-permissions.sh
ws_permissions_fix() {
    local APP="$WS_ROOT/$APP_NAME.app"
    local KC="$HOME/Library/Keychains/login.keychain-db"
    local T rc PW ff r

    _ws_valid() { security find-identity -v -p codesigning 2>/dev/null | grep -qF "\"$SIGN_ID\""; }
    _ws_can_sign() {   # a real signing test: the only check that catches key-access problems
        ff="$(mktemp)"; cp /bin/echo "$ff"
        perl -e 'alarm 20; exec @ARGV' codesign --force --sign "$SIGN_ID" "$ff" >/dev/null 2>&1
        r=$?
        rm -f "$ff"; return $r
    }

    printf '\342\226\270 signing identity\n'
    if ! _ws_valid; then
        T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
        printf '[req]\ndistinguished_name=dn\nx509_extensions=ext\nprompt=no\n[dn]\nCN=%s\n[ext]\nbasicConstraints=critical,CA:false\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=critical,codeSigning\n' "$SIGN_ID" > "$T/c.cnf"
        # /usr/bin/openssl (LibreSSL): its .p12 imports cleanly, brew's v3 needs -legacy
        /usr/bin/openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -config "$T/c.cnf" -keyout "$T/k.pem" -out "$T/c.pem" 2>/dev/null \
          && /usr/bin/openssl pkcs12 -export -inkey "$T/k.pem" -in "$T/c.pem" -out "$T/c.p12" -passout pass:ws \
          && security import "$T/c.p12" -k "$KC" -P ws -T /usr/bin/codesign >/dev/null \
          && security add-trusted-cert -r trustRoot -p codeSign -k "$KC" "$T/c.pem" \
          || ws_die "could not create/trust '$SIGN_ID'"
        _ws_valid || ws_die "'$SIGN_ID' still not valid — Keychain Access ▸ it ▸ Trust ▸ Code Signing: Always Trust"
        echo "  created + trusted '$SIGN_ID'"
    fi
    if ! _ws_can_sign; then
        [ -t 0 ] || ws_die "codesign can't use the key and needs your login password — run in a normal Terminal"
        read -rs -p "  login password (to authorize the key for codesign): " PW; echo
        { security unlock-keychain -p "$PW" "$KC" \
          && security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$PW" "$KC"; } >/dev/null 2>&1
        rc=$?; unset PW
        [ $rc = 0 ] && _ws_can_sign || ws_die "codesign still can't use '$SIGN_ID' (wrong password?) — delete it in Keychain Access and re-run"
    fi
    echo "  \342\234\224 '$SIGN_ID' usable"

    echo "\342\226\270 clearing stale Screen Recording grant"
    tccutil reset ScreenCapture "$BUNDLE_ID" >/dev/null 2>&1

    echo "\342\226\270 rebuilding"
    "$WS_ROOT/ws" build --force --build-only || ws_die "build failed"
    codesign -dvv "$APP" 2>&1 | grep -qF "Authority=$SIGN_ID" && ! codesign -d -r- "$APP" 2>&1 | grep -q cdhash \
        || ws_die "app is ad-hoc signed — grants would reset on every rebuild"
    echo "  \342\234\224 signed with '$SIGN_ID'"

    pkill -f "$APP_NAME.app/Contents/MacOS" 2>/dev/null; sleep 1
    "$WS_ROOT/bin/kitchen_sink.sh" window >/dev/null 2>&1 &
    open "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
    echo; echo "\342\234\224 done — press Hyper+X and click Allow once (quit + reopen the app if it still complains)."
}

# =============================================================== fake fixtures
ws_cmd_fake() {
    case "${1:-}" in
        confluence)         shift; ws_fake_confluence "$@" ;;
        jira|jira-tab)      shift; ws_fake_jira "$@" ;;
        *)                  ws_die "usage: ws fake confluence|jira [start|stop|status]" ;;
    esac
}

# was bin/fake-confluence.sh
ws_fake_confluence() {
    local ROOT="$WS_ROOT"
    local PORT="${FAKE_CONF_PORT:-8765}"
    local RUN_DIR="$HOME/.cache/confluence"
    local PID_FILE="$RUN_DIR/fake.pid"
    local LOG_FILE="$RUN_DIR/fake.log"
    local FAKE_CFG="$HOME/.config/confluence/fake.json"

    _fc_running() { [ -f "$PID_FILE" ] && kill -0 "$(cat "$PID_FILE")" 2>/dev/null; }
    _fc_set_conf() {  # VALUE|"" -> [confluence] config
        python3 - "$1" <<PY
import sys
sys.path.insert(0, "$ROOT/confluence")
import confluence_config as cc
cc.set_section_value("confluence", "config", sys.argv[1] or None)
PY
    }
    _fc_write_cfg() {
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
    }

    case "${1:-status}" in
    start)
        mkdir -p "$RUN_DIR" "$(dirname "$FAKE_CFG")"
        if _fc_running; then
            echo "fake Confluence already running (pid $(cat "$PID_FILE"))"
        else
            nohup python3 "$ROOT/confluence/fake_confluence.py" serve --port "$PORT" --context /wiki \
                >"$LOG_FILE" 2>&1 &
            echo $! >"$PID_FILE"
            for _ in $(seq 1 30); do
                curl -s -o /dev/null "http://127.0.0.1:$PORT/wiki/" && break
                sleep 0.1
            done
            _fc_running || { echo "fake Confluence failed to start - see $LOG_FILE" >&2; exit 1; }
            echo "fake Confluence: http://127.0.0.1:$PORT/wiki (pid $(cat "$PID_FILE"), log $LOG_FILE)"
        fi
        _fc_write_cfg || ws_die "could not write the fake config"
        _fc_set_conf "~/.config/confluence/fake.json" || ws_die "could not point [confluence] config at the fake site"
        echo "app now uses $FAKE_CFG ([confluence] config); 'ws fake confluence stop' switches back"
        ;;
    stop)
        if _fc_running; then
            kill "$(cat "$PID_FILE")" && echo "fake Confluence stopped"
        else
            echo "fake Confluence was not running"
        fi
        rm -f "$PID_FILE"
        _fc_set_conf "" || ws_die "could not reset [confluence] config"
        echo "app uses ~/.config/confluence/config.json again"
        ;;
    status)
        if _fc_running; then echo "running: http://127.0.0.1:$PORT/wiki (pid $(cat "$PID_FILE"))"; else echo "not running"; fi
        python3 "$ROOT/confluence/confluence_api.py" --check
        ;;
    *)
        ws_die "usage: ws fake confluence [start|stop|status]"
        ;;
    esac
}

# was bin/fake-jira-tab.sh
ws_fake_jira() {
    local ROOT="$WS_ROOT"
    local FAKE_DIR="$HOME/.cache/kitchen-sink/jira_fake"
    local ORIG="$FAKE_DIR/sources.orig"

    _fj_conf() {  # get | set VALUE|"" -> [jira] sources
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
        local n="${2:-20000}" cur
        mkdir -p "$FAKE_DIR"
        cur="$(_fj_conf get)"
        # remember the real value once (a second start keeps the first one)
        [ -f "$ORIG" ] || [ "$cur" = "~/.cache/kitchen-sink/jira_fake" ] || printf '%s' "$cur" >"$ORIG"
        rm -f "$FAKE_DIR"/bench-*.json
        python3 "$ROOT/jira/fake_jira_tab.py" "$FAKE_DIR/bench-$n.json" --count "$n" \
            || ws_die "could not write the fake tab"
        _fj_conf set "~/.cache/kitchen-sink/jira_fake" || ws_die "could not point [jira] sources at the fake tab"
        echo "Jira window now reads $FAKE_DIR; 'ws fake jira stop' switches back"
        ;;
    stop)
        if [ -f "$ORIG" ]; then
            _fj_conf set "$(cat "$ORIG")" || ws_die "could not restore [jira] sources"
            rm -f "$ORIG"
            echo "[jira] sources = $(_fj_conf get)"
        else
            echo "fake tab was not active ([jira] sources = $(_fj_conf get))"
        fi
        ;;
    status)
        echo "[jira] sources = $(_fj_conf get)"
        ls -la "$FAKE_DIR" 2>/dev/null || true
        ;;
    *)
        ws_die "usage: ws fake jira [start [N]|stop|status]"
        ;;
    esac
}

# =============================================================== passthrough
ws_cmd_install()   { exec "$WS_ROOT/INSTALL.sh" "$@"; }
ws_cmd_uninstall() { exec "$WS_ROOT/UNINSTALL.sh" "$@"; }
ws_cmd_check()     { exec "$WS_ROOT/bin/preflight.sh" "$@"; }
ws_cmd_links()     { exec "$WS_ROOT/symlinks.sh" "$@"; }
ws_cmd_monitors()  { exec "$WS_ROOT/bin/aerospace-monitors.sh" "$@"; }
ws_cmd_home()      { exec "$WS_ROOT/bin/setup-home.sh" "$@"; }
ws_cmd_doctor()    { exec "$WS_ROOT/jira/jira-doctor.sh" "$@"; }

ws_cmd_test() {
    case "${1:-all}" in
        ui)      exec "$WS_ROOT/bin/ui-test.sh" "${@:2}" ;;
        vim)     exec "$WS_ROOT/bin/ui-test-vim.sh" "${@:2}" ;;
        focus)   exec "$WS_ROOT/bin/ui-test-focus.py" "${@:2}" ;;
        install) exec "$WS_ROOT/Tests/test_install.sh" "${@:2}" ;;
        *)       exec "$WS_ROOT/bin/run-tests.sh" "$@" ;;
    esac
}
