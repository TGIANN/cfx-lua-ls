"""Extracts LuaLS's type inference cases into JSON for crates/ide/tests/luals_types.rs.

    python -I tools/extract_luals_types.py ref/luals/test crates/ide/tests/data/luals_types.json

Each `TEST 'view' [[...]]` (also `TEST "view"` and `TEST [[view]]`) becomes
{"file", "n", "want", "script"} with the `<?x?>` marker left in.
"""

import json
import re
import sys
from pathlib import Path

LONG = r"\[(=*)\[(.*?)\]\1\]"
WANT = r"(?:'([^']*)'|\"([^\"]*)\"|\[(=*)\[(.*?)\]\3\])"


def cases(text):
    for m in re.finditer(r"^TEST\s*" + WANT + r"\s*" + r"\[(=*)\[(.*?)\]\5\]", text, re.S | re.M):
        want = next(g for g in (m.group(1), m.group(2), m.group(4)) if g is not None)
        script = m.group(6)
        if script.startswith("\n"):
            script = script[1:]
        yield want, script


def main():
    root, out = Path(sys.argv[1]), Path(sys.argv[2])
    result = []
    for name in ["common", "param_match", "field_override", "generic_operator"]:
        text = (root / "type_inference" / f"{name}.lua").read_text(encoding="utf-8")
        for n, (want, script) in enumerate(cases(text), 1):
            if "<?" in script:
                result.append({"file": name, "n": n, "want": want, "script": script})
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(result, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    print(len(result), "cases")


main()
