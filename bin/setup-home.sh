#!/usr/bin/env bash
# setup-home.sh — set up ~/.config/kitchen-sink ("the home"), the ONE
# path every external config points at (aerospace.toml hotkeys, the jira
# launchd agent, commands.toml). What sits behind it depends on how
# the app was installed:
#
#   repo install   the home IS the git checkout (or a link to it)
#   app install    a real directory: the user's own commands.toml, rules/ and
#                  config/ (seeded from the bundle), links for the code
#                  (bin jira … -> kitchen-sink.app/Contents/Resources/…)
#                  and ONE absolute link, kitchen-sink.app -> the app
#
#   setup-home.sh app APP [--switch]   the app runs this on every launch
#                                      (fast no-op when nothing changed; a
#                                      moved / updated app heals itself).
#                                      Home owned by a checkout: exit 3 and
#                                      touch nothing, unless --switch.
#   setup-home.sh repo                 INSTALL.sh: mark the home as the
#                                      checkout's (refuses while an app
#                                      install owns it)
#   setup-home.sh stack                link the aerospace / borders
#                                      configs + start the services
#   setup-home.sh status               who owns the home: repo | app | none
#
# Never runs git, never moves or deletes a real file / directory (a git
# checkout least of all): only links are replaced; anything else in the way
# stops with an error. $WS_HOME overrides the home (tests).
set -uo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
ROOT="$(cd "$DIR/.." && pwd -P)"
# shellcheck source=../install.conf
. "$ROOT/install.conf"
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"
# brew keeps its tap trust list (`brew trust`) under $XDG_CONFIG_HOME when the
# shell sets it; an app launched from Finder has no XDG_CONFIG_HOME, so brew
# would look in ~/.homebrew and refuse the borders / aerospace taps
if [ -z "${XDG_CONFIG_HOME:-}" ] && [ -f "$HOME/.config/homebrew/trust.json" ] \
    && [ ! -f "$HOME/.homebrew/trust.json" ]; then
    export XDG_CONFIG_HOME="$HOME/.config"
fi

WS_HOME="${WS_HOME:-$WS_HOME_DEFAULT}"
MARK="$WS_HOME/.install"

say()  { printf '%s\n' "$*"; }
die()  { printf 'setup-home: %s\n' "$*" >&2; exit 1; }
mark() { [ -f "$MARK" ] && sed -n "s/^$1=//p" "$MARK" | head -1; return 0; }

# who owns the home: a link or a git checkout = repo, a marker says the rest
owner() {
    if [ -L "$WS_HOME" ] || [ -e "$WS_HOME/.git" ]; then echo repo; return; fi
    local m; m="$(mark mode)"
    if [ -n "$m" ]; then echo "$m"; return; fi
    [ -e "$WS_HOME" ] && [ -n "$(ls -A "$WS_HOME" 2>/dev/null)" ] && { echo unknown; return; }
    echo none
}

sha() { shasum -a 256 "$1" 2>/dev/null | cut -d' ' -f1; }

# ---------------------------------------------------------------- app
# seed DEFAULT -> home/REL, remembering the default's hash. On a later
# version: an untouched file follows the new default; an edited one is kept
# and the new default lands beside it as REL.new.
SEEDS=""
seed() {   # default rel [keep-deleted]
    local def="$1" rel="$2" dst="$WS_HOME/$2" new old cur
    new="$(sha "$def")"
    old="$(printf '%s\n' "$OLD_SEEDS" | awk -v r="$rel" '$1 == "seed" && substr($0, index($0, $3)) == r { print $2; exit }')"
    SEEDS="$SEEDS
seed $new $rel"
    if [ ! -e "$dst" ]; then
        # a rule the user deleted stays deleted
        [ -n "${3:-}" ] && [ -n "$old" ] && return 0
        mkdir -p "$(dirname "$dst")"
        cp -p "$def" "$dst"
        return 0
    fi
    cur="$(sha "$dst")"
    [ "$cur" = "$new" ] && { rm -f "$dst.new"; return 0; }
    [ -z "$old" ] && return 0                 # the user's own file from before
    if [ "$cur" = "$old" ]; then              # untouched: follow the default
        cp -p "$def" "$dst"
        rm -f "$dst.new"
    elif [ "$old" != "$new" ]; then           # edited + the default moved on
        cp -p "$def" "$dst.new"
        say "new=$dst.new"
    fi
}

# a link in the home; a real file / directory in the way is left alone
link() {   # target name
    local t="$1" l="$WS_HOME/$2"
    if [ -L "$l" ]; then
        [ "$(readlink "$l")" = "$t" ] && return 0
        rm "$l"
    elif [ -e "$l" ]; then
        die "$l is a real file/dir, not a link — left alone (remove it yourself, then re-run)"
    fi
    ln -s "$t" "$l"
}

cmd_app() {
    local app="${1:-}" switch="${2:-}" res own d f rel
    [ -n "$app" ] || die "usage: setup-home.sh app /path/to/$APP_NAME.app [--switch]"
    app="${app%/}"
    res="$app/Contents/Resources"
    [ -f "$res/commands.default.toml" ] || die "$app is not a distribution build (no Contents/Resources/commands.default.toml)"
    case "$app" in */AppTranslocation/*|/Volumes/*) die "the app is running from the disk image — move it to Applications first" ;; esac

    own="$(owner)"
    # fast path (every launch): same app, same version, links in place
    if [ "$own" = app ] && [ "$(mark app)" = "$app" ] && [ "$(mark version)" = "$APP_VERSION" ] \
        && [ "$(readlink "$WS_HOME/$APP_NAME.app" 2>/dev/null)" = "$app" ] \
        && [ -f "$WS_HOME/commands.toml" ] && [ -e "$WS_HOME/bin/setup-home.sh" ]; then
        say "result=ok"
        return 0
    fi

    if [ "$own" = repo ]; then
        if [ "$switch" != "--switch" ]; then
            say "result=repo"
            say "detail=$WS_HOME is a developer checkout — left alone"
            return 3
        fi
        # a real checkout is never moved: the user does that by hand
        [ -L "$WS_HOME" ] || die "$WS_HOME is a git checkout — left alone (move it out of the way yourself, then re-run)"
        # hand over: the link is removed (the checkout it points at is
        # untouched) and the user's settings are carried over
        # (rules + the aerospace / borders configs included:
        # the per-file links in ~/.config keep resolving to the same content)
        local keep old
        keep="$(mktemp -d)"
        old="$(cd "$WS_HOME" 2>/dev/null && pwd -P)"
        [ -f "$WS_HOME/commands.toml" ] && cp -p "$WS_HOME/commands.toml" "$keep/commands.toml"
        for d in $RESOURCE_SEED_DIRS; do
            [ -d "$WS_HOME/$d" ] && cp -Rp "$WS_HOME/$d" "$keep/$d"
        done
        # the repo-built daemon is the wrong one from here on
        pkill -f "$WS_HOME/$APP_NAME.app/Contents/MacOS" 2>/dev/null || true
        [ -n "$old" ] && pkill -f "$old/$APP_NAME.app/Contents/MacOS" 2>/dev/null || true
        say "kept=$(readlink "$WS_HOME")"
        rm "$WS_HOME"
        mkdir -p "$WS_HOME"
        [ -f "$keep/commands.toml" ] && cp -p "$keep/commands.toml" "$WS_HOME/commands.toml"
        for d in $RESOURCE_SEED_DIRS; do
            [ -d "$keep/$d" ] && cp -Rp "$keep/$d" "$WS_HOME/$d"
        done
        rm -rf "$keep"
    elif [ "$own" = unknown ]; then
        # files of ours without a marker (a hand-made folder): keep them,
        # they simply count as the user's own
        :
    fi

    mkdir -p "$WS_HOME" || die "cannot create $WS_HOME"
    OLD_SEEDS="$(grep '^seed ' "$MARK" 2>/dev/null || true)"

    seed "$res/commands.default.toml" commands.toml
    for d in $RESOURCE_SEED_DIRS; do
        [ -d "$res/$d" ] || continue
        while IFS= read -r f; do
            rel="${f#"$res"/}"
            if [ "$d" = rules ]; then seed "$f" "$rel" keep-deleted; else seed "$f" "$rel"; fi
        done < <(find "$res/$d" -type f ! -name '.DS_Store' | sort)
    done

    # ONE absolute link (the app), everything else relative through it: a
    # moved app needs one link fixed
    link "$app" "$APP_NAME.app"
    for d in $RESOURCE_LINK_DIRS; do
        link "$APP_NAME.app/Contents/Resources/$d" "$d"
    done
    link "$APP_NAME.app/Contents/Resources/install.conf" install.conf

    {
        printf 'mode=app\napp=%s\nversion=%s\n' "$app" "$APP_VERSION"
        printf '%s\n' "$SEEDS" | grep '^seed '
    } > "$MARK.tmp" && mv "$MARK.tmp" "$MARK"
    mkdir -p "$HOME/.cache/$APP_NAME"
    say "result=ok"
}

# --------------------------------------------------------------- repo
cmd_repo() {
    local own
    own="$(owner)"
    case "$ROOT" in *.app/Contents/Resources) die "'repo' is for a git checkout (run ./INSTALL.sh there)" ;; esac
    if [ "$own" = app ] && [ "$(cd "$WS_HOME" 2>/dev/null && pwd -P)" != "$ROOT" ]; then
        die "$WS_HOME belongs to the installed app — run its UNINSTALL.sh (or remove the folder) first"
    fi
    # the repo is the home, or symlinks.sh links the home to it next
    printf 'mode=repo\nroot=%s\n' "$ROOT" > "$ROOT/.install"
    say "result=ok"
}

# -------------------------------------------------------------- stack
cmd_stack() {
    local res svc f rc=0
    # run from the app bundle: the configs are the home's copies, whoever
    # owns the home (without WS_LINK_ROOT symlinks.sh would try to make the
    # home a link to the bundle)
    case "$ROOT" in *.app/Contents/Resources)
        export WS_LINK_ROOT="$WS_HOME"
        res="$ROOT"
        # precompiled unread-count helpers (no swiftc on an end user's Mac);
        # touched so they count as newer than their sources
        if [ -d "$res/helpers-bin" ]; then
            mkdir -p "$HOME/.cache/kitchen-sink/helpers"
            for f in "$res/helpers-bin/"*; do
                cp -p "$f" "$HOME/.cache/kitchen-sink/helpers/" \
                    && touch "$HOME/.cache/kitchen-sink/helpers/$(basename "$f")"
            done
        fi
        ;;
    esac
    # shellcheck source=../symlinks.sh
    . "$ROOT/symlinks.sh"
    ensure_sym_links || die "linking the configs failed"
    # (tests link into a temp home and must not poke the real services)
    if [ -n "${WS_NO_SERVICES:-}" ]; then say "result=ok"; return 0; fi
    if command -v brew >/dev/null 2>&1; then
        for svc in $BREW_SERVICES; do
            brew services start "$svc" || { printf 'setup-home: brew services start %s failed\n' "$svc" >&2; rc=1; }
        done
    fi
    command -v aerospace >/dev/null 2>&1 && aerospace reload-config
    [ "$rc" = 0 ] || return "$rc"
    say "result=ok"
}

case "${1:-}" in
    app)    shift; cmd_app "$@" ;;
    repo)   cmd_repo ;;
    stack)  cmd_stack ;;
    status) owner ;;
    *)      sed -n '2,27p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 2 ;;
esac
