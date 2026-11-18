---@meta coroutine

---
---@class coroutinelib
coroutine = {}

---Creates a new coroutine, with body `f`. `f` must be a function. Returns this new coroutine, an object with type `"thread"`.
---@param f async fun(...):...
---@return thread
---@nodiscard
function coroutine.create(f) end

---@param co? thread
---@return boolean
---@nodiscard
function coroutine.isyieldable(co) end

---@version >5.4
---Closes coroutine `co` , closing all its pending to-be-closed variables and putting the coroutine in a dead state.
---@param co thread
---@return boolean noerror
---@return any errorobject
function coroutine.close(co) end

---Starts or continues the execution of coroutine `co`.
---@param co    thread
---@param val1? any
---@return boolean success
---@return any ...
function coroutine.resume(co, val1, ...) end

---Returns the running coroutine plus a boolean, true when the running coroutine is the main one.
---@return thread running
---@return boolean ismain
---@nodiscard
function coroutine.running() end

---Returns the status of coroutine `co`.
---@param co thread
---@return
---| '"running"'   # ---#DESTAIL 'costatus.running'
---| '"suspended"' # ---#DESTAIL 'costatus.suspended'
---| '"normal"'    # ---#DESTAIL 'costatus.normal'
---| '"dead"'      # ---#DESTAIL 'costatus.dead'
---@nodiscard
function coroutine.status(co) end

---Creates a new coroutine, with body `f`; `f` must be a function. Returns a function that resumes the coroutine each time it is called.
---@param f async fun(...):...
---@return fun(...):...
---@nodiscard
function coroutine.wrap(f) end

---Suspends the execution of the calling coroutine.
---@async
---@return any ...
function coroutine.yield(...) end

return coroutine

