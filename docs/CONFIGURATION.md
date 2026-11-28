# Configuration

cfx-lua-ls reads lua-language-server's own settings: `.luarc.json` in the
workspace, and `Lua.*` in the editor. A project already set up for LuaLS works
unchanged.

## Rules

The rules (LuaLS's, FiveM API, sandbox, ox_lib suggestions, performance, naming)
are listed in [RULES.md](RULES.md). Turn them off or change their level in
`.luarc.json` with LuaLS's own keys:

```json
{
  "diagnostics.disable": ["fivem/citizen-prefix"],
  "diagnostics.severity": { "naming/local": "hint" },
  "diagnostics.globals": ["Config"]
}
```

or per line with `---@diagnostic disable-next-line: <code>`. `diagnostics.neededFileStatus`
turns a rule on (`"Any"`, `"Opened"`) or off (`"None"`), and
`diagnostics.groupSeverity` / `diagnostics.groupFileStatus` do the same for
LuaLS's groups (`codestyle`, `unused`, `type-check`, ...), with LuaLS's
precedence: a group's value beats a code's own unless that ends in `!`.

## LuaLS switches

LuaLS's type switches are read too: `type.weakNilCheck`, `type.weakUnionCheck`,
`type.castNumberToInteger`, `type.checkTableShape`,
`diagnostics.unusedLocalExclude`, `doc.privateName` / `doc.protectedName` /
`doc.packageName` / `doc.regengine`, `runtime.special` (`{ "xx.assert": "assert" }`: a name that
acts as `assert`, `error`, `os.exit`, `type`, `setmetatable`, `pcall`, `xpcall`,
`rawset` or `require` does), `nameStyle.config` and `spell.dict` (for the
`name-style-check` and `spell-check` codes), `format.defaultConfig` (the style
`codestyle-check` compares with when no `.editorconfig` applies), and in the
editor `hover.expandAlias` and `hint.enable` (inlay hints: parameter names and
local types, off by default).

## `cfx.lualsCompat`

By default this server is narrower than LuaLS in a few places where it would
guess on FiveM code; `"cfx.lualsCompat": true` gives LuaLS's own behaviour
there: nil is checked against declared types (unless `type.weakNilCheck` is
set), a local keeps the type of its first value (`cast-local-type`), library
globals of another Lua version (`unpack`) are `deprecated`, an empty function
body still needs its `---@return` values, and every unknown code in
`---@diagnostic` is reported.

## Libraries outside the workspace

When you open a single resource, point `Lua.workspace.library` at folders that
hold the resources it builds on (a copy of ox_lib, your core resource): their
definitions show in hover and completion, they are not diagnosed, and a
workspace resource of the same name wins over its library copy.

## `check` options

| option | |
|---|---|
| `--format text\|json\|sarif` | SARIF 2.1.0 for GitHub code scanning |
| `--level <severity>` | lowest level reported (default `warning`) |
| `--fail-on <severity>` | exit 1 at this level or above (default `error`) |
| `--fix` | apply the quick fixes in place |
| `--all-files` | also check files no manifest loads |
