---@meta package

---@param modname string
---@return unknown
---@return unknown loaderdata
function require(modname) end

---
---@class packagelib
---The path used by `require` to search for a C loader.
---@field cpath     string
---A table used by `require` to control which modules are already loaded.
---@field loaded    table
---The path used by `require` to search for a Lua loader.
---@field path      string
---A table to store loaders for specific modules.
---@field preload   table
package = {}

---A string describing some compile-time configurations for packages.
package.config = [[
/
;
?
!
-]]

---@version <5.1
---A table used by `require` to control how to load modules.
package.loaders = {}

---Dynamically links the host program with the C library `libname`.
---@param libname string
---@param funcname string
---@return any
function package.loadlib(libname, funcname) end

---A table used by `require` to control how to load modules.
---@version >5.2
package.searchers = {}

---Searches for the given `name` in the given `path`.
---@version >5.2,JIT
---@param name string
---@param path string
---@param sep? string
---@param rep? string
---@return string? filename
---@return string? errmsg
---@nodiscard
function package.searchpath(name, path, sep, rep) end

---Sets a metatable for `module` with its `__index` field referring to the global environment, so that this module inherits values from the global environment. To be used as an option to function `module` .
---@version <5.1
---@param module table
function package.seeall(module) end

return package

