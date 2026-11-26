# Changelog

## [1.4.0](https://github.com/TGIANN/cfx-lua-ls/compare/v1.3.0...v1.4.0) (2026-11-26)


### New Features

* **lint:** calls to documented functions of other files are checked, overloads included ([c425451](https://github.com/TGIANN/cfx-lua-ls/commit/c42545100929f5294134810c67f0c312d33b310c))
* **lint:** check calls through exports, lone locals and agreeing definitions ([2a6ce4c](https://github.com/TGIANN/cfx-lua-ls/commit/2a6ce4c67ce1aa8f15a3efa5e5724b882c90c9e1))
* **lint:** fivem/event-argument-count and fivem/event-missing-arguments check triggers against their handlers ([8f682bd](https://github.com/TGIANN/cfx-lua-ls/commit/8f682bddeabf4b52a2214bf256dce742f422d2e2))
* **lint:** missing-fields and type mismatches check against classes of the resource's other files ([529a6ff](https://github.com/TGIANN/cfx-lua-ls/commit/529a6ffc459bc44ec0e1ac3d9540e34359fbbde8))
* **lint:** ox/missing-locale reports locale keys no locale file has ([d057ec6](https://github.com/TGIANN/cfx-lua-ls/commit/d057ec619da7a6ae6ea034c2e4eab9a7c29a1465))
* **lint:** security/sql-concatenation reports values joined into MySQL queries ([7269ef9](https://github.com/TGIANN/cfx-lua-ls/commit/7269ef961c9b2b5736747be410f130d0dcad027b))


### Bug Fixes

* **lint:** cast-local-type checks an a and b value assigned to a local ([59815c8](https://github.com/TGIANN/cfx-lua-ls/commit/59815c85aba8aba7fe1e472661719289ab15aaa2))
* **lint:** cross-file call checks skip a local shadowing the global and a signature whose types mean other classes here ([6a0d7ac](https://github.com/TGIANN/cfx-lua-ls/commit/6a0d7acc3cfd49977560e4e778ddd1990b0ffa33))
* **lint:** perf/concat-in-loop skips a local declared in the loop ([923723a](https://github.com/TGIANN/cfx-lua-ls/commit/923723a0e185fe85b666f8b4c3fa474eb04eeacf))
* **lint:** unsigned-hash skips the hash fields game events send unsigned ([288ddb1](https://github.com/TGIANN/cfx-lua-ls/commit/288ddb1c7867389a2303cb15356d8be3df6e17b4))
* **lsp:** hover and completion follow a table an export returns through an alias ([3ec7434](https://github.com/TGIANN/cfx-lua-ls/commit/3ec7434b54f5b8ed62775f8860d44b5af7758250))
* **types:** a doc comment above a blank line is not the next statement's, as in LuaLS ([7012f31](https://github.com/TGIANN/cfx-lua-ls/commit/7012f31ed90d9e5911b62b675b0d749ae9665484))
* **types:** a type() test of a field narrows the field ([a6f7b8d](https://github.com/TGIANN/cfx-lua-ls/commit/a6f7b8dd869031be1dd413dc33f10f6aa4b5b966))
* **types:** a type() test whose branch rewrites the local narrows it after the if ([515d7dd](https://github.com/TGIANN/cfx-lua-ls/commit/515d7ddc1aed7fc9f6a58405b8a6358f66b068b8))
* **types:** type(x) tests narrow when false, after an early return and in else branches ([f88de69](https://github.com/TGIANN/cfx-lua-ls/commit/f88de6972282ddeb07c95e1c5f49bcf145de8a09))
* **types:** x == 'a' or x == 'b' narrows a union of literals to the values tested ([9a2fd65](https://github.com/TGIANN/cfx-lua-ls/commit/9a2fd650b02a8b1967f626df147ecfab949990ef))


### Performance

* **types:** field type narrowing reads only the conditions above the field ([5e4e3d3](https://github.com/TGIANN/cfx-lua-ls/commit/5e4e3d3870ce7f3e14e9becfffc2a01186ca2e75))

## [1.3.0](https://github.com/TGIANN/cfx-lua-ls/compare/v1.2.0...v1.3.0) (2026-11-26)


### New Features

* **lsp:** Lua.workspace.library folders give definitions to a single opened resource ([a842781](https://github.com/TGIANN/cfx-lua-ls/commit/a84278126ed326711da6c8100163ce08572a9bfa))


### Bug Fixes

* **lsp:** _G.X and _ENV.X define the global X, so ox_lib's lib.onCache and server lib.notify resolve ([55413df](https://github.com/TGIANN/cfx-lua-ls/commit/55413df45a36d0bc153db7dd7ef128e4ed7f9cbd))

## [1.2.0](https://github.com/TGIANN/cfx-lua-ls/compare/v1.1.0...v1.2.0) (2026-11-26)


### New Features

* **vscode:** extension icon, gallery banner and repository links ([2e64e91](https://github.com/TGIANN/cfx-lua-ls/commit/2e64e91e4342689cc036201b7015015fb9dd2359))

## [1.1.0](https://github.com/TGIANN/cfx-lua-ls/compare/v1.0.0...v1.1.0) (2026-11-26)


### New Features

* callback params from overloads, function keyword shows the expected fun, class overload calls, type() keeps fun shapes ([8b3266f](https://github.com/TGIANN/cfx-lua-ls/commit/8b3266f2b54c69d382e531408b477006d2e2c70c))
* closures read upvalues from where they are defined; a local with no value is unknown until written; writes in nested functions do not flow ([7515538](https://github.com/TGIANN/cfx-lua-ls/commit/75155382eb8998dec378d525334ce95d5e69d6ac))
* diagnostics for enum fields, generic bounds, class overload calls, dot-defined methods, union missing-fields ([8a4c342](https://github.com/TGIANN/cfx-lua-ls/commit/8a4c342fbf8608aa1df3f1edd089d07473499b40))
* fields written on a class variable are seen through its values; strings read stringlib ([1af746f](https://github.com/TGIANN/cfx-lua-ls/commit/1af746fc15f48a620fef5264476f3a808b21a624))
* generic arguments of parent classes, methods found through the object's class, class generics in method returns ([7517971](https://github.com/TGIANN/cfx-lua-ls/commit/7517971e89b1c260c01a767de77ddb254d7f1689))
* generic class operators, params from parent fun fields, LuaLS overload scoring ([56ecf8e](https://github.com/TGIANN/cfx-lua-ls/commit/56ecf8e80baa3f464e696febe9b0aa14afe607f7))
* generics bound late on returned funs; self of : functions; inherited member functions ([2d2c763](https://github.com/TGIANN/cfx-lua-ls/commit/2d2c763b7d16ee2b824857bbe1c965baedf26ace))
* **ide:** Lua.runtime.special for every builtin the type engine knows, Lua.hover.expandAlias, inference 524 -&gt; 608 of 608 (see docs/ROADMAP.md phase 5) ([8d3ee8b](https://github.com/TGIANN/cfx-lua-ls/commit/8d3ee8b2ae31e446393ff41e1033e72549443a44))
* **ide:** package visibility across files (---[@field](https://github.com/field) package, ---[@package](https://github.com/package), Lua.doc.packageName) reports invisible ([d7d8bab](https://github.com/TGIANN/cfx-lua-ls/commit/d7d8bab6585018763cd69bafd8c006c53b09afa0))
* inferred types print as LuaLS prints them; type cases run with their LuaLS settings ([ecf1cc8](https://github.com/TGIANN/cfx-lua-ls/commit/ecf1cc882bfa27d5cd7eadf8b903bc5aadc1a4fe))
* inline fun&lt;T&gt; generics, callback returns and params bind generics, generic aliases, library generics (next) ([9274ab8](https://github.com/TGIANN/cfx-lua-ls/commit/9274ab83b63a952734218cba4bfc187e8d25c5a4))
* **lint:** add name-style-check, spell-check, codestyle-check and unnecessary-assert ([cd6f1d3](https://github.com/TGIANN/cfx-lua-ls/commit/cd6f1d316e623b958930222eedc08ee76f3ff638))
* **lint:** LuaLS diagnostic groups, first finding per place, narrower unnecessary-assert ([c3c7a25](https://github.com/TGIANN/cfx-lua-ls/commit/c3c7a258ddac7e153b8d2fdbad724df87c094c71))
* LuaLS type switches, cfx.lualsCompat, every LuaLS diagnostic case scored with its settings ([bb027ec](https://github.com/TGIANN/cfx-lua-ls/commit/bb027ecc1908dd3dbdb2e7ab59ff7fec1d2a3898))
* type checks compare table shapes, class-tagged local values and fields typed by the class's own writes ([384181c](https://github.com/TGIANN/cfx-lua-ls/commit/384181c29f7253ad20a8752ab0de4a2fd848c0cd))
* variadic return tails, generic shapes, closure params from bound generics, bare generic classes, integer-keyed constructor reads ([9fd9456](https://github.com/TGIANN/cfx-lua-ls/commit/9fd9456ea9f345217e7b2eb83cd5d869c7b4ce26))


### Bug Fixes

* **ide:** hover paths are read in a loop, so a.b.b... chains thousands of links long no longer overflow the stack ([5b3face](https://github.com/TGIANN/cfx-lua-ls/commit/5b3face02bc431446564f54359d5029415414bf7))
* **lint:** unnecessary-assert checks version and side; VS Code declares Lua.spell.dict and Lua.nameStyle.config ([a9aebaf](https://github.com/TGIANN/cfx-lua-ls/commit/a9aebaf5acae330bda57b9451133cc683e0e7cc9))
* LuaLS's nil rules under strict nil checks; harness guards compat cases and runs compat alone ([901ad2a](https://github.com/TGIANN/cfx-lua-ls/commit/901ad2a5b76dca0d325867bebcb22df9fffc1d16))


### Performance

* index class field writes and class-tagged globals once per file; library calls parse docs only for generics ([15b1be9](https://github.com/TGIANN/cfx-lua-ls/commit/15b1be9247e331a46efc9ea693a4e516e39beca5))


### Documentation

* **roadmap:** record the corpus A baseline measurement ([8965621](https://github.com/TGIANN/cfx-lua-ls/commit/8965621ac98f5282ea7bcbf593bfc643f55484f8))

## 1.0.0 (2026-11-18)


### New Features

* assign-type-mismatch checks table constructor entries against the declared type ([6341350](https://github.com/TGIANN/cfx-lua-ls/commit/6341350ea3a9464f24229e3bd3ff695cc546d02b))
* cast-local-type and assign-type-mismatch read ---[@type](https://github.com/type) on assignments, literal types, fractional numbers and table&lt;K, V&gt; ([85ecfaf](https://github.com/TGIANN/cfx-lua-ls/commit/85ecfaf081a857dbb38a3dac456d98016b6d21bc))
* cfx-lua-ls, a Lua language server for FiveM (CfxLua), ported from LuaLS ([8e94d8e](https://github.com/TGIANN/cfx-lua-ls/commit/8e94d8ebd8bea3bed0d087cfc92859a53f7a5f2b))
* inference narrows the falsy side, a truthy boolean to true, and type() tests inside and ([6fb7eb0](https://github.com/TGIANN/cfx-lua-ls/commit/6fb7eb098ced89f67c491e09bdc290761e46b014))
* inference reads detached ---[@cast](https://github.com/cast), same-line ---[@type](https://github.com/type) and types and/or as LuaLS does ([21d7eda](https://github.com/TGIANN/cfx-lua-ls/commit/21d7edac67bd7938d548d0f3e4e123ef68cbb3fe))
* type checks read enums and aliases, typed local values, and multi-value calls returned last ([9787701](https://github.com/TGIANN/cfx-lua-ls/commit/978770179491d04ab1141b4e63a7b2a7949b330a))
* typed references follow a call's later values and generic returns ([027aba4](https://github.com/TGIANN/cfx-lua-ls/commit/027aba4eaeea87a109909c0af04847e84f478c0e))


### Bug Fixes

* typed field rename refuses a global table handed on in the resources searched ([6a059ba](https://github.com/TGIANN/cfx-lua-ls/commit/6a059ba3ae27e478f61c14ee7f71b3be5e4dfd4f))


### Performance

* cache each local's declaring statement and the typed field writes, linear on long blocks ([06c563d](https://github.com/TGIANN/cfx-lua-ls/commit/06c563dcaf0000d4d879f6acd8add541b00f13ee))


### Miscellaneous Chores

* release 1.0.0 ([ef91832](https://github.com/TGIANN/cfx-lua-ls/commit/ef91832519cddc43dd3170e190b8f8c17ce8264b))
