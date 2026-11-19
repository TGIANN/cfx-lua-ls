# Spell-check dictionaries

`spell-check` reads these two lists, as LuaLS does:

- `dictionary.txt`: the English frequency dictionary of SymSpell (Wolf Garbe, MIT), as LuaLS 3.19.2 ships it in `meta/spell/dictionary.txt` (LuaLS is MIT, commit a58fc7b).
- `lua_dict.txt`: LuaLS's `meta/spell/lua_dict.txt`, Lua words the English list lacks.

Line endings are LF; the words and counts are unchanged.
