-- Formats every file of a list with LuaLS's own formatter (EmmyLuaCodeStyle, the
-- `code_format` module), to compare with cfx-lua-fmt. Run with the LuaLS binary:
--   ref/luals-bin/bin/lua-language-server.exe tools/luals_format.lua <list.txt> <out_dir>
-- Writes <out_dir>/<n>.lua (n = the file's 1-based line in the list) for each file
-- it formats, with the default style as LuaLS passes it (non-standard symbols on).
-- A file their parser rejects gets no output.

local list, out = ...
local cf = require 'code_format'
cf.set_nonstandard_symbol()

local n, done = 0, 0
for path in io.lines(list) do
    n = n + 1
    local f = io.open(path, 'rb')
    if f then
        local text = f:read('a')
        f:close()
        local ok, result = cf.format('file:///x.lua', text, {})
        if ok and result then
            local o = assert(io.open(out .. '/' .. n .. '.lua', 'wb'))
            o:write(result)
            o:close()
            done = done + 1
        end
    end
end
print(('%d files, %d formatted'):format(n, done))
