-- snippets.lua — VSCode-format snippets (snippets/<filetype>.json beside this
-- file) with an auto-popup menu. Native only: vim.snippet expands, the
-- built-in completion menu lists. Typing 2+ characters of a snippet's prefix
-- opens the menu; Tab accepts the first / selected entry, then jumps between
-- placeholders (S-Tab back).
--
-- Insert picker (M.pick): <leader>i (normal mode) / :Insert —
-- a fuzzy search over EVERY snippet of the buffer's filetype, grouped by the
-- snippet's `category`, with a preview of what lands. Type words to filter
-- ("callout risk", "table"), a size ("3x4" = 3 body rows x 4 columns) builds
-- that table, "clipboard" turns a copied CSV / TSV into a table.
-- C-n / C-p / arrows move, Enter inserts, Esc cancels, C-u clears, C-w word.
local M = {}

local dir = vim.fn.fnamemodify(debug.getinfo(1, 'S').source:sub(2), ':p:h') .. '/snippets'
local MIN = 2

-- `${HOME}` in a body is text, not an LSP variable: escape the non-LSP ones
local function body_text(body)
  local s = type(body) == 'table' and table.concat(body, '\n') or tostring(body)
  return (s:gsub('%${([%a_][%w_]*)}', function(name)
    if name:match('^TM_') or name:match('^CURRENT_') or name:match('^CLIPBOARD')
      or name:match('^RANDOM') or name:match('^UUID') or name:match('^LINE_COMMENT')
      or name:match('^BLOCK_COMMENT') or name:match('^WORKSPACE_') then
      return nil
    end
    return '\\${' .. name .. '}'
  end))
end

local cache = {}
local function load(ft)
  if cache[ft] then return cache[ft] end
  local list = {}
  local f = io.open(dir .. '/' .. ft .. '.json', 'r')
  if f then
    local ok, data = pcall(vim.json.decode, f:read('*a'))
    f:close()
    if ok and type(data) == 'table' then
      for name, s in pairs(data) do
        local prefixes = type(s.prefix) == 'table' and s.prefix or { s.prefix }
        for _, p in ipairs(prefixes) do
          list[#list + 1] = { prefix = p, body = body_text(s.body or ''), desc = s.description or name,
                              category = s.category or 'Other', inline = s.inline == true }
        end
      end
      table.sort(list, function(a, b) return a.prefix < b.prefix end)
    end
  end
  cache[ft] = list
  return list
end

local busy = false

local function popup()
  if busy or vim.fn.pumvisible() == 1 or vim.snippet.active() then return end
  local snippets = load(vim.bo.filetype)
  if #snippets == 0 then return end
  local col = vim.fn.col('.')
  local before = vim.api.nvim_get_current_line():sub(1, col - 1)
  local word = before:match('[%w_%-]+$')
  if not word or #word < MIN then return end
  local lw, items = word:lower(), {}
  for _, s in ipairs(snippets) do
    if s.prefix:lower():find(lw, 1, true) then
      items[#items + 1] = {
        word = s.prefix, abbr = s.prefix, menu = '[snippet]', info = s.desc .. '\n\n' .. s.body:gsub('\\%$', '$'),
        user_data = { ws_snippet = s.body },
      }
    end
  end
  if #items > 0 then vim.fn.complete(col - #word, items) end
end

local function expand_chosen()
  local item = vim.v.completed_item
  local ud = type(item) == 'table' and item.user_data
  if type(ud) ~= 'table' or not ud.ws_snippet then return end
  local word, body = item.word, ud.ws_snippet
  busy = true
  vim.schedule(function()
    local row, col = unpack(vim.api.nvim_win_get_cursor(0))
    vim.api.nvim_buf_set_text(0, row - 1, col - #word, row - 1, col, {})
    vim.api.nvim_win_set_cursor(0, { row, col - #word })
    vim.snippet.expand(body)
    busy = false
  end)
end

local function feed(keys)
  vim.api.nvim_feedkeys(vim.api.nvim_replace_termcodes(keys, true, false, true), 'n', false)
end


-- ── insert picker ──────────────────────────────────────────────────────────
local function gen_table(rows, cols)
  local head, sep, n = {}, {}, 0
  for c = 1, cols do n = n + 1; head[c] = ('${%d:Header %d}'):format(n, c); sep[c] = '---' end
  local lines = { '| ' .. table.concat(head, ' | ') .. ' |', '| ' .. table.concat(sep, ' | ') .. ' |' }
  for _ = 1, rows do
    local cells = {}
    for c = 1, cols do n = n + 1; cells[c] = '$' .. n end
    lines[#lines + 1] = '| ' .. table.concat(cells, ' | ') .. ' |'
  end
  return table.concat(lines, '\n') .. '$0'
end

-- clipboard CSV / TSV → a pipe table (literal text, no placeholders)
local function clipboard_table()
  local text = vim.fn.getreg('+')
  if text == '' then text = vim.fn.getreg('"') end
  local rows = {}
  for line in vim.gsplit((text:gsub('\r', '')), '\n', { plain = true }) do
    if line:match('%S') then rows[#rows + 1] = line end
  end
  if #rows < 1 then return nil end
  local sep = rows[1]:find('\t', 1, true) and '\t' or ','
  local cells, width = {}, 0
  for i, r in ipairs(rows) do
    cells[i] = vim.split(r, sep, { plain = true })
    for j, c in ipairs(cells[i]) do cells[i][j] = vim.trim(c):gsub('|', '\\|') end
    width = math.max(width, #cells[i])
  end
  if width < 2 then return nil end
  local out = {}
  for i, r in ipairs(cells) do
    for j = #r + 1, width do r[j] = '' end
    out[#out + 1] = '| ' .. table.concat(r, ' | ') .. ' |'
    if i == 1 then
      local d = {}
      for j = 1, width do d[j] = '---' end
      out[#out + 1] = '| ' .. table.concat(d, ' | ') .. ' |'
    end
  end
  return table.concat(out, '\n')
end

-- the entries the picker lists for `query`: the filetype's snippets + dynamic ones
function M.items(ft, query)
  local items = {}
  for _, s in ipairs(load(ft)) do items[#items + 1] = s end
  if ft == 'markdown' then
    local r, c = (query or ''):match('(%d+)%s*[x×]%s*(%d+)')
    if r and c then
      r, c = math.min(tonumber(r), 30), math.min(tonumber(c), 12)
      if r >= 1 and c >= 1 then
        table.insert(items, 1, { prefix = ('table %dx%d'):format(r, c), category = 'Tables',
          desc = ('Table: %d body rows x %d columns (+ header)'):format(r, c), body = gen_table(r, c) })
      end
    end
    items[#items + 1] = { prefix = 'table from clipboard', category = 'Tables',
      desc = 'Turn the copied CSV / TSV (or spreadsheet cells) into a table', clipboard = true }
  end
  return items
end

-- every word of `query` must appear in "category prefix description" (case-blind);
-- prefix hits rank first, then category order
function M.filter(items, query)
  local words = {}
  for w in (query or ''):lower():gmatch('%S+') do words[#words + 1] = w end
  local out = {}
  for i, it in ipairs(items) do
    local hay = (it.category .. ' ' .. it.prefix .. ' ' .. it.desc):lower()
    local pre, ok = 0, true
    for _, w in ipairs(words) do
      if not hay:find(w, 1, true) then ok = false; break end
      if it.prefix:lower():find(w, 1, true) then pre = pre + 1 end
    end
    if ok then out[#out + 1] = { it = it, pre = pre, i = i } end
  end
  table.sort(out, function(a, b)
    if a.pre ~= b.pre then return a.pre > b.pre end
    if a.it.category ~= b.it.category then return a.it.category < b.it.category end
    return a.i < b.i
  end)
  return vim.tbl_map(function(x) return x.it end, out)
end

-- what the snippet looks like once expanded (placeholders → their defaults)
local function preview_lines(it)
  local text = it.clipboard and (clipboard_table() or '(the clipboard holds no CSV / TSV)') or it.body
  text = text:gsub('%${%d+:([^}]*)}', '%1'):gsub('%${%d+}', ''):gsub('%$%d+', ''):gsub('\\%$', '$')
  return vim.split(text, '\n', { plain = true })
end

local function place(it)
  local inline = it.inline
  local mode = vim.api.nvim_get_mode().mode:sub(1, 1)
  local row, col = unpack(vim.api.nvim_win_get_cursor(0))
  local line = vim.api.nvim_get_current_line()
  if not inline and line:match('%S') then
    -- block items start on a fresh line under the current one
    vim.api.nvim_buf_set_lines(0, row, row, false, { '' })
    row, col = row + 1, 0
    vim.api.nvim_win_set_cursor(0, { row, 0 })
  elseif mode ~= 'i' and #line > 0 and inline then
    col = col + 1  -- normal mode: after the character under the cursor
    vim.api.nvim_win_set_cursor(0, { row, col })
  end
  if it.clipboard then
    local t = clipboard_table()
    if not t then vim.notify('clipboard holds no CSV / TSV table', vim.log.levels.WARN); return end
    vim.api.nvim_buf_set_lines(0, row - 1, row, false, vim.split(t, '\n', { plain = true }))
    return
  end
  vim.cmd('startinsert')
  vim.schedule(function() vim.snippet.expand(it.body) end)
end

function M.pick()
  local ft = vim.bo.filetype
  if #load(ft) == 0 then vim.notify('no snippets for filetype "' .. ft .. '"', vim.log.levels.INFO); return end
  local cols, lines = vim.o.columns, vim.o.lines
  local W = math.min(118, cols - 4)
  local H = math.min(22, lines - 6)
  local LW = math.floor(W * 0.5)
  local row0, col0 = math.floor((lines - H) / 2) - 1, math.floor((cols - W) / 2)
  local lb, pb = vim.api.nvim_create_buf(false, true), vim.api.nvim_create_buf(false, true)
  local function float(buf, c, w, title)
    return vim.api.nvim_open_win(buf, false, { relative = 'editor', row = row0, col = c, width = w, height = H,
      style = 'minimal', border = 'rounded', title = title, title_pos = 'left', focusable = false, zindex = 80 })
  end
  local lw = float(lb, col0, LW, ' Insert ')
  local pw = float(pb, col0 + LW + 3, W - LW - 3, ' Preview ')
  local ns = vim.api.nvim_create_namespace('ws_insert_picker')
  local q, sel, top, result = '', 1, 1, nil

  local function render()
    local shown = M.filter(M.items(ft, q), q)
    if sel > #shown then sel = #shown end
    if sel < 1 then sel = 1 end
    local vis = H - 2
    if sel < top then top = sel elseif sel > top + vis - 1 then top = sel - vis + 1 end
    local out, marks = { '› ' .. q .. '▏' }, {}
    out[#out + 1] = ''
    local last
    for i = top, math.min(#shown, top + vis - 1) do
      local it = shown[i]
      out[#out + 1] = (i == sel and '▌' or ' ') .. ' ' .. it.prefix:gsub('^markdown_', '')
      marks[#out] = { sel = i == sel, cat = it.category }
      last = it
    end
    if #shown == 0 then out[#out + 1] = '  (nothing matches)' end
    vim.api.nvim_buf_set_lines(lb, 0, -1, false, out)
    vim.api.nvim_buf_clear_namespace(lb, ns, 0, -1)
    vim.api.nvim_buf_add_highlight(lb, ns, 'Title', 0, 0, -1)
    for l, m in pairs(marks) do
      if m.sel then vim.api.nvim_buf_add_highlight(lb, ns, 'PmenuSel', l - 1, 0, -1) end
      vim.api.nvim_buf_set_extmark(lb, ns, l - 1, 0, { virt_text = { { m.cat, 'Comment' } }, virt_text_pos = 'right_align' })
    end
    local it = shown[sel]
    local pv = it and { it.desc, '' } or { '' }
    if it then vim.list_extend(pv, preview_lines(it)) end
    vim.api.nvim_buf_set_lines(pb, 0, -1, false, pv)
    vim.api.nvim_buf_clear_namespace(pb, ns, 0, -1)
    vim.api.nvim_buf_add_highlight(pb, ns, 'Comment', 0, 0, -1)
    return shown
  end

  local KEYS = { up = vim.api.nvim_replace_termcodes('<Up>', true, false, true),
                 down = vim.api.nvim_replace_termcodes('<Down>', true, false, true),
                 bs = vim.api.nvim_replace_termcodes('<BS>', true, false, true),
                 pgup = vim.api.nvim_replace_termcodes('<PageUp>', true, false, true),
                 pgdn = vim.api.nvim_replace_termcodes('<PageDown>', true, false, true) }
  vim.g.ws_picking = 1  -- the app's Ctrl+N (new note) must not fire while the picker owns the keys
  while true do
    local shown = render()
    vim.cmd('redraw')
    local ok, ch = pcall(vim.fn.getcharstr)
    if not ok or ch == '\27' or ch == '\3' then break end
    if ch == '\r' then result = shown[sel]; break
    elseif ch == '\14' or ch == KEYS.down then sel = sel + 1
    elseif ch == '\16' or ch == KEYS.up then sel = sel - 1
    elseif ch == KEYS.pgdn then sel = sel + 8
    elseif ch == KEYS.pgup then sel = sel - 8
    elseif ch == '\127' or ch == '\8' or ch == KEYS.bs then q = q:sub(1, math.max(0, #q - #(vim.fn.matchstr(q, '.$')))); sel = 1
    elseif ch == '\21' then q = ''; sel = 1
    elseif ch == '\23' then q = q:gsub('%s*%S+%s*$', ''); sel = 1
    elseif #ch >= 1 and ch:byte(1) >= 32 and ch:byte(1) ~= 128 then q = q .. ch; sel = 1 end
  end
  vim.g.ws_picking = 0
  for _, w in ipairs({ lw, pw }) do if vim.api.nvim_win_is_valid(w) then vim.api.nvim_win_close(w, true) end end
  if result then place(result) end
end

function M.setup()
  vim.opt.completeopt = { 'menu', 'menuone', 'noselect' }
  local g = vim.api.nvim_create_augroup('ws_snippets', { clear = true })
  vim.api.nvim_create_autocmd('TextChangedI', { group = g, callback = popup })
  vim.api.nvim_create_autocmd('CompleteDone', { group = g, callback = expand_chosen })
  vim.keymap.set({ 'i', 's' }, '<Tab>', function()
    if vim.fn.pumvisible() == 1 then
      feed(vim.fn.complete_info({ 'selected' }).selected == -1 and '<C-n><C-y>' or '<C-y>')
    elseif vim.snippet.active({ direction = 1 }) then
      vim.snippet.jump(1)
    else
      feed('<Tab>')
    end
  end, { desc = 'accept snippet / next placeholder' })
  vim.keymap.set({ 'i', 's' }, '<S-Tab>', function()
    if vim.snippet.active({ direction = -1 }) then vim.snippet.jump(-1) else feed('<S-Tab>') end
  end, { desc = 'previous placeholder' })
  vim.api.nvim_create_user_command('Insert', M.pick, { desc = 'search + insert a snippet' })
  vim.keymap.set('n', '<leader>i', M.pick, { desc = 'insert: search snippets' })
end

return M
