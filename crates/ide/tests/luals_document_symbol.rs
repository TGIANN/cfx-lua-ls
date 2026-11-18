//! LuaLS `test/document_symbol/init.lua`, extracted to `data/luals_document_symbol.json`
//! by `tools/extract_luals_document_symbol.lua`. Every case must match exactly:
//! name, detail, kind, range, selection range, value range and nesting.

use cfx_lua_ide::{DocumentSymbol, TextRange, document_symbols, parse};
use serde_json::Value;

/// LuaLS positions are `line * 10000 + byte column`.
fn offset(text: &str, pos: u64) -> u64 {
    let (line, col) = ((pos / 10000) as usize, pos % 10000);
    let line_start: usize = text.split_inclusive('\n').take(line).map(str::len).sum();
    line_start as u64 + col
}

fn range_eq(text: &str, got: TextRange, want: &Value) -> bool {
    let (s, e) = (want[0].as_u64().unwrap(), want[1].as_u64().unwrap());
    u64::from(u32::from(got.start())) == offset(text, s) && u64::from(u32::from(got.end())) == offset(text, e)
}

fn check(text: &str, got: &[DocumentSymbol], want: &Value, path: &str, errors: &mut Vec<String>) {
    let want = want.as_array().unwrap();
    if got.len() != want.len() {
        errors.push(format!("{path}: {} symbols, expected {}", got.len(), want.len()));
        return;
    }
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        let here = format!("{path}[{i}] {}", g.name);
        let mut bad = Vec::new();
        if g.name != w["name"].as_str().unwrap() {
            bad.push(format!("name {:?} != {}", g.name, w["name"]));
        }
        if g.detail != w["detail"].as_str().unwrap() {
            bad.push(format!("detail {:?} != {}", g.detail, w["detail"]));
        }
        if format!("{:?}", g.kind) != w["kind"].as_str().unwrap() {
            bad.push(format!("kind {:?} != {}", g.kind, w["kind"]));
        }
        if !range_eq(text, g.range, &w["range"]) {
            bad.push(format!("range {:?} != {}", g.range, w["range"]));
        }
        if !range_eq(text, g.selection_range, &w["selectionRange"]) {
            bad.push(format!("selectionRange {:?} != {}", g.selection_range, w["selectionRange"]));
        }
        match (g.value_range, w.get("valueRange")) {
            (None, None) => {}
            (Some(r), Some(v)) if range_eq(text, r, v) => {}
            (r, v) => bad.push(format!("valueRange {r:?} != {v:?}")),
        }
        if !bad.is_empty() {
            errors.push(format!("{here}: {}", bad.join("; ")));
        }
        match w.get("children") {
            None if g.children.is_empty() => {}
            None => errors.push(format!("{here}: {} unexpected children", g.children.len())),
            Some(Value::String(s)) if s == "<exists>" => {
                if g.children.is_empty() {
                    errors.push(format!("{here}: expected children"));
                }
            }
            Some(children) => check(text, &g.children, children, &here, errors),
        }
    }
}

#[test]
fn luals_document_symbol() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("data/luals_document_symbol.json")).unwrap();
    assert!(cases.len() >= 30, "only {} cases", cases.len());
    let mut errors = Vec::new();
    for (i, case) in cases.iter().enumerate() {
        let text = case["script"].as_str().unwrap();
        let got = document_symbols(&parse(text), text);
        let mut case_errors = Vec::new();
        check(text, &got, &case["expect"], "", &mut case_errors);
        if !case_errors.is_empty() {
            errors.push(format!("case {i}: {text:?}\n  {}", case_errors.join("\n  ")));
        }
    }
    assert!(errors.is_empty(), "{} of {} cases failed:\n{}", errors.len(), cases.len(), errors.join("\n"));
}
