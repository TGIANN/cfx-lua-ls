# Roadmap

Goal: everything lua-language-server (LuaLS) does, rewritten in Rust for FiveM's
CfxLua (Lua 5.4 + power patches), at a fraction of the memory, plus a FiveM layer
LuaLS does not have.

Each phase ends with something usable and a check that proves it.

## Baseline (LuaLS 3.19.2, 2026-11-15)

`lua-language-server --check --checklevel=Warning`, peak memory summed over its
two processes:

| folder | files | LuaLS 3.19.2 | target |
|---|---|---|---|
| corpus A `[scripts]/[tgiann]` without escrow files | 623 (6.3 MB) | 1,021 MB, 33-36 s | < 300 MB, < 2 s |
| same folder as installed, 68 % escrow files | 1,955 | 1,673 MB, 25 s | |

FiveM escrow files (encrypted, `FXAP` header) are `.lua` files, and LuaLS has no
check for them. During the run on the installed folder LuaLS also hit an internal
error ("invalid UTF-8 code").

## Phases

- [x] **0. Setup:** cargo workspace, LuaLS / EmmyLuaCodeStyle / CfxLua sources in `ref/`.
- [x] **1. Parser:** lossless CfxLua lexer and parser with error recovery.
  - Check: every non-escrow `.lua` in three server trees (about 9,600 files,
    71 MB) round-trips byte for byte, and no valid file gets an error. Every file
    that does get one was checked by hand: pasted emote snippets, a diff, a JSON
    file named `.lua`, real unterminated strings. corpus A `resources` alone: 3,969
    files, 24.8 MB in 255 ms on 16 threads.
  - Tests cover CfxLua syntax, error recovery, doc-comment attachment (including
    same-line comments that must not attach) and operator precedence shapes.
- [x] **1b. Compile-time checks:** errors Lua reports when loading a chunk (rules
  from `ref/cfxlua/lparser.c`): `break` outside a loop, `...` outside a vararg
  function, goto without a visible label, goto into a local's scope, duplicate
  labels, assignment to `<const>`/`<close>` locals (compound and `in` assignment
  included), unknown attributes, more than one `<close>`, more than 200 active
  locals, keywords used as names. Targeted messages for syntax from other Lua
  versions (`local <const> x`, `...name`, `function t[k]()`, `"x":f()`, `..=`).
  Every error has a stable code (LuaLS type names where one exists).
  - Check: all 62 Lua 5.4 cases of LuaLS `test/parser_test/syntax_check.lua`
    pass with the same code and range (extracted to JSON under the LuaLS binary,
    `tools/extract_luals_syntax_check.lua`); skip list empty. The corpus gives no
    new error. Single-thread parse of corpus A `resources` (`examples/bench.rs`):
    +4-6 % with the checks (836-865 ms against 803-814 ms).
  - 20,000-level `a.b.b...` / `a + a + ...` chains (valid Lua) parse and free
    without overflowing the stack: the green tree is built without rowan's
    `GreenNodeBuilder` (its node cache rehashes recursively) and `Parse` frees
    deep trees iteratively.
- [x] **2. LSP shell + VS Code extension:** stdio server, document sync, syntax
  diagnostics, document symbols, folding, selection range, semantic tokens.
  - Done: stdio server with incremental sync, UTF-8 / UTF-16 positions and syntax
    diagnostics (LuaLS codes and source); escrow files get nothing.
  - Done: document symbols, a port of LuaLS `core/document-symbol.lua`. Check: all
    31 cases of LuaLS `test/document_symbol` match name, detail, kind, ranges and
    nesting (`tools/extract_luals_document_symbol.lua`); the corpus (A, B,
    C: 1.7 million symbols) has no symbol outside its file, its range or its
    parent (`crates/ide/examples/gate.rs`). The symbol pass takes about 1 s
    for all 3,968 corpus A files on one thread; through the server, the largest file
    (800 KB of data) answers in 360-500 ms with 4.9 MB of JSON, most of it
    conversion and serialization, not the symbol pass (about 40 ms).
  - 20,000-link call chains (`f(...)(...)...`, `a:b(...):c(...)...`) stay linear:
    only nodes that take comments search for leading trivia
    (`SyntaxKind::attaches_comments`).
  - Done: folding ranges, a port of LuaLS `core/folding.lua` with its line rule (a
    block's closing line stays visible; comments and regions fold whole). LuaLS has
    no folding fixtures; unit and protocol tests cover each construct. Doc blocks:
    every run of two or more `---` lines folds (LuaLS folds the bound ones; docs are
    parsed in phase 5). Corpus: 447,000 folds, none outside its file; corpus A takes
    about 0.8 s on one thread, most of it walking the red tree token by token.
  - Done: selection ranges (LuaLS has none): the token under the cursor (a string's
    contents first), then each enclosing node without its comments, up to the file;
    each step strictly holds the one before; chains over 200 steps keep their
    inner and outer 100. Corpus: 785,000 positions, every chain nested and ending
    at the whole file; about 3 µs a position.
  - Done: semantic tokens (full document), LuaLS legend and the rules of
    `core/semantic-tokens.lua` the syntax tree decides: keywords, operators,
    integer / float numbers, strings with valid / invalid escapes, comments and the
    LuaCATS `@tag`, parameter and local declarations (`<const>`, `<close>`,
    function values), fields, methods, `goto` / labels. Uses of locals, globals and
    library globals wait for name resolution (phase 4); `` `hash` `` colours as a
    string. Tokens over several lines go out one piece per line. Corpus: 4.3
    million tokens, ordered and disjoint; the largest file (800 KB) answers in
    54 ms through the server.
  - Done: VS Code extension in `editors/vscode` (vscode-languageclient 9, esbuild):
    server from `cfxLuaLs.server.path` (user settings only, absolute), else a bundled
    `server/` binary, else an absolute `PATH` folder; never a bare name (Windows
    would look in the workspace first); restart command; trace setting. A
    workspace `cfxLuaLs.server.path` is ignored (checked in VS Code). Checked in VS Code 1.146 with an
    isolated profile: initialize, diagnostics, document symbols, folding and
    semantic tokens round-trip with no client error (protocol log). Checked by
    eye in the editor (2026-11-16): token types and modifiers (parameter /
    function declaration, method, `<const>` read-only, hash string), the syntax
    error squiggle, region fold, expand selection and the restart command. The
    `"` in a CfxLua `/* " */` comment does not leak into later lines: the
    TextMate scopes there stay `source.lua` and the semantic `comment` token
    colours the line, so no injection grammar is needed. Marketplace publishing
    is phase 7.
- [x] **3. Formatter:** EmmyLuaCodeStyle behaviour and options (`.editorconfig`,
  `Lua.format.defaultConfig`), indent first.
  - Done: `crates/fmt` (`cfx-lua-fmt`), a port of EmmyLuaCodeStyle (commit
    `fdbaa0f`, MIT, credited in `NOTICE`): its tree, the seven analyzers (space,
    line break, indent, align, token, semicolon, doc comments incl.
    `---@format disable`), every style option, range formatting and the
    `.editorconfig` reader. Deep expressions are walked with explicit stacks, not
    recursion. Escrow files and files with syntax errors are never formatted.
  - Check: EmmyLuaCodeStyle's test cases, 201 of 209 pass (5 of them range
    formatting); the 8 skipped are syntax errors in CfxLua (trailing comma in call
    arguments, `return` before the end of a block, non-ASCII names, table fields
    without separators), listed with the reason in `crates/fmt/tests/codestyle.rs`.
  - Check against LuaLS 3.19.2's own formatter (`code_format`, run by
    `tools/luals_format.lua`) on every file it formats: corpus A 3,939 of 3,944 files
    byte-identical, corpus B 3,524 of 3,531, corpus C 2,016 of 2,020. Every
    difference is the one deliberate deviation: inside a long string (`[[ ]]`)
    trailing spaces are kept, LuaLS drops them and so changes the string's value
    (in long comments both drop them). No output with syntax errors, no token or comment changed.
    Range formatting (5 ranges per corpus A file): 19,716 of 19,720 identical, the 4
    others the same long-string case.
  - LuaLS is not idempotent: a break it adds inside a one-line `if ... end` opens
    the whole block on the next run (corpus A 688 files, corpus B 529, corpus C 412
    change on a second run). The check is therefore "our second run equals LuaLS's
    second run", and it holds with the same exceptions; the original "idempotent
    on the corpus" check would have meant deviating from LuaLS.
  - Done: `textDocument/formatting` and `rangeFormatting` (the asked lines grown
    to whole tokens, replaced from column 0 to the start of the next line, as
    LuaLS). Settings are LuaLS's own keys, pushed by `workspace/didChangeConfiguration`
    (`Lua.format.enable`, `Lua.format.defaultConfig`); the VS Code extension syncs
    the `Lua.format` section and does not declare it (the sumneko extension does).
    The nearest `.editorconfig` from the file's folder up to its workspace folder
    replaces `defaultConfig`; the editor's `tabSize` / `insertSpaces` apply last.
- [ ] **4. Names:** scopes, locals, globals, go to definition, references, rename,
  highlight. Diagnostics: `undefined-global`, `unused-local`, `redefined-local`,
  `lowercase-global`, ...
  - Done: name resolution inside a file (`ide/src/names.rs`, one iterative
    preorder walk): locals, `local function`, parameters, loop variables, method
    `self`, CfxLua `local a, b in t`, labels and `goto` (within the function),
    globals; read / write / field-set access per reference; shadowed locals;
    used and unused varargs. Definition (a local's declaration, its function value
    and its assignments, as LuaLS), references, document highlight (names, keyword
    partners, literals) and rename (with prepare), in the server.
  - Check: LuaLS `test/definition` (local, label, arg, set, function),
    `references`, `highlight` and `rename`: 118 of 136 cases pass; the 18
    skipped are a function value through returns, a duplicate label, Lua 5.5 `global`,
    `_G.x`, a rename to a non-name or of a global's field, or commented out in
    LuaLS; each listed with the reason in
    `crates/ide/tests/luals_names.rs` (`tools/extract_luals_names.py`).
  - Done: field references, highlight and rename inside a file
    (`ide/src/fields.rs`). A field is named by its path from a local or a
    global: `t.a`, `t['a']`, `t[ [[a]] ]`, constructor fields under `local t =`
    / `t =` (nested too), `function t.a()`, `function mt:x()` with `self.x` /
    `self:x()` in its methods. Rename only for a direct field of a local
    table built in the file (`local t` or `local t = { ... }`; not a
    parameter, a loop variable or a call's result) that stays there: never
    passed, returned, reassigned to a non-table or indexed with a computed
    key, and `t:m()` only for a `function t:m()` of the file. A global's field
    is refused, other files may use it. Gate on corpus A: 11,771 positions, every set the same from each
    of its members, 0.55 ms a call, slowest 18 ms.
  - Done: renaming a parameter also renames its `---@param` line, and the
    rename can start on that line. Corpus
    gate: 966,000 references, each a name token resolving to an earlier
    declaration of its name; corpus A in 0.6 s on one thread.
  - Done: references and rename by type across the workspace
    (`ide/src/workspace/query/refs.rs`, LuaLS `core/reference.lua` /
    `core/rename.lua`). Class, alias and enum names: their declarations and
    every type written in a doc comment (`---@param x A|B[]`, a class's
    parents, `--[[@as A]]`), not where a `---@generic` of the same name
    shadows them. Class fields: `---@field`, `C.x = v`, `function C:x()`,
    constructor keys of a value of the class, `o.x` / `o:x()` / `o["x"]` on a
    value of the class or a subclass (`---@type`, `---@param`, `---@return`,
    a field's type, `xs[i]` / `local v = xs[i]` / `for _, v in ipairs(xs)` of
    an `A[]` or `table<K, A>`), owned by the class in the chain that declares
    the field. A name counts in the files that see the same declaring files
    (two resources' own `Config` classes stay apart); a field is looked for
    only in the resources that name its class. The open documents' text wins
    over the disk. Rename refuses a library or builtin name, a taken or
    invalid new name, an escrow resource in scope, a search over the step
    budget, a field name the class chain already has, a site whose value may
    also be another class with the field (`---@type A|B`), and a field when
    any same-named access in scope is on a value of unknown type (an untyped
    parameter, a call's result, a value stored in a field, a constructor,
    local or global table handed to a call, returned or stored elsewhere); LuaLS
    renames what it finds instead. Continued doc lines (`---@type {` over
    several lines) keep each name's own offset. On corpus D a field
    search reads the server in 0.5 to 0.9 s. A call's later values
    (`local a, b = f()`) and a `---@generic T` returned as `T` (the argument's
    class) are followed too.
  - Done: lint (`crates/ide/src/lint`, one walk over the tree feeds every rule):
    15 LuaLS diagnostics that need no types (`undefined-global`, `lowercase-global`,
    `unused-local`, `redefined-local`, `duplicate-index`, `count-down-loop`, `deprecated`, ...)
    and 50 FiveM / sandbox / manifest / export / framework / ox_lib / performance / naming rules, listed with
    levels and fixes in [RULES.md](RULES.md); a test fails when a rule has no row
    there. Globals are resolved across files per resource and side (phase 6).
  - Check: unit tests for every rule (most with negative cases), and the corpus:
    hits on corpora A, B and C were read rule by rule (every hit of the
    rare rules, samples of the frequent ones such as `trailing-space` or
    `fivem/backtick-hash`), and each false-positive class found was fixed or the
    rule narrowed (manifest directives, globals used across files or by escrow
    files, ox_lib / framework imports, `for` headers, `exports["x"]`, count-down
    loops with negative literals, own functions named like natives). The hits
    read include real bugs in those servers' code
    (undefined `ok` / `multiplier`, a trailing comma that turns a call into a
    value, `source` after `Wait`, `local _, b = c and f() or g()` truncating to
    one value, duplicate table keys).
- [ ] **5. Types:** LuaCATS doc parser, type engine (semantics of LuaLS `script/vm`,
  not its memory model), Lua 5.4 standard library meta, hover, completion,
  signature help, type diagnostics.
  - Done: the embedded library (`crates/ide/library`, 150 files, 5 MB): LuaLS's
    Lua 5.4 meta and fivem-lls-addon's natives (about 14,700 functions with game
    and side) and runtime, generated by `tools/gen_library.lua` (applies the
    `---#if` / `#DES` templating of LuaLS meta) and indexed on first use.
  - Done without types: hover (library declarations and docs, locals with their
    leading comments), completion (locals in scope, the file's globals, keywords,
    library globals filtered by side and game, `a.` fields from the library and
    from the file) and signature help (library functions, active parameter, even
    in an unclosed call), in `crates/ide/src/assist.rs`.
  - Done: the LuaCATS parser (`crates/ide/src/luacats.rs`: every tag of LuaLS's
    doc grammar, the full type syntax, multi-line `{`/`(` types; over the
    corpora 17,059 files and 124,992 tags parse, the only errors are the
    authors'), `---@class` / `---@field` / `---@operator` / `[K]` index fields /
    class generics in the file summary.
  - Done: types across the workspace in the query (`Base::Class`, `fun` types,
    weak `---@return table` not hiding the value returned, values stored under a
    computed key, class bound to a local inside a constructor, library classes
    such as `vector3`, natives' `---@return`): completion after any expression,
    `: type` on hover for locals and parameters. ESX 1.12.4
    `ESX.GetPlayerFromId(1).` lists the 74 members of `xPlayer` in about 5 ms;
    1.13.4 (`---@class xPlayer: StaticPlayer`) its 79.
  - Done: single-file inference (`crates/ide/src/types.rs`): literals and
    operators (with `---@operator`), multiple returns, `---@generic` binding from
    arguments and iterator state, `pairs` / `ipairs` and other iterators, flow
    narrowing (`if x then`, `x and`, `if not x then return end`, `assert(x)`,
    `type(x) == 'string'`, writes in earlier branches), `---@cast`,
    `--[[@as T]]`, aliases; memoized per expression, so `s = s:gsub():gsub()`
    chains stay linear. Checked by `crates/ide/tests/luals_types.rs` against
    LuaLS's `test/type_inference` (610 cases, extracted by
    `tools/extract_luals_types.py` with the LuaLS settings each runs under:
    `expandAlias`, `enumsLimit`, `inferParamType`): 524 of the 608 scored
    cases pass; the `??` operator case (`runtime.nonstandardSymbol`) and the
    `runtime.special` one are not scored. The list of passing cases is kept so
    a regression fails the test. The inferred type prints as LuaLS prints it:
    union members sorted (basic types first, `nil` last), five then
    `...(+N)`, aliases expanded.
  - Done: `undefined-doc-name`, silent when the resource or one it depends on
    or imports from has escrow files (corpus A's tgiann-core declares
    `ChangeClotheData` in an escrow file). 128 hits on the five corpora, all
    read: ox_inventory / ox_target types their releases do not ship
    (`inventory`, `OxServerItem`, `OxTargetBoxZone`, ...), Qbox's `Source` in a
    server without qbx_core, a type from prp-minigames that is not installed,
    descriptions written where the type goes (esx_license's
    `---@param licenseType license type`), `func(t: Item)` for `fun(...)`.
  - Done: the type diagnostics, on a compatibility check after LuaLS's
    `vm.canCastType` (`Infer::cast`: nil is not checked, as with
    `Lua.type.weakNilCheck`; `unknown` anywhere keeps a rule silent; integer and
    number mix; a class and a table fit each other): `param-type-mismatch`,
    `assign-type-mismatch`, `cast-local-type`, `return-type-mismatch`,
    `missing-fields`, `need-check-nil`, `undefined-field`, plus
    `missing-parameter` / `redundant-parameter` across the workspace,
    `missing-return` / `missing-return-value` / `redundant-return-value`,
    `discard-returns`, `duplicate-set-field`, `close-non-object`,
    `newfield-call`, `undefined-doc-class`, `circle-doc-class`,
    `duplicate-doc-alias`. Each is narrower than LuaLS where the inference
    would guess (RULES.md lists every deviation): types from the same file
    only, values the flow may have narrowed (names, `a or b`) left alone,
    upvalues not followed. LuaLS's `test/diagnostics` (537 cases, each run
    with the LuaLS switches its test sets, `crates/ide/tests/luals_diags.rs`):
    529 pass by default and all 537 with `cfx.lualsCompat` on (the
    deliberate deviations below, LuaLS's behaviour behind one switch: nil
    checked as LuaLS does, a local typed by its first value, `unpack`
    deprecated, empty bodies, every unknown `---@diagnostic` code); the Lua
    5.5 `global` case is not scored (CfxLua is 5.4). Over the five corpora the
    new rules give 440 hits (194 `redundant-parameter`, 86
    `missing-parameter`, 52 `missing-return-value`, 32 `duplicate-set-field`,
    31 `need-check-nil`, the rest under 20 each), read in buckets by cause,
    every hit of the rarer rules read (`need-check-nil`: 18 where the doc
    allows nil and nothing checks it, 13 behind a flag variable or a
    `#(x or {})` loop, the known gaps); they include real bugs
    (a `---@return boolean` function with no final return, calls with more
    arguments than the function takes, a number passed to a `---@param`
    string, a field ESX's own progress bar reads but its class does not
    declare).
  - Done: the rest of LuaLS's diagnostic codes: `inject-field`, `invisible`,
    `cast-type-mismatch`, `global-in-nil-env`, `undefined-env-child`,
    `different-requires`, and off by default as in LuaLS `await-in-sync`,
    `not-yieldable`, `incomplete-signature-doc`, `missing-global-doc`,
    `missing-local-export-doc`, `global-element`, `no-unknown` (a configured
    severity or `diagnostics.neededFileStatus` turns them on). Left out:
    `codestyle-check` (the `format` command and the formatter's range
    formatting cover it), `name-style-check` and `spell-check` (EmmyLuaCodeStyle
    configuration and a dictionary), `unnecessary-assert` (LuaLS ships it
    disabled). `deprecated` also sees the
    file's own `---@deprecated` definitions and marks `T.x` / `T:f` whole, as
    LuaLS does. `inject-field` takes the object's type where it is written
    (`x = x or {}` makes it `C|table`, which takes anything). Over the five
    corpora: 19 hits, all fields ox_inventory and ox_target write that their
    classes do not declare. LuaLS 3.19.2 (`neededFileStatus` `Any!`) on the
    same resources gives 17 of them and about 80 more; ours stays narrower
    (one class per file, no shapes guessed).
  - Done: LuaLS's hover and completion cases
    (`crates/ide/tests/luals_assist.rs`, extracted by
    `tools/extract_luals_cases.lua`), with a list of passing cases. Hover moved
    to LuaLS's layout: `function f(a: any)` plus `  -> T` return lines,
    `local x: T = literal`, loop variables as locals, `(parameter) a: any`, a
    constructor's fields in source order or a class's `---@field`s by name (50
    at most); 25 -> 75 of 190 cases exact, 66 -> 92 of 140 on the type alone.
    Completion: 54 of 207 (CFX natives dropped from our list, LuaLS's call
    snippets from its); `s:` on a string offers the string library, nothing
    while naming a parameter. The rest is LuaLS-specific: word completion
    (`Text`), its item kinds (`Enum`, `Property`), snippets, string enum
    arguments.
  - Done: completion 54 -> 207 of 207 (`crates/ide/src/doc_complete.rs` and
    `assist.rs`): LuaLS's fuzzy `matchKey`, its item kinds (a global or field
    given a literal is `Enum`, a function `Function`, one only read here
    `Text`, table keys `Property`, values `EnumMember`), word completion
    (`showWord`: `Fallback` in the editor as LuaLS ships it, `Enable` in the
    harness as its tests run), doc comments (tags, types, `@param` names,
    `@cast` locals, `@diagnostic` modes and codes, `@operator`, the `---`
    function snippet), literal values and table keys a position takes
    (`---@alias`, `---@enum`, `---|` lines, overloads, class `---@overload`s,
    `` `T` `` generics bound from a string), the trigger rules (after `(`,
    `,`, `{`, `#`, `@`, `|` or a space only those values), `continue`, the
    `do` of a `while` / `for`, names with non-ASCII characters (read with a
    `UNICODE_NAME` error, as LuaLS does). How the harness scores: snippets and
    LuaLS's `function` call snippets are dropped on both sides (ours are
    written for CfxLua), globals score as `Field`, the trigger is the
    character before the cursor as in LuaLS's tests. Not scored, with their
    reason in the extractor: cases that need non-default settings, LuaLS's
    `ContinueTyping` follow-up requests and its `<function>` placeholder
    items. A completion in a 1 MB data table costs about 25 ms; the
    re-parse with a probe name runs only where a typed value is asked for.
  - Done: hover 75 -> 183 of 183 scored cases exact (`crates/ide/src/types/hover.rs`,
    `consts.rs`): LuaLS's titles (`(global)`, `(field)`, `(upvalue)`,
    `(self)`, `(method)`), literal values (`= 1|2`, escapes, `0xff` as `255`,
    constants folded through operators and calls), table shapes sorted as
    LuaLS sorts them (50 at most, `...(+n)`), fields reached through
    `setmetatable` and `__index`, library signatures with their doc types and
    aliases expanded, a library table global with its members, `require` of a
    library module, string types with the quote they were written with,
    union members in LuaLS's order, `{...}` as an array of the vararg type,
    `---@vararg`, `---@param` above a `for`, a `---@type` at the end of the
    line, `---|` lines under a `---@type`, names inside doc comments
    (`(class) A`, `(enum) A`, `(alias) A Expand to T`, `---@see A.x`), the
    `function` keyword, a method of a string value (`('x'):sub`), and a call
    that only an `---@overload` fits (`local f: fun(x: number):number`). Not
    scored, with their reason in `HOVER_NOT_SCORED`: 29, 30 and 107 (LuaLS's
    Chinese locale text), 40 and 43 (CfxLua's `utf8` has `strcmputf8i` and
    `strlenutf8`), 58 (`_G` holds the natives in FiveM), 191 (Lua 5.5 named
    vararg).
  - Done: inference 374 -> 397: `---@overload` resolution by argument count and
    types (literals compared as literals), a parameter without `---@param`
    typed from its overloads, an overriding method's parameters from the class
    or a parent (depth-bounded), `C:new()` on `function C.new(self)` binding
    `---@param self T`, a function returned under `---@return fun(...)`, a
    function in a `---@type {[integer]: fun}` constructor, `...` under
    `---@param ... T`. No change on the corpora.
  - Done: inference 397 -> 408: a `---@cast` standing alone between blank
    lines, `local x ---@type T` on one line, `a and b` / `a or b` as LuaLS
    types them (`b` plus what of `a` is falsy; what of `a` is truthy plus `b`),
    `true` / `false` locals never written again read as their value. On the
    corpora two new `param-type-mismatch` hits, both real (a `table<string, _>`
    key passed as a `number`, a value that may be `false` passed on).
  - Done: inference 408 -> 414: the falsy side narrows (`if not x`, the
    `else` of `if x`, `x == nil`, `not x and x`), a truthy `boolean` is
    `true`, a `type(x)` test inside `a and b` narrows. On the corpora one
    `param-type-mismatch` false alarm gone (`x ~= nil and type(x) == 'number'`).
  - Not done: 196 inference cases, sorted by cost: values past a call's last `---@return`, operators of generic
    classes (`Box<string>`), `stringlib` members on string values, overload
    choice, generics bound through returned `fun`s and callbacks, members
    written onto a class table seen through an instance, union print order,
    alias expansion in the shown type (the harness has no settings, so it
    would trade cases), and 16 cases that need non-default LuaLS settings or
    are LuaLS-internal views.
  - Done: `Lua.doc.privateName` / `protectedName` globs for `invisible`, from
    `.luarc.json` or the client's settings, on `---@field`s and on the class's
    own table fields (LuaLS fixtures 6-9 give the same results through the
    CLI; the diagnostic harness does not score cases with settings), and
    with `Lua.doc.regengine` `lua` as Lua patterns (`string.find`; a port of
    `lstrlib.c`'s matcher, `ide/src/luapat.rs`; fixture 10 in a unit test).
- [ ] **6. FiveM layer:** `fxmanifest.lua` resource model, client / server / shared
  sides, natives, exports, events, resource-scoped types (below), escrow files
  skipped.
  - Natives and runtime globals come from overextended/fivem-lls-addon (MIT,
    regenerated weekly): `library/natives/**` carries a `client` / `server` tag
    per native (for wrong-side calls), `library/runtime/*.lua` types `Citizen`,
    `exports`, events, `json`, `msgpack`, `promise`, vectors and the manifest
    directives.
  - Done: manifest reader (sides, `files`, globs incl. `**`, `@res/file` imports,
    dynamic manifests detected), workspace model (`crates/ide/src/workspace.rs`:
    every resource under the workspace folders, escrow files, per-file side, game
    and visible globals, updated on change, delete and manifest edits), side-aware
    native checks, the sandbox rules and the ox_lib suggestions.
  - Done: the workspace API index (`crates/ide/src/api.rs`, `workspace/query.rs`).
    Each file keeps a summary of what it defines: globals, file-level locals,
    table fields, function signatures, the module's return, exports (`exports()`
    calls, manifest `export` entries, `for k, v in pairs(T) do exports(k, v)`,
    wrappers registering `__cfx_export_<res>_%s`), events and leading doc
    comments. A query follows these across files and resources under a step
    budget: `exports.res:Name()` through `provide`, values returned by exports,
    `require`d modules, ox_lib's lazily loaded `lib` modules. It feeds completion,
    hover, signature help and go-to-definition, the export / deprecation /
    framework rules and the MCP `exports` / `members` / `events` tools. Literal
    exports of JavaScript scripts are read too; a resource with a computed
    JavaScript export or a .NET script counts as not fully known.
  - Done: event names (completion inside event calls, definition from a trigger
    to its handlers and back, references), fxmanifest.lua checked against the
    disk.
  - Measured: corpus A `check` 2.2 s and about 140 MB peak (1.8 s and 117 MB before the
    rules; 66 MB before the index); editor features at every 20th `.` / `:` of
    corpus A, 19,336 positions, slowest 83 ms. After the type layer (2026-11-17,
    warm, against the commit before it): corpus A `check` 2.8-3.3 s either way;
    the slowest editor position is still `exports["tgiann-phone"]:` in
    tgiann-realparking (152 exports): complete + hover + definition about
    85 ms in both builds, 79-138 ms in gate runs depending on machine load, so
    the 100 ms line is not met there every run. After the inference work of
    2026-11-19, interleaved with the v1.0.0 build: corpus A `check` 3.7-3.9 s in
    both (the machine is slower than in the earlier lines), gate slowest 96 ms
    at the same position.
  - Checked against: corpus A (Qbox: qbx_core 1.23 with its qb bridge, ox_lib 3.39),
    corpus B (qb-core). ESX is not modelled beyond what any resource's own code
    shows; no ESX copy was in the corpus.
  - Done: `fivem/net-event-same-side` (a server net event that only server code
    fires). On the corpus it found `esx_license:addLicense` in ESX's own addons
    (any client can grant itself a licence), Discord-log events and framework
    `PlayerLoaded` handlers clients can send.
  - Checked against ESX Legacy 1.12.4 and 1.13.4 (two test servers): `ESX` from
    `exports.es_extended:getSharedObject()` / `@es_extended/imports.lua`
    completes per side; exports registered through tgiann-core's `Export`
    (both first-letter casings) and minified JavaScript with an aliased `exports`
    are handled.
  - Done: `esx/shared-object-event` (Information): `TriggerEvent('esx:getSharedObject', cb)`
    when the workspace's `es_extended` exports `getSharedObject`. Both versions
    still answer the event; 1.12.4 prints a warning on every use. Two hits on
    an ESX Legacy corpus (tgiann-sellnpc, dpemotes).
  - Done: `fivem/unchecked-net-arg` (Warning): a server net handler passing a
    parameter it never read before into `addMoney`, `addAccountMoney`,
    `addInventoryItem`, `AddItem`, `setJob` and the like. 23 hits on the five
    corpora, read one by one: most are item names and amounts the client
    chooses (`tgiann-chair:give-chair`, `opod-artHeist:GiveInventoryItem`,
    `zeu:rcCarAl`), `qb-admin:giveWeapon` with no permission check, a stash
    deposit that adds any weapon and count the client names before removing
    it from the player (`tgiann-stashhouse:putStockItems`), item metadata from
    the client. Where something is checked first (an item removed, a distance
    to the heist), the item name or the count still comes from the client;
    an unused `key` parameter checks nothing. `setJob`'s duty flag
    (codem-phone) is no longer reported. `fivem/source-65535` and
    `fivem/client-net-no-source-check` not adopted (reasons in RULES.md).
  - Done: `perf/closure-in-loop` (Hint; a closure given to a Lua library
    function in a loop that never waits, capturing nothing of the loop: 10
    hits, all `table.sort` / `pcall` comparators) and `perf/global-in-loop`
    (off: about 700 hits per server, mostly loops that run once).
  - Done: resource-scoped types. A class is looked up in the file's own
    resource, then in the resources its manifest names (`dependency`, `@res/`
    imports), then in the whole workspace; two resources declaring the same
    `Config` no longer mix. Globals were already per resource and side;
    aliases are not resolved across files.
- [ ] **7. Rest of LuaLS + release:** code actions, inlay hints, workspace
  diagnostics, `.luarc.json` and `Lua.*` settings compatibility, CLI `check`,
  GitHub Action (SARIF), GitHub release, VS Code Marketplace (publisher `tgiann`,
  owner decision).
  - Done: quick fixes and "disable for this line / file" code actions;
    `.luarc.json` / `.luarc.jsonc` and `Lua.diagnostics` (`enable`, `disable`,
    `severity`, `globals`, flat or nested keys); `---@diagnostic` comments.
  - Done: `cfx-lua-ls check <paths>` with `--format text|json|sarif`, `--level`,
    `--fail-on`, `--fix`, `--all-files`. corpus A (3,922 files, 404 resources) in
    1.1-1.6 s and about 65 MB peak; LuaLS needs 33 s and 1 GB on a sixth of it, but
    it also infers types, so the gap will narrow once phase 5 is done.
  - Done: `cfx-lua-ls mcp`, a Model Context Protocol server (stdio) for AI
    agents: `check` (diagnostics of a path or unsaved text, with fixed text),
    `lint_snippet` (code before it is written to a file), `native` (look up or
    search natives with side and docs), `rules`, `resource_info`, and over a
    folder of resources `exports`, `members`, `events`.
  - Done: workspace symbols (globals, their fields, exports, event handlers;
    fuzzy), the MCP `symbols` tool, exports in `resource_info`, and a kept MCP
    workspace: a folder scanned once is refreshed from file sizes and times
    (corpus A: first call 1.2 s, later ones about 0.27 s, most of it listing the
    folder).
  - Done: inlay hints (parameter names of natives, library, file and workspace
    functions; the type of a local whose value does not show it), off unless
    `Lua.hint.enable`, as in LuaLS; `Lua.hint.paramName: "Disable"` leaves the
    names out; the VS Code extension sends `Lua.hint`, and the server asks the
    client to fetch hints again when it changes. Labels drop `unknown`, skip
    unbound generics and show long table shapes as `table`; parameter names
    line up for `obj:f()` on `function obj.f(self, a)` and the reverse.
    Questions to the workspace stop after 40 ms per request (so a request
    near the limit may show a hint more or less from one run to the next).
    Gate, a 4 KB window at a time: corpus A 163,935 hints, slowest window 62 ms;
    ESX 1.12.4 184,522, slowest 85-149 ms by machine load (the 149 ms window,
    tgiann-phone-apps arcade, measures 44-66 ms alone); ESX 1.13.4 32,605,
    slowest 34 ms.
  - Done: `.github/workflows/ci.yml` (tests and clippy on
    Linux and Windows, the extension's type check and build);
    `.github/workflows/release.yml` (release-please; on a release, the server
    for Windows x64, Linux x64 and macOS arm64 / x64 attached to the GitHub
    release, a VS Code extension per platform with the server inside, and the
    Marketplace upload when the `VSCE_PAT` secret is set); `action.yml`, the
    repository as an action (downloads a release, runs `check`, uploads SARIF
    to code scanning, fails at `fail-on`).
  - Done: the repository is pushed (private, a fresh history without local
    paths) and v1.0.0 is released: the server for the four targets and a
    `.vsix` per platform on the GitHub release; CI runs on every push.
  - Not done: the Marketplace upload. It needs the owner's `VSCE_PAT` secret
    for the `tgiann` publisher; once the secret is set, the release workflow
    uploads on the next release.

## Design decisions

- **Lua 5.4 / CfxLua only.** The CfxLua syntax is taken from
  `ref/cfxlua/lparser.c` and the defines FiveM builds with (`code/vendor/lua.lua`
  in citizenfx/fivem): compound operators, `in` unpacking, set constructors, safe
  navigation `?.`/`?[`, C block comments, back-tick hashes. `defer` is a function
  in FiveM, not a keyword.
- **Lossless tree** (rowan). Comments and whitespace are part of the tree, so the
  formatter and doc comments work on the same tree as everything else. LuaCATS
  comment blocks directly above a statement or table field belong to that node.
- **Every global, class and alias is keyed by (resource, name).** Lookup order:
  the open file's resource, then resources it imports through the manifest
  (`@res/file.lua`, `dependency`), then the rest. The same type in two resources
  is a warning, not a duplicate error; the same type twice in one resource stays
  an error.
- **Quality rules are on by default.** Rules that improve code structure (for
  example global names starting with an upper-case letter, `lowercase-global`)
  ship enabled. The FiveM, performance and naming rules beyond LuaLS, with their
  sources, are listed in [RULES.md](RULES.md).
- **LuaLS behaviour is ported, not its design.** LuaLS keeps full ASTs and lazy
  caches for every file. Here a closed file keeps only a compact declaration index.
