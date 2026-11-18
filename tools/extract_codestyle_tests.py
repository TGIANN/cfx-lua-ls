"""Extracts EmmyLuaCodeStyle's format tests into JSON for crates/fmt/tests.

    python -I tools/extract_codestyle_tests.py ref/emmylua-codestyle/Test/src crates/fmt/tests/data/codestyle_tests.json

Each TEST block is replayed in order: `LuaStyle name;` starts a style, every
`name.field = value;` / `name.field.SetAll(value);` adds to it (cumulative, as in
C++), and every TestFormatted / TestRangeFormatted call becomes a case carrying the
assignments made so far to the style it passes (none for the default style).
Right-hand sides stay C++ text; the Rust test maps them and fails on unknown ones.
"""

import json
import re
import sys
from pathlib import Path

FILES = ["FormatStyle_unitest.cpp", "FormatResult_unitest.cpp", "RangeFormat_unitest.cpp"]

TEST_RE = re.compile(r"^TEST\((\w+),\s*(\w+)\)\s*\{", re.M)


def raw_string(src, at):
    """The raw string literal starting at `at` (R"delim( ... )delim"), and the index after it."""
    assert src.startswith('R"', at), src[at:at + 20]
    open_paren = src.index("(", at)
    delim = src[at + 2:open_paren]
    close = ")" + delim + '"'
    end = src.index(close, open_paren + 1)
    return src[open_paren + 1:end], end + len(close)


def split_args(src, at):
    """Arguments of the call whose '(' is at `at`: raw strings decoded, the rest stripped."""
    assert src[at] == "("
    args, i, cur = [], at + 1, ""
    depth = 0
    while True:
        c = src[i]
        if src.startswith('R"', i) and not cur.strip():
            text, i = raw_string(src, i)
            args.append(("raw", text))
            cur = None
            continue
        if c == "(":
            depth += 1
        elif c == ")":
            if depth == 0:
                if cur is not None and cur.strip():
                    args.append(("code", cur.strip()))
                return args, i + 1
            depth -= 1
        elif c == "," and depth == 0:
            if cur is not None and cur.strip():
                args.append(("code", cur.strip()))
            cur = ""
            i += 1
            continue
        if cur is None:
            cur = ""
        cur += c
        i += 1


def blocks(src):
    starts = [m for m in TEST_RE.finditer(src)]
    for n, m in enumerate(starts):
        end = starts[n + 1].start() if n + 1 < len(starts) else len(src)
        yield f"{m.group(1)}.{m.group(2)}", src[m.end():end]


STMT_RE = re.compile(
    r"LuaStyle\s+(\w+)\s*;"
    r"|(\w+)\.([\w.]+)\s*=\s*([^;]+);"
    r"|(\w+)\.([\w.]+)\.SetAll\(([^;]*)\)\s*;"
    r"|TestHelper::(TestFormatted|TestRangeFormatted)\s*\("
)


def extract(path):
    src = path.read_text(encoding="utf-8-sig")
    cases = []
    for name, body in blocks(src):
        styles = {}
        i = 0
        while True:
            m = STMT_RE.search(body, i)
            if not m:
                break
            if m.group(1):
                styles[m.group(1)] = []
                i = m.end()
            elif m.group(2) and m.group(2) in styles:
                styles[m.group(2)].append([m.group(3), m.group(4).strip()])
                i = m.end()
            elif m.group(5) and m.group(5) in styles:
                styles[m.group(5)].append([m.group(6) + ".SetAll", m.group(7).strip()])
                i = m.end()
            elif m.group(8):
                args, i = split_args(body, m.end() - 1)
                case = {"name": name}
                if m.group(8) == "TestRangeFormatted":
                    case["range"] = [int(args[0][1]), int(args[1][1])]
                    args = args[2:]
                assert args[0][0] == "raw" and args[1][0] == "raw", (name, args)
                case["input"] = args[0][1]
                case["expected"] = args[1][1]
                if len(args) > 2:
                    var = args[2][1]
                    assert var in styles, (name, var)
                    case["style"] = [list(a) for a in styles[var]]
                else:
                    case["style"] = []
                cases.append(case)
            else:
                i = m.end()
    return cases


def main():
    src_dir, out = Path(sys.argv[1]), Path(sys.argv[2])
    cases = []
    for f in FILES:
        found = extract(src_dir / f)
        for c in found:
            c["file"] = f
        cases.extend(found)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(cases, indent=1, ensure_ascii=False) + "\n", encoding="utf-8", newline="\n")
    print(f"{len(cases)} cases")


if __name__ == "__main__":
    main()
