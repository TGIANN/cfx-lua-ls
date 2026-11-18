-- Range formatting with LuaLS's own formatter, to compare with cfx-lua-fmt.
-- Run with the LuaLS binary:
--   ref/luals-bin/bin/lua-language-server.exe tools/luals_range_format.lua <list.txt> <out_dir>
-- For each file n of the list, formats a few line ranges picked from its line
-- count (the same ranges `corpus check-range` asks for) and writes
-- <out_dir>/<n>.txt: per range a header `start end ok replaced_start replaced_end
-- byte_length`, a newline, then the text.

local list, out = ...
local cf = require 'code_format'
cf.set_nonstandard_symbol()

local function ranges(lines)
    local r = {}
    for _, f in ipairs { 0.1, 0.3, 0.5, 0.7, 0.9 } do
        local s = math.floor(lines * f)
        r[#r + 1] = { s, s + 3 }
    end
    return r
end

local n = 0
for path in io.lines(list) do
    n = n + 1
    local f = io.open(path, 'rb')
    if f then
        local text = f:read('a')
        f:close()
        local _, breaks = text:gsub('\n', '')
        local parts = {}
        for _, r in ipairs(ranges(breaks + 1)) do
            local ok, result, s, e = cf.range_format('file:///x.lua', text, r[1], r[2], {})
            if ok and result then
                parts[#parts + 1] = ('%d %d 1 %d %d %d\n%s'):format(r[1], r[2], s, e, #result, result)
            else
                parts[#parts + 1] = ('%d %d 0 0 0 0\n'):format(r[1], r[2])
            end
        end
        local o = assert(io.open(out .. '/' .. n .. '.txt', 'wb'))
        o:write(table.concat(parts))
        o:close()
    end
end
print(('%d files'):format(n))
