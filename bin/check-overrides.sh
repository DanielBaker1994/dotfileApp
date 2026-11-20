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
set -u
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
BASE="${WS_COMMANDS_CONF:-$ROOT/commands.toml}"
DIR="$(python3 -c "import sys; sys.path.insert(0, '$ROOT/pylib'); import jira_paths; print(jira_paths.CONFIG_DIR)" 2>/dev/null)"
[ -n "$DIR" ] || DIR="${WS_CONFIG_DIR:-$HOME/.config/kitchen-sink-local}"
t() { case "$1" in "$HOME"/*) printf '~%s' "${1#"$HOME"}" ;; *) printf '%s' "$1" ;; esac; }
row() { # label path
    if [ -e "$2" ]; then printf '  ✔ %-22s %s\n' "$1" "$(t "$2")"
    else printf '  · %-22s %s (not created yet)\n' "$1" "$(t "$2")"; fi
}

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
