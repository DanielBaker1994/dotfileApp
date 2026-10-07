-- init.lua — loaded (-u) by the notes window's embedded nvim pane.
--
-- Goal: the pane should feel like a native editor, not a terminal. No
-- statusline / ruler / mode echo chrome, soft-wrapped prose, the notepad's
-- own colors showing through, and every edit landing on disk on its own
-- (the host also flushes with :wall on tab switch / hide).
--
-- The host passes the theme in before this file runs (vim.g.ws_fg / ws_dim /
-- ws_sel / ws_line + ws_accent / accent2 / ok / warn / err / info / deep: the
-- theme palette for headings, links, code, search, menus) and re-applies it
-- with `doautocmd ColorScheme` when the theme changes.
-- Point commands.toml `[notes] vim-init` at your own file to replace this one.
-- Needs a current nvim (0.11+); no vimscript, no backwards compatibility.

local here = vim.fn.fnamemodify(debug.getinfo(1, 'S').source:sub(2), ':p:h')
local o = vim.o

o.termguicolors = true
o.mouse = 'a'
-- system clipboard for y/d/p — only when a clipboard tool is reachable, so
-- a yank can never raise a blocking "Press ENTER" provider error
if vim.fn.executable('pbcopy') == 1 and vim.fn.executable('pbpaste') == 1 then
  o.clipboard = 'unnamedplus'
end
-- yank/copy -> system clipboard (unnamedplus above); every form of delete /
-- change goes to the black-hole register so it never clobbers the clipboard
-- (mirrors ~/.config/nvim/init.lua). Visual p replaces without yanking.
for _, k in ipairs({ 'd', 'D', 'c', 'C', 'x', 'X', 's', 'S' }) do
  vim.keymap.set({ 'n', 'x' }, k, '"_' .. k)
end
vim.keymap.set('x', 'p', 'P')
-- Space s f = find among the open notes, Space s g = ripgrep their contents:
-- asks the app (its command socket, g:ws_sock) for the popup
-- (NoteFindWindow.swift); Return there switches to the note
vim.g.mapleader = ' '
local function ask_app(msg)
  local sock = vim.g.ws_sock
  if not sock or sock == '' then return end
  local uv = vim.uv or vim.loop
  local pipe = uv.new_pipe(false)
  pipe:connect(sock, function(err)
    if err then pipe:close() return end
    pipe:write(msg, function() pipe:close() end)
  end)
end
vim.keymap.set('n', '<leader>sf', function() ask_app('notes-find') end,
  { desc = 'find among the open notes (popup)', silent = true })
vim.keymap.set('n', '<leader>sg', function() ask_app('notes-grep') end,
  { desc = 'ripgrep the open notes (popup)', silent = true })
o.hidden = true
o.autoread = true
o.autowriteall = true
o.updatetime = 400
o.undofile = true
local undodir = vim.fn.expand('~/.cache/kitchen-sink/nvim-undo')
vim.fn.mkdir(undodir, 'p')
o.undodir = undodir
o.swapfile = false

-- --- no terminal chrome ---------------------------------------------------
o.laststatus = 0
o.showmode = false
o.ruler = false
o.showcmd = false
o.cmdheight = 1
vim.opt.shortmess:append('FIWcs')
o.signcolumn = 'no'
o.number = true
o.relativenumber = false
o.numberwidth = 3
-- highlight the cursor's line (like iTerm2's cursor guide); the number
-- column's current line is brightened too
o.cursorline = true
o.cursorlineopt = 'both'
vim.opt.fillchars = { eob = ' ', vert = ' ' }
o.foldenable = false
o.belloff = 'all'

-- --- prose-friendly editing -------------------------------------------------
-- wrap is window-local: a filetype plugin / modeline / :set nowrap in one
-- buffer must not leave the notes pane scrolling sideways
local function soft_wrap()
  vim.wo.wrap, vim.wo.linebreak, vim.wo.breakindent = true, true, true
end
soft_wrap()
vim.api.nvim_create_autocmd({ 'BufWinEnter', 'WinEnter', 'FileType' }, {
  group = vim.api.nvim_create_augroup('ws_wrap', { clear = true }),
  callback = soft_wrap,
})
o.scrolloff = 3
o.expandtab, o.shiftwidth, o.tabstop = true, 2, 2
o.ignorecase, o.smartcase = true, true
o.spelllang = 'en_us'

-- --- colors: transparent background, notepad text color --------------------
-- like `:highlight GROUP k=v`: merges into the group instead of replacing it
local function hl(name, opts)
  local cur = vim.api.nvim_get_hl(0, { name = name, link = false })
  vim.api.nvim_set_hl(0, name, vim.tbl_extend('force', cur, opts))
end

local function theme()
  local g = vim.g
  local fg, dim = g.ws_fg or '#CAD3F5', g.ws_dim or '#939AB7'
  local sel, line = g.ws_sel or '#3F4A5A', g.ws_line or '#2E3440'
  for _, grp in ipairs({ 'Normal', 'NormalNC', 'NormalFloat', 'EndOfBuffer', 'NonText',
    'SignColumn', 'LineNr', 'FoldColumn', 'MsgArea', 'StatusLine',
    'StatusLineNC', 'VertSplit', 'WinSeparator' }) do
    hl(grp, { bg = 'NONE', ctermbg = 'NONE' })
  end
  hl('Normal', { fg = fg })
  hl('MsgArea', { fg = dim })
  hl('Visual', { bg = sel, fg = 'NONE' })
  hl('CursorLine', { bg = line, bold = false, italic = false, underline = false })
  hl('LineNr', { fg = dim, bg = 'NONE' })
  hl('Pmenu', { bg = sel, fg = fg })
  -- the rest of the palette: each element its own role (like a themed
  -- tmux/nvim port) instead of one tint everywhere
  local acc, acc2 = g.ws_accent or '#C6A0F6', g.ws_accent2 or '#8AADF4'
  local ok, warn = g.ws_ok or '#A6DA95', g.ws_warn or '#EED49F'
  local err, info = g.ws_err or '#ED8796', g.ws_info or '#8BD5CA'
  local deep = g.ws_deep or '#181926'
  hl('CursorLineNr', { fg = acc, bg = line, bold = true })
  hl('PmenuSel', { bg = acc, fg = deep, bold = true })
  hl('Search', { bg = warn, fg = deep })
  hl('IncSearch', { bg = acc, fg = deep })
  hl('CurSearch', { bg = acc, fg = deep })
  hl('MatchParen', { fg = acc, bg = 'NONE', bold = true, underline = true })
  hl('Title', { fg = acc, bold = true })
  hl('Comment', { fg = dim, italic = true })
  hl('Constant', { fg = warn })
  hl('String', { fg = ok })
  hl('Identifier', { fg = acc2 })
  hl('Function', { fg = acc2 })
  hl('Statement', { fg = acc, bold = false })
  hl('PreProc', { fg = info })
  hl('Type', { fg = warn, bold = false })
  hl('Special', { fg = info })
  hl('Underlined', { fg = acc2, underline = true })
  hl('Directory', { fg = acc2 })
  hl('Todo', { fg = deep, bg = warn, bold = true })
  hl('Error', { fg = err, bg = 'NONE', bold = true })
  hl('ErrorMsg', { fg = err, bg = 'NONE' })
  hl('WarningMsg', { fg = warn })
  hl('Question', { fg = ok })
  hl('SpellBad', { sp = err, undercurl = true })
  -- markdown: headings in the accent, links in accent2, code in green
  for n = 1, 6 do
    hl('markdownH' .. n, { fg = acc, bold = true })
    hl('markdownH' .. n .. 'Delimiter', { fg = acc })
    -- render-markdown: heading text in the accent, no banded backgrounds
    hl('RenderMarkdownH' .. n, { fg = acc, bold = true })
    hl('RenderMarkdownH' .. n .. 'Bg', { bg = 'NONE' })
  end
  hl('markdownLinkText', { fg = acc2, underline = true })
  hl('markdownUrl', { fg = dim, underline = true })
  hl('markdownCode', { fg = ok })
  hl('markdownCodeBlock', { fg = ok })
  hl('markdownCodeDelimiter', { fg = dim })
  hl('markdownListMarker', { fg = acc })
  hl('markdownOrderedListMarker', { fg = acc })
  hl('markdownBlockquote', { fg = dim, italic = true })
  hl('markdownBold', { fg = fg, bold = true })
  hl('markdownItalic', { fg = fg, italic = true })
  hl('markdownError', { fg = 'NONE', bg = 'NONE' })
  hl('RenderMarkdownCode', { bg = line })
  hl('RenderMarkdownCodeInline', { fg = ok, bg = line })
  hl('RenderMarkdownBullet', { fg = acc })
  hl('RenderMarkdownQuote', { fg = dim })
  hl('RenderMarkdownLink', { fg = acc2 })
  for from, to in pairs({
    ['@markup.heading'] = 'Title', ['@markup.link.label'] = 'markdownLinkText',
    ['@markup.link.url'] = 'markdownUrl', ['@markup.raw'] = 'markdownCode',
    ['@markup.list'] = 'markdownListMarker', ['@markup.quote'] = 'markdownBlockquote',
  }) do
    vim.api.nvim_set_hl(0, from, { link = to })
  end
end
vim.api.nvim_create_autocmd('ColorScheme', {
  group = vim.api.nvim_create_augroup('ws_theme', { clear = true }),
  callback = theme,
})
theme()

-- --- always on disk ----------------------------------------------------------
local save = vim.api.nvim_create_augroup('ws_autosave', { clear = true })
vim.api.nvim_create_autocmd({ 'InsertLeave', 'TextChanged', 'FocusLost', 'BufLeave', 'CursorHold', 'CursorHoldI' }, {
  group = save,
  command = 'silent! wall',
})
-- external writes (voice dictation, other editors) reload silently
vim.api.nvim_create_autocmd({ 'FocusGained', 'BufEnter', 'CursorHold' }, {
  group = save,
  command = 'silent! checktime',
})

-- --- plugins -----------------------------------------------------------------
-- lazy.nvim, shared with the regular nvim install (same data dir: plugins
-- already there are reused). Its own lockfile, no update checks, and the
-- user's ~/.config/nvim is taken off the runtimepath so a personal
-- plugin can never block the pane.
local lazypath = vim.fn.stdpath('data') .. '/lazy/lazy.nvim'
if not vim.uv.fs_stat(lazypath) and vim.fn.executable('git') == 1 then
  vim.fn.system({ 'git', 'clone', '--filter=blob:none', '--branch=stable',
    'https://github.com/folke/lazy.nvim.git', lazypath })
end
if vim.uv.fs_stat(lazypath) then
  -- ~/.config/nvim (and its after/ dir) stays out; no rtp reset, it would put it back
  for _, p in ipairs({ vim.fn.stdpath('config'), vim.fn.stdpath('config') .. '/after' }) do
    vim.opt.rtp:remove(p)
  end
  vim.opt.rtp:prepend(lazypath)
  require('lazy').setup({
    {
      'MeanderingProgrammer/render-markdown.nvim',
      dependencies = { 'nvim-tree/nvim-web-devicons' },
      config = function()
        require('render-markdown').setup({
          code = { enabled = true, render_modes = true, language_icon = true, language_name = true },
          document = { enabled = true, render_modes = true },
        })
      end,
    },
  }, {
    lockfile = vim.fn.expand('~/.cache/kitchen-sink/nvim-lazy-lock.json'),
    change_detection = { enabled = false, notify = false },
    checker = { enabled = false },
    performance = { rtp = { reset = false } },
    install = { colorscheme = { 'default' } },
    ui = { border = 'none' },
  })
end

-- snippets (vim/snippets/<filetype>.json, VSCode format) + auto popup
dofile(here .. '/snippets.lua').setup()

-- --- inline images -------------------------------------------------------------
-- Markdown image links (![alt](path)) get vim.g.ws_img_rows blank virtual lines
-- under them; the notes window draws the real image over those rows. The
-- screen rows of every visible image are written to vim.g.ws_img_file as JSON
-- whenever the view changes (scroll, edit, resize, buffer switch).
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
