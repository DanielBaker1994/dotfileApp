#!/usr/bin/env bash
# setup.sh — one-command install of the workspace-switcher ecosystem:
# aerospace + sketchybar + borders + karabiner + the Swift switcher app.
#
# Idempotent: safe to re-run (existing configs are backed up, not clobbered).
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
UID_="$(id -u)"

say() { printf '\033[1;36m== %s ==\033[0m\n' "$*"; }

. "$ROOT/symlinks.sh"

case "${1:-}" in
    validate|--validate|check)
        case "${2:-}" in
            --check|check) validate_sym_links check ;;
            --fix|fix)     validate_sym_links fix ;;
            *)             validate_sym_links prompt ;;
        esac
        exit $?
        ;;
    --fix|fix)
        validate_sym_links fix
        exit $?
        ;;
    ""|--install|install)
        ;;
    *)
        printf 'Usage: %s [validate [--check|--fix]]\n' "$0" >&2
        exit 2
        ;;
esac

# ---------------------------------------------------------------- 1. brew
say "1/6 brew dependencies"
for f in aerospace sketchybar borders jq ripgrep; do
    brew list "$f" >/dev/null 2>&1 || brew install "$f"
done
for c in karabiner-elements font-sketchybar-app-font font-hack-nerd-font; do
    brew list --cask "$c" >/dev/null 2>&1 || brew install --cask "$c"
done

# start the menu-bar stack so sketchybar + borders actually RUN (previously
# installed but never started)
say "starting brew services (sketchybar, borders)"
brew services start sketchybar >/dev/null 2>&1 || true
brew services start borders >/dev/null 2>&1 || true

# ------------------------------------------------- 2. configs (with backup)
say "2/6 install configs (symlinked into the repo; anything in the way backed up)"
ensure_sym_links

# ---------------------------- 3. workspace-switcher stable XDG location
say "3/6 workspace-switcher XDG location"
info "~/.config/workspace-switcher -> $ROOT (handled by the manifest above)"

# --------------------------------------------------- 4. launchd jira poll
say "4/6 launchd jira poll agent"
PLIST="$HOME/Library/LaunchAgents/com.jira.poll.plist"
sed "s|__WS_CONFIG__|$HOME/.config/workspace-switcher|g" \
    "$ROOT/jira/com.jira.poll.plist" >"$PLIST"
launchctl bootout "gui/$UID_" "$PLIST" 2>/dev/null || true
launchctl bootstrap "gui/$UID_" "$PLIST" 2>/dev/null || true

# -------------------------------------------------- 5. build + TCC grants
say "5/6 build + TCC grants"
mkdir -p "$HOME/.cache/workspace-switcher/jira_json"
"$ROOT/bin/voice-permissions.sh" >/dev/null 2>&1 || true
# --build-only: compile + grant, but never auto-launch the window — the
# installer ends on its Done screen and the user launches explicitly.
"$ROOT/build.sh" --build-only || exit 1

# ------------------------------------------------------- 6. manual steps
say "6/6 done"
printf 'Manual steps (one-time, cannot be scripted):\n'
printf '  1. karabiner-elements: allow the system extension (System Settings >\n'
printf '     Privacy & Security), then confirm caps_lock = Hyper (config written\n'
printf '     to ~/.config/karabiner/karabiner.json by the installer)\n'
printf '  2. jira creds: create %s via: %s --init\n' \
    "$HOME/.config/jira/config" "$ROOT/jira/jira-api.sh"
printf '  3. system settings: give aerospace + karabiner-elements Accessibility\n'
printf '  4. dictation: System Settings > Keyboard > Dictation ON (for voice notes)\n'
printf '  5. reload aerospace: aerospace reload-config (or restart the app)\n'