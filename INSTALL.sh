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
# every name/path this script uses (brew deps, bundle id, launchd agent, …);
# absent only in a standalone copy, which clones the repo and re-runs below
[ -f "$ROOT/install.conf" ] && . "$ROOT/install.conf"
# (not in install.conf: needed exactly when that file isn't here yet)
REPO_URL="https://github.com/DanielBaker1994/dotfileApp.git"

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
        '  ./INSTALL.sh validate    check every symlink (prompt to fix)' \
        '  ./INSTALL.sh validate --check  read-only; exit 1 if wrong' \
        '  ./INSTALL.sh validate --fix    repair without asking' \
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

# If this script is NOT running from a git checkout (e.g. you downloaded just
# this file, or piped it through curl), clone the whole app first — that is
# the foolproof path: one script, it fetches everything itself.
if [ ! -d "$ROOT/.git" ]; then
    printf "\n${CYAN}This looks like a standalone copy — the full app is in a git repo.${RESET}\n"
    printf "${CYAN}I'll clone it and continue the install from the fresh copy.${RESET}\n\n"
    DEFAULT="$HOME/workspace-switcher"
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
│            workspace-switcher — one-command install          │
└──────────────────────────────────────────────────────────────┘
${RESET}"
printf "${DIM}This script installs a macOS app that gives you a notes window, a\nvoice-to-text window (dictate → text), a Jira issue browser, and a\nswitcher popup — plus the sketchybar menu bar and AeroSpace.${RESET}\n"

# ---------------------------------------------------------------- 0. sanity
STEP="checking your Mac"
step "0/6 checking your Mac"
# ONE list of checks for both doors (this script and the installed app's
# Setup window): bin/preflight.sh. Required ones stop here — macOS older than
# MACOS_MIN (the app would not launch, LaunchServices error -10825; the same
# gate runs in bin/build-app.sh), no Swift compiler. The rest are warnings:
# missing brew packages are installed below, and a Mac without Apple's
# on-device model simply has no AI view.
if ! command -v swiftc >/dev/null 2>&1 || ! xcode-select -p >/dev/null 2>&1; then
    warn "Xcode command-line tools missing — installing them"
    xcode-select --install || true
    die "run again after the tools finish installing"
fi
"$ROOT/bin/preflight.sh" --mode repo || die "this Mac cannot run the app (see the ✘ lines above)"
ok "this Mac can build and run the app (! lines = optional features that are off)"

# the stable home (~/.config/workspace-switcher) must not belong to an
# installed app (DMG) — setup-home.sh stops if it does, nothing is moved
"$ROOT/bin/setup-home.sh" repo >/dev/null || die "could not prepare ~/.config/workspace-switcher"
# a second copy with the same bundle id confuses LaunchServices and the
# privacy grants — this install runs the one built in the checkout
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

# ------------------------------------------------------------- 1. homebrew
STEP="installing Homebrew"
step "1/6 Homebrew (the package manager)"
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

# ------------------------------------------------------------- 3. configs
STEP="installing configs"
step "3/6 configs (symlinked into the repo)"
# Every file under config/<name> becomes a symlink in ~/.config/<name>, so
# editing either path edits the repo copy (one file, tracked by git). Real
# directories are kept, so files that aren't in the repo stay where they are.
# A real file in the way stops the install and is left alone (remove it,
# re-run); a link that's already right is left alone (rerunnable).
ensure_sym_links || die "a real file is in the way of a config link (see ✘ above)"
ok "configs linked to the repo ($CONFIG_DIRS)"

# ------------------------------------------------------------- 4. build
STEP="building the app"
step "4/6 building workspace-switcher.app (compiling the Swift sources)"
# the same build script ./build.sh uses (bin/build-app.sh): every top-level
# *.swift is compiled, SwiftTerm is precompiled once, then sign + the privacy
# grants (mic, speech, Downloads / Desktop / Documents: bin/grant-permissions.sh)
info "compiling (first run also precompiles SwiftTerm — a few minutes)…"
"$ROOT/bin/build-app.sh" --force
ok "compiled + code-signed + permissions granted"

# ------------------------------------------------------------- 5. services
STEP="starting menu-bar services"
step "5/6 starting sketchybar + borders (menu-bar stack)"
for svc in $BREW_SERVICES; do
    brew services start "$svc" >/dev/null 2>&1 || true
done
ok "$BREW_SERVICES running"

# (the jira poll launchd agent is not installed here: the app keeps it in
# step with [jira] enabled on every launch and config reload —
# syncJiraLaunchAgent)

# ------------------------------------------------------------- 6. launch
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
printf "     Jira window tab, written to ~/.cache/workspace-switcher/jira_json/).\n"
printf "  4. Use it:\n"
printf "       Hyper+S            switcher popup (search '/' for commands)\n"
printf "       menu bar wrench    notes / jira / voice / health checks\n"
printf "       voice window       red button = dictate, pause/resume, stop → text\n"
printf "\nUninstall anytime with: ./UNINSTALL.sh\n"
