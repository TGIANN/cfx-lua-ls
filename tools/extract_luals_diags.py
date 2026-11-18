"""Extracts LuaLS's diagnostic cases into JSON for crates/ide/tests/luals_diags.rs.

    python -I tools/extract_luals_diags.py ref/luals/test/diagnostics crates/ide/tests/data/luals_diags.json

Each `TEST [[...]]` (also `TEST([[...]], 'Lua 5.4')` and `TEST [=[...]=]`) of
`<code>.lua` becomes {"code", "n", "script", "config", "settings"} with the
`<!x!>` markers left in. `config` lists the `config.set` / `config.add`
switches in force for the case that differ from the test's defaults, and a
runtime version other than the default; `settings` maps each switch to its
value (a bool, a string or a list of strings).
"""

import json
import re
import sys
from pathlib import Path

TEST = re.compile(r"^TEST\s*\(?\s*\[(=*)\[(.*?)\]\1\]", re.S | re.M)
CONFIG = re.compile(r"^config\.(set|add|remove)\(\s*nil\s*,\s*'([^']+)'\s*,\s*(.*?)\)\s*$", re.M | re.S)
VERSION = re.compile(r"\s*,\s*'(Lua [0-9.]+|LuaJIT)'")


def lua_value(value):
    """`true`, `'lua'`, `{ 'a', 'b' }` as JSON values."""
    if value in ("true", "false"):
        return value == "true"
    strings = re.findall(r"'([^']*)'|\"([^\"]*)\"", value)
    strings = [a or b for a, b in strings]
    if value.lstrip().startswith("{"):
        return strings
    return strings[0] if strings else value


def cases(text):
    events = [(m.start(), "test", m) for m in TEST.finditer(text)]
    events += [(m.start(), "config", m) for m in CONFIG.finditer(text)]
    events.sort(key=lambda e: e[0])
    active = {}
    for _, kind, m in events:
        if kind == "config":
            op, key, value = m.group(1), m.group(2), m.group(3).strip()
            if op == "remove" or value in ("nil", "false"):
                active.pop(key, None)
            elif op == "add":
                active.setdefault(key, []).append(lua_value(value))
            else:
                active[key] = lua_value(value)
            continue
        script = m.group(2)
        if script.startswith("\n"):
            script = script[1:]
        config = sorted(active)
        settings = dict(active)
        version = VERSION.match(text, m.end())
        if version:
            config.append("Lua.runtime.version=" + version.group(1))
            settings["Lua.runtime.version"] = version.group(1)
        yield script, config, settings


def main():
    root, out = Path(sys.argv[1]), Path(sys.argv[2])
    result = []
    for path in sorted(root.glob("*.lua")):
        if path.stem == "init":
            continue
        text = path.read_text(encoding="utf-8")
        for n, (script, config, settings) in enumerate(cases(text)):
            result.append({"code": path.stem, "n": n, "script": script, "config": config, "settings": settings})
    out.write_text(json.dumps(result, indent=0, ensure_ascii=False) + "\n", encoding="utf-8", newline="\n")
    print(f"{len(result)} cases from {root}")


if __name__ == "__main__":
    main()
