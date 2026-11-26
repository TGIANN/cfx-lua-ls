//! One line per rule: what it flags and why. Shown by `check --format sarif`, the
//! MCP `rules` tool and docs/RULES.md.

pub const RULE_DOCS: &[(&str, &str)] = &[
    ("unused-local", "A local variable, function or parameter that is never read (LuaLS)."),
    ("unused-vararg", "A function takes `...` and never uses it (LuaLS)."),
    ("unused-label", "A `::label::` no `goto` jumps to (LuaLS)."),
    ("redefined-local", "A local that hides another local of the same name from an outer scope (LuaLS)."),
    ("trailing-space", "Spaces or tabs at the end of a line (LuaLS)."),
    ("empty-block", "An `if`, loop or `for` with an empty body (LuaLS)."),
    ("code-after-break", "Statements after `break` in the same block never run (LuaLS)."),
    (
        "undefined-global",
        "A global no file of the same resource and side defines, that is not in the library, `diagnostics.globals` or a manifest import. Silent when the resource has escrow files, a dynamic manifest or an import outside the workspace.",
    ),
    (
        "lowercase-global",
        "A global set with a lower-case name that no other file of the resource reads: usually a missing `local` (LuaLS; silent when escrow files could read it).",
    ),
    ("unbalanced-assignments", "Fewer values than variables, and the last value cannot return several (LuaLS)."),
    ("redundant-value", "More values than variables: the extra values are dropped (LuaLS)."),
    ("duplicate-index", "The same key twice in one table constructor: the first value is lost (LuaLS)."),
    ("newline-call", "A line starting with `(` continues the previous line as a call (LuaLS)."),
    ("count-down-loop", "A numeric `for` from a larger to a smaller bound without a negative step never runs (LuaLS)."),
    (
        "deprecated",
        "A field, global or export whose definition in the workspace is marked `---@deprecated` (LuaLS); the message carries the definition's advice. Framework bridges mark their old API this way.",
    ),
    (
        "unreachable-code",
        "Statements after an `if` whose every branch (with an `else`) returns or calls `error`, a `do` / `repeat` block that does, or a `while true` loop without `break` (LuaLS; a `return` inside a `for` or `while` loop does not count, the loop may not run).",
    ),
    ("redundant-return", "A `return` without values as the last statement of a function (LuaLS)."),
    (
        "unused-function",
        "A function stored in a local that nothing calls, or that only other unused functions call (LuaLS).",
    ),
    (
        "ambiguity-1",
        "`a + b or 0` and `a or 0 + 1`: `or` binds looser than arithmetic and `..`, so the literal is likely in the wrong place (LuaLS; `a or 'text' .. b` is left alone, the fallback message it builds is the usual intent).",
    ),
    ("duplicate-doc-field", "A `---@field` key given twice for one class in the file (`fun` fields are overloads and may repeat) (LuaLS)."),
    ("duplicate-doc-param", "A `---@param` name given twice in one doc block (LuaLS)."),
    (
        "undefined-doc-param",
        "A `---@param` naming no parameter of the function the doc block sits on, or a block on a statement without a function (LuaLS).",
    ),
    ("doc-field-no-class", "A `---@field` in a doc block without a `---@class` above it (LuaLS)."),
    ("unknown-cast-variable", "`---@cast x` where no local `x` is visible and no global `x` is known (LuaLS)."),
    ("unknown-diag-code", "A code in `---@diagnostic` that neither LuaLS nor this server has and that is one or two letters from one of theirs: a typo (LuaLS reports every unknown code; servers run other linters too)."),
    ("unknown-operator", "A `---@operator` name LuaLS does not know (`add`, `sub`, `concat`, `call`, ...) (LuaLS)."),
    ("duplicate-doc-alias", "A `---@alias` declared twice in the file, or named like a `---@class` of the file (LuaLS)."),
    (
        "undefined-doc-class",
        "A `---@class` parent that no class of the workspace or the library declares; silent where `undefined-doc-name` is (LuaLS).",
    ),
    ("circle-doc-class", "A `---@class` that, through its parents, inherits from itself (LuaLS)."),
    ("newfield-call", "A call in a table constructor whose arguments start on a later line: `{ print\\n'a' }` is one field, `print('a')` (LuaLS)."),
    (
        "duplicate-set-field",
        "One function field of one object defined twice in the same branch of the file (`function M.f()` twice); other `if` branches and other files are left alone (LuaLS).",
    ),
    ("close-non-object", "`local x <close> = v` where `v` is a number, string or boolean, or there is no value (LuaLS)."),
    (
        "missing-return",
        "A function with a required `---@return` (not optional, not nil-able, not `any`) whose end can be reached without a `return`; an empty body is a declaration and is left alone in every file, not only `---@meta` ones (LuaLS).",
    ),
    (
        "missing-return-value",
        "A `return` with fewer values than the function's `---@return` lines require; a call or `...` last counts as enough (LuaLS).",
    ),
    ("redundant-return-value", "A `return` with more values than the function's `---@return` lines give (LuaLS)."),
    (
        "redundant-parameter",
        "More arguments than any definition of the function takes (a `:` call counts `self`). Functions of the caller's own resource and the Lua library only (the library's definition of a shadowed global counts too); silent when a definition takes `...`, has `---@overload` or `---@type`, is not a plain function, or is a local assigned again (LuaLS).",
    ),
    (
        "missing-parameter",
        "Fewer arguments than every definition of the function requires: documented parameters that are not optional, nil-able or `any`, and that the body does not test for nil (`if not x`, `x or d`, `x == nil`, `type(x)`; LuaLS goes by the doc alone). A `:` method counts `self`; a call or `...` last counts as enough; only functions of the caller's own resource (LuaLS).",
    ),
    ("discard-returns", "A call statement whose function is `---@nodiscard` (a local function, a workspace function or the library's: `string.format(...)` alone) (LuaLS)."),
    (
        "param-type-mismatch",
        "An argument whose type surely does not fit the `---@param` type of a function of the same file. Silent when either side is a type declared elsewhere, a generic, an alias or a `fun` shape, and for the rest of a file whose flow budget ran out (LuaLS).",
    ),
    (
        "assign-type-mismatch",
        "A value written to a field of a `---@class` of the same file that surely does not fit the field's `---@field` type. Deviations from LuaLS: the value must be a literal, constructor, call or a fixed `---@type` local (names and `a or b` may be narrowed where the inference does not see), and nil is not checked.",
    ),
    (
        "cast-local-type",
        "A local assigned a value that surely does not fit its `---@type` (or a parameter its `---@param`). Deviations from LuaLS: a local typed by its first value is not checked (`local found = false`, then `found = item`), the value must be a literal, constructor, call or a fixed `---@type` local, and nil is not checked.",
    ),
    (
        "return-type-mismatch",
        "A returned literal, table or function that surely does not fit the function's `---@return` type at that position; other values (names, calls) and functions with `---@overload` are left alone, and nil is not checked (LuaLS).",
    ),
    (
        "missing-fields",
        "A table constructor where a `---@class` of the same file is expected (`---@type`, a `---@param`, a field) that leaves out required `---@field`s: not optional, not nil-able; parents included. Silent when a parent is declared in another file or a key is computed (LuaLS).",
    ),
    (
        "need-check-nil",
        "A local used as an object (`x.f`, `x:m()`, `x()`) or written key (`t[x] = v`) that may still be nil there: its doc comment says so (`---@type T?`, `---@param x? T`) and no check before it rules nil out (`if x`, `x and`, `if not x then return end`, `type(x) == 'table'`, `assert(x)`). `x?.f` is checked by the operator. Deviation from LuaLS: upvalues read inside another function are not checked, and locals typed only by their values never are. Known gaps that still warn: a flag guarding the local (`local ok = x and f(x)`, then `if ok then x.y end`) and a loop bounded by `#(x or {})`.",
    ),
    (
        "inject-field",
        "A field written (`v.f = x`, `function v.f()`) on a value of a `---@class` that the class does not declare and no code defines on the class's own variable (`---@class C` above its `local` or global, `self` in its methods, its table constructor); on a `---@type { a: T }` local, a field its shape does not have. With `---@class (exact) C`, writes on the class's own variable and constructor fields beyond the `---@field`s too. The object's type is taken where it is written: after `x = x or {}` it is `C|table`, which takes any field. Narrower than LuaLS: only classes declared in this file alone (parents too), without `[K]` index fields; a name written as a field in another file of the resource keeps it silent.",
    ),
    (
        "invisible",
        "A `---@field private` field read or written outside its class (the class's own variable, `self` in its methods, a function stored on it), or a `---@field protected` one outside the class and its subclasses. Only classes of this file; `Lua.doc.privateName` / `protectedName` patterns are not read.",
    ),
    (
        "await-in-sync",
        "Off by default, as in LuaLS. A call of a `---@async` function (or one taking an async function it calls) from a function not marked `---@async`; the main chunk counts as async. Async functions: those with `---@async` above them or on their header's line (`function () ---@async`), locals and parameters typed `async fun()`, and the library's `coroutine.yield`. FiveM's `Wait` is not marked async in the library, so plain `Wait` calls are not reported.",
    ),
    (
        "not-yieldable",
        "Off by default, as in LuaLS. An async function passed to a parameter that is not typed `async fun()` or `any` (and is not `...`), when the called function does not call it itself.",
    ),
    (
        "cast-type-mismatch",
        "`---@cast x T` (without `+` / `-`) on a local or parameter whose doc comment declares a type `T` does not fit, as LuaLS's `canCastType` (a subclass or one member of a union fits: `---@type string|boolean`, then `---@cast x number` does not). Locals typed only by their values are not checked.",
    ),
    (
        "global-in-nil-env",
        "A global read or written where `_ENV` is nil: after `local _ENV`, `local _ENV = nil` or (later in the file) `_ENV = nil`.",
    ),
    (
        "undefined-env-child",
        "A global read or written where `_ENV` holds a value typed `---@type C` that has no such field (`---@type iolib` then `_ENV = io`: `stderr` is there, `print` is not). An `_ENV` without a type annotation silences undefined-global instead.",
    ),
    (
        "incomplete-signature-doc",
        "Off by default, as in LuaLS. A function with some `---@param` / `---@return` lines that leaves out a parameter (not `self` or `_`) or a returned value.",
    ),
    (
        "missing-global-doc",
        "Off by default, as in LuaLS. A global function (`function F()`, `F = function`) without a doc comment, or with parameters or returned values its doc comment does not describe.",
    ),
    (
        "missing-local-export-doc",
        "Off by default, as in LuaLS. The same for a local function stored on a local table the file returns (`M.f = f`, `return M`).",
    ),
    (
        "global-element",
        "Off by default, as in LuaLS. Any global written (`G = 1`, `function G()`) that is not in `diagnostics.globals`, the library or under a `---@class`: for code bases that keep everything local.",
    ),
    (
        "different-requires",
        "`require 'x'` naming a file that this file or another file of the resource requires under another name (`a.b` and `a/b`, `@res.a.b` from inside `res`), resolved as ox_lib's module loader does.",
    ),
    (
        "no-unknown",
        "Off by default, as in LuaLS. A local or loop variable whose type the inference cannot tell. Only locals: LuaLS also checks globals and fields.",
    ),
    (
        "name-style-check",
        "Off by default, as in LuaLS. A local, parameter, function, global, table field, module, required module, `class(...)` result or `<const>` name that breaks its rule in `Lua.nameStyle.config` (EmmyLuaCodeStyle's keys: `local_name_style`, `function_name_style`, ...; `snake_case`, `upper_snake_case`, `camel_case`, `pascal_case`, `ignore` lists and `pattern` regexes with `$1` group rules). The defaults are EmmyLuaCodeStyle's: snake_case, globals and constants also UPPER_SNAKE, classes and required modules also PascalCase.",
    ),
    (
        "spell-check",
        "Off by default, as in LuaLS. A lower-case word of more than three letters in a name (`getPlayerNmae` -> `Nmae`) or in a short string that is not in LuaLS's English and Lua dictionaries or `Lua.spell.dict`. The quick fixes add the word to `Lua.spell.dict` or replace it with one of the closest dictionary words.",
    ),
    (
        "codestyle-check",
        "Off by default, as in LuaLS. Where the file differs from what the formatter would write: indentation, spaces around tokens, quote style, statement semicolons, call parentheses and the final line break, with the style of the file's `.editorconfig` (else `Lua.format.defaultConfig`).",
    ),
    (
        "unnecessary-assert",
        "Off by default; LuaLS 3.19.2 has the check but does not register it. `assert(x)` whose first argument the code fixes as never nil or false: a literal, a table or function expression, a library or native function that exists where the file runs (`assert(string.lower)`; not `unpack`, not a native of the other side). Locals, fields, lookups and calls are left alone even when their type says truthy: a `---@param x string` checked at run time may still be nil.",
    ),
    (
        "undefined-field",
        "A field read (`v.f`, `v:m()`) on a value of a `---@class` that neither the class nor its parents declare and no code writes under that name. Narrower than LuaLS: only classes declared in this file alone (parents too), without `[K]` index fields or a basic parent such as `table`; a name written as a field anywhere in the file or in another file of the resource, or a computed write in the file, keeps it silent.",
    ),
    (
        "undefined-doc-name",
        "A type in a `---@type`, `---@param`, `---@return`, `---@field`, `---@class` parent, `---@alias` or `---@cast` that no `---@class`, `---@alias` or `---@enum` of the workspace or the library declares, that is not builtin and not a generic of the file (LuaLS).",
    ),
    (
        "fivem/unknown-export",
        "`exports.res:Name()` where `res` is in the workspace, all its exports are known, and none is called `Name`: the call fails with \"No such export\".",
    ),
    (
        "fivem/export-wrong-side",
        "An export the resource registers only on the other side: exports live in each side's Lua state, so a server script cannot call a client export or the reverse.",
    ),
    (
        "fivem/export-dot-call",
        "`exports.res.Name(a)`: exports take `self` first, so `a` is dropped; call with `:`.",
    ),
    (
        "fivem/nui-message-encoded",
        "`SendNUIMessage(json.encode(t))`: `SendNUIMessage` encodes the table itself, so the page gets a JSON string.",
    ),
    (
        "qb/unchecked-player",
        "A QBCore / Qbox player from `Functions.GetPlayer`, `GetPlayerByCitizenId` or the qbx_core exports, used before a nil check: both return `nil` for a player who is not loaded or has left.",
    ),
    ("fivem/manifest-missing-file", "A script or `ui_page` in fxmanifest.lua that does not exist in the resource."),
    (
        "fivem/manifest-path-case",
        "A manifest path whose file exists with a different letter case: found on Windows, missing on a Linux server.",
    ),
    ("fivem/manifest-no-match", "A manifest glob, or a `files` entry, that matches no file of the resource."),
    (
        "fivem/manifest-missing-import",
        "`@res/file` in fxmanifest.lua where `res` is in the workspace and has no such file.",
    ),
    (
        "esx/shared-object-event",
        "`TriggerEvent('esx:getSharedObject', cb)` where the workspace's `es_extended` exports `getSharedObject`: ESX Legacy imports ESX with the export or `@es_extended/imports.lua`; the event is kept for old scripts and 1.12 prints a warning on every use.",
    ),
    (
        "qb/core-object-event",
        "`TriggerEvent('QBCore:GetObject', cb)` where the workspace's `qb-core` (qbx_core on Qbox) is fully known and no file handles the event: the callback never runs.",
    ),
    (
        "fivem/net-event-same-side",
        "`RegisterNetEvent` in a server file for an event that the workspace only fires with `TriggerEvent` and never sends from a client: any client can still send it.",
    ),
    (
        "fivem/event-argument-count",
        "A net trigger (`TriggerServerEvent`, `TriggerClientEvent`, the latent ones) sending more arguments than the handlers of the event in the sender's resource, on the side it reaches, take.",
    ),
    (
        "fivem/event-missing-arguments",
        "A trigger sending fewer arguments than the handlers of the event require (a `---@param` whose type does not allow nil and that the body never tests for nil).",
    ),
    (
        "fivem/wrong-side-native",
        "A client-only native in a server file or a server-only native in a client file (side from fxmanifest.lua, native side from the native docs).",
    ),
    ("fivem/backtick-hash", "A CfxLua backtick hash: hashes are written with `joaat(\"name\")` (owner decision)."),
    ("fivem/gethashkey", "`GetHashKey(x)` goes through the native bridge; `joaat(x)` is a plain Lua function."),
    ("fivem/citizen-prefix", "`Citizen.Wait` / `Citizen.CreateThread` / `Citizen.SetTimeout`: the globals without the prefix are the same functions."),
    ("fivem/native-distance", "`GetDistanceBetweenCoords` / `Vdist` / `Vdist2` call a native; `#(a - b)` is a vector operation in Lua."),
    ("fivem/getplayerped-self", "`GetPlayerPed(-1)`: `PlayerPedId()` (or `cache.ped` with ox_lib) returns the local ped directly."),
    (
        "fivem/loop-without-wait",
        "A `while` loop in a thread whose body can run without reaching `Wait`, a yield, `break` or `return`: it freezes the game.",
    ),
    ("fivem/invariant-native-in-loop", "`PlayerPedId()`, `PlayerId()` and the like called on every loop pass; hoist them to a local."),
    (
        "fivem/unsigned-hash",
        "An integer above 2^31 - 1 compared with a hash or used as a table key: FiveM hashes are signed 32-bit, so it never matches.",
    ),
    (
        "fivem/unchecked-net-arg",
        "A server net event handler (`RegisterNetEvent(name, fn)`, or `AddEventHandler` for a name the file registers as a net event) passing one of its parameters, read nowhere before, straight into a money, item, weapon, job or permission call (`addMoney`, `addAccountMoney`, `addInventoryItem`, `AddItem`, `setJob`, ...). The client chooses that value: check its type and range, and that the player may do it. Any earlier read of the parameter (a check, `tonumber`, a lookup) keeps it silent.",
    ),
    (
        "fivem/source-after-wait",
        "`source` read after a `Wait` / yield in an event handler: the scheduler restores the previous `source` at the first yield. Copy it to a local on the first line.",
    ),
    ("fivem/sandbox-blocked", "`os.execute`, `io.tmpfile` or `io.popen` with a command other than `ls` / `dir`: the FiveM sandbox blocks them."),
    ("fivem/sandbox-path", "A file path with `..`: the FiveM sandbox rejects it."),
    (
        "fivem/sandbox-write",
        "Writing into another resource's folder: needs `add_filesystem_permission <own> write <target>` in server.cfg.",
    ),
    ("fivem/readdir-pairs", "`pairs(io.readdir(...))`: `io.readdir` returns a handle; iterate it with `:lines()`."),
    ("fivem/sandbox-setlocale", "`os.setlocale` cannot change the locale in the FiveM sandbox."),
    (
        "security/sql-concatenation",
        "A value joined with `..` into the query of a MySQL call (`'... WHERE id = ' .. id`): pass it as a `?` parameter, or a quote in it changes the query.",
    ),
    ("ox/cache-ped", "ox_lib keeps the local ped in `cache.ped`."),
    ("ox/cache-playerid", "ox_lib keeps the local player id in `cache.playerId`."),
    ("ox/cache-serverid", "ox_lib keeps the local player's server id in `cache.serverId`."),
    ("ox/cache-vehicle", "ox_lib keeps the local player's vehicle in `cache.vehicle` (`false` on foot, not `0`)."),
    ("ox/cache-weapon", "ox_lib keeps the current weapon in `cache.weapon` (`false` when unarmed)."),
    ("ox/cache-resource", "ox_lib keeps the resource name in `cache.resource`."),
    ("ox/request-asset", "A request-and-wait loop for a model / dict / asset: `lib.requestModel` and friends do it with a timeout."),
    ("ox/callback", "Framework callbacks (ESX / QBCore): `lib.callback` / `lib.callback.register`."),
    ("ox/notify", "Native or framework notifications: `lib.notify`."),
    ("ox/text-ui", "Per-frame help text: `lib.showTextUI` / `lib.hideTextUI`."),
    ("ox/progress", "Framework progress bars: `lib.progressBar` / `lib.progressCircle`."),
    ("ox/closest", "Entity pool scans and framework closest-entity helpers: `lib.getClosest*` / `lib.getNearby*`."),
    ("ox/keybind", "Key polling in a loop or `RegisterKeyMapping`: `lib.addKeybind`."),
    ("ox/add-command", "Manual `chat:addSuggestion`: `lib.addCommand` registers the command with its suggestion and permissions."),
    ("ox/raycast", "Hand-rolled shape tests: `lib.raycast.fromCoords` / `lib.raycast.cam`."),
    ("perf/concat-in-loop", "`s = s .. x` on the same variable in a loop is quadratic; collect parts and `table.concat` them."),
    ("perf/table-insert-append", "`table.insert(t, v)` is a function call; `t[#t + 1] = v` is not."),
    ("perf/sequential-index-keys", "`{ [1] = a, [2] = b }` fills the hash part; `{ a, b }` fills the array part."),
    ("perf/constant-table-in-loop", "A constant table passed to a library function is rebuilt on every loop pass; hoist it."),
    (
        "perf/closure-in-loop",
        "A function expression passed to a Lua library function inside a loop that never waits (`table.sort(t, function ...)`), using no local of the loop (its variables or the locals of its body): the same closure is built on every iteration; create it once before the loop. Closures that capture a loop value, closures given to other APIs (event handlers, callbacks), closures stored in tables and loops with `Wait` are left alone.",
    ),
    (
        "perf/global-in-loop",
        "Off by default (several hundred hits per server, mostly loops that run once). `math.floor(x)` (math, string, table, utf8, bit32) called in a `for` loop that never waits: the global and the field are looked up on every iteration; `local floor = math.floor` before the loop. Once per name and loop.",
    ),
    ("perf/runtime-load", "`load` / `loadstring` compiles code at run time."),
    ("style/redundant-boolean-return", "`if c then return true else return false end` is `return c` (when `c` is a boolean)."),
    ("style/string-key-access", "`t[\"name\"]` with an identifier-like key reads the same as `t.name`."),
    ("naming/local", "A local variable or function not in camelCase."),
    ("naming/const", "A `<const>` local holding a literal not in UPPER_SNAKE_CASE."),
];

pub fn describe(code: &str) -> Option<&'static str> {
    RULE_DOCS.iter().find(|(c, _)| *c == code).map(|(_, d)| *d)
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_rule_is_described() {
        for (code, _) in super::super::RULES {
            assert!(super::describe(code).is_some(), "{code}");
        }
        assert_eq!(super::RULE_DOCS.len(), super::super::RULES.len());
    }

    #[test]
    fn rules_md_lists_every_rule() {
        let md = include_str!("../../../../docs/RULES.md");
        for (code, _) in super::super::RULES {
            assert!(md.contains(&format!("| `{code}` |")), "docs/RULES.md has no row for {code}");
        }
    }
}
