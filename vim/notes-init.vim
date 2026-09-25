" notes-init.vim — loaded (-u) by the notes window's embedded vim pane.
"
" Goal: the pane should feel like a native editor, not a terminal. No
" statusline / ruler / mode echo chrome, soft-wrapped prose, the notepad's
" own colors showing through, and every edit landing on disk on its own
" (the host also flushes with :wall on tab switch / hide).
"
" The host passes the theme in before this file runs:
"   --cmd "let g:ws_fg='#RRGGBB'" --cmd "let g:ws_dim='#RRGGBB'"
"   --cmd "let g:ws_sel='#RRGGBB'" --cmd "let g:ws_line='#RRGGBB'"
"   (+ g:ws_accent / ws_accent2 / ws_ok / ws_warn / ws_err / ws_info /
"    ws_deep: the theme palette for headings, links, code, search, menus)
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
" yank/copy -> system clipboard (unnamedplus above); every form of delete /
" change goes to the black-hole register so it never clobbers the clipboard
" (mirrors ~/.config/nvim/init.lua). Visual p replaces without yanking.
for s:k in ['d', 'D', 'c', 'C', 'x', 'X', 's', 'S']
  execute 'nnoremap ' . s:k . ' "_' . s:k
  execute 'xnoremap ' . s:k . ' "_' . s:k
endfor
xnoremap p P
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
set number norelativenumber
set numberwidth=3
" highlight the cursor's line (like iTerm2's cursor guide); the number
" column's current line is brightened too
set cursorline
set cursorlineopt=both
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
  let line = get(g:, 'ws_line', '#2E3440')
  for grp in ['Normal', 'NormalNC', 'NormalFloat', 'EndOfBuffer', 'NonText',
        \ 'SignColumn', 'LineNr', 'FoldColumn', 'MsgArea', 'StatusLine',
        \ 'StatusLineNC', 'VertSplit', 'WinSeparator']
    execute 'highlight ' . grp . ' guibg=NONE ctermbg=NONE'
  endfor
  execute 'highlight Normal guifg=' . fg
  execute 'highlight MsgArea guifg=' . dim
  execute 'highlight Visual guibg=' . sel . ' guifg=NONE'
  execute 'highlight CursorLine guibg=' . line . ' gui=NONE cterm=NONE'
  execute 'highlight LineNr guifg=' . dim . ' guibg=NONE'
  execute 'highlight CursorLineNr guifg=' . fg . ' guibg=' . line . ' gui=bold'
  execute 'highlight Pmenu guibg=' . sel . ' guifg=' . fg
  " the rest of the palette: each element its own role (like a themed
  " tmux/nvim port) instead of one tint everywhere
  let acc = get(g:, 'ws_accent', '#C6A0F6')
  let acc2 = get(g:, 'ws_accent2', '#8AADF4')
  let ok = get(g:, 'ws_ok', '#A6DA95')
  let warn = get(g:, 'ws_warn', '#EED49F')
  let err = get(g:, 'ws_err', '#ED8796')
  let info = get(g:, 'ws_info', '#8BD5CA')
  let deep = get(g:, 'ws_deep', '#181926')
  execute 'highlight CursorLineNr guifg=' . acc . ' guibg=' . line . ' gui=bold'
  execute 'highlight PmenuSel guibg=' . acc . ' guifg=' . deep . ' gui=bold'
  execute 'highlight Search guibg=' . warn . ' guifg=' . deep
  execute 'highlight IncSearch guibg=' . acc . ' guifg=' . deep
  execute 'highlight CurSearch guibg=' . acc . ' guifg=' . deep
  execute 'highlight MatchParen guifg=' . acc . ' guibg=NONE gui=bold,underline'
  execute 'highlight Title guifg=' . acc . ' gui=bold'
  execute 'highlight Comment guifg=' . dim . ' gui=italic'
  execute 'highlight Constant guifg=' . warn
  execute 'highlight String guifg=' . ok
  execute 'highlight Identifier guifg=' . acc2
  execute 'highlight Function guifg=' . acc2
  execute 'highlight Statement guifg=' . acc . ' gui=NONE'
  execute 'highlight PreProc guifg=' . info
  execute 'highlight Type guifg=' . warn . ' gui=NONE'
  execute 'highlight Special guifg=' . info
  execute 'highlight Underlined guifg=' . acc2 . ' gui=underline'
  execute 'highlight Directory guifg=' . acc2
  execute 'highlight Todo guifg=' . deep . ' guibg=' . warn . ' gui=bold'
  execute 'highlight Error guifg=' . err . ' guibg=NONE gui=bold'
  execute 'highlight ErrorMsg guifg=' . err . ' guibg=NONE'
  execute 'highlight WarningMsg guifg=' . warn
  execute 'highlight Question guifg=' . ok
  execute 'highlight SpellBad guisp=' . err . ' gui=undercurl'
  " markdown: headings in the accent, links in accent2, code in green
  for n in range(1, 6)
    execute 'highlight markdownH' . n . ' guifg=' . acc . ' gui=bold'
    execute 'highlight markdownH' . n . 'Delimiter guifg=' . acc
  endfor
  execute 'highlight markdownLinkText guifg=' . acc2 . ' gui=underline'
  execute 'highlight markdownUrl guifg=' . dim . ' gui=underline'
  execute 'highlight markdownCode guifg=' . ok
  execute 'highlight markdownCodeBlock guifg=' . ok
  execute 'highlight markdownCodeDelimiter guifg=' . dim
  execute 'highlight markdownListMarker guifg=' . acc
  execute 'highlight markdownOrderedListMarker guifg=' . acc
  execute 'highlight markdownBlockquote guifg=' . dim . ' gui=italic'
  execute 'highlight markdownBold guifg=' . fg . ' gui=bold'
  execute 'highlight markdownItalic guifg=' . fg . ' gui=italic'
  execute 'highlight markdownError guifg=NONE guibg=NONE'
  highlight! link @markup.heading Title
  highlight! link @markup.link.label markdownLinkText
  highlight! link @markup.link.url markdownUrl
  highlight! link @markup.raw markdownCode
  highlight! link @markup.list markdownListMarker
  highlight! link @markup.quote markdownBlockquote
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

" --- inline images -------------------------------------------------------------
" Markdown image links (![alt](path)) get g:ws_img_rows blank virtual lines
" under them; the notes window draws the real image over those rows. The
" screen rows of every visible image are written to g:ws_img_file as JSON
" whenever the view changes (scroll, edit, resize, buffer switch).
lua << LUA
local out = vim.g.ws_img_file
if not out or out == '' then return end
local rows = tonumber(vim.g.ws_img_rows) or 10
local ns = vim.api.nvim_create_namespace('ws_images')
local pattern = '!%[[^%]]*%]%(([^%)]+)%)'

-- PNG pixel size from the IHDR header (pasted images are PNG); other
-- formats fall back to the full row budget
local function png_size(p)
  local f = io.open(p, 'rb')
  if not f then return nil end
  local h = f:read(24)
  f:close()
  if not h or #h < 24 or h:sub(2, 4) ~= 'PNG' then return nil end
  local function u32(x) local a, b, c, d = x:byte(1, 4); return ((a * 256 + b) * 256 + c) * 256 + d end
  return u32(h:sub(17, 20)), u32(h:sub(21, 24))
end

-- rows an image needs at the pane's cell height (g:ws_cell_h, set by the
-- window), capped at g:ws_img_rows
local function rows_for(p)
  local _, h = png_size(p)
  local cell = tonumber(vim.g.ws_cell_h) or 16
  if not h then return rows end
  return math.max(2, math.min(rows, math.ceil(h / cell) + 1))
end

local function resolve(buf, rel)
  rel = rel:gsub('%s+"[^"]*"%s*$', '')           -- ![](path "title")
  if rel:match('^%a[%w+.-]*://') then return nil end
  rel = vim.fn.expand(rel)
  if rel:sub(1, 1) ~= '/' then
    rel = vim.fn.fnamemodify(vim.api.nvim_buf_get_name(buf), ':p:h') .. '/' .. rel
  end
  local p = vim.fn.fnamemodify(rel, ':p')
  if vim.fn.filereadable(p) == 1 then return p end
  return nil
end

-- (re)place the virtual lines only when the set of image lines changed
local marked = {}
local function mark(buf)
  local found, sig = {}, {}
  for i, l in ipairs(vim.api.nvim_buf_get_lines(buf, 0, -1, false)) do
    local rel = l:match(pattern)
    local p = rel and resolve(buf, rel)
    if p then
      local n = rows_for(p)
      found[#found + 1] = { i, p, n }
      sig[#sig + 1] = i .. p .. ':' .. n
    end
  end
  local s = table.concat(sig, '|')
  if marked[buf] ~= s then
    marked[buf] = s
    vim.api.nvim_buf_clear_namespace(buf, ns, 0, -1)
    for _, f in ipairs(found) do
      local blank = {}
      for _ = 1, f[3] do blank[#blank + 1] = { { ' ', 'Normal' } } end
      pcall(vim.api.nvim_buf_set_extmark, buf, ns, f[1] - 1, 0, { virt_lines = blank })
    end
  end
  return found
end

local last = ''
local function place()
  local buf, win = vim.api.nvim_get_current_buf(), vim.api.nvim_get_current_win()
  local imgs = {}
  local top, bot = vim.fn.line('w0'), vim.fn.line('w$')
  for _, f in ipairs(mark(buf)) do
    local lnum, p, n = f[1], f[2], f[3]
    if lnum >= top and lnum <= bot then
      -- screen row of the link line's LAST character (a wrapped line spans
      -- several rows); the image starts on the next row
      local pos = vim.fn.screenpos(win, lnum, math.max(1, #vim.fn.getline(lnum)))
      if pos.row > 0 then
        imgs[#imgs + 1] = string.format('{"path":%s,"row":%d,"rows":%d}',
          vim.fn.json_encode(p), pos.row, n)
      end
    end
  end
  local json = string.format('{"lines":%d,"columns":%d,"images":[%s]}',
    vim.o.lines, vim.o.columns, table.concat(imgs, ','))
  if json ~= last then
    last = json
    vim.fn.writefile({ json }, out)
  end
end

-- the window calls this after a font change (new cell height)
_G.ws_images_refresh = function() marked = {}; last = ''; place() end

vim.api.nvim_create_autocmd({ 'BufEnter', 'BufWinEnter', 'TextChanged', 'TextChangedI',
  'WinScrolled', 'WinResized', 'VimResized', 'CursorMoved', 'CursorMovedI', 'BufWritePost' }, {
  group = vim.api.nvim_create_augroup('ws_images', { clear = true }),
  callback = function() vim.schedule(place) end,
})
LUA
