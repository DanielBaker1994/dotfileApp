#!/usr/bin/env bash
set -uo pipefail

_sl_self="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
ROOT="${ROOT:-$_sl_self}"
[ -f "$ROOT/install.conf" ] && . "$ROOT/install.conf"
LINK_ROOT="${WS_LINK_ROOT:-$ROOT}"
WS_HOME_DEFAULT="${WS_HOME_DEFAULT:-$HOME/.config/kitchen-sink}"

command -v ok   >/dev/null 2>&1 || ok()   { printf '\033[32m  \342\234\224 %s\033[0m\n' "$*"; }
command -v fail >/dev/null 2>&1 || fail() { printf '\033[31m  \342\234\230 %s\033[0m\n' "$*"; }
command -v warn >/dev/null 2>&1 || warn() { printf '\033[33m  ! %s\033[0m\n' "$*"; }
command -v step >/dev/null 2>&1 || step() { printf '\n\033[1;36m== %s\033[0m\n' "$*"; }
command -v info >/dev/null 2>&1 || info() { printf '\033[2m    %s\033[0m\n' "$*"; }

_sl_usage() {
    printf '%s\n' \
        'symlinks.sh — validate / repair this repo'"'"'s symlinks' \
        '' \
        '  ./symlinks.sh            report + prompt to fix' \
        '  ./symlinks.sh validate   same' \
        '  ./symlinks.sh --check    read-only; exit 1 if wrong' \
        '  ./symlinks.sh --fix      repair without asking'
}

_ws_build_manifest() {
    MAN_TARGET=()
    MAN_SOURCE=()
    local d f rel
    for d in $CONFIG_DIRS; do
        [ -d "$LINK_ROOT/config/$d" ] || continue
        while IFS= read -r f; do
            rel="${f#"$LINK_ROOT/config/$d"/}"
            MAN_TARGET+=("$HOME/.config/$d/$rel")
            MAN_SOURCE+=("$f")
        done < <(find "$LINK_ROOT/config/$d" -type f ! -name '*.new' ! -name '.DS_Store' 2>/dev/null | sort)
    done
    if [ -z "${WS_LINK_ROOT:-}" ]; then
        MAN_TARGET+=("${WS_HOME:-$WS_HOME_DEFAULT}")
        MAN_SOURCE+=("$ROOT")
    fi
}

_sl_state() {
    local t="$1" s="$2" rt rs
    [ -e "$s" ] || { printf 'BROKEN'; return; }
    rt="$(realpath "$t" 2>/dev/null)"
    rs="$(realpath "$s" 2>/dev/null)"
    [ -n "$rt" ] && [ "$rt" = "$rs" ] && { printf 'OK'; return; }
    if [ -L "$t" ]; then
        [ "$(readlink "$t")" = "$s" ] && printf 'OK' || printf 'WRONG'
    elif [ -e "$t" ]; then
        printf 'CONFLICT'
    else
        printf 'MISSING'
    fi
}

_ws_parent_links() {
    local d
    for d in $CONFIG_DIRS; do
        [ -L "$HOME/.config/$d" ] && printf '%s\n' "$HOME/.config/$d"
    done
    return 0
}

_sl_repair() {
    local -a idx=("$@")
    local i t s d rc=0
    for d in $CONFIG_DIRS; do
        [ -L "$HOME/.config/$d" ] && rm "$HOME/.config/$d"
    done
    [ "${#idx[@]}" -eq 0 ] && return 0
    for i in "${idx[@]}"; do
        t="${MAN_TARGET[$i]}"; s="${MAN_SOURCE[$i]}"
        if [ ! -e "$s" ]; then warn "skip (source missing): ~${s#$HOME}"; continue; fi
        [ -L "$t" ] && rm "$t"
        if [ -e "$t" ]; then
            fail "~${t#$HOME} is a real file/dir, not a link — left alone (remove it yourself, then re-run)"
            rc=1
            continue
        fi
        mkdir -p "$(dirname "$t")"
        ln -s "$s" "$t"
        ok "~${t#$HOME} -> ~${s#$HOME}"
    done
    return "$rc"
}

validate_sym_links() {
    local mode="${1:-prompt}"
    _ws_build_manifest
    step "symlinks (kitchen-sink)"
    info "configs = $LINK_ROOT/config"
    local i t s st bad=0 total=0
    local -a bad_idx=()
    for i in "${!MAN_TARGET[@]}"; do
        t="${MAN_TARGET[$i]}"; s="${MAN_SOURCE[$i]}"
        total=$((total + 1))
        st="$(_sl_state "$t" "$s")"
        case "$st" in
            OK)
                if [ "$t" = "$s" ]; then
                    printf '  \033[32m\342\234\224\033[0m %s \033[2m(this repo)\033[0m\n' "~${t#$HOME}"
                else
                    printf '  \033[32m\342\234\224\033[0m %s \033[2m-> %s\033[0m\n' "~${t#$HOME}" "~${s#$HOME}"
                fi
                ;;
            MISSING)  printf '  \033[33m\302\267\033[0m %s  \033[2m(missing -> %s)\033[0m\n' "~${t#$HOME}" "~${s#$HOME}"; bad=$((bad + 1)); bad_idx+=("$i") ;;
            WRONG)    printf '  \033[31m\342\234\230\033[0m %s  \033[2m(points to %s, expected %s)\033[0m\n' "~${t#$HOME}" "$(readlink "$t")" "~${s#$HOME}"; bad=$((bad + 1)); bad_idx+=("$i") ;;
            CONFLICT) printf '  \033[31m\342\234\230\033[0m %s  \033[2m(real file/dir in the way)\033[0m\n' "~${t#$HOME}"; bad=$((bad + 1)); bad_idx+=("$i") ;;
            BROKEN)   printf '  \033[31m\342\234\230\033[0m %s  \033[2m(source missing: %s)\033[0m\n' "~${t#$HOME}" "~${s#$HOME}"; bad=$((bad + 1)) ;;
        esac
    done
    local parent
    while IFS= read -r parent; do
        [ -n "$parent" ] || continue
        printf '  \033[31m\342\234\230\033[0m %s  \033[2m(whole-directory link — split into real dir)\033[0m\n' "~${parent#$HOME}"
        bad=$((bad + 1))
    done < <(_ws_parent_links)

    if [ "$bad" -eq 0 ]; then
        ok "all $total symlinks OK"
        return 0
    fi
    case "$mode" in
        check)
            fail "$bad of $total symlinks need attention"
            return 1
            ;;
        fix)
            _sl_repair "${bad_idx[@]}"
            return $?
            ;;
        *)
            if [ -t 0 ]; then
                printf '\n  Fix %d symlink(s)? [y/N] ' "$bad"
                read -r ans
                case "$ans" in y|Y|yes|YES) _sl_repair "${bad_idx[@]}"; return $? ;; esac
            else
                warn "not a terminal — re-run with --fix to repair"
            fi
            return 1
            ;;
    esac
}

ensure_sym_links() {
    _ws_build_manifest
    local i st parents
    local -a bad=()
    parents="$(_ws_parent_links)"
    for i in "${!MAN_TARGET[@]}"; do
        st="$(_sl_state "${MAN_TARGET[$i]}" "${MAN_SOURCE[$i]}")"
        [ "$st" = OK ] || bad+=("$i")
    done
    if [ "${#bad[@]}" -eq 0 ] && [ -z "$parents" ]; then
        ok "symlinks already correct ($CONFIG_DIRS + repo link)"
        return 0
    fi
    _sl_repair "${bad[@]}"
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
    case "${1:-validate}" in
        validate|"")   validate_sym_links prompt ;;
        --check|check) validate_sym_links check ;;
        --fix|fix)     validate_sym_links fix ;;
        -h|--help|help) _sl_usage; exit 0 ;;
        *) printf 'Unknown option: %s\n\n' "$1" >&2; _sl_usage >&2; exit 2 ;;
    esac
fi
