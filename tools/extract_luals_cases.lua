-- Extracts LuaLS's hover and completion cases into JSON for the Rust tests.
-- Run with the LuaLS binary (it is a Lua 5.5 interpreter):
--   ref/luals-bin/bin/lua-language-server.exe tools/extract_luals_cases.lua hover <out.json> ref/luals/test/hover/init.lua
--   ref/luals-bin/bin/lua-language-server.exe tools/extract_luals_cases.lua completion <out.json> ref/luals/test/completion/common.lua ref/luals/test/completion/continue.lua
-- Each case: { file, n, script, expect, config, flags }. `script` keeps LuaLS's
-- `<?x?>` / `<??>` markers. Hover: `expect` is the text LuaLS's first ```lua
-- block must equal. Completion: `expect` is null (no completion), "<function>"
-- (a custom check), "<exists>" (some completion), or { include, items =
-- [{ label, kind }] }; kinds are LSP
-- CompletionItemKind names. `config` lists the `config.set` switches in force
-- that differ from the suite's defaults (such cases are kept but not scored;
-- `callSnippet` `Both` / `Disable` count as defaults);
-- `flags` holds `ContinueTyping` / `IgnoreFunction` / `insertText` when set.

local mode, out_path = ...
local files = { select(3, ...) }

local cases = {}

-- Switch values the Rust test makes no difference for: it drops call snippets.
local neutral = {
    ['Lua.completion.callSnippet'] = { ["'Both'"] = true, ["'Disable'"] = true },
}

local function run(path)
    local src = assert(io.open(path, 'rb')):read('a')
    local name = path:match('([^/\\]+)%.lua$')
    -- Drop the harness (everything before the first test) and run the cases under ours.
    local body_start = assert(src:find('\nTEST%s*[%[%(]'), 'no TEST found in ' .. path)
    local head = src:sub(1, body_start)
    local body = src:sub(body_start + 1)
    local active = {}
    local first_case = #cases + 1
    local env = setmetatable({}, { __index = _G })
    env.EXISTS = setmetatable({}, { __name = 'EXISTS' })
    env.Cared = {}
    env.define = { CompletionItemKind = setmetatable({}, { __index = function(_, k) return k end }) }
    local function snapshot()
        local copy = {}
        for k, v in pairs(active) do
            copy[k] = v
        end
        return copy
    end
    -- A value as the harness writes it (`'Both'`, `false`, `50`, `{ '//' }`).
    local function show(value)
        if type(value) == 'string' then
            return "'" .. value .. "'"
        elseif type(value) == 'table' then
            local parts = {}
            for _, x in ipairs(value) do
                parts[#parts + 1] = show(x)
            end
            return '{' .. table.concat(parts, ',') .. '}'
        end
        return tostring(value)
    end
    -- The value each key has before the first test, and the last one set.
    local before, last = {}, {}
    local n = 0
    local function set(_, key, value)
        active[key] = show(value)
        last[key] = active[key]
        if n == 0 then
            before[key] = active[key]
        end
    end
    env.config = { set = set, add = set, remove = set, get = function() return nil end }
    env.require = function(m)
        if m == 'config' then return env.config end
        if m == 'proto.define' then return env.define end
        return setmetatable({}, { __index = function() return function() end end })
    end
    function env.TEST(script, ...)
        local extra = { ... }
        return function(expect)
            local flags = {}
            if rawget(env, 'ContinueTyping') then flags[#flags + 1] = 'ContinueTyping' end
            if rawget(env, 'IgnoreFunction') then flags[#flags + 1] = 'IgnoreFunction' end
            if env.Cared.insertText then flags[#flags + 1] = 'insertText' end
            local e = expect
            if mode == 'completion' then
                if type(expect) == 'function' then
                    e = '<function>'
                elseif expect == env.EXISTS then
                    e = '<exists>'
                elseif type(expect) == 'table' then
                    local items = {}
                    for _, item in ipairs(expect) do
                        items[#items + 1] = { label = item.label, kind = item.kind or '' }
                    end
                    e = { include = expect.include == true, items = items }
                end
            end
            cases[#cases + 1] = { file = name, n = n, script = script, expect = e == nil and '<nil>' or e, config = snapshot(), flags = flags }
            n = n + 1
        end
    end
    -- The config.set calls in the harness part of the file.
    for key, value in head:gmatch("config%.set%(%s*nil%s*,%s*'([^']+)'%s*,%s*([^%)]-)%s*%)") do
        active[key] = value
        before[key] = value
    end
    assert(load(body, '=' .. name, 't', env))()
    -- A switch's default: its value before the first test, else the value the
    -- file leaves it at (the suites set a switch for some cases, then reset it).
    for i = first_case, #cases do
        local c = cases[i]
        local keys = {}
        for k, v in pairs(c.config) do
            local default = before[k] or last[k]
            if v ~= default and not (neutral[k] and neutral[k][v]) then
                keys[#keys + 1] = k
            end
        end
        table.sort(keys)
        c.config = keys
    end
end

for _, f in ipairs(files) do
    run(f)
end

local function encode(v, out)
    local t = type(v)
    if v == nil then
        out[#out + 1] = 'null'
    elseif t == 'boolean' or t == 'number' then
        out[#out + 1] = tostring(v)
    elseif t == 'string' then
        if v == '<nil>' then
            out[#out + 1] = 'null'
            return
        end
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
    encode(c, out)
end
out[#out + 1] = '\n]\n'
assert(io.open(out_path, 'wb')):write(table.concat(out))
print(('%d cases'):format(#cases))
