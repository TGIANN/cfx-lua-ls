# Lint rules

The diagnostics `cfx-lua-ls check`, the language server and the MCP server
report, beyond syntax errors. The list and default levels are the `RULES` table
in `crates/ide/src/lint/mod.rs`; the one-line descriptions (`lint/docs.rs`) are
what SARIF output and the MCP `rules` tool show. Rules that improve code
structure are **on by default** (owner decision). Every rule can be turned off or
re-levelled in `.luarc.json` / `Lua.diagnostics` (`disable`, `severity`,
`globals`), and per line with `---@diagnostic disable-next-line: <code>`.

Sources for the rules beyond LuaLS: Manason/effective-fivem-lua, cfx forum "Best
practice: improve your resource performance" (t/105509), ESX docs "Coding
practices", electron-services "optimization", pityus.net "FiveM Lua performance
optimization", docs.fivem.net "Server security" (developers/server-security) and
"Sandbox" (developers/sandbox), FiveM's Lua `scheduler.lua`, the ox_lib API
(v3.39, read for API facts only), Roberto Ierusalimschy "Lua Performance Tips"
(Lua Programming Gems; Lua 5.1 numbers, the ratios matter).

Level: E error, W warning, I information, H hint. Fix: the quick fix that
`check --fix`, the editor and the MCP `check` tool apply.

## How resources are read

- Each resource and side is its own Lua state. A global is visible to the files
  of the same resource on the same side (shared files on both), and through
  manifest imports (`@ox_lib/init.lua`, `@oxmysql/lib/MySQL.lua`, `@es_extended/...`,
  `@qb-core/...`, `@qbx_core/...`, any `@res/file.lua` in the workspace).
- `undefined-global` is silent for a resource with escrow (`FXAP`) files, a
  dynamic manifest, or an import outside the workspace or encrypted: the names
  those define cannot be known.
- Files no manifest loads are skipped by `check` (`--all-files` checks them),
  unless the resource loads modules itself (`require`, `load`, `lib.load`) or
  another manifest imports them.
- Escrow files and files with syntax errors get no lint and are never fixed.

## LuaLS rules

| id | flags | level | fix |
|---|---|---|---|
| `undefined-global` | a global no visible file defines, not in the library or `diagnostics.globals` | W | |
| `lowercase-global` | a global set with a lower-case name no other file of the resource reads (usually a missing `local`) | I | |
| `unused-local` | a local, local function or parameter never read; `diagnostics.unusedLocalExclude` globs are skipped | H | |
| `unused-vararg` | `...` never used | H | |
| `unused-label` | a label no `goto` jumps to | H | |
| `redefined-local` | a local hiding an outer local of the same name | H | |
| `trailing-space` | spaces at the end of a line | H | |
| `empty-block` | an `if` / loop with an empty body | H | |
| `code-after-break` | statements after `break` | H | |
| `unbalanced-assignments` | fewer values than variables, last value single-valued | W | |
| `redundant-value` | more values than variables | W | |
| `duplicate-index` | the same key twice in a table constructor | W | |
| `newline-call` | a line starting with `(` continues the previous line as a call | W | |
| `count-down-loop` | `for i = 10, 1` without a negative step | W | |
| `deprecated` | a field, global or export whose definition in the workspace is marked `---@deprecated`; the message carries the definition's advice (Qbox's qb bridge marks its old API this way); with `cfx.lualsCompat` also a library global of another Lua version (`unpack`, kept by CfxLua) | W | |
| `unreachable-code` | statements after an `if` whose every branch (with an `else`) returns or calls `error` / `os.exit`, a `do` / `repeat` block that does, or a `while true` loop without `break`; unlike LuaLS a `return` inside a `for` / `while` loop does not count | H | |
| `redundant-return` | a `return` without values as the last statement of a function | H | |
| `unused-function` | a function stored in a local that nothing calls, or that only other unused functions call | H | |
| `ambiguity-1` | `a + b or 0`, `a or 0 + 1`: `or` binds looser than arithmetic and `..`; unlike LuaLS `a or "text" .. b` is left alone | W | |
| `duplicate-doc-field` | a `---@field` key given twice for one class in the file; `fun` fields are overloads and may repeat | W | |
| `duplicate-doc-param` | a `---@param` name given twice in one doc block | W | |
| `undefined-doc-param` | a `---@param` naming no parameter of the function the block sits on (`self` counts for `function t:m()`), or a block on a statement without a function | W | |
| `doc-field-no-class` | a `---@field` in a doc block without a `---@class` above it | W | |
| `unknown-cast-variable` | `---@cast x` where no local `x` is visible and no global `x` is known | W | |
| `unknown-diag-code` | a code in `---@diagnostic` that neither LuaLS nor this server has and that is one or two letters from one of theirs (a typo); unlike LuaLS other codes are left alone, servers run other linters too (Qbox's `unused-argument`); `cfx.lualsCompat` reports every unknown code | W | |
| `unknown-operator` | a `---@operator` name LuaLS does not know | W | |
| `duplicate-doc-alias` | a `---@alias` declared twice in the file, or named like a `---@class` of the file | W | |
| `undefined-doc-class` | a `---@class` parent that no class of the workspace or the library declares; silent where `undefined-doc-name` is | W | |
| `circle-doc-class` | a `---@class` that, through its parents, inherits from itself | W | |
| `newfield-call` | a call in a table constructor whose arguments start on a later line (`{ print`, then `'a' }` on the next line, is `print('a')`) | W | |
| `duplicate-set-field` | one function field of one object defined twice in the same branch of the file; other `if` branches and other files are left alone | W | |
| `close-non-object` | `local x <close> = v` where `v` is a number, string or boolean, or there is no value | W | |
| `missing-return` | a function with a required `---@return` (not optional, not nil-able, not `any`) whose end can be reached without a `return`; an empty body is a declaration, in every file (LuaLS: `---@meta` files only; `cfx.lualsCompat` checks it) | W | |
| `missing-return-value` | a `return` with fewer values than the `---@return` lines require; a call or `...` last counts as enough | W | |
| `redundant-return-value` | a `return` with more values than the `---@return` lines give | W | |
| `redundant-parameter` | more arguments than any definition of the function takes (a `:` call counts `self`); functions of the caller's own resource and the Lua library only, silent on `...`, `---@overload`, `---@type`, values that are not plain functions and locals assigned again | W | |
| `missing-parameter` | fewer arguments than every definition requires (documented, not optional, nil-able or `any`, and not tested for nil in the body: LuaLS goes by the doc alone); a call or `...` last counts as enough; own resource only | W | |
| `discard-returns` | a call statement whose function is `---@nodiscard` (a local function, a workspace function or the library's, such as `string.format(...)` alone) | W | |
| `param-type-mismatch` | an argument whose type surely does not fit the `---@param` type of a function of the same file; aliases and `---@enum`s of literal values are checked by their members (a table constructor is none of an enum's table values), a literal argument by its value, a `---@generic T: B` parameter by its bound, `C:f()` on `function C.f(a)` passes `C` as `a` (reported at the `:`), `C(x)` on a `---@class C` against its `---@overload`s, table shapes field by field; the classes of the resource's other files and of the resources its manifest names count as declared (the file's own declaration of a name wins); a documented function of another file of the resource or a resource it names, called through a global (`F(x)`, `A.b(x)`, `A:m(x)`, or a field set to it, `U.send = F`), a local the file declares once and never rewrites (`local send = F`) or an export (`exports.res:f(x)`, also through a name holding `exports['res']`; the `:` passes the proxy, not a parameter, and `exports.res.f(x)` is left alone), is checked by the `---@overload` (or the function) the arguments pick; a function defined more than once (client and server copies) only when every definition has the same signature; silent on `fun` shapes | W | |
| `assign-type-mismatch` | a value given to a `---@type` local where it is declared, a value written to a field of a `---@class` of the same file that surely does not fit its `---@field` type (or the `---@type` written above another assignment of that field), or a table constructor entry that does not fit the type the constructor is declared with (`---@type T[]`, a field's type, `{ x: T, [1]: U }`, a parameter's type); the value must be a literal, constructor, call or fixed `---@type` local, nil is not checked | W | |
| `cast-local-type` | a local assigned a value that surely does not fit its `---@type` (a parameter: its `---@param`); a `---@type` above the assignment gives the value's type, a literal keeps its value against literal types (`---@type 1\|2`), a fractional number does not fit `integer`, `table<K, V>` compares its arguments; a local typed by its first value is not checked (LuaLS checks it; so does `cfx.lualsCompat`), names and `a or b` values are left alone (`a and b` is checked: `n and tostring(n)` is `string?`) (`type.checkTableShape` checks a local table constructor field by field), nil is not checked (unless `type.weakNilCheck` is `false` or `cfx.lualsCompat` is on), `number` and `integer` fit each other otherwise (`type.castNumberToInteger` lets a `number` fit `integer` always; `type.weakUnionCheck` lets a union fit when one member does) | W | |
| `return-type-mismatch` | a returned literal, table or function that surely does not fit the `---@return` type at that position, or a call returned last whose own `---@return` lines (a function of this file) do not fit the positions from there on; names, calls and `---@overload` functions are left alone, nil is not checked | W | |
| `missing-fields` | a table constructor where a `---@class` (or a union of them: the table must fit one) is expected, a `---@type` on a `return` included, that leaves out required `---@field`s (parents included, a class's own `---@field x? T` hiding a parent's `x`, `---@field [1] T` too); the class may be declared in the file, another file of the resource or a resource its manifest names (declarations of one name in one resource are merged, `(partial)` honoured); silent when a parent is declared elsewhere or a key is computed | W | |
| `need-check-nil` | a local used as an object, callee or written key that may still be nil there: its `---@type T?` / `---@param x? T` says so and no check before it rules nil out; upvalues in other functions are not checked; a flag guarding the local (`ok = x and f(x)`, `if ok then x.y`) still warns | W | |
| `undefined-field` | a field read on a value of a `---@class` that the class (parents included) does not declare and no code writes; only classes declared in this file alone, without `[K]` index fields or a basic parent; also `X.C` on an `---@enum X` of named fields, and a field of neither class of a `---@generic T: A|B`; a name written as a field in the resource, or in the file on a value not known to be of another class, keeps it silent | W | |
| `inject-field` | a field written on a value of a `---@class` that the class does not declare and no code defines on the class's own variable (its constructor, `self` in its methods); a field beyond a `---@type { a: T }` shape; with `(exact)`, writes on the class's own variable too; only classes declared in this file alone | W | |
| `invisible` | a `---@field private` field used outside its class, a `protected` one outside the class and its subclasses; only classes of this file; `Lua.doc.privateName` / `protectedName` globs (`m_*`) make a field without its own visibility private / protected: a `---@field` or a field of the class's own table (constructor, `C.f = v`, `self.f = v` in its methods); case-sensitive, as LuaLS; with `Lua.doc.regengine` `lua` they are Lua patterns found anywhere in the name (`string.find`, `^_[%w_]*_$`), a malformed one matches nothing; a `---@field package` field, a field written on the class under `---@package`, or one named by a `Lua.doc.packageName` glob, used from a file other than the one declaring it (the class declared only in other files, parents followed) | W | |
| `await-in-sync` | off by default (as in LuaLS): a `---@async` function called from a function not marked `---@async` | W | |
| `not-yieldable` | off by default (as in LuaLS): an async function passed to a parameter not typed `async fun()` / `any` | W | |
| `cast-type-mismatch` | `---@cast x T` on a local whose doc type `T` does not fit (a subclass or a union member does) | W | |
| `global-in-nil-env` | a global used where `_ENV` is nil (`local _ENV`, `_ENV = nil` earlier) | W | |
| `undefined-env-child` | a global used where `_ENV` holds a `---@type C` value without that field | I | |
| `incomplete-signature-doc` | off by default (as in LuaLS): `---@param` / `---@return` lines leaving out a parameter or returned value | W | |
| `missing-global-doc` | off by default (as in LuaLS): a global function without docs, or with undescribed parameters / returns | W | |
| `missing-local-export-doc` | off by default (as in LuaLS): the same for local functions the file's returned table exposes | W | |
| `global-element` | off by default (as in LuaLS): any global written outside `diagnostics.globals` | W | |
| `different-requires` | one file required under two names in a resource (`a.b` / `a/b`) | W | |
| `no-unknown` | off by default (as in LuaLS): a local whose type the inference cannot tell | W | |
| `name-style-check` | off by default (as in LuaLS): a name that breaks its `Lua.nameStyle.config` rule (EmmyLuaCodeStyle's `NameStyleChecker`, its quirks too: only the first name of a top-level `local a, b` and never a numeric `for` index); `pattern` rules use Rust's `regex` syntax, which has no look-around or back-references (such a pattern counts as `off`, as an invalid one does in LuaLS); same findings as LuaLS 3.19.2 on 158 server files, with and without a config | W | |
| `spell-check` | off by default (as in LuaLS): a word of a name or short string missing from LuaLS's dictionaries and `Lua.spell.dict`; quick fixes: add to `Lua.spell.dict` (offered to clients that send `initializationOptions.spellAddWord`, as the VS Code extension does; other editors get the replacements only) and up to 14 closest words; same findings as LuaLS 3.19.2 on 158 server files | I | |
| `codestyle-check` | off by default (as in LuaLS): where the text differs from the formatter's output (EmmyLuaCodeStyle's `CodeStyleChecker`), with the file's `.editorconfig` or `Lua.format.defaultConfig`; same messages and ranges as LuaLS 3.19.2 on 158 server files with the default style and on 42 of them under four other `.editorconfig`s (tabs, quote styles, semicolon modes, call parentheses, no final newline), except "must end with new line", which LuaLS ends one column past the end of the file | W | |
| `unnecessary-assert` | off by default: `assert(x)` with an always-truthy `x`; LuaLS 3.19.2 has this check but leaves it unregistered (its positive test is disabled over `assert(arg[1])`), so only a configured severity turns it on here. Narrower than LuaLS's: only a literal, a table or function expression, or a library or native function that exists where the file runs (`local lower = assert(string.lower)`; not `unpack` and other Lua versions' globals, not a native of the other side, not one the file assigns); LuaLS's type-based version also flags locals, fields, lookups and calls, and on the test servers those were the asserts guarding `CLASSES[invType]`, `instances[i]`, `---@param` values checked at run time and a local assigned in a loop | W | |
| `undefined-doc-name` | a type in a doc tag that no `---@class` / `---@alias` / `---@enum` of the workspace or the library declares (`int`, `float`, `object` get Lua's name in the message); silent when the resource, or one it depends on or imports `@res/` files from, has escrow files | W | |

## FiveM API

| id | flags | level | fix |
|---|---|---|---|
| `fivem/wrong-side-native` | a client-only native in a server file or the reverse (manifest side + native apiset) | E | |
| `fivem/backtick-hash` | a CfxLua hash string `` `name` `` (owner decision: hashes are written with `joaat`) | W | `joaat("name")` |
| `fivem/gethashkey` | `GetHashKey(x)` | I | `joaat(x)` |
| `fivem/citizen-prefix` | `Citizen.Wait`, `Citizen.CreateThread`, `Citizen.SetTimeout` | H | drop the prefix |
| `fivem/native-distance` | `GetDistanceBetweenCoords`, `Vdist`, `Vdist2` | W | |
| `fivem/getplayerped-self` | `GetPlayerPed(-1)` | W | `PlayerPedId()` (or `cache.ped` with ox_lib) |
| `fivem/loop-without-wait` | a `while` loop in a thread whose body (nested functions excluded) can run without reaching `Wait` / `coroutine.yield` / `break` / `return` | W | |
| `fivem/invariant-native-in-loop` | `PlayerPedId()`, `PlayerId()`, `GetPlayerServerId(PlayerId())` inside a loop body | I | |
| `fivem/unsigned-hash` | an integer above 2^31 - 1 compared with a hash or used as a table key (FiveM hashes are signed 32-bit); not when compared with a field the server's game events send unsigned (`weaponDamageEvent` `weaponType`, `ptFxEvent` `effectHash` / `assetHash`, `startNetworkSyncedSceneEvent` `cameraAnimHash` / `animDictHash`) | W | signed value |
| `fivem/source-after-wait` | `source` read after a `Wait` / yield in an event handler (the scheduler restores the previous `source` at the first yield) | W | |
| `fivem/unchecked-net-arg` | a server net event handler passing a parameter it never read before into a money, item, weapon, job or permission call (`addMoney`, `addAccountMoney`, `addInventoryItem`, `AddItem`, `setJob`, ...); for `setJob` only the job and grade, not the duty flag | W | |

## Exports, NUI and frameworks

The export rules read the exporting resource from the workspace: its
`exports('name', fn)` calls, manifest `export` / `server_export` entries,
`exports(k, v)` loops over a table, wrappers that register
`__cfx_export_<res>_%s`, `provide` and the literal `exports('name', ...)` calls
of its JavaScript scripts. They stay silent when the resource is not in the
workspace or its exports cannot all be known (escrow files, a dynamic manifest,
a computed export name, a .NET script). The framework rules were checked against
the qb-core and qbx_core sources; nothing from them is built in.

| id | flags | level | fix |
|---|---|---|---|
| `fivem/unknown-export` | `exports.res:Name()` where `res` has no export `Name` (fails with "No such export"), with a "did you mean" | W | |
| `fivem/export-wrong-side` | an export `res` registers only on the other side (each side's Lua state has its own exports) | E | |
| `fivem/export-dot-call` | `exports.res.Name(a)`: exports take `self` first (`function(self, ...)` in scheduler.lua), so `a` is lost | W | `:` |
| `fivem/nui-message-encoded` | `SendNUIMessage(json.encode(t))`: `SendNUIMessage` encodes the table itself, the page gets a string | I | |
| `qb/unchecked-player` | a player from `Functions.GetPlayer` / `GetPlayerByCitizenId` (QBCore object or `exports.qbx_core`) indexed before a nil check: both return `nil` for a player who is not loaded or has left (often safe where the player is known loaded, such as a usable-item callback) | I | |
| `esx/shared-object-event` | `TriggerEvent('esx:getSharedObject', cb)` where the workspace's `es_extended` exports `getSharedObject` (ESX Legacy 1.12.4 and 1.13.4 still answer the event in `shared/main.lua`; 1.12.4 prints a warning on each use) | I | |
| `qb/core-object-event` | `TriggerEvent('QBCore:GetObject', cb)` where no file handles the event and the resource answering `qb-core` is fully known (qbx_core has no handler; qb-core has one, sometimes with extra checks the export skips, so no automatic fix) | W | |
| `fivem/net-event-same-side` | `RegisterNetEvent('x', ...)` in a server file when the workspace fires `x` only with `TriggerEvent` and no client sends it (`TriggerServerEvent`): a net event accepts any client's call, so code meant for the server alone becomes callable by players. Silent when the name appears in another string (a wrapper argument, a table of names) or an escrow file may send it | I | |

## fxmanifest.lua

Each manifest's literal entries checked against the resource folder on disk
(URLs and paths with `\` skipped).

| id | flags | level | fix |
|---|---|---|---|
| `fivem/manifest-missing-file` | a script or `ui_page` that does not exist | E | |
| `fivem/manifest-path-case` | the file exists with another letter case (works on Windows, not on a Linux server) | W | |
| `fivem/manifest-no-match` | a glob, or a `files` entry, matching no file | W | |
| `fivem/manifest-missing-import` | `@res/file` where `res` is in the workspace and has no such file | E | |

## FiveM sandbox

From docs.fivem.net "Sandbox": what the server's Lua sandbox blocks or limits.

| id | flags | level | fix |
|---|---|---|---|
| `fivem/sandbox-blocked` | `os.execute`, `io.tmpfile`, `io.popen` with a command other than `ls` / `dir` | E | |
| `fivem/sandbox-path` | a constant file path with a `..` segment | E | |
| `fivem/sandbox-write` | writing into another resource's folder (needs `add_filesystem_permission` in server.cfg) | W | |
| `fivem/readdir-pairs` | `pairs(io.readdir(...))`: it returns a handle, iterate with `:lines()` | E | |
| `fivem/sandbox-setlocale` | `os.setlocale` (cannot change the locale in the sandbox) | W | |

## ox_lib

Only where the manifest loads `@ox_lib/init.lua` for the file's side.

| id | flags | level | fix |
|---|---|---|---|
| `ox/cache-ped` | `PlayerPedId()` | I | `cache.ped` |
| `ox/cache-playerid` | `PlayerId()` | I | `cache.playerId` |
| `ox/cache-serverid` | `GetPlayerServerId(PlayerId())` | I | `cache.serverId` |
| `ox/cache-vehicle` | `GetVehiclePedIsIn(<own ped>, false)` (`cache.vehicle` is `false` on foot, not `0`) | I | |
| `ox/cache-weapon` | `GetSelectedPedWeapon(<own ped>)` (`cache.weapon` is `false` when unarmed) | H | |
| `ox/cache-resource` | `GetCurrentResourceName()` | I | `cache.resource` |
| `ox/request-asset` | a request-and-wait loop for a model / dict / asset | I | |
| `ox/callback` | ESX / QBCore callbacks | H | |
| `ox/notify` | native or framework notifications | H | |
| `ox/text-ui` | per-frame help text | H | |
| `ox/progress` | framework progress bars | H | |
| `ox/closest` | entity pool scans, framework closest-entity helpers | H | |
| `ox/keybind` | key polling in a loop, `RegisterKeyMapping` | H | |
| `ox/add-command` | manual `chat:addSuggestion` | H | |
| `ox/raycast` | hand-rolled shape tests | H | |

## Lua performance

| id | flags | level | fix |
|---|---|---|---|
| `perf/concat-in-loop` | `s = s .. x` on the same variable inside a loop (quadratic); not a local declared in that loop, which starts over on every iteration | W | |
| `perf/table-insert-append` | `table.insert(t, v)` with two arguments | H | `t[#t + 1] = v` |
| `perf/sequential-index-keys` | `{ [1] = a, [2] = b, ... }` (goes to the hash part) | I | `{ a, b, ... }` |
| `perf/constant-table-in-loop` | a constant table passed to a library function inside a loop body | H | |
| `perf/closure-in-loop` | a function passed to a Lua library function (`table.sort`, `string.gsub`, `pcall`) in a loop that never waits, using no local of the loop | H | |
| `perf/global-in-loop` | off by default: `math.floor(x)` (math, string, table, utf8, bit32) called in a `for` loop that never waits, once per name and loop | H | |
| `perf/runtime-load` | `load` / `loadstring` with a non-constant chunk | I | |

## Readability and naming

| id | flags | level | fix |
|---|---|---|---|
| `style/redundant-boolean-return` | `if c then return true else return false end` with a boolean `c` | H | `return c` |
| `style/string-key-access` | `t["name"]` with an identifier-like key (not on `exports`) | H | `t.name` |
| `naming/local` | a local variable or local function not in camelCase | I | rename |
| `naming/const` | a `<const>` local holding a literal not in UPPER_SNAKE | I | |

## Syntax messages

- `..=` gets its own message ("`..=` is not supported in CfxLua; write `s = s .. x`")
  instead of the generic error; the ESX docs list it as a Lua 5.4 feature, but FiveM
  does not build the patch for it.

## Not adopted

Advice from the sources that is wrong or conflicts with our conventions:

- `fivem/client-net-no-source-check` (a client net handler that never tests
  `source`): the same page says client checks "can be easily be overridden"
  and cheats trigger events in any context; the boundary is the server, which
  `fivem/unchecked-net-arg` covers. A hint on nearly every client handler
  would bury the rest.
- `fivem/source-65535` (flagging a client handler that compares `source` with
  `65535`): docs.fivem.net "Server security" recommends exactly that check
  (`if source ~= 65535 then return end`; the server sends `65535`).

- `table.remove(t, i)` replaced by `t[i] = nil` (ESX): leaves a hole in the array.
- "`#t` is O(n)" (effective-fivem-lua): it is O(log n); a running count is still
  faster in an append loop.
- `if GetInvokingResource() then return end` as net event protection: not verified; the FiveM "Server security" page does not mention it.
- Event naming `resource:side:event` and "past tense" event names: style, varies by
  project; no diagnostic.
- Function-size, parameter-count and "no boolean parameters" advice: judgement
  calls, no diagnostic.
