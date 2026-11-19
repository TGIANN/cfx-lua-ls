# cfx-lua-ls

A fast language server and checker for FiveM Lua (CfxLua), written in Rust.

The aim is everything [lua-language-server](https://github.com/LuaLS/lua-language-server)
offers, at a fraction of its memory use, plus what FiveM needs: `fxmanifest.lua`
awareness, client / server sides, natives, the server sandbox, exports, events,
ox_lib and the frameworks (QBCore, Qbox).

**Status:** early, not released. Done: the CfxLua parser, formatter
(EmmyLuaCodeStyle behaviour), names (definition, references, rename), 113 lint
rules with quick fixes, hover / completion / signature help from the natives, the
Lua library and the workspace's own resources (exports, framework objects, ox_lib's
`lib`), event names, a `check` command and an MCP server for AI agents. The type
layer reads LuaCATS annotations (`---@class`, `---@type`, `---@param`, ...) across
the workspace for completion after any expression (`ESX.GetPlayerFromId(src).`,
`GetEntityCoords(ped).`), local and parameter types on hover, inlay hints
(parameter names, local types; `Lua.hint.enable`) and the type diagnostics
(`param-type-mismatch`, `assign-type-mismatch`, `cast-local-type`,
`return-type-mismatch`, `missing-fields`, `need-check-nil`, `undefined-field`,
`missing-parameter` / `redundant-parameter`, `missing-return`, `inject-field`,
`invisible`, ...), each kept narrower than LuaLS's where it would guess. Every
LuaLS diagnostic code is implemented except `codestyle-check`, `name-style-check`,
`spell-check` and `unnecessary-assert`, which LuaLS ships off (the others it
ships off stay off here too); it passes 594 of LuaLS's 608 scored type inference cases and all 537 of its diagnostic
cases: by default 529, the other 8 with `cfx.lualsCompat` on, and all 537 with
that switch on for every case (the Lua 5.5 `global` case is not scored, CfxLua
is 5.4). Hover follows LuaLS's layout (`function f(a: any)` with its return lines,
`local x: integer = 1`, a table's or a class's fields) and matches all 183 scored
of its 190 hover cases word for word (the other 7 want LuaLS's Chinese locale
text, plain Lua 5.4's `utf8` and `_G`, or Lua 5.5 syntax); completion passes
all 207 of its completion cases.
See [docs/ROADMAP.md](docs/ROADMAP.md).

## Check a server

```sh
cargo build --release
target/release/cfx-lua-ls check path/to/resources
```

Every resource is read with its manifest: which files run on the client, the
server or both, what they import (`@ox_lib/init.lua`, `@oxmysql/...`, frameworks)
and which globals each file can see. Escrow-encrypted files are skipped.

| option | |
|---|---|
| `--format text\|json\|sarif` | SARIF 2.1.0 for GitHub code scanning |
| `--level <severity>` | lowest level reported (default `warning`) |
| `--fail-on <severity>` | exit 1 at this level or above (default `error`) |
| `--fix` | apply the quick fixes in place |
| `--all-files` | also check files no manifest loads |

The rules (LuaLS's, FiveM API, sandbox, ox_lib suggestions, performance, naming)
are listed in [docs/RULES.md](docs/RULES.md). Turn them off or change their level
in `.luarc.json` with LuaLS's own keys:

```json
{
  "diagnostics.disable": ["fivem/citizen-prefix"],
  "diagnostics.severity": { "naming/local": "hint" },
  "diagnostics.globals": ["Config"]
}
```

or per line with `---@diagnostic disable-next-line: <code>`.

LuaLS's type switches are read too: `type.weakNilCheck`, `type.weakUnionCheck`,
`type.castNumberToInteger`, `type.checkTableShape`,
`diagnostics.unusedLocalExclude`, `doc.privateName` / `doc.protectedName` /
`doc.regengine`. By default this server is narrower than LuaLS in a few places
where it would guess on FiveM code; `"cfx.lualsCompat": true` gives LuaLS's own
behaviour there: nil is checked against declared types (unless
`type.weakNilCheck` is set), a local keeps the type of its first value
(`cast-local-type`), library globals of another Lua version (`unpack`) are
`deprecated`, an empty function body still needs its `---@return` values, and
every unknown code in `---@diagnostic` is reported.

### In GitHub Actions

The repository is also an action: it downloads a released `cfx-lua-ls`, checks
the paths given, sends the results to code scanning and fails the step at the
chosen level.

```yaml
permissions:
  contents: read
  security-events: write
steps:
  - uses: actions/checkout@v4
  - uses: TGIANN/cfx-lua-ls@v1
    with:
      path: resources      # folders or files, separated by spaces (default .)
      fail-on: error       # error, warning, information, hint or none
      sarif: true          # upload to code scanning
```

## Exports, frameworks and ox_lib

Nothing from a framework is built in: the server reads the copies in your
workspace. `exports.res:` lists `res`'s exports (also through `provide`, so
`exports['qb-core']:` on a Qbox server lists what qbx_core registers for it);
a value set from an export, such as `local QBCore = exports['qb-core']:GetCoreObject()`,
completes its fields (`QBCore.Functions.`) with the parameters, doc comments and
`---@deprecated` advice of their definitions, and go-to-definition opens them.
With `@ox_lib/init.lua` loaded, `lib.` lists ox_lib's modules for the file's side.
Inside `TriggerServerEvent('` and the other event calls, completion offers the
workspace's event names, and go-to-definition jumps from a trigger to its handlers.

The same index drives rules: unknown exports (with a "did you mean"), exports
called from the wrong side, `exports.res.Name(x)` with a dot, uses of deprecated
framework functions, fxmanifest.lua entries that point at missing files.

## Use it in VS Code

`cfx-lua-ls lsp` speaks the Language Server Protocol over stdio. The VS Code
extension is in [editors/vscode](editors/vscode) (not on the Marketplace yet).

## Use it from an AI agent (MCP)

`cfx-lua-ls mcp` is a [Model Context Protocol](https://modelcontextprotocol.io)
server over stdio. For Claude Code:

```sh
claude mcp add cfx-lua-ls -- /path/to/cfx-lua-ls mcp
```

| tool | |
|---|---|
| `check` | diagnostics for a file or folder, or for unsaved text; optional fixed text |
| `lint_snippet` | lint code before it is written to a file (side, ox_lib, known globals) |
| `native` | a native by name or a search: side, game, parameters, docs |
| `rules` | what each rule flags and why |
| `resource_info` | a file's resource, side, manifest imports and globals |
| `exports` | a resource's exports, from the server's own copy: parameters, side, deprecation, docs, file:line |
| `members` | what an expression holds at the end of a file (`QBCore.Functions`, `lib.callback`) |
| `events` | where an event is registered and triggered, or a search over event names |

## License

MIT. Behaviour, tests and library definitions come from lua-language-server,
EmmyLuaCodeStyle and fivem-lls-addon, all MIT; see [NOTICE](NOTICE).
