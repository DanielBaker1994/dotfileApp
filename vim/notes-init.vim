" notes-init.vim — loaded (-u) by the notes window's embedded vim pane.
"
" Goal: the pane should feel like a native editor, not a terminal. No
" statusline / ruler / mode echo chrome, soft-wrapped prose, the notepad's
" own colors showing through, and every edit landing on disk on its own
" (the host also flushes with :wall on tab switch / hide).
"
" The host passes the theme in before this file runs:
"   --cmd "let g:ws_fg='#RRGGBB'" --cmd "let g:ws_dim='#RRGGBB'"
"   --cmd "let g:ws_sel='#RRGGBB'"
" Point commands.conf `[notes] vim-init` at your own file to replace this one.

set nocompatible
set encoding=utf-8
set termguicolors
set mouse=a
" system clipboard for y/d/p — only when a clipboard tool is reachable, so
" a yank can never raise a blocking "Press ENTER" provider error
if executable('pbcopy') && executable('pbpaste')
  set clipboard=unnamedplus
endif
set hidden
set autoread
set autowriteall
set updatetime=400
set undofile
let &undodir = expand('~/.cache/workspace-switcher/nvim-undo')
call mkdir(&undodir, 'p')
set noswapfile

" --- no terminal chrome ---------------------------------------------------
set laststatus=0
set noshowmode
set noruler
set noshowcmd
set cmdheight=1
set shortmess+=FIWcs
set signcolumn=no
set nonumber norelativenumber
set fillchars=eob:\ ,vert:\
set nofoldenable
set belloff=all

" --- prose-friendly editing -------------------------------------------------
set wrap linebreak breakindent
set scrolloff=3
set expandtab shiftwidth=2 tabstop=2
set ignorecase smartcase
set spelllang=en_us
filetype plugin indent on
syntax on

" --- colors: transparent background, notepad text color --------------------
function! s:WsTheme() abort
  let fg = get(g:, 'ws_fg', '#CAD3F5')
  let dim = get(g:, 'ws_dim', '#939AB7')
  let sel = get(g:, 'ws_sel', '#3F4A5A')
  for grp in ['Normal', 'NormalNC', 'NormalFloat', 'EndOfBuffer', 'NonText',
        \ 'SignColumn', 'LineNr', 'FoldColumn', 'MsgArea', 'StatusLine',
        \ 'StatusLineNC', 'VertSplit', 'WinSeparator']
    execute 'highlight ' . grp . ' guibg=NONE ctermbg=NONE'
  endfor
  execute 'highlight Normal guifg=' . fg
  execute 'highlight MsgArea guifg=' . dim
  execute 'highlight Visual guibg=' . sel . ' guifg=NONE'
  execute 'highlight CursorLine guibg=NONE'
  execute 'highlight Pmenu guibg=' . sel . ' guifg=' . fg
endfunction
augroup ws_theme
  autocmd!
  autocmd ColorScheme * call s:WsTheme()
augroup END
call s:WsTheme()

" --- always on disk ----------------------------------------------------------
augroup ws_autosave
  autocmd!
  autocmd InsertLeave,TextChanged,FocusLost,BufLeave * silent! wall
  autocmd CursorHold,CursorHoldI * silent! wall
  " external writes (voice dictation, other editors) reload silently
  autocmd FocusGained,BufEnter,CursorHold * silent! checktime
augroup END

" markdown notes: show the raw text exactly as it is on disk
augroup ws_markdown
  autocmd!
  autocmd FileType markdown setlocal conceallevel=0
augroup END
