"""Extracts LuaLS's definition, references, highlight and rename cases into JSON
for crates/ide/tests/luals_names.rs.

    python -I tools/extract_luals_names.py ref/luals/test crates/ide/tests/data/luals_names.json

Each `TEST [[...]]` becomes {"suite", "file", "n", "script"} with the markers
(`<!x!>`, `<?x?>`, `<~x~>`) left in; rename cases `TEST ('a', 'b') [[old]] [[new]]`
become {"suite": "rename", "old", "new", "before", "after"}.
"""

import json
import re
import sys
from pathlib import Path

LONG = r"\[(=*)\[(.*?)\]\1\]"


def blocks(text):
    """`TEST [[...]]` / `TEST([[...]])` scripts in order."""
    for m in re.finditer(r"\bTEST\s*\(?\s*" + LONG, text, re.S):
        script = m.group(2)
        if script.startswith("\n"):
            script = script[1:]
        yield script


def renames(text):
    pat = r"\bTEST\s*\(\s*'([^']*)'\s*,\s*'([^']*)'\s*\)\s*\[(=*)\[(.*?)\]\3\]\s*\[(=*)\[(.*?)\]\5\]"
    for m in re.finditer(pat, text, re.S):
        before, after = m.group(4), m.group(6)
        yield m.group(1), m.group(2), before.removeprefix("\n"), after.removeprefix("\n")


def main():
    root, out = Path(sys.argv[1]), Path(sys.argv[2])
    cases = []
    for suite, files in [
        ("definition", ["local", "label", "arg", "set", "function"]),
        ("references", ["common", "all"]),
        ("highlight", ["init"]),
    ]:
        for name in files:
            text = (root / suite / f"{name}.lua").read_text(encoding="utf-8")
            for n, script in enumerate(blocks(text), 1):
                cases.append({"suite": suite, "file": name, "n": n, "script": script})
    text = (root / "rename" / "init.lua").read_text(encoding="utf-8")
    for n, (old, new, before, after) in enumerate(renames(text), 1):
        cases.append({"suite": "rename", "file": "init", "n": n, "old": old, "new": new, "before": before, "after": after})
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(cases, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    print(len(cases), "cases")


main()
