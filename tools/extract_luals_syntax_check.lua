-- Extracts LuaLS `test/parser_test/syntax_check.lua` into JSON for the Rust tests.
-- Run with the LuaLS binary (it is a Lua 5.5 interpreter):
--   ref/luals-bin/bin/lua-language-server.exe tools/extract_luals_syntax_check.lua <syntax_check.lua> <out.json>
-- Each case: { script, version, expect = null | { type, multi?, version?, info? }, targets = [[start, finish], ...] }
-- Targets are byte offsets (0-based, finish exclusive) into `script` after the `<! !>` markers are removed.

local src_path, out_path = ...
local src = assert(io.open(src_path, 'rb')):read('a')

-- Drop the harness (everything before the first test) and run the cases under ours.
local body_start = assert(src:find('\nTEST%s*%['), 'no TEST found')
local body = src:sub(body_start + 1)

local cases = {}

local function strip(script)
    local out, targets, pos = {}, {}, 1
    local len = 0
    while true do
        local a = script:find('<!', pos, true)
        if not a then break end
        local b = assert(script:find('!>', a + 2, true), 'unclosed marker')
        out[#out + 1] = script:sub(pos, a - 1)
        len = len + (a - pos)
        local inner = script:sub(a + 2, b - 1)
        out[#out + 1] = inner
        targets[#targets + 1] = { len, len + #inner }
        len = len + #inner
        pos = b + 2
    end
    out[#out + 1] = script:sub(pos)
    return table.concat(out), targets
end

local env = setmetatable({}, { __index = _G })
env.Version = 'Lua 5.4'
env.EXISTS = setmetatable({}, { __name = 'EXISTS' })

local function record(version, script, expect)
    local stripped, targets = strip(script)
    cases[#cases + 1] = { script = stripped, version = version, expect = expect, targets = targets }
end

function env.TEST(script)
    return function(expect)
        record(env.Version, script, expect)
    end
end

function env.TestWith(version)
    return function(script)
        return function(expect)
            record(version, script, expect)
        end
    end
end

local chunk = assert(load(body, '=syntax_check.lua', 't', env))
chunk()

local function encode(v, out)
    local t = type(v)
    if v == nil then
        out[#out + 1] = 'null'
    elseif v == env.EXISTS then
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
    encode({ script = c.script, version = c.version, expect = c.expect or false, targets = c.targets }, out)
end
out[#out + 1] = '\n]\n'
assert(io.open(out_path, 'wb')):write(table.concat(out))
print(('%d cases'):format(#cases))
