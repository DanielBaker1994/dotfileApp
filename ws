#!/usr/bin/env bash
set -uo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
. "$DIR/bin/lib.sh"

ws_usage() {
    cat <<'EOF'
ws — kitchen-sink entry point

  ws                                interactive menu
  ws install                        install everything (INSTALL.sh)
  ws uninstall                      remove everything (UNINSTALL.sh)
  ws build [--force|--build-only]   compile + relaunch
  ws dmg [--no-notarize]            build the distributable .dmg
  ws test [all|SUITE|install]
                                    unit tests (SUITE = config, compare, …).
                                    UI suites were deleted (owner decision)
  ws doctor [--fix]                 health check of the whole stack
  ws check [--json]                 preflight this Mac
  ws permissions grant|fix          macOS privacy permissions
  ws links [check|fix]              validate / repair config symlinks
  ws monitors main|inverse|toggle|status
                                    which screen owns which workspaces
  ws fake confluence|jira [start|stop|status]
                                    fake Confluence / Jira fixtures
  ws home app|repo|stack|status     the ~/.config/kitchen-sink home
  ws help                           this

Old names (bin/make-dmg.sh, bin/fix-permissions.sh,
bin/fake-confluence.sh, bin/fake-jira-tab.sh) are thin shims.
EOF
}

ws_menu() {
    local choice a
    while true; do
        printf '\n\033[1;36mkitchen-sink\033[0m\n'
        printf '   1) install       install everything\n'
        printf '   2) build         compile + relaunch\n'
        printf '   3) dmg           build the distributable .dmg\n'
        printf '   4) test          unit tests\n'
        printf '   5) doctor        health check of the whole stack\n'
        printf '   6) check         preflight this Mac\n'
        printf '   7) permissions   grant / fix macOS permissions\n'
        printf '   8) links         validate / repair config symlinks\n'
        printf '   9) monitors      which screen owns which workspaces\n'
        printf '  10) fake          fake Confluence / Jira fixtures\n'
        printf '  11) home          the ~/.config/kitchen-sink home\n'
        printf '  12) uninstall     remove everything\n'
        printf '   h) help\n'
        printf '   q) quit\n\n'
        read -r -p 'choice: ' choice || { echo; return 0; }
        case "$choice" in
            1)  ws_cmd_install ;;
            2)  ws_cmd_build ;;
            3)  ws_cmd_dmg ;;
            4)  ws_cmd_test ;;
            5)  ws_cmd_doctor ;;
            6)  ws_cmd_check ;;
            7)  read -r -p 'grant or fix? [grant] ' a; ws_cmd_permissions "${a:-grant}" ;;
            8)  ws_cmd_links ;;
            9)  read -r -p 'main|inverse|toggle|status [status] ' a; ws_cmd_monitors "${a:-status}" ;;
            10) read -r -p 'confluence|jira [confluence] ' a; ws_cmd_fake "${a:-confluence}" ;;
            11) read -r -p 'app|repo|stack|status [status] ' a; ws_cmd_home "${a:-status}" ;;
            12) ws_cmd_uninstall ;;
            h|H|help) ws_usage ;;
            q|Q|quit) return 0 ;;
            *)  ws_warn "unknown choice: $choice" ;;
        esac
    done
}

cmd="${1:-}"
[ $# -gt 0 ] && shift
case "$cmd" in
    ""|menu)
        if [ -t 0 ] && [ -t 1 ]; then ws_menu; else ws_usage; fi ;;
    help|-h|--help) ws_usage ;;
    install)        ws_cmd_install "$@" ;;
    uninstall)      ws_cmd_uninstall "$@" ;;
    build)          ws_cmd_build "$@" ;;
    dmg)            ws_cmd_dmg "$@" ;;
    test)           ws_cmd_test "$@" ;;
    doctor)         ws_cmd_doctor "$@" ;;
    check)          ws_cmd_check "$@" ;;
    permissions)    ws_cmd_permissions "$@" ;;
    links)          ws_cmd_links "$@" ;;
    monitors)       ws_cmd_monitors "$@" ;;
    fake)           ws_cmd_fake "$@" ;;
    home)           ws_cmd_home "$@" ;;
    *)              ws_die "unknown command '$cmd' (try: ws help)" ;;
esac
