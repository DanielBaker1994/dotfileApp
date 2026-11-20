#!/usr/bin/env bash
# check-overrides.sh — report the ONE folder kitchen-sink reads personal
# config from, and what is in it.
#
# The folder is pylib/paths.json `configDir` (or $WS_CONFIG_DIR); default
# ~/.config/kitchen-sink-local. It holds:
#   commands.toml          override — any [section] key here wins over the
#                          repo's commands.toml (the full default/backup)
#   files-favorites.json   the Files sidebar's pinned folders
#   jira/config.json       Jira site + token (chmod 600), jira/team.json
#   confluence/config.json Confluence site + token
# Prints a short human report; never fails the caller (always exits 0).
#
#   check-overrides.sh           report only
#   check-overrides.sh --offer   also offer to set the folder up: create it
#                                with a starter commands.toml, and move/copy
#                                files still in the old places (~/.config/jira,
#                                ~/.config/confluence, ~/.cache/kitchen-sink/
#                                files-favorites.json). Asks on a terminal;
#                                without one it prints the commands instead.
#                                Never overwrites a file already in the folder.
set -u
OFFER=""; [ "${1:-}" = "--offer" ] && OFFER=1
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
BASE="${WS_COMMANDS_CONF:-$ROOT/commands.toml}"
DIR="$(python3 -c "import sys; sys.path.insert(0, '$ROOT/pylib'); import jira_paths; print(jira_paths.CONFIG_DIR)" 2>/dev/null)"
[ -n "$DIR" ] || DIR="${WS_CONFIG_DIR:-$HOME/.config/kitchen-sink-local}"
t() { case "$1" in "$HOME"/*) printf '~%s' "${1#"$HOME"}" ;; *) printf '%s' "$1" ;; esac; }
row() { # label path
    if [ -e "$2" ]; then printf '  ✔ %-22s %s\n' "$1" "$(t "$2")"
    else printf '  · %-22s %s (not created yet)\n' "$1" "$(t "$2")"; fi
}

# --- offer to set the folder up (before the report, so it shows the result)
# Ask only on a terminal (KS_ASK=1 forces it, for tests); prompts go to stderr.
interactive() { [ -t 0 ] || [ -n "${KS_ASK:-}" ]; }
ask() { # question -> m|c|s (default m); no terminal -> empty
    interactive || { echo ""; return; }
    local a; printf '%s [M]ove / [c]opy / [s]kip: ' "$1" >&2
    read -r a || a=s
    case "$a" in c|C) echo c ;; s|S|n|N) echo s ;; *) echo m ;; esac
}
offer() { # src dst
    local src="$1" dst="$2" how
    [ -e "$src" ] || return 0
    if [ -e "$dst" ]; then
        cmp -s "$src" "$dst" || printf '  ! both %s and %s exist and differ — using the second; merge by hand\n' "$(t "$src")" "$(t "$dst")"
        return 0
    fi
    how="$(ask "  $(t "$src") is in the old place → $(t "$dst")?")"
    case "$how" in
        m) mkdir -p "$(dirname "$dst")" && mv "$src" "$dst" && printf '  ✔ moved %s → %s\n' "$(t "$src")" "$(t "$dst")" ;;
        c) mkdir -p "$(dirname "$dst")" && cp -p "$src" "$dst" && printf '  ✔ copied %s → %s\n' "$(t "$src")" "$(t "$dst")" ;;
        s) printf '  · left %s where it is (not read any more)\n' "$(t "$src")" ;;
        *) printf '  → to use it: mkdir -p %s && mv %s %s\n' "$(t "$(dirname "$dst")")" "$(t "$src")" "$(t "$dst")" ;;
    esac
}
if [ -n "$OFFER" ]; then
    if [ ! -d "$DIR" ]; then
        if interactive; then
            printf 'Create your config folder %s? [Y/n]: ' "$(t "$DIR")" >&2
            read -r a || a=n
        else a=y; fi
        case "$a" in n|N) printf '  · not created — the app uses the repo defaults only\n' ;;
            *) mkdir -p "$DIR" && printf '  ✔ created %s\n' "$(t "$DIR")" ;; esac
    fi
    if [ -d "$DIR" ] && [ ! -e "$DIR/commands.toml" ]; then
        cat > "$DIR/commands.toml" <<STUB
# Personal overrides for kitchen-sink (machine-local, not in the repo).
# Any [section] key set here wins over the same key in
# $(t "$BASE"), which stays the full default/backup.
# Example — the Files sidebar's pinned folders:
#
# [files]
# favorites = "~/notes, ~/Desktop"
STUB
        printf '  ✔ wrote a starter %s (all commented out)\n' "$(t "$DIR/commands.toml")"
    fi
    for f in config.json team.json config; do offer "$HOME/.config/jira/$f" "$DIR/jira/$f"; done
    rm -f "$HOME/.config/jira/config.json.lock" 2>/dev/null
    if [ -d "$HOME/.config/confluence" ]; then
        for f in "$HOME/.config/confluence/"*; do [ -e "$f" ] && offer "$f" "$DIR/confluence/$(basename "$f")"; done
    fi
    offer "$HOME/.cache/kitchen-sink/files-favorites.json" "$DIR/files-favorites.json"
    rmdir "$HOME/.config/jira" "$HOME/.config/confluence" 2>/dev/null
    echo
fi

printf 'defaults/backup: %s\n' "$(t "$BASE")"
if [ -d "$DIR" ]; then
    printf 'config folder (CHECKED AND USED — set in pylib/paths.json configDir): %s\n' "$(t "$DIR")"
else
    printf 'config folder (checked, does not exist yet — set in pylib/paths.json configDir): %s\n' "$(t "$DIR")"
fi
row "commands.toml override" "$DIR/commands.toml"
row "pinned folders" "$DIR/files-favorites.json"
row "jira config + token" "$DIR/jira/config.json"
row "jira team" "$DIR/jira/team.json"
row "confluence config" "$DIR/confluence/config.json"
for old in "$HOME/.config/jira" "$HOME/.config/confluence"; do
    [ -e "$old" ] && printf '  ! old folder %s is no longer read — move its files into %s\n' "$(t "$old")" "$(t "$DIR")"
done
exit 0
