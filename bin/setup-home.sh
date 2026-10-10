#!/usr/bin/env bash
set -uo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
ROOT="$(cd "$DIR/.." && pwd -P)"
. "$ROOT/install.conf"
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"
if [ -z "${XDG_CONFIG_HOME:-}" ] && [ -f "$HOME/.config/homebrew/trust.json" ] \
    && [ ! -f "$HOME/.homebrew/trust.json" ]; then
    export XDG_CONFIG_HOME="$HOME/.config"
fi

WS_HOME="${WS_HOME:-$WS_HOME_DEFAULT}"
MARK="$WS_HOME/.install"

say()  { printf '%s\n' "$*"; }
die()  { printf 'setup-home: %s\n' "$*" >&2; exit 1; }
mark() { [ -f "$MARK" ] && sed -n "s/^$1=//p" "$MARK" | head -1; return 0; }

owner() {
    if [ -L "$WS_HOME" ] || [ -e "$WS_HOME/.git" ]; then echo repo; return; fi
    local m; m="$(mark mode)"
    if [ -n "$m" ]; then echo "$m"; return; fi
    [ -e "$WS_HOME" ] && [ -n "$(ls -A "$WS_HOME" 2>/dev/null)" ] && { echo unknown; return; }
    echo none
}

sha() { shasum -a 256 "$1" 2>/dev/null | cut -d' ' -f1; }

SEEDS=""
seed() {
    local def="$1" rel="$2" dst="$WS_HOME/$2" new old cur
    new="$(sha "$def")"
    old="$(printf '%s\n' "$OLD_SEEDS" | awk -v r="$rel" '$1 == "seed" && substr($0, index($0, $3)) == r { print $2; exit }')"
    SEEDS="$SEEDS
seed $new $rel"
    if [ ! -e "$dst" ]; then
        [ -n "${3:-}" ] && [ -n "$old" ] && return 0
        mkdir -p "$(dirname "$dst")"
        cp -p "$def" "$dst"
        return 0
    fi
    cur="$(sha "$dst")"
    [ "$cur" = "$new" ] && { rm -f "$dst.new"; return 0; }
    [ -z "$old" ] && return 0
    if [ "$cur" = "$old" ]; then
        cp -p "$def" "$dst"
        rm -f "$dst.new"
    elif [ "$old" != "$new" ]; then
        cp -p "$def" "$dst.new"
        say "new=$dst.new"
    fi
}

link() {
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
        [ -L "$WS_HOME" ] || die "$WS_HOME is a git checkout — left alone (move it out of the way yourself, then re-run)"
        local keep old
        keep="$(mktemp -d)"
        old="$(cd "$WS_HOME" 2>/dev/null && pwd -P)"
        [ -f "$WS_HOME/commands.toml" ] && cp -p "$WS_HOME/commands.toml" "$keep/commands.toml"
        for d in $RESOURCE_SEED_DIRS; do
            [ -d "$WS_HOME/$d" ] && cp -Rp "$WS_HOME/$d" "$keep/$d"
        done
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

cmd_repo() {
    local own
    own="$(owner)"
    case "$ROOT" in *.app/Contents/Resources) die "'repo' is for a git checkout (run ./INSTALL.sh there)" ;; esac
    if [ "$own" = app ] && [ "$(cd "$WS_HOME" 2>/dev/null && pwd -P)" != "$ROOT" ]; then
        die "$WS_HOME belongs to the installed app — run its UNINSTALL.sh (or remove the folder) first"
    fi
    printf 'mode=repo\nroot=%s\n' "$ROOT" > "$ROOT/.install"
    say "result=ok"
}

cmd_stack() {
    local res svc f rc=0
    case "$ROOT" in *.app/Contents/Resources)
        export WS_LINK_ROOT="$WS_HOME"
        res="$ROOT"
        if [ -d "$res/helpers-bin" ]; then
            mkdir -p "$HOME/.cache/kitchen-sink/helpers"
            for f in "$res/helpers-bin/"*; do
                cp -p "$f" "$HOME/.cache/kitchen-sink/helpers/" \
                    && touch "$HOME/.cache/kitchen-sink/helpers/$(basename "$f")"
            done
        fi
        ;;
    esac
    . "$ROOT/symlinks.sh"
    ensure_sym_links || die "linking the configs failed"
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
