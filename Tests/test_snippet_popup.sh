#!/usr/bin/env bash
# Regression test for the "intellisense stopped mid-session" failure modes in
# vim/snippets.lua, run against a real headless nvim:
#   1. a stale vim.snippet session must not survive <Esc> (popup() bails while one is active)
#   2. a failed expansion must not wedge the popup (the old `busy` flag stuck true)
#   3. a wiped autocmd group must re-arm itself on the next InsertEnter
#   4. the plain path still works (popup shows, Tab expands)
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
S=$(mktemp -u /private/tmp/kssnip.XXXXXX)   # short path: macOS AF_UNIX limit is 103
nvim --headless --clean --listen "$S" -c "set ft=markdown" \
  -c "lua dofile('$ROOT/vim/snippets.lua').setup()" >/dev/null 2>&1 &
NV=$!; trap 'kill $NV 2>/dev/null; rm -f "$S"' EXIT; sleep 1
ev()   { nvim --server "$S" --remote-expr "$1" 2>&1; }
send() { nvim --server "$S" --remote-send "$1"; sleep "${2:-0.4}"; }
fail=0
check() { if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1 (got '$2', want '$3')"; fail=1; fi; }

send 'ibash' 0.6;  check "popup shows after typing a prefix" "$(ev 'pumvisible()')" 1
send '<Tab>' 0.6;  check "Tab expands the snippet" "$(ev 'line("$") > 1')" 1
send '<Esc>' 0.3;  send ':%d<CR>' 0.3

send ':lua vim.snippet.expand("a $1 b $2")<CR>' 0.4; send 'i' 0.2; send '<Esc>' 0.4
check "snippet session cleared by <Esc>" "$(ev 'luaeval("vim.snippet.active()")')" false
send ':lua vim.snippet.stop()<CR>:%d<CR>' 0.3

send ':lua _G.__o=vim.snippet.expand; vim.snippet.expand=function() error("boom") end<CR>' 0.3
send 'ibash' 0.6; send '<Tab>' 0.8; send '<Esc>' 0.3
send ':lua vim.snippet.expand=_G.__o<CR>:%d<CR>' 0.3
send 'ibash' 0.8;  check "popup recovers after a failed expansion" "$(ev 'pumvisible()')" 1
send '<Esc>' 0.3;  send ':%d<CR>' 0.3

send ':lua vim.api.nvim_del_augroup_by_name("ws_snippets")<CR>' 0.3
send 'ibash' 0.6; send '<Esc>' 0.3; send ':%d<CR>' 0.3
send 'ibash' 0.8;  check "autocmds re-armed after the group was wiped" "$(ev 'pumvisible()')" 1
exit $fail
