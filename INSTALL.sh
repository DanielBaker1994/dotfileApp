#!/usr/bin/env bash
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
[ -f "$ROOT/install.conf" ] && . "$ROOT/install.conf"
REPO_URL="https://github.com/DanielBaker1994/dotfileApp.git"

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
        '  ./INSTALL.sh validate    check every symlink (prompt to fix)' \
        '  ./INSTALL.sh validate --check  read-only; exit 1 if wrong' \
        '  ./INSTALL.sh validate --fix    repair without asking' \
        '  ./INSTALL.sh uninstall   remove everything (same as UNINSTALL.sh)' \
        '  ./INSTALL.sh help        show this' \
        '' \
        "That's it. If you are not sure what to run:  ./INSTALL.sh"
}

case "${1:-}" in
    help|-h|--help)
        usage
        exit 0
        ;;
    validate|--validate|check)
        . "$ROOT/symlinks.sh"
        case "${2:-}" in
            --check|check) validate_sym_links check ;;
            --fix|fix)     validate_sym_links fix ;;
            *)             validate_sym_links prompt ;;
        esac
        exit $?
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

if [ ! -d "$ROOT/.git" ]; then
    printf "\n${CYAN}This looks like a standalone copy — the full app is in a git repo.${RESET}\n"
    printf "${CYAN}I'll clone it and continue the install from the fresh copy.${RESET}\n\n"
    DEFAULT="$HOME/kitchen-sink"
    read -r -p "Where should I install the app? [$DEFAULT]: " DEST
    DEST="${DEST:-$DEFAULT}"
    if [ -d "$DEST" ]; then
        printf "${YELLOW}  ! $DEST already exists — installing from there${RESET}\n"
    else
        printf "${DIM}    git clone $REPO_URL $DEST${RESET}\n"
        git clone "$REPO_URL" "$DEST" || {
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

. "$ROOT/symlinks.sh"

printf "${CYAN}
┌──────────────────────────────────────────────────────────────┐
│            kitchen-sink — one-command install          │
└──────────────────────────────────────────────────────────────┘
${RESET}"
printf "${DIM}This script installs a macOS app that gives you a notes window, a\nvoice-to-text window (dictate → text), a Jira issue browser, and a\nswitcher popup — plus AeroSpace and window borders.${RESET}\n"

STEP="checking your Mac"
step "0/6 checking your Mac"
if ! command -v swiftc >/dev/null 2>&1 || ! xcode-select -p >/dev/null 2>&1; then
    warn "Xcode command-line tools missing — installing them"
    xcode-select --install || true
    die "run again after the tools finish installing"
fi
"$ROOT/bin/preflight.sh" --mode repo || die "this Mac cannot run the app (see the ✘ lines above)"
ok "this Mac can build and run the app (! lines = optional features that are off)"

"$ROOT/bin/setup-home.sh" repo >/dev/null || die "could not prepare ~/.config/kitchen-sink"
for other in "/Applications/$APP_NAME.app" "$HOME/Applications/$APP_NAME.app"; do
    [ -d "$other" ] || continue
    warn "$other is still installed (the DMG copy)"
    if [ -t 0 ]; then
        read -r -p "    Move it to the Trash? [y/N] " ans
        case "$ans" in y|Y|yes|YES)
            pkill -f "$other/Contents/MacOS" 2>/dev/null || true
            mv "$other" "$HOME/.Trash/$APP_NAME-$(date +%s).app" && ok "moved to the Trash" ;;
        esac
    fi
done
if "$ROOT/bin/clean-stale-permissions.sh" >/dev/null 2>&1; then
    ok "no stale workspace-switcher privacy grants"
else
    warn "could not clear stale workspace-switcher privacy grants — harmless, check System Settings ▸ Privacy if they linger"
fi

STEP="installing Homebrew"
step "1/6 Homebrew (the package manager)"
if command -v brew >/dev/null 2>&1; then
    ok "Homebrew already installed ($(brew --version | head -1))"
else
    info "Homebrew not found — installing it (this needs your password once)"
    brew_installer="$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)" \
        || die "could not download the Homebrew installer — check your network and re-run"
    /bin/bash -c "$brew_installer" || die "the Homebrew installer failed"
    eval "$(/opt/homebrew/bin/brew shellenv)"
    ok "Homebrew installed"
fi

STEP="installing dependencies"
step "2/6 dependencies (brew)"
for f in $BREW_FORMULAE; do
    if brew list "$f" >/dev/null 2>&1; then
        ok "$f already installed"
    else
        info "installing $f…"
        brew install "$f" >/dev/null
        ok "$f installed"
    fi
done
for c in $BREW_CASKS; do
    if brew list --cask "$c" >/dev/null 2>&1; then
        ok "$c already installed"
    else
        info "installing $c…"
        brew install --cask "$c" >/dev/null
        ok "$c installed"
    fi
done
ok "all dependencies present (font-hack-nerd-font powers the terminal glyphs)"

STEP="installing configs"
step "3/6 configs (symlinked into the repo)"
ensure_sym_links || die "a real file is in the way of a config link (see ✘ above)"
ok "configs linked to the repo ($CONFIG_DIRS)"

STEP="building the app"
step "4/6 building kitchen-sink.app (compiling the Swift sources)"
info "compiling (first run also precompiles SwiftTerm — a few minutes)…"
"$ROOT/bin/build-app.sh" --force
ok "compiled + code-signed + permissions granted"

STEP="starting services"
step "5/6 starting borders (focused-window border)"
for svc in $BREW_SERVICES; do
    brew services start "$svc" >/dev/null 2>&1 || true
done
ok "$BREW_SERVICES running"

STEP="opening the app"
step "6/6 opening the app"
pkill -f "$APP_NAME.app/Contents/MacOS" 2>/dev/null || true
sleep 1
open -n -g "$ROOT/$APP_NAME.app" --args voice >/dev/null 2>&1 || true
ok "app launched — the voice window should be on screen"

printf "\n${GREEN}══════════════════════════════════════════════════════════════${RESET}\n"
printf "${GREEN}  INSTALL COMPLETE${RESET}\n"
printf "${GREEN}══════════════════════════════════════════════════════════════${RESET}\n"
printf "${DIM}One-time manual steps (cannot be automated by macOS):${RESET}\n"
printf "  1. Hyper key: the hotkeys are Hyper+S / N / T, Hyper = ctrl+alt+cmd+shift\n"
printf "     (map a key to it with the tool you already use).\n"
printf "  2. AeroSpace: allow Accessibility access when prompted, then run:\n"
printf "       aerospace reload-config\n"
printf "  3. Jira (optional): menu bar wrench → Enable Jira (asks for site +\n"
printf "     token), then Open Jira Config Window. Poll jobs = the \"endpoints\"\n"
printf "     list in ~/.config/jira/config.json (one job → one JSON file → one\n"
printf "     Jira window tab, written to ~/.cache/kitchen-sink/jira_json/).\n"
printf "  4. Use it:\n"
printf "       Hyper+S            switcher popup (search '/' for commands)\n"
printf "       menu bar wrench    notes / jira / voice / health checks\n"
printf "       voice window       red button = dictate, pause/resume, stop → text\n"
printf "\nUninstall anytime with: ./UNINSTALL.sh\n"
