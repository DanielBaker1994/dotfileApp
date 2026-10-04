-- snippets.lua — VSCode-format snippets (snippets/<filetype>.json beside this
-- file) with an auto-popup menu. Native only: vim.snippet expands, the
-- built-in completion menu lists. Typing 2+ characters of a snippet's prefix
-- opens the menu; Tab accepts the first / selected entry, then jumps between
-- placeholders (S-Tab back).
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
          list[#list + 1] = { prefix = p, body = body_text(s.body or ''), desc = s.description or name }
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
end

return M
