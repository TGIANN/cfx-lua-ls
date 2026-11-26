# cfx-lua-ls: working notes

Plan, baseline and design decisions: `docs/ROADMAP.md`. Read it first.

## Reference sources (`ref/`, git-ignored, read-only)

Re-create with these pinned versions if missing:

| folder | source | version |
|---|---|---|
| `ref/luals` | github.com/LuaLS/lua-language-server | commit `a58fc7b` (2026-10-20) |
| `ref/luals-bin` | LuaLS release `lua-language-server-3.19.2-win32-x64.zip` | 3.19.2 |
| `ref/emmylua-codestyle` | github.com/CppCXY/EmmyLuaCodeStyle | commit `fdbaa0f` (2026-07-13) |
| `ref/fivem-lls-addon` | github.com/overextended/fivem-lls-addon (natives, runtime meta, LuaLS plugin) | commit `407f24a` (2026-11-15) |
| `ref/cfxlua-vscode` | github.com/overextended/cfxlua-vscode (installer extension for the addon) | commit `13a7b16` (2026-04-25) |
| `ref/effective-fivem-lua` | github.com/Manason/effective-fivem-lua (style and performance handbook, source of docs/RULES.md) | commit `ee23c81` (2024-05-24) |
| `ref/cfxlua` | github.com/citizenfx/lua branch `luaglm-dev/cfx`: `llex.c/h`, `lparser.c`, `luaconf.h`, `README.md`, `EXTENDED.md` | |

Do not read Qbox Lua (qbx-editor) source for ideas: it is GPL-3, this project is MIT.

## Rules

- Port LuaLS behaviour, not its memory model.
- Every global / class / alias is keyed by (resource, name) from the first index on.
- Quality rules ship enabled by default (owner request).
- Escrow files (`FXAP` header) are never parsed.
- Compile-time rules follow `ref/cfxlua/lparser.c`; LuaLS fixtures decide only the
  error range. A LuaLS case that disagrees goes to the test's `SKIP` list with the reason.
- `Parse` must be the last owner of a tree: it frees deep trees without recursion,
  a cloned `SyntaxNode` / `GreenNode` that outlives it frees recursively.
- The compile-time checks walk the green tree (`validate.rs`), not `SyntaxNode`:
  a red-tree walk made parsing 50 % slower.
- Before calling a parser change done: `cargo test`, `cargo clippy --all-targets`
  (no warnings), and the corpus runs below with 0 not-lossless files and the
  error-file counts unchanged (each of those files is non-Lua or really broken):

  The corpus folders are machine-specific: `CLAUDE.local.md` (git-ignored) maps
  each name to its folder on this machine.

  | corpus | files | files with errors |
  |---|---|---|
  | A (Qbox) | 3,969 | 24 |
  | B (qb-core) | 3,621 | 89 |
  | C (qb-core) | 2,026 | 6 |

## Checks

```sh
cargo test
cargo build --release
target/release/cfx-lua-ls parse <corpus A>
# parse speed, files in memory, one thread (compare against the previous commit)
cargo run --release -p cfx-lua-syntax --example bench -- <corpus A>
# refresh the LuaLS syntax cases after moving ref/luals
ref/luals-bin/bin/lua-language-server.exe tools/extract_luals_syntax_check.lua ref/luals/test/parser_test/syntax_check.lua crates/syntax/tests/data/luals_syntax_check.json
ref/luals-bin/bin/lua-language-server.exe tools/extract_luals_document_symbol.lua ref/luals/test/document_symbol/init.lua crates/ide/tests/data/luals_document_symbol.json
# editor features over a corpus: no panic; symbols inside their file, range and parent; folds in the file; selections nested; semantic tokens disjoint
cargo run --release -p cfx-lua-ide --example gate -- <corpus A>
# a file's syntax tree
cargo run -p cfx-lua-syntax --example dump -- file.lua
# formatter against LuaLS's own; LuaLS is not idempotent, so each side's second run is compared too
cargo run --release -p cfx-lua-fmt --example corpus -- list <tmp>/list.txt <corpus A>
ref/luals-bin/bin/lua-language-server.exe tools/luals_format.lua <tmp>/list.txt <tmp>/luals
# list2.txt: `<tmp>/luals/<n>.lua` for n = 1 .. lines of list.txt (a missing file is skipped, the numbering holds)
ref/luals-bin/bin/lua-language-server.exe tools/luals_format.lua <tmp>/list2.txt <tmp>/luals2
cargo run --release -p cfx-lua-fmt --example corpus -- check <tmp>/list.txt <tmp>/luals <tmp>/luals2
ref/luals-bin/bin/lua-language-server.exe tools/luals_range_format.lua <tmp>/list.txt <tmp>/range
cargo run --release -p cfx-lua-fmt --example corpus -- check-range <tmp>/list.txt <tmp>/range
# refresh EmmyLuaCodeStyle's cases after moving ref/emmylua-codestyle
python -I tools/extract_codestyle_tests.py ref/emmylua-codestyle/Test/src crates/fmt/tests/data/codestyle_tests.json
# refresh the LuaLS definition / references / highlight / rename cases
python -I tools/extract_luals_names.py ref/luals/test crates/ide/tests/data/luals_names.json
# regenerate the embedded library after moving ref/luals or ref/fivem-lls-addon
ref/luals-bin/bin/lua-language-server.exe tools/gen_library.lua .
# lint over a corpus (text summary per code on stderr; --format json|sarif, --level hint, --fix)
target/release/cfx-lua-ls check <corpus A>
# MCP server smoke test: one JSON-RPC message per line on stdin
target/release/cfx-lua-ls mcp < requests.jsonl
```

Workspace API index (`ide/src/api.rs` builds a file's summary, `ide/src/workspace/query.rs`
answers): values stay references and are resolved at query time under a step
budget, a visited set and depth caps; only file-level locals are kept, data
tables with more than 32 keyed fields do not expand their children. Before
calling a change there done, run the gate (it times completion / hover /
signature help / definition at every 20th `.` / `:`; slowest call under 100 ms)
and `api_probe`:

```sh
cargo run --release -p cfx-lua-ide --example gate -- <corpus A>
cargo run --release -p cfx-lua-ide --example api_probe -- <folder> exports qb-core server
cargo run --release -p cfx-lua-ide --example api_probe -- <folder> members <file.lua> QBCore Functions
cargo run --release -p cfx-lua-ide --example api_probe -- <folder> expr <file.lua> "ESX.GetPlayerFromId(1)"
cargo run --release -p cfx-lua-ide --example at_probe -- <folder> <file.lua> <offset>
cargo run --release -p cfx-lua-ide --example inlay_probe -- <folder> <file.lua> <start> <end>
```

`GATE_TRACE=1` prints each position before it is asked (for a crash or a
stack overflow). The gate also asks inlay hints a 4 KB window at a time.

Types: `ide/src/luacats.rs` parses doc comments (`doc_probe` runs it over a
corpus), `ide/src/types.rs` infers one file, the query adds the workspace.
`crates/ide/tests/luals_types.rs` runs LuaLS's type inference cases and keeps
the passing ones in `data/luals_types_pass.txt`; a listed case that fails or an
unlisted one that passes fails the test. After a change:
`LUALS_TYPES_SHOW=1` prints the failures, `LUALS_TYPES_WRITE=1` rewrites the
list (only when nothing listed was lost). The cases come from
`python -I tools/extract_luals_types.py ref/luals/test crates/ide/tests/data/luals_types.json`.
Everything in `types.rs` is memoized per expression, local and function body:
without it `s = s:gsub():gsub()` chains are exponential.

Framework facts in rules and docs come from the framework's own source in the
corpus (qbx_core, qb-core, ESX Legacy 1.12.4 / 1.13.4 on the two ESX test
servers) or FiveM's `scheduler.lua`, never from memory. Files a manifest names are read only through
plain relative paths inside the resource (no absolute, UNC or `..`), size checked
on the open handle.

Lint (`crates/ide/src/lint`): a new rule gets a `RULES` entry (code + default
level), a `RULE_DOCS` line and a row in `docs/RULES.md` (tests fail otherwise),
unit tests with negative cases, and a corpus run; every hit on the corpus is read
until the false positives are gone. A rule that needs types or cross-resource
events waits (RULES.md "Planned").

The embedded library is built on first use on scoped threads, not rayon: built
from a rayon worker (lint in `check` / MCP) while the other workers waited on its
`OnceLock`, it deadlocked, and a dedicated rayon pool (`install` from the
worker) hung the same way. Likely cause, not observed: a worker waiting in rayon
runs other queued jobs, one of which calls `library()` again on the same thread.
Code that runs under `OnceLock::get_or_init` uses plain threads, no rayon pool.

Formatter (`crates/fmt`): before calling a change done, the corpus `check` must keep
every difference from LuaLS to long strings (trailing spaces kept inside `[[ ]]`),
with 0 output syntax errors and 0 changed tokens. `CODESTYLE_FAILS=<file>` makes
the codestyle test write each failing case there.

VS Code extension (`editors/vscode`): `npm ci`, `npm run typecheck`, `npm run build`.
To try it without touching the owner's VS Code profile or extensions, start
`code --user-data-dir <tmp>/ud --extensions-dir <tmp>/ext --log trace --extensionDevelopmentPath=<repo>/editors/vscode <folder>`
with `cfxLuaLs.server.path` and `"cfxLuaLs.trace.server": "verbose"` in
`<tmp>/ud/User/settings.json`; the protocol log lands in
`<tmp>/ud/logs/*/window1/exthost/tgiann.cfx-lua-ls/CfxLua Language Server.log`.
vscode-languageclient 11 writes the trace at the channel's trace level, so without
`--log trace` the file holds only the server's stderr (`--log tgiann.cfx-lua-ls:trace`
keeps VS Code 1.147 from opening at all). A window that is killed leaves `ud/code.lock`
behind and the next `code` call on that folder exits silently: use a new folder.

Feature code reads node ranges through the first and last real token (`trimmed` in
`ide/src/util.rs`): a statement's node starts at the comments before it, and
`first_token` / `last_token` stop at an empty child node.

rowan's `token_at_offset`, `first_token`, `last_token`, `next_token` and
`prev_token` recurse once per tree level: on a 5,000-level `a.b.b...` chain
`token_at_offset` overflowed a 2 MB test stack in a debug build. Feature code uses
`child_or_token_at_range` per level (`token_at` in `ide/src/selection.rs`) or the
green tree instead; `descendants`, `ancestors` and `parent` are iterative.

The `cfx-lua-ls parse` timing includes file reading and is noisy (±50 ms); judge
speed with the bench, old and new build run alternately.

Observed: from Git Bash, a `/c/...` path containing `[...]` reaches the exe
unconverted ("path not found"). Use a `C:/...` path or PowerShell.

Baseline memory measurement (LuaLS spawns 2 processes; sum them):
`ref/luals-bin/bin/lua-language-server.exe --check=<folder> --checklevel=Warning --logpath=<dir>`.

## Git and releases (same scheme as ai-core)

- Commits: Conventional Commits in English, imperative, one line (`feat(syntax): ...`,
  `fix(lsp): ...`, `perf(...)`, `docs(...)`; `feat!:` or a `BREAKING CHANGE:` footer
  for a breaking change). One commit = one change. No AI attribution lines. Commits
  before `950f31c` are not conventional; release-please skips them.
- Versions: SemVer from release-please (`googleapis/release-please-action@v5`,
  `release-please-config.json`, type `simple`; the workspace version in `Cargo.toml`
  and every workspace crate in `Cargo.lock` follow through toml extra-files; a new
  crate needs its own `Cargo.lock` entry there; the VS Code extension's `package.json`
  and `package-lock.json` versions follow through json extra-files). A breaking
  change bumps major. When release-please would pick another version, set it with
  a `Release-As: x.y.z` commit footer.
- release-please reads TOML with a tagged parser: a value is `{value, start, end}`,
  so a JSONPath filter compares `@.name.value`, not `@.name` (checked with
  release-please 17.6.0's own `GenericToml` on our `Cargo.lock`).
- The repo allows Actions to create pull requests (Settings > Actions > Workflow
  permissions); without it the release job fails after pushing its branch.
- The first release is 1.0.0 (owner's choice), set by a `Release-As: 1.0.0` footer;
  the manifest said 0.0.0 before it.
- Remote: `origin` is the private TGIANN/cfx-lua-ls, created again on 2026-11-18
  with a fresh history (no local path in it). Push, merging the release pull
  request, tags and GitHub releases only when the owner asks. Local paths and the
  owner's server folder names stay out of tracked files (`CLAUDE.local.md` is
  git-ignored; `docs/ROADMAP.md` names corpora by letter). After a release pull
  request is merged: `git pull --rebase origin main` before the next commit.
