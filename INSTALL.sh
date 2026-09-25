#!/usr/bin/env bash
# INSTALL.sh — THE one command for this app.
#
#   ./INSTALL.sh             install everything (direct, verbose)
#   ./INSTALL.sh uninstall   remove everything (same as UNINSTALL.sh)
#   ./INSTALL.sh help        show this
#
# That's it. If you are not sure what to run:  ./INSTALL.sh
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
UID_="$(id -u)"

# This script can run from stripped environments (curl|bash, cron) where
# /opt/homebrew/bin is NOT on PATH — make sure brew and friends are always
# reachable.
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"

GREEN='\033[32m'; RED='\033[31m'; YELLOW='\033[33m'; CYAN='\033[1;36m'; DIM='\033[2m'; RESET='\033[0m'
ok()   { printf "${GREEN}  ✔ %s${RESET}\n" "$*"; }
fail() { printf "${RED}  ✘ %s${RESET}\n" "$*"; }
warn() { printf "${YELLOW}  ! %s${RESET}\n" "$*"; }
step() { printf "\n${CYAN}== %s ==${RESET}\n" "$*"; }
info() { printf "${DIM}    %s${RESET}\n" "$*"; }

usage() {
    printf '%s\n' \
        'INSTALL.sh — THE one command for this app.' \
        '' \
        '  ./INSTALL.sh             install everything (direct, verbose)' \
        '  ./INSTALL.sh uninstall   remove everything (same as UNINSTALL.sh)' \
        '  ./INSTALL.sh help        show this' \
        '' \
        "That's it. If you are not sure what to run:  ./INSTALL.sh"
}

# --------------------------------------------------------------- commands
case "${1:-}" in
    help|-h|--help)
        usage
        exit 0
        ;;
    uninstall|--uninstall)
        "$ROOT/UNINSTALL.sh"
        exit 0
        ;;
    "")
        ;;
    *)
        printf "${RED}Unknown option: %s${RESET}\n" "$1" >&2
        usage >&2
        exit 1
        ;;
esac

# If this script is NOT running from a git checkout (e.g. you downloaded just
# this file, or piped it through curl), clone the whole app first — that is
# the retard-proof path: one script, it fetches everything itself.
if [ ! -d "$ROOT/.git" ]; then
    printf "\n${CYAN}This looks like a standalone copy — the full app is in a git repo.${RESET}\n"
    printf "${CYAN}I'll clone it and continue the install from the fresh copy.${RESET}\n\n"
    DEFAULT="$HOME/workspace-switcher"
    read -r -p "Where should I install the app? [$DEFAULT]: " DEST
    DEST="${DEST:-$DEFAULT}"
    if [ -d "$DEST" ]; then
        printf "${YELLOW}  ! $DEST already exists — installing from there${RESET}\n"
    else
        printf "${DIM}    git clone https://github.com/DanielBaker1994/workspace-switcher.git $DEST${RESET}\n"
        git clone https://github.com/DanielBaker1994/workspace-switcher.git "$DEST" || {
            printf "\n${RED}Clone failed — check your network and try again.${RESET}\n" >&2
            exit 1
        }
    fi
    exec bash "$DEST/INSTALL.sh"
fi

STEP="starting"
die() {
    printf "\n${RED}INSTALL FAILED at step: %s${RESET}\n" "$STEP"
    printf "${YELLOW}Fix the issue above and run ./INSTALL.sh again — it is safe to re-run.${RESET}\n" >&2
    exit 1
}
trap die ERR
set -e

printf "${CYAN}
┌──────────────────────────────────────────────────────────────┐
│            workspace-switcher — one-command install          │
└──────────────────────────────────────────────────────────────┘
${RESET}"
printf "${DIM}This script installs a macOS app that gives you a notes window, a\nvoice-to-text window (dictate → text), a Jira issue browser, and a\nswitcher popup — plus the sketchybar menu bar and AeroSpace.${RESET}\n"

# ---------------------------------------------------------------- 0. sanity
STEP="checking your Mac"
step "0/7 checking your Mac"
command -v swiftc >/dev/null 2>&1 || { warn "Xcode command-line tools missing — installing them"; xcode-select --install; die "run again after the tools finish installing"; }
ok "macOS + Swift compiler present"

# ------------------------------------------------------------- 1. homebrew
STEP="installing Homebrew"
step "1/7 Homebrew (the package manager)"
if command -v brew >/dev/null 2>&1; then
    ok "Homebrew already installed ($(brew --version | head -1))"
else
    info "Homebrew not found — installing it (this needs your password once)"
    /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
    eval "$(/opt/homebrew/bin/brew shellenv)"
    ok "Homebrew installed"
fi

# ------------------------------------------------------------- 2. deps
STEP="installing dependencies"
step "2/7 dependencies (brew)"
for f in aerospace sketchybar borders jq ripgrep; do
    if brew list "$f" >/dev/null 2>&1; then
        ok "$f already installed"
    else
        info "installing $f…"
        brew install "$f" >/dev/null
        ok "$f installed"
    fi
done
for c in karabiner-elements font-sketchybar-app-font font-hack-nerd-font; do
    if brew list --cask "$c" >/dev/null 2>&1; then
        ok "$c already installed"
    else
        info "installing $c…"
        brew install --cask "$c" >/dev/null
        ok "$c installed"
    fi
done
ok "all dependencies present (font-hack-nerd-font powers the terminal glyphs)"

# ------------------------------------------------------------- 3. configs
STEP="installing configs"
step "3/7 configs (backed up if they already exist)"
install_config() {
    local src="$1" dst="$2"
    if [ -e "$dst" ] && [ ! -L "$dst" ]; then
        local BACKUP="/tmp/ws-backup-$(date +%s)"
        mkdir -p "$BACKUP"
        mv "$dst" "$BACKUP/"
        warn "backed up existing $dst -> $BACKUP/"
    elif [ -L "$dst" ]; then
        rm "$dst"
    fi
    mkdir -p "$(dirname "$dst")"
    cp -R "$src" "$dst"
}
install_config "$ROOT/config/aerospace"  "$HOME/.config/aerospace"
install_config "$ROOT/config/sketchybar" "$HOME/.config/sketchybar"
install_config "$ROOT/config/borders"    "$HOME/.config/borders"
ok "configs installed (aerospace / sketchybar / borders)"

# ------------------------------------------------------------- 4. build
STEP="building the app"
step "4/7 building workspace-switcher.app (compiling the Swift sources)"
# SwiftTerm (the embedded terminal) is precompiled once into a static lib
# the daemon links against
TERM_LIB="$ROOT/.build/SwiftTerm/libSwiftTerm.a"
TERM_MOD_DIR="$ROOT/.build/SwiftTerm"
info "precompiling SwiftTerm (terminal engine)…"
mkdir -p "$TERM_MOD_DIR"
swiftc -O -swift-version 5 -parse-as-library -emit-library -static -module-name SwiftTerm \
    "$ROOT"/Vendor/SwiftTerm/Sources/SwiftTerm/*.swift \
    "$ROOT"/Vendor/SwiftTerm/Sources/SwiftTerm/Apple/*.swift \
    "$ROOT"/Vendor/SwiftTerm/Sources/SwiftTerm/Apple/Metal/*.swift \
    "$ROOT"/Vendor/SwiftTerm/Sources/SwiftTerm/Mac/*.swift \
    "$ROOT"/Vendor/SwiftTerm/Sources/SwiftTerm/Portable/*.swift \
    "$ROOT"/Vendor/SwiftTerm/Generated/*.swift \
    -emit-module -emit-module-path "$TERM_MOD_DIR/SwiftTerm.swiftmodule" \
    -o "$TERM_LIB"
ok "SwiftTerm compiled"

mkdir -p "$ROOT/workspace-switcher.app/Contents/MacOS"
BUILD_TMP="$(mktemp "${TMPDIR:-/tmp}/ws-install.XXXXXX")"
swiftc -O -swift-version 5 \
    -Xlinker -sectcreate -Xlinker __TEXT -Xlinker __info_plist -Xlinker "$ROOT/Info.plist" \
    -I "$TERM_MOD_DIR" -Xlinker "$TERM_LIB" \
    "$ROOT/PopupWindow.swift" "$ROOT/workspace_switcher.swift" "$ROOT/main.swift" \
    -o "$BUILD_TMP"
mv "$BUILD_TMP" "$ROOT/workspace-switcher.app/Contents/MacOS/workspace-switcher"
cp "$ROOT/Info.plist" "$ROOT/workspace-switcher.app/Contents/Info.plist"
codesign --force --sign - --identifier dev.danielbaker.workspace-switcher \
    "$ROOT/workspace-switcher.app" >/dev/null 2>&1
ok "compiled + code-signed"

# ------------------------------------------------------------- 5. TCC
STEP="granting microphone + speech permissions"
step "5/7 microphone + speech-recognition permissions (voice notes need these)"
"$ROOT/bin/voice-permissions.sh" >/dev/null 2>&1
ok "mic + speech granted to the app (no System Settings needed)"

# ------------------------------------------------------------- 6. services
STEP="starting menu-bar services"
step "6/7 starting sketchybar + borders (menu-bar stack)"
brew services start sketchybar >/dev/null 2>&1 || true
brew services start borders >/dev/null 2>&1 || true
ok "sketchybar + borders running"

# jira poll agent: only load when [jira] enabled = true in commands.conf
JIRA_ENABLED=""
JIRA_BG_POLL=""
if [ -f "$ROOT/commands.conf" ]; then
    in_jira=0
    while IFS= read -r line; do
        s="$(echo "$line" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')"
        case "$s" in
            "["*"]") [ "$s" = "[jira]" ] && in_jira=1 || in_jira=0 ;;
            *=*)
                if [ "$in_jira" = "1" ]; then
                    key="$(echo "$s" | cut -d= -f1 | sed 's/[[:space:]]//g')"
                    val="$(echo "$s" | cut -d= -f2- | sed 's/^[[:space:]]*//')"
                    [ "$key" = "enabled" ] && JIRA_ENABLED="$val"
                    [ "$key" = "poll-when-disabled" ] && JIRA_BG_POLL="$val"
                fi
                ;;
        esac
    done < "$ROOT/commands.conf"
fi
# poll-when-disabled = true (menu "Disable Jira" → Keep Polling) keeps it too
case "${JIRA_ENABLED,,} ${JIRA_BG_POLL,,}" in
    true*|yes*|1*|on*|*" true"|*" yes"|*" 1"|*" on")
        step "6b/7 jira poll agent (launchd)"
        PLIST="$HOME/Library/LaunchAgents/com.jira.poll.plist"
        sed "s|__WS_CONFIG__|$HOME/.config/workspace-switcher|g" \
            "$ROOT/jira/com.jira.poll.plist" >"$PLIST"
        launchctl bootout "gui/$UID_" "$PLIST" 2>/dev/null || true
        launchctl bootstrap "gui/$UID_" "$PLIST" 2>/dev/null || true
        ok "poll agent loaded"
        ;;
    *)
        step "6b/7 jira poll agent (disabled — [jira] enabled != true)"
        # make sure any previously loaded agent is stopped
        PLIST="$HOME/Library/LaunchAgents/com.jira.poll.plist"
        launchctl bootout "gui/$UID_" "$PLIST" 2>/dev/null || true
        ok "poll agent not loaded (jira disabled)"
        ;;
esac

# ------------------------------------------------------------- 7. launch
STEP="opening the app"
step "7/7 opening the app"
pkill -f "workspace-switcher.app" 2>/dev/null || true
sleep 1
open -n -g "$ROOT/workspace-switcher.app" --args voice >/dev/null 2>&1 || true
ok "app launched — the voice window should be on screen"

printf "\n${GREEN}══════════════════════════════════════════════════════════════${RESET}\n"
printf "${GREEN}  INSTALL COMPLETE${RESET}\n"
printf "${GREEN}══════════════════════════════════════════════════════════════${RESET}\n"
printf "${DIM}One-time manual steps (cannot be automated by macOS):${RESET}\n"
printf "  1. Karabiner: allow its system extension (System Settings >\n"
printf "     Privacy & Security), restart it — then caps_lock = Hyper works\n"
printf "     and Hyper+S opens the switcher.\n"
printf "  2. AeroSpace: allow Accessibility access when prompted, then run:\n"
printf "       aerospace reload-config\n"
printf "  3. Jira (optional): menu bar wrench → Enable Jira (asks for site +\n"
printf "     token), then Open Jira Config Window. Poll jobs = the \"endpoints\"\n"
printf "     list in ~/.config/jira/config.json (one job → one JSON file → one\n"
printf "     Jira window tab, written to ~/.cache/workspace-switcher/jira_json/).\n"
printf "  4. Use it:\n"
printf "       Hyper+S            switcher popup (search '/' for commands)\n"
printf "       menu bar wrench    notes / jira / voice / health checks\n"
printf "       voice window       red button = dictate, pause/resume, stop → text\n"
printf "\nUninstall anytime with: ./UNINSTALL.sh\n"