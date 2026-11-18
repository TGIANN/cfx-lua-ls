//! LuaLS `test/parser_test/syntax_check.lua` (commit in CLAUDE.md), extracted to
//! `data/luals_syntax_check.json` by `tools/extract_luals_syntax_check.lua`.
//!
//! Only the Lua 5.4 cases run; the others target Lua 5.1, 5.3, 5.5 or LuaJIT.
//! Cases before the file's first `Version = ...` line ran in LuaLS with no
//! version set; they are checked against Lua 5.4 (`lparser.c` decides whether
//! something is an error, the fixture decides the range).
//!
//! - no expectation: no errors at all;
//! - an expectation: exactly one error, with that code and range;
//! - `multi`: LuaLS reports several errors; one of ours has that code and range.

use cfx_lua_syntax::parse;
use serde_json::Value;

/// Lua 5.4 cases we do not follow, with the reason.
const SKIP: &[(&str, &str)] = &[];

#[test]
fn luals_syntax_check() {
    let data = include_str!("data/luals_syntax_check.json");
    let cases: Vec<Value> = serde_json::from_str(data).expect("valid JSON");
    let mut failures = Vec::new();
    let mut ran = 0;
    for (i, case) in cases.iter().enumerate() {
        if case["version"] != "Lua 5.4" {
            continue;
        }
        let script = case["script"].as_str().expect("script");
        if SKIP.iter().any(|(s, _)| *s == script) {
            continue;
        }
        ran += 1;
        let parse = parse(script);
        let errors = parse.errors();
        let shown: Vec<String> = errors
            .iter()
            .map(|e| format!("{} {}..{} {}", e.code, u32::from(e.range.start()), u32::from(e.range.end()), e.message))
            .collect();
        let expect = &case["expect"];
        let ok = match expect {
            Value::Bool(false) => errors.is_empty(),
            _ => {
                let code = expect["type"].as_str().expect("type");
                let target = &case["targets"][0];
                let (start, end) = (target[0].as_u64().expect("start"), target[1].as_u64().expect("end"));
                let matches = |e: &cfx_lua_syntax::SyntaxError| {
                    e.code == code
                        && u64::from(u32::from(e.range.start())) == start
                        && u64::from(u32::from(e.range.end())) == end
                };
                if expect.get("multi").is_some() { errors.iter().any(matches) } else { errors.len() == 1 && matches(&errors[0]) }
            }
        };
        if !ok {
            failures.push(format!("case {i}: {script:?}\n  expected {expect} at {}\n  got {shown:?}", case["targets"]));
        }
    }
    assert!(ran > 50, "only {ran} cases ran");
    assert!(failures.is_empty(), "{} of {ran} cases failed:\n{}", failures.len(), failures.join("\n"));
}
