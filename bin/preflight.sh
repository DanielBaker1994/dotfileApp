#!/usr/bin/env bash
# preflight.sh — THE list of things this Mac needs, for both doors:
# INSTALL.sh (repo) prints it, the app's Setup window (DMG install) reads
# `--json`. One script so the two can never disagree.
#
#   bin/preflight.sh                  repo install, coloured text
#   bin/preflight.sh --mode app --app /Applications/workspace-switcher.app
#   bin/preflight.sh --json           one JSON object on stdout
#
# Exit 1 only when a REQUIRED check fails. Everything else is a warning: the
# app runs, a feature is off (no Apple model -> no AI view, no python3 -> no
# Jira / Confluence / notification counts, no Homebrew stack -> no hotkeys).
#
# Each check: id, group (core | features | stack | dev), level (required |
# warn), ok, title, detail, fix (what to do, for people) and action (what the
# Setup window's Fix button runs: move-app | setup-home | brew:NAME |
# cask:NAME | stack | url:… | "").
set -uo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
ROOT="$(cd "$DIR/.." && pwd -P)"
# shellcheck source=../install.conf
. "$ROOT/install.conf"
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"

MODE=repo; APP=""; JSON=0
case "$ROOT" in *.app/Contents/Resources) MODE=app; APP="${ROOT%/Contents/Resources}" ;; esac
while [ $# -gt 0 ]; do
    case "$1" in
        --json) JSON=1 ;;
        --mode) MODE="${2:-}"; shift ;;
        --app)  APP="${2:-}"; shift ;;
        -h|--help) sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "preflight: unknown option $1" >&2; exit 2 ;;
    esac
    shift
done
WS_HOME="${WS_HOME:-$WS_HOME_DEFAULT}"
# the user's commands.toml: the repo's own, or the one in the home
CONF="$ROOT/commands.toml"
[ "$MODE" = app ] && CONF="$WS_HOME/commands.toml"
[ -f "$CONF" ] || CONF="$ROOT/commands.default.toml"

IDS=(); GROUPS_=(); LEVELS=(); OKS=(); TITLES=(); DETAILS=(); FIXES=(); ACTIONS=()
add() {   # id group level ok(0|1) title detail fix action
    IDS+=("$1"); GROUPS_+=("$2"); LEVELS+=("$3"); OKS+=("$4")
    TITLES+=("$5"); DETAILS+=("$6"); FIXES+=("${7:-}"); ACTIONS+=("${8:-}")
}

# value of KEY in [SECTION] of the config (first match, quotes stripped)
conf_value() {
    [ -f "$CONF" ] || return 0
    awk -v sec="[$1]" -v key="$2" '
        /^[ \t]*\[/ { cur = $0; gsub(/^[ \t]+|[ \t]+$/, "", cur); next }
        cur == sec {
            line = $0; sub(/^[ \t]+/, "", line)
            if (line ~ "^" key "[ \t]*=") {
                sub(/^[^=]*=[ \t]*/, "", line); gsub(/^"|"[ \t]*$/, "", line)
                print line; exit
            }
        }' "$CONF"
}

# ---------------------------------------------------------------- core
HOST="$(sw_vers -productVersion 2>/dev/null || echo '?')"
if "$DIR/check-macos.sh" >/dev/null 2>&1; then
    add macos core required 1 "macOS $MACOS_MIN or newer" "this Mac runs macOS $HOST"
else
    add macos core required 0 "macOS $MACOS_MIN or newer" "this Mac runs macOS $HOST" \
        "Update macOS in System Settings ▸ General ▸ Software Update." "url:x-apple.systempreferences:com.apple.Software-Update-Settings.extension"
fi

if [ "$MODE" = app ]; then
    ARCH="$(uname -m)"
    if [ "$ARCH" = arm64 ]; then
        add arch core required 1 "Apple silicon" "$ARCH"
    else
        add arch core required 0 "Apple silicon" "this Mac is $ARCH — the app is built for Apple silicon" \
            "Install from the git repository instead (./INSTALL.sh builds for this Mac)."
    fi
    case "$APP" in
        */AppTranslocation/*|/Volumes/*)
            add location core required 0 "Installed in Applications" \
                "running from the disk image — nothing can be saved from here" \
                "Move the app to the Applications folder." move-app ;;
        /Applications/*|"$HOME/Applications/"*)
            add location core required 1 "Installed in Applications" "$APP" ;;
        "")
            ;;
        *)
            add location core warn 0 "Installed in Applications" "running from $APP" \
                "Move the app to the Applications folder so updates and hotkeys find it." move-app ;;
    esac
fi

PARENT="$(dirname "$WS_HOME")"
if { [ -d "$WS_HOME" ] && [ -w "$WS_HOME" ]; } || { [ ! -e "$WS_HOME" ] && { [ -w "$PARENT" ] || [ ! -e "$PARENT" ]; }; }; then
    add home core required 1 "Settings folder writable" "${WS_HOME/#$HOME/~}"
else
    add home core required 0 "Settings folder writable" "cannot write ${WS_HOME/#$HOME/~}" \
        "Fix the folder's permissions: chmod u+w '${WS_HOME/#$HOME/~}'"
fi

# which install owns the home right now
MARK_MODE=""
[ -f "$WS_HOME/.install" ] && MARK_MODE="$(sed -n 's/^mode=//p' "$WS_HOME/.install" | head -1)"
if [ -z "$MARK_MODE" ] && { [ -L "$WS_HOME" ] || [ -e "$WS_HOME/.git" ]; }; then MARK_MODE=repo; fi
if [ -n "$MARK_MODE" ] && [ "$MARK_MODE" != "$MODE" ]; then
    if [ "$MODE" = app ]; then
        add other-install core warn 0 "No other install in the way" \
            "${WS_HOME/#$HOME/~} is a developer checkout (git)" \
            "Keep using the checkout, or switch this Mac to the installed app (the checkout is kept, your settings are carried over)." setup-home
    else
        add other-install core warn 0 "No other install in the way" \
            "${WS_HOME/#$HOME/~} belongs to the installed app (DMG)" \
            "INSTALL.sh takes it over: your commands.toml and rules are backed up first."
    fi
else
    add other-install core warn 1 "No other install in the way" "${MARK_MODE:-first install}"
fi

# a second copy of the app on disk: same bundle id -> LaunchServices and the
# privacy grants can pick either one
COPIES="$(mdfind "kMDItemCFBundleIdentifier == '$BUNDLE_ID'" 2>/dev/null \
    | grep -v -e '/\.build/' -e '/\.Trash/' -e '^/Volumes/' | sort -u)"
NCOPIES="$(printf '%s\n' "$COPIES" | grep -c . || true)"
if [ "${NCOPIES:-0}" -gt 1 ]; then
    add copies core warn 0 "One copy of the app" "$(printf '%s' "$COPIES" | tr '\n' ' ')" \
        "Keep one and move the others to the Trash."
else
    add copies core warn 1 "One copy of the app" "${COPIES:-not indexed yet}"
fi

# ------------------------------------------------------------- features
# Apple's on-device model (the AI view). Never required.
FM_BIN="$(conf_value ai fm-bin)"; FM_BIN="${FM_BIN:-/usr/bin/fm}"
FM_BIN="${FM_BIN/#\~/$HOME}"
if [ ! -x "$FM_BIN" ]; then
    add apple-model features warn 0 "Apple on-device model" "$FM_BIN not found" \
        "The AI view will not be usable on this Mac. Everything else works."
else
    FM_OUT="$(perl -e 'alarm 10; exec @ARGV' "$FM_BIN" available 2>&1 | head -1)"; FM_RC=${PIPESTATUS[0]}
    if [ "$FM_RC" = 0 ]; then
        add apple-model features warn 1 "Apple on-device model" "${FM_OUT:-available}"
    else
        add apple-model features warn 0 "Apple on-device model" "${FM_OUT:-not available}" \
            "The AI view will not be usable until Apple Intelligence is turned on (System Settings ▸ Apple Intelligence & Siri). Everything else works." \
            "url:x-apple.systempreferences:com.apple.Siri-Settings.extension"
    fi
fi

# python3: /usr/bin/python3 without the command-line tools is a stub that
# pops an install dialog when run — never run it to find out
PY="$(command -v python3 2>/dev/null || true)"
PY_OK=0; PY_DETAIL="not found"
if [ -n "$PY" ]; then
    if [ "$PY" = /usr/bin/python3 ] && ! xcode-select -p >/dev/null 2>&1; then
        PY_DETAIL="only Apple's placeholder (command-line tools not installed)"
    elif PY_DETAIL="$("$PY" -c 'import sys; print("python %d.%d" % sys.version_info[:2])' 2>/dev/null)"; then
        PY_OK=1; PY_DETAIL="$PY_DETAIL ($PY)"
    else
        PY_DETAIL="$PY does not run"
    fi
fi
if [ "$PY_OK" = 1 ]; then
    add python features warn 1 "python3" "$PY_DETAIL"
else
    add python features warn 0 "python3" "$PY_DETAIL" \
        "Jira, Confluence and the notification counts need python3: run  xcode-select --install  (or  brew install python)." "term:xcode-select --install"
fi

VIM_BIN="$(conf_value notes vim-bin)"; VIM_BIN="${VIM_BIN:-nvim}"; VIM_BIN="${VIM_BIN/#\~/$HOME}"
if command -v "$VIM_BIN" >/dev/null 2>&1; then
    add nvim features warn 1 "Neovim (notes editor)" "$(command -v "$VIM_BIN")"
else
    add nvim features warn 0 "Neovim (notes editor)" "$VIM_BIN not found" \
        "Notes fall back to the built-in editor. For the vim pane:  brew install neovim" brew:neovim
fi

# ---------------------------------------------------------------- stack
# hotkeys + menu bar: optional for an app install, part of the repo install
HAVE_BREW=0
if command -v brew >/dev/null 2>&1; then
    HAVE_BREW=1
    add brew stack warn 1 "Homebrew" "$(command -v brew)"
    FORMULAE=" $(brew list --formula -1 2>/dev/null | tr '\n' ' ') "
    CASKS=" $(brew list --cask -1 2>/dev/null | tr '\n' ' ') "
else
    add brew stack warn 0 "Homebrew" "not installed" \
        "Needed for the hotkeys and the menu bar (AeroSpace, sketchybar, borders). Install it from https://brew.sh — it asks for your password once." "url:https://brew.sh"
    FORMULAE=" "; CASKS=" "
fi
for f in $BREW_FORMULAE; do
    # a formula installed another way (own build, MacPorts) counts too
    if [[ "$FORMULAE" == *" $f "* ]] || command -v "$f" >/dev/null 2>&1 \
        || { [ "$f" = ripgrep ] && command -v rg >/dev/null 2>&1; }; then
        add "brew-$f" stack warn 1 "$f" "installed"
    else
        add "brew-$f" stack warn 0 "$f" "not installed" "brew install $f" "brew:$f"
    fi
done
for c in $BREW_CASKS; do
    if [[ "$CASKS" == *" $c "* ]]; then
        add "cask-$c" stack warn 1 "$c" "installed"
    else
        add "cask-$c" stack warn 0 "$c" "not installed" "brew install --cask $c" "cask:$c"
    fi
done
[ "$HAVE_BREW" = 1 ] || true

# the per-file config links (~/.config/aerospace/… -> the home's config/…)
if [ -f "$ROOT/symlinks.sh" ]; then
    # (app install: the copies live in the home; repo: in the checkout)
    LINK_ENV=(); [ "$MODE" = app ] && LINK_ENV=(WS_LINK_ROOT="$WS_HOME")
    if LINKS="$(env "${LINK_ENV[@]}" bash "$ROOT/symlinks.sh" --check 2>&1)"; then
        add links stack warn 1 "Config links" "all in place"
    else
        BAD="$(printf '%s\n' "$LINKS" | grep -c -e '✘' -e '·' || true)"
        add links stack warn 0 "Config links" "$BAD link(s) missing or pointing elsewhere" \
            "Link the aerospace / sketchybar / borders configs (anything in the way is backed up)." stack
    fi
fi

# ------------------------------------------------------------------ dev
if [ "$MODE" = repo ]; then
    if command -v swiftc >/dev/null 2>&1 && xcode-select -p >/dev/null 2>&1; then
        add swiftc dev required 1 "Swift compiler" "$(swiftc --version 2>/dev/null | head -1)"
    else
        add swiftc dev required 0 "Swift compiler" "not installed" \
            "Install the command-line tools:  xcode-select --install"
    fi
    if command -v git >/dev/null 2>&1 && xcode-select -p >/dev/null 2>&1; then
        add git dev required 1 "git" "$(git --version 2>/dev/null)"
    else
        add git dev required 0 "git" "not installed" "xcode-select --install"
    fi
fi

# ---------------------------------------------------------------- output
FAILED=0
for i in "${!IDS[@]}"; do
    [ "${LEVELS[$i]}" = required ] && [ "${OKS[$i]}" = 0 ] && FAILED=1
done

if [ "$JSON" = 1 ]; then
    esc() { printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' | tr '\n\t' '  '; }
    printf '{"mode":"%s","app":"%s","home":"%s","version":"%s","ok":%s,"checks":[' \
        "$MODE" "$(esc "$APP")" "$(esc "$WS_HOME")" "$APP_VERSION" "$([ "$FAILED" = 0 ] && echo true || echo false)"
    for i in "${!IDS[@]}"; do
        [ "$i" -gt 0 ] && printf ','
        printf '{"id":"%s","group":"%s","level":"%s","ok":%s,"title":"%s","detail":"%s","fix":"%s","action":"%s"}' \
            "${IDS[$i]}" "${GROUPS_[$i]}" "${LEVELS[$i]}" "$([ "${OKS[$i]}" = 1 ] && echo true || echo false)" \
            "$(esc "${TITLES[$i]}")" "$(esc "${DETAILS[$i]}")" "$(esc "${FIXES[$i]}")" "$(esc "${ACTIONS[$i]}")"
    done
    printf ']}\n'
    exit "$FAILED"
fi

group_title() {
    case "$1" in
        core) echo "this Mac" ;;
        features) echo "features (optional)" ;;
        stack) echo "hotkeys + menu bar" ;;
        dev) echo "building from source" ;;
    esac
}
LAST=""
for i in "${!IDS[@]}"; do
    if [ "${GROUPS_[$i]}" != "$LAST" ]; then
        LAST="${GROUPS_[$i]}"
        printf '\033[2m  %s\033[0m\n' "$(group_title "$LAST")"
    fi
    if [ "${OKS[$i]}" = 1 ]; then
        printf '  \033[32m✔\033[0m %s \033[2m%s\033[0m\n' "${TITLES[$i]}" "${DETAILS[$i]}"
    elif [ "${LEVELS[$i]}" = required ]; then
        printf '  \033[31m✘ %s\033[0m — %s\n' "${TITLES[$i]}" "${DETAILS[$i]}"
        [ -n "${FIXES[$i]}" ] && printf '      %s\n' "${FIXES[$i]}"
    else
        printf '  \033[33m! %s\033[0m — %s\n' "${TITLES[$i]}" "${DETAILS[$i]}"
        [ -n "${FIXES[$i]}" ] && printf '\033[2m      %s\033[0m\n' "${FIXES[$i]}"
    fi
done
exit "$FAILED"
