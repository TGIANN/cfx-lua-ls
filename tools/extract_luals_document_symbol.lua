-- Extracts LuaLS `test/document_symbol/init.lua` into JSON for the Rust tests.
-- Run with the LuaLS binary (it is a Lua 5.5 interpreter):
--   ref/luals-bin/bin/lua-language-server.exe tools/extract_luals_document_symbol.lua <init.lua> <out.json>
-- Each case: { script, expect = [symbol] }; a symbol is { name, detail, kind, range,
-- selectionRange, valueRange?, children? }. Kinds are LSP SymbolKind names. Ranges
-- stay in LuaLS positions (line * 10000 + byte column); `children = "<exists>"`
-- means LuaLS only checks that there are some.

local src_path, out_path = ...
local src = assert(io.open(src_path, 'rb')):read('a')

-- Drop the harness (everything before the first test) and run the cases under ours.
local body_start = assert(src:find('\nTEST%s*%['), 'no TEST found')
local body = src:sub(body_start + 1)

local cases = {}
local env = setmetatable({}, { __index = _G })
env.EXISTS = setmetatable({}, { __name = 'EXISTS' })
env.define = { SymbolKind = setmetatable({}, { __index = function(_, k) return k end }) }

function env.TEST(script)
    return function(expect)
        cases[#cases + 1] = { script = script, expect = expect }
    end
end

assert(load(body, '=document_symbol', 't', env))()

local function encode(v, out)
    local t = type(v)
    if v == env.EXISTS then
        out[#out + 1] = '"<exists>"'
    elseif t == 'boolean' or t == 'number' then
        out[#out + 1] = tostring(v)
    elseif t == 'string' then
        out[#out + 1] = '"' .. v:gsub('[%c"\\]', function(c)
            local map = { ['"'] = '\\"', ['\\'] = '\\\\', ['\n'] = '\\n', ['\r'] = '\\r', ['\t'] = '\\t' }
            return map[c] or ('\\u%04x'):format(c:byte())
        end) .. '"'
    elseif t == 'table' then
        if #v > 0 or next(v) == nil then
            out[#out + 1] = '['
            for i, x in ipairs(v) do
                if i > 1 then out[#out + 1] = ',' end
                encode(x, out)
            end
            out[#out + 1] = ']'
        else
            local keys = {}
            for k in pairs(v) do keys[#keys + 1] = tostring(k) end
            table.sort(keys)
            out[#out + 1] = '{'
            for i, k in ipairs(keys) do
                if i > 1 then out[#out + 1] = ',' end
                encode(k, out)
                out[#out + 1] = ':'
                encode(v[k], out)
            end
            out[#out + 1] = '}'
        end
    else
        error('cannot encode ' .. t)
    end
end

local out = { '[\n' }
for i, c in ipairs(cases) do
    if i > 1 then out[#out + 1] = ',\n' end
    encode({ script = c.script, expect = c.expect }, out)
end
out[#out + 1] = '\n]\n'
assert(io.open(out_path, 'wb')):write(table.concat(out))
print(('%d cases'):format(#cases))
