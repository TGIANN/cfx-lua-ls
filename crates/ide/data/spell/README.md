# Spell-check dictionaries

`spell-check` reads these two lists, as LuaLS does:

- `dictionary.txt`: LuaLS's `meta/spell/dictionary.txt` (LuaLS 3.19.2, commit
  a58fc7b), an English word list with counts; EmmyLuaCodeStyle ships the same
  file as `resources/dictionary.txt`.
- `lua_dict.txt`: LuaLS's `meta/spell/lua_dict.txt`, Lua words the English list
  lacks.

Both come from lua-language-server, MIT License, which `LICENSE` here carries.
Line endings are LF; the words and counts are unchanged.
