---@meta io

---
---@class iolib
---standard input.
---@field stdin  file*
---standard output.
---@field stdout file*
---standard error.
---@field stderr file*
io = {}

---@alias openmode
---|>"r"   # ---#DESTAIL 'openmode.r'
---| "w"   # ---#DESTAIL 'openmode.w'
---| "a"   # ---#DESTAIL 'openmode.a'
---| "r+"  # ---#DESTAIL 'openmode.r+'
---| "w+"  # ---#DESTAIL 'openmode.w+'
---| "a+"  # ---#DESTAIL 'openmode.a+'
---| "rb"  # ---#DESTAIL 'openmode.rb'
---| "wb"  # ---#DESTAIL 'openmode.wb'
---| "ab"  # ---#DESTAIL 'openmode.ab'
---| "r+b" # ---#DESTAIL 'openmode.r+b'
---| "w+b" # ---#DESTAIL 'openmode.w+b'
---| "a+b" # ---#DESTAIL 'openmode.a+b'

---Close `file` or default output file.
---@param file? file*
---@return true?     suc
---@return exitcode? exitcode
---@return integer?  code
function io.close(file) end

---Saves any written data to default output file.
function io.flush() end

---Sets `file` as the default input file.
---@overload fun():file*
---@param file string|file*
function io.input(file) end

---------
---```lua
---for c in io.lines(filename, ...) do
---    body
---end
---```
---@param filename string?
---@param ... readmode
---@return fun():any, ...
function io.lines(filename, ...) end

---Opens a file, in the mode specified in the string `mode`.
---@param filename string
---@param mode?    openmode
---@return file*?
---@return string? errmsg
---@nodiscard
function io.open(filename, mode) end

---Sets `file` as the default output file.
---@overload fun():file*
---@param file string|file*
function io.output(file) end

---@alias popenmode
---| "r" # ---#DESTAIL 'popenmode.r'
---| "w" # ---#DESTAIL 'popenmode.w'

---Starts program prog in a separated process.
---@param prog  string
---@param mode? popenmode
---@return file*?
---@return string? errmsg
function io.popen(prog, mode) end

---Reads the `file`, according to the given formats, which specify what to read.
---@param ... readmode
---@return any
---@return any ...
---@nodiscard
function io.read(...) end

---In case of success, returns a handle for a temporary file.
---@return file*
---@nodiscard
function io.tmpfile() end

---@alias filetype
---| "file"        # ---#DESTAIL 'filetype.file'
---| "closed file" # ---#DESTAIL 'filetype.closed file'
---| `nil`         # ---#DESTAIL 'filetype.nil'

---Checks whether `obj` is a valid file handle.
---@param file file*
---@return filetype
---@nodiscard
function io.type(file) end

---Writes the value of each of its arguments to default output file.
---@return file*
---@return string? errmsg
function io.write(...) end

---
---@class file*
local file = {}

---@alias readmode integer|string
---| "n"  # ---#DESTAIL 'readmode.n'
---| "a"  # ---#DESTAIL 'readmode.a'
---|>"l"  # ---#DESTAIL 'readmode.l'
---| "L"  # ---#DESTAIL 'readmode.L'

---@alias exitcode "exit"|"signal"

---@return true?     suc
---@return exitcode? exitcode
---@return integer?  code
function file:close() end

function file:flush() end

---@param ... readmode
---@return fun():any, ...
function file:lines(...) end

---@param ... readmode
---@return any
---@return any ...
---@nodiscard
function file:read(...) end

---@alias seekwhence
---| "set" # ---#DESTAIL 'seekwhence.set'
---|>"cur" # ---#DESTAIL 'seekwhence.cur'
---| "end" # ---#DESTAIL 'seekwhence.end'

---@param whence? seekwhence
---@param offset? integer
---@return integer offset
---@return string? errmsg
function file:seek(whence, offset) end

---@alias vbuf
---| "no"   # ---#DESTAIL 'vbuf.no'
---| "full" # ---#DESTAIL 'vbuf.full'
---| "line" # ---#DESTAIL 'vbuf.line'

---@param mode vbuf
---@param size? integer
function file:setvbuf(mode, size) end

---@param ... string|number
---@return file*?
---@return string? errmsg
function file:write(...) end

return io

