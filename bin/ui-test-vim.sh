#!/usr/bin/env bash
#
# ui-test-vim.sh — end-to-end tests for the notes window's embedded vim pane.
#
# Drives the REAL window like a user (System Events keystrokes, cliclick
# clicks) and checks the result through nvim's RPC socket + the files on disk.
# Works on scratch notes under /tmp/ws-vim-test; commands.conf and the
# dismissed-notes list are backed up first and restored on exit.
#
# Every keystroke is guarded: it is only sent while workspace-switcher is the
# frontmost app, so keys can never leak into another window.
#
# Usage: bin/ui-test-vim.sh [--keep]   (--keep: leave scratch notes + app up)

set -o pipefail
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONF="$ROOT/commands.conf"
DISMISSED="$HOME/.cache/workspace-switcher/dismissed-notes.json"
T=/tmp/ws-vim-test
BK="$(mktemp -d /tmp/ws-vim-bk.XXXXXX)"
KEEP=0; [[ "${1:-}" == "--keep" ]] && KEEP=1

command -v cliclick >/dev/null || { echo "cliclick not found — brew install cliclick"; exit 1; }
command -v nvim >/dev/null || { echo "nvim not found"; exit 1; }

PASS=0 FAIL=0
pass() { printf 'PASS: %s\n' "$*"; PASS=$((PASS + 1)); }
fail() { printf 'FAIL: %s\n' "$*" >&2; FAIL=$((FAIL + 1)); }
check() { # check "desc" "actual" "expected"
    if [[ "$2" == "$3" ]]; then pass "$1"; else fail "$1 — got [$2] want [$3]"; fi
}

# --- helpers ------------------------------------------------------------------

WSPID() { pgrep -x workspace-switcher | head -1; }
SOCK() { echo "$HOME/.cache/workspace-switcher/nvim-notes-$(WSPID).sock"; }
NOTESOCK="${TMPDIR%/}/ws-notes.sock"
ws_send() { printf '%s' "$1" | nc -U -w 1 "$NOTESOCK"; }
vx() { timeout 4 nvim --headless --clean --server "$(SOCK)" --remote-expr "$1" 2>/dev/null; }
front() { lsappinfo info -only name "$(lsappinfo front)" | sed -E 's/.*="(.*)"/\1/'; }
guard() { [[ "$(front)" == "workspace-switcher" ]] || { echo "ABORT: frontmost is '$(front)'" >&2; return 1; }; }
# all keys go through System Events: cliclick's kp:/t: events can carry stale
# fn/numpad flags (the terminal then drops them) and drop characters
typ() {
    guard || return 1
    local esc=${1//\\/\\\\}; esc=${esc//\"/\\\"}
    osascript -e "tell application \"System Events\" to keystroke \"$esc\""
}
sk() {
    guard || return 1
    local using=""; [[ -n "${2:-}" ]] && using=" using {$2}"
    osascript -e "tell application \"System Events\" to key code $1$using"
}
ESC() { sk 53; }
RET() { sk 36; }
CMD() { guard && osascript -e "tell application \"System Events\" to keystroke \"$1\" using {command down}"; }
CTRL() { guard && osascript -e "tell application \"System Events\" to keystroke \"$1\" using {control down}"; }
wframe() { osascript -e 'tell application "System Events" to tell process "workspace-switcher"
  repeat with w in windows
    set s to size of w
    if item 2 of s > 200 then
      set p to position of w
      return (item 1 of p as text) & "," & (item 2 of p as text) & "," & (item 1 of s as text) & "," & (item 2 of s as text)
    end if
  end repeat
end tell' 2>/dev/null | tr -d ' '; }
wcount() { osascript -e 'tell application "System Events" to count (windows of process "workspace-switcher")' 2>/dev/null; }
vim_pid() { pgrep -f "nvim --embed --listen $(SOCK)" | head -1; }
disk() { tr '\n' '|' < "$1"; }
buf() { vx "join(getline(1,'\$'),'|')"; }
# click inside the vim pane (upper third of the window)
click_pane() {
    local f; f="$(wframe)"; IFS=, read -r x y w h <<<"$f"
    cliclick c:$((x + w / 2)),$((y + 120))
}

cleanup() {
    cp "$BK/commands.conf" "$CONF"
    if [[ -f "$BK/dismissed.json" ]]; then cp "$BK/dismissed.json" "$DISMISSED"; else rm -f "$DISMISSED"; fi
    if (( ! KEEP )); then
        rm -rf "$T"
        # restart so the daemon's in-memory tab list matches the restored config
        kill "$(WSPID)" 2>/dev/null
    fi
    rm -rf "$BK"
}
cp "$CONF" "$BK/commands.conf"
[[ -f "$DISMISSED" ]] && cp "$DISMISSED" "$BK/dismissed.json"
trap cleanup EXIT

# --- setup ----------------------------------------------------------------------

grep -Eq '^vim-mode *= *true' "$CONF" || { echo "set [notes] vim-mode = true first"; exit 1; }
rm -rf "$T"; mkdir -p "$T"
printf 'first line\n' > "$T/zz-a.md"
printf 'b note\n' > "$T/zz-b.md"

if [[ -z "$(WSPID)" ]]; then
    ("$ROOT/bin/workspace_switcher.sh" notes >/dev/null 2>&1 &)
fi
for _ in $(seq 1 60); do [[ -S "$(SOCK)" ]] && break; sleep 1; done
[[ -S "$(SOCK)" ]] || { echo "vim socket never appeared"; exit 1; }
pass "vim pane started (socket $(SOCK))"

ws_send "open:$T/zz-a.md"; sleep 1.2
click_pane; sleep 0.5
guard || exit 1
check "open: switches vim to the note" "$(vx "expand('%:t')")" "zz-a.md"

# --- 1. typing reaches vim, :w saves -------------------------------------------
echo "== typing =="
ESC; typ "Go"; typ "Hello from vim"; ESC; typ ":w"; RET; sleep 0.6
check "typed text saved to disk" "$(disk "$T/zz-a.md")" "first line|Hello from vim|"
check "Esc returns to Normal mode" "$(vx 'mode()')" "n"

# --- 2. motions / undo / clipboard ---------------------------------------------
echo "== motions =="
typ "ggdd"; sleep 0.4
check "ggdd deletes line 1" "$(buf)" "Hello from vim"
check "dd yanks to the system clipboard" "$(pbpaste)" "first line"
typ "u"; sleep 0.4
check "u undoes" "$(buf)" "first line|Hello from vim"
typ "Gyyp"; sleep 0.4
check "yyp duplicates" "$(buf)" "first line|Hello from vim|Hello from vim"
typ "u"; sleep 1.2
check "autosave keeps disk in sync" "$(disk "$T/zz-a.md")" "first line|Hello from vim|"

# --- 3. Esc never hides the window ---------------------------------------------
echo "== Esc =="
W0="$(wcount)"; ESC; ESC; sleep 0.4
check "Esc stays in the window" "$(wcount)" "$W0"
typ "i"; sleep 0.2; ESC; sleep 0.4
check "i + Esc -> Normal" "$(vx 'mode()')" "n"

# --- 4. edit shortcuts (rule 1) ---------------------------------------------------
echo "== edit shortcuts =="
printf 'P-CMDV' | pbcopy; typ "Go"; CMD v; sleep 0.5
check "Cmd+V pastes in Insert mode" "$(vx "getline('.')")" "P-CMDV"
ESC; sleep 0.4
check "Esc after paste -> Normal" "$(vx 'mode()')" "n"
printf 'P-CTRLV' | pbcopy; typ "o"; CTRL v; sleep 0.5
check "Ctrl+V pastes" "$(vx "getline('.')")" "P-CTRLV"
ESC; sleep 0.3
CMD z; sleep 0.4
check "Cmd+Z undoes" "$(vx "getline('\$')")" "P-CMDV"
CMD a; sleep 0.4
check "Cmd+A selects all (Visual line)" "$(vx 'mode()')" "V"
printf 'x' | pbcopy; CMD c; sleep 0.6
check "Cmd+C copies the selection" "$(pbpaste | tr '\n' '|')" "first line|Hello from vim|P-CMDV|"
ESC; sleep 0.3
typ "Gdd"; sleep 1.2

# --- 5. tab switch mid-insert: no data loss, lands in Normal --------------------
echo "== tab switch =="
ws_send "open:$T/zz-b.md"; sleep 1.2
check "second note opens in the same vim" "$(vx "expand('%:t')")" "zz-b.md"
PID1="$(vim_pid)"
typ "A typed-in-b"; sleep 0.3
check "still inserting before switch" "$(vx 'mode()')" "i"
ws_send "open:$T/zz-a.md"; sleep 1.2
check "tab switch follows" "$(vx "expand('%:t')")" "zz-a.md"
check "switch lands in Normal mode" "$(vx 'mode()')" "n"
check "outgoing note saved" "$(disk "$T/zz-b.md")" "b note typed-in-b|"
check "other note untouched" "$(disk "$T/zz-a.md")" "first line|Hello from vim|"
check "no relaunch on tab switch" "$(vim_pid)" "$PID1"
sleep 2.5
check "watcher never overwrites vim's file" "$(disk "$T/zz-b.md")" "b note typed-in-b|"

# --- 6. hide / re-show keeps the session and focus -------------------------------
echo "== hide / show =="
CMD w; sleep 0.8
check "Cmd+W hides the window" "$(wcount)" "0"
ws_send notes; sleep 1.2
check "re-show keeps the same editor" "$(vim_pid)" "$PID1"
typ "Go"; typ "after reshow"; ESC; sleep 0.8
check "typing works right after re-show" "$(vx "getline('\$')")" "after reshow"

# --- 7. :q restarts the editor on the current note -------------------------------
echo "== :q =="
typ ":q"; RET; sleep 1.5
PID2="$(vim_pid)"
[[ -n "$PID2" && "$PID2" != "$PID1" ]] && pass ":q relaunches the editor" || fail ":q relaunch ($PID1 -> $PID2)"
check "relaunch reopens the current note" "$(vx "expand('%:t')")" "zz-a.md"
typ "Go"; typ "after relaunch"; ESC; sleep 0.8
check "relaunched editor takes input" "$(tail -1 "$T/zz-a.md")" "after relaunch"

# --- 8. host-side append (voice path) lands in the buffer -------------------------
echo "== external append =="
printf 'external line\n' >> "$T/zz-a.md"; sleep 2.5
check "external write reloads into vim" "$(vx "getline('\$')")" "external line"

echo
echo "vim pane: $PASS passed, $FAIL failed"
(( FAIL == 0 ))
