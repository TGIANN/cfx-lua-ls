# Changelog

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
