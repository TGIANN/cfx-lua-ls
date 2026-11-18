-- Builds the embedded library under crates/ide/library from the reference sources:
--   ref/luals-bin/bin/lua-language-server.exe tools/gen_library.lua <repo>
-- * lua54/: LuaLS meta templates (ref/luals/meta/template) for Lua 5.4: `---#if`
--   branches resolved with VERSION = 5.4, JIT = false; `---#DES 'key'` replaced by
--   the English text from ref/luals/locale/en-us/meta.lua.
-- * fivem/: overextended/fivem-lls-addon `library/` (natives per game, runtime), as is.
-- Then writes crates/ide/src/library/files.rs, the list of embedded files.

local root = ...
assert(root, 'usage: gen_library.lua <repo>')
local fs = require 'bee.filesystem'

local function read(path)
    local f = assert(io.open(path, 'rb'))
    local s = f:read('a')
    f:close()
    return s
end

local function write(path, s)
    fs.create_directories(fs.path(path):parent_path())
    local f = assert(io.open(path, 'wb'))
    f:write(s)
    f:close()
end

-- Descriptions: the locale file is a list of `key = 'text'` / `key = [[text]]`.
local des = {}
do
    -- Keys can be dotted (`loadmode.b = ...`): reads return a proxy for the prefix.
    local function proxy(prefix)
        return setmetatable({}, {
            __newindex = function (_, k, v) des[prefix and (prefix .. '.' .. k) or k] = v end,
            __index = function (_, k) return proxy(prefix and (prefix .. '.' .. k) or k) end,
        })
    end
    local env = proxy(nil)
    local chunk = assert(load(read(root .. '/ref/luals/locale/en-us/meta.lua'), 'meta', 't', env))
    chunk()
end

local function cond(expr)
    local f = assert(load('return ' .. expr, 'cond', 't', { VERSION = 5.4, JIT = false, DISABLE = function () end }))
    return f()
end

local function compile(text)
    local out = {}
    -- Stack of { active = this branch is taken, done = a branch was taken, outer = enclosing active }.
    local stack = {}
    local function active()
        local top = stack[#stack]
        return not top or top.active
    end
    for raw in (text .. '\n'):gmatch('([^\n]*)\n') do
        local line = raw:gsub('\r$', '')
        local directive = line:match('^%-%-%-#(.*)$')
        if directive then
            -- One line: `---#if not JIT then DISABLE() end` (only DISABLE is used so).
            local one, body = directive:match('^if%s+(.-)%s+then%s+(.-)%s+end%s*$')
            local c = not one and directive:match('^if%s+(.-)%s+then')
            if one then
                if active() and cond(one) and body == 'DISABLE()' then
                    return ''
                end
            elseif c then
                local outer = active()
                local yes = outer and cond(c)
                stack[#stack + 1] = { active = yes, done = yes, outer = outer }
            elseif directive:match('^elseif') then
                local top = stack[#stack]
                local yes = top.outer and not top.done and cond(directive:match('^elseif%s+(.-)%s+then'))
                top.active, top.done = yes, top.done or yes
            elseif directive:match('^else') then
                local top = stack[#stack]
                top.active = top.outer and not top.done
                top.done = true
            elseif directive:match('^end') then
                stack[#stack] = nil
            elseif directive:match('^DES') then
                if active() then
                    local key = directive:match("^DES%s+'(.-)'")
                    local d = key and des[key]
                    if d then
                        d = d:gsub('^\n', ''):gsub('\n$', '')
                        for l in (d .. '\n'):gmatch('([^\n]*)\n') do
                            out[#out + 1] = '---' .. l
                        end
                    end
                end
            end
        elseif active() then
            out[#out + 1] = line
        end
    end
    return table.concat(out, '\n') .. '\n'
end

local files = {}

for _, name in ipairs { 'builtin', 'basic', 'coroutine', 'debug', 'io', 'math', 'os', 'package', 'string', 'table', 'utf8' } do
    local rel = 'lua54/' .. name .. '.lua'
    write(root .. '/crates/ide/library/' .. rel, compile(read(root .. '/ref/luals/meta/template/' .. name .. '.lua')))
    files[#files + 1] = rel
end

local addon = root .. '/ref/fivem-lls-addon/library'
local copied = {}
local function walk(dir)
    for entry in fs.pairs(dir) do
        if fs.is_directory(entry) then
            walk(entry)
        else
            local p = entry:string():gsub('\\', '/')
            if p:match('%.lua$') then
                copied[#copied + 1] = p:sub(#addon + 2)
            end
        end
    end
end
walk(fs.path(addon))
table.sort(copied)
for _, rel in ipairs(copied) do
    write(root .. '/crates/ide/library/fivem/' .. rel, read(addon .. '/' .. rel))
    files[#files + 1] = 'fivem/' .. rel
end
write(root .. '/crates/ide/library/fivem/LICENSE', read(root .. '/ref/fivem-lls-addon/LICENSE'))
write(root .. '/crates/ide/library/lua54/LICENSE', read(root .. '/ref/luals/LICENSE'))

local rs = { '// Generated by tools/gen_library.lua: the embedded library files.', '', 'pub(super) const FILES: &[(&str, &str)] = &[' }
for _, rel in ipairs(files) do
    rs[#rs + 1] = ('    ("%s", include_str!("../../library/%s")),'):format(rel, rel)
end
rs[#rs + 1] = '];'
write(root .. '/crates/ide/src/library/files.rs', table.concat(rs, '\n') .. '\n')
print(#files .. ' files')
