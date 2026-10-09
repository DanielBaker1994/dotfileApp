-- expand every markdown snippet of vim/snippets.lua (+ a generated table)
-- with the real vim.snippet.expand → $OUT/<prefix>.md (placeholders keep
-- their defaults). Run: nvim --headless --clean -u NONE -c 'luafile expand.lua' -c 'qa!'
local M = dofile(vim.env.WS_REPO .. '/vim/snippets.lua')
local n = 0
for _, it in ipairs(M.items('markdown', '3x4')) do
  if not it.clipboard then
    vim.cmd('enew!'); vim.bo.filetype = 'markdown'
    local ok, err = pcall(vim.snippet.expand, it.body)
    vim.snippet.stop()
    if not ok then io.stderr:write('EXPAND FAIL ' .. it.prefix .. ': ' .. tostring(err) .. '\n') end
    local f = assert(io.open(vim.env.OUT .. '/' .. it.prefix:gsub('[^%w_%-]', '_') .. '.md', 'w'))
    f:write(table.concat(vim.api.nvim_buf_get_lines(0, 0, -1, false), '\n') .. '\n'); f:close()
    n = n + 1
  end
end
io.stderr:write('expanded ' .. n .. '\n')
