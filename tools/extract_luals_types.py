"""Extracts LuaLS's type inference cases into JSON for crates/ide/tests/luals_types.rs.

    python -I tools/extract_luals_types.py ref/luals/test crates/ide/tests/data/luals_types.json

Each `TEST 'view' [[...]]` (also `TEST "view"` and `TEST [[view]]`) becomes
{"file", "n", "want", "script", "settings"} with the `<?x?>` marker left in;
`settings` maps the `config.set` switches in force for the case that differ
from the test's defaults to their values (a bool, number, string or list).
"""

import json
import re
import sys
from pathlib import Path

LONG = r"\[(=*)\[(.*?)\]\1\]"
WANT = r"(?:'([^']*)'|\"([^\"]*)\"|\[(=*)\[(.*?)\]\3\])"


CONFIG = re.compile(r"^config\.set\(\s*nil\s*,\s*['\"]([^'\"]+)['\"]\s*,\s*(.*?)\)\s*$", re.M | re.S)
# The values the test file starts from.
DEFAULTS = {"Lua.hover.expandAlias": True, "Lua.hover.enumsLimit": 5, "Lua.type.inferParamType": False}


def lua_value(value):
    """`true`, `5`, `'lua'`, `{ 'a', 'b' }` as JSON values."""
    value = value.strip()
    if value in ("true", "false"):
        return value == "true"
    if value == "nil":
        return None
    if re.fullmatch(r"-?[0-9]+", value):
        return int(value)
    strings = [a or b for a, b in re.findall(r"'([^']*)'|\"([^\"]*)\"", value)]
    if value.startswith("{"):
        return strings
    return strings[0] if strings else value


def cases(text):
    tests = [(m.start(), "test", m) for m in re.finditer(r"^TEST\s*" + WANT + r"\s*" + r"\[(=*)\[(.*?)\]\5\]", text, re.S | re.M)]
    tests += [(m.start(), "config", m) for m in CONFIG.finditer(text)]
    tests.sort(key=lambda e: e[0])
    active = {}
    for _, kind, m in tests:
        if kind == "config":
            key, value = m.group(1), lua_value(m.group(2))
            if value is None or DEFAULTS.get(key, None) == value or (value == [] and key not in DEFAULTS):
                active.pop(key, None)
            else:
                active[key] = value
            continue
        want = next(g for g in (m.group(1), m.group(2), m.group(4)) if g is not None)
        script = m.group(6)
        if script.startswith("\n"):
            script = script[1:]
        yield want, script, dict(active)


def main():
    root, out = Path(sys.argv[1]), Path(sys.argv[2])
    result = []
    for name in ["common", "param_match", "field_override", "generic_operator"]:
        text = (root / "type_inference" / f"{name}.lua").read_text(encoding="utf-8")
        for n, (want, script, settings) in enumerate(cases(text), 1):
            if "<?" in script:
                result.append({"file": name, "n": n, "want": want, "script": script, "settings": settings})
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(result, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    print(len(result), "cases")


main()
