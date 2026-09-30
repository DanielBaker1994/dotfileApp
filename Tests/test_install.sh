#!/usr/bin/env bash
# test_install.sh — the two install kinds and the hand-over between them,
# against a throwaway $HOME (nothing of yours is touched).
#
#   Tests/test_install.sh            needs the dist bundle (.build/dist):
#                                    bin/build-app.sh --dist  (or make-dmg.sh)
#
# Covers bin/setup-home.sh (fresh app install, idempotent re-run, moved app,
# seeded files: untouched -> updated, edited -> kept + .new, deleted rule
# stays deleted), symlinks.sh in both modes (backup of a file in the way),
# bin/preflight.sh --json (missing Apple model = a warning, never a failure),
# app -> repo and repo -> app.
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
. "$ROOT/install.conf"
DIST="$ROOT/.build/dist/$APP_NAME.app"
[ -d "$DIST" ] || { echo "no dist bundle — run bin/build-app.sh --dist first" >&2; exit 2; }

T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
export HOME="$T/home"
export WS_NO_SERVICES=1
unset WS_HOME WS_LINK_ROOT
mkdir -p "$HOME/.config" "$T/Applications"
APP="$T/Applications/$APP_NAME.app"
cp -Rp "$DIST" "$APP"
WSH="$HOME/.config/workspace-switcher"
SETUP="$APP/Contents/Resources/bin/setup-home.sh"

PASS=0; FAIL=0
t() {   # description, then a command that must succeed
    local d="$1"; shift
    if "$@" >/dev/null 2>&1; then PASS=$((PASS + 1)); printf '  \033[32m✔\033[0m %s\n' "$d"
    else FAIL=$((FAIL + 1)); printf '  \033[31m✘ %s\033[0m\n' "$d"; fi
}
is_link_to() { [ -L "$1" ] && [ "$(realpath "$1")" = "$(realpath "$2")" ]; }
real_file()  { [ -f "$1" ] && [ ! -L "$1" ]; }

echo "fresh app install"
OUT="$(bash "$SETUP" app "$APP")"
t "setup-home app succeeds" grep -q '^result=ok' <<<"$OUT"
t "commands.toml is the user's own file" real_file "$WSH/commands.toml"
t "no personal note paths in the default" bash -c "! grep -q 'ZimaSetup\|fake.json' '$WSH/commands.toml'"
t "rules seeded" real_file "$WSH/rules/grammar-check.md"
t "configs are copies, not links into the app" real_file "$WSH/config/aerospace/aerospace.toml"
t "bin resolves into the bundle" is_link_to "$WSH/bin" "$APP/Contents/Resources/bin"
t "jira resolves into the bundle" test -f "$WSH/jira/jira_poll.py"
t "hotkey path reaches the binary" test -x "$WSH/$APP_NAME.app/Contents/MacOS/$APP_NAME"
t "marker says app" grep -q '^mode=app' "$WSH/.install"
t "owner = app" test "$(bash "$SETUP" status)" = app

echo "re-run"
BEFORE="$(cat "$WSH/.install")"
bash "$SETUP" app "$APP" >/dev/null
t "marker unchanged" test "$BEFORE" = "$(cat "$WSH/.install")"

echo "moved app"
mkdir -p "$T/Elsewhere"
mv "$APP" "$T/Elsewhere/"
APP="$T/Elsewhere/$APP_NAME.app"; SETUP="$APP/Contents/Resources/bin/setup-home.sh"
t "links dangle before healing" test ! -e "$WSH/bin/setup-home.sh"
bash "$SETUP" app "$APP" >/dev/null
t "links heal" test -f "$WSH/bin/setup-home.sh"
t "marker follows the app" grep -q "^app=$APP\$" "$WSH/.install"

echo "new version: seeded files"
echo "my own line" >> "$WSH/rules/grammar-check.md"
rm "$WSH/rules/ask.md"
RES="$APP/Contents/Resources"
echo "# new default" >> "$RES/rules/grammar-check.md"
echo "# new default" >> "$RES/rules/markdown-format.md"
sed -i '' 's/^APP_VERSION=.*/APP_VERSION="9.9.9"/' "$RES/install.conf"
OUT="$(bash "$SETUP" app "$APP")"
t "edited rule kept" grep -q 'my own line' "$WSH/rules/grammar-check.md"
t "its new default written beside it" grep -q 'new default' "$WSH/rules/grammar-check.md.new"
t "reported to the caller" grep -q '^new=.*grammar-check.md.new' <<<"$OUT"
t "untouched rule follows the default" grep -q 'new default' "$WSH/rules/markdown-format.md"
t "deleted rule stays deleted" test ! -e "$WSH/rules/ask.md"
t "version recorded" grep -q '^version=9.9.9' "$WSH/.install"

echo "stack: config links"
mkdir -p "$HOME/.config/aerospace"
echo "mine" > "$HOME/.config/aerospace/aerospace.toml"
OUT="$(bash "$SETUP" stack 2>&1)"
t "stack succeeds" grep -q 'result=ok' <<<"$OUT"
t "aerospace.toml links to the home's copy" is_link_to "$HOME/.config/aerospace/aerospace.toml" "$WSH/config/aerospace/aerospace.toml"
t "sketchybarrc linked" is_link_to "$HOME/.config/sketchybar/sketchybarrc" "$WSH/config/sketchybar/sketchybarrc"
t "the file in the way was backed up" bash -c "grep -rqx mine '$HOME/.config/workspace-switcher-backups'"
t "helpers precompiled into the cache" test -x "$HOME/.cache/sketchybar/menubar_watch"
t "symlinks --check passes" env WS_LINK_ROOT="$WSH" bash "$RES/symlinks.sh" --check

echo "preflight"
sed -i '' 's|^fm-bin *=.*|fm-bin = "/nonexistent/fm"|' "$WSH/commands.toml"
JSON="$(bash "$RES/bin/preflight.sh" --json --mode app --app "$APP")"; RC=$?
t "valid JSON" python3 -c 'import json,sys; json.loads(sys.argv[1])' "$JSON"
t "missing Apple model is a warning" python3 -c '
import json, sys
c = {x["id"]: x for x in json.loads(sys.argv[1])["checks"]}["apple-model"]
assert c["ok"] is False and c["level"] == "warn" and "AI view" in c["fix"]' "$JSON"
t "and does not fail the preflight" python3 -c '
import json, sys
d = json.loads(sys.argv[1])
bad = [x["id"] for x in d["checks"] if x["level"] == "required" and not x["ok"]]
assert (int(sys.argv[2]) == 0) == (not bad)
assert "apple-model" not in bad' "$JSON" "$RC"
JSON="$(bash "$RES/bin/preflight.sh" --json --mode app --app "/Volumes/ws/$APP_NAME.app")"
t "running from the disk image is required-failed" python3 -c '
import json, sys
c = {x["id"]: x for x in json.loads(sys.argv[1])["checks"]}["location"]
assert not c["ok"] and c["level"] == "required" and c["action"] == "move-app"' "$JSON"
t "setup-home refuses the disk image" bash -c "! bash '$SETUP' app '/Volumes/ws/$APP_NAME.app'"

echo "app -> repo"
REPO="$T/checkout"
mkdir -p "$REPO/bin" "$REPO/.git"
cp "$ROOT/install.conf" "$ROOT/symlinks.sh" "$REPO/"
cp "$ROOT/bin/setup-home.sh" "$REPO/bin/"
cp -R "$ROOT/config" "$REPO/config"
echo "# the checkout's config" > "$REPO/commands.toml"
echo "# edited in the app install" >> "$WSH/commands.toml"
OUT="$(bash "$REPO/bin/setup-home.sh" repo)"
BK="$(sed -n 's/^backup=//p' <<<"$OUT")"
t "the app's home was moved to a backup" test -f "$BK/commands.toml"
t "with the user's edits" grep -q 'edited in the app install' "$BK/commands.toml"
t "backup is not in /tmp" bash -c "case '$BK' in '$HOME'/.config/workspace-switcher-backups/*) exit 0 ;; esac; exit 1"
ROOT="$REPO" bash "$REPO/symlinks.sh" --fix >/dev/null 2>&1
t "home links to the checkout" is_link_to "$WSH" "$REPO"
t "aerospace.toml now points into the checkout" is_link_to "$HOME/.config/aerospace/aerospace.toml" "$REPO/config/aerospace/aerospace.toml"
t "owner = repo" test "$(bash "$REPO/bin/setup-home.sh" status)" = repo

echo "repo -> app"
OUT="$(bash "$SETUP" app "$APP")"; RC=$?
t "a checkout is left alone" test "$RC" = 3
t "still the link" is_link_to "$WSH" "$REPO"
echo "# edited in the checkout" >> "$REPO/config/aerospace/aerospace.toml"
OUT="$(bash "$SETUP" app "$APP" --switch)"
t "--switch hands over" grep -q '^result=ok' <<<"$OUT"
t "home is a real directory" bash -c "[ -d '$WSH' ] && [ ! -L '$WSH' ]"
t "the checkout is kept" test -f "$REPO/commands.toml"
t "its commands.toml carried over" grep -q "the checkout's config" "$WSH/commands.toml"
t "its config edits carried over" grep -q 'edited in the checkout' "$WSH/config/aerospace/aerospace.toml"
t "the checkout itself is untouched by the app" test ! -e "$REPO/bin/jira"
bash "$SETUP" stack >/dev/null 2>&1
t "links re-pointed at the home" is_link_to "$HOME/.config/aerospace/aerospace.toml" "$WSH/config/aerospace/aerospace.toml"

echo "repo -> app (the checkout IS the home)"
rm -rf "$WSH"; cp -R "$REPO" "$WSH"
OUT="$(bash "$SETUP" app "$APP" --switch)"
KEPT="$(sed -n 's/^kept=//p' <<<"$OUT")"
t "checkout renamed, not deleted" test -d "$KEPT/.git"
t "home set up for the app" grep -q '^mode=app' "$WSH/.install"

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" = 0 ]
