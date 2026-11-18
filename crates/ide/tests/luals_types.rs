//! LuaLS type inference cases, extracted to `data/luals_types.json` by
//! `tools/extract_luals_types.py`: the type at `<?x?>` against LuaLS's view.
//!
//! `data/luals_types_pass.txt` lists the cases that pass (`file n`, one per
//! line). A listed case that fails, or a passing case not listed, fails the test,
//! so the list stays true. `LUALS_TYPES_WRITE=1` rewrites the list;
//! `LUALS_TYPES_SHOW=1` prints every failing case.

use cfx_lua_ide::{TextSize, names, parse, type_at};
use serde_json::Value;
use std::collections::BTreeSet;

fn run(script: &str) -> Option<String> {
    let at = script.find("<?")?;
    let text = script.replacen("<?", "", 1).replacen("?>", "", 1);
    let text = text.replace("<?", "").replace("?>", "");
    let p = parse(&text);
    let n = names(&p, &text);
    type_at(&p, &n, &text, TextSize::from(at as u32))
}

#[test]
fn luals_type_inference() {
    let data: Vec<Value> = serde_json::from_str(include_str!("data/luals_types.json")).unwrap();
    let listed: BTreeSet<String> = include_str!("data/luals_types_pass.txt").lines().map(str::to_string).filter(|l| !l.is_empty()).collect();
    let show = std::env::var_os("LUALS_TYPES_SHOW").is_some();
    let mut passing = BTreeSet::new();
    let mut lost = Vec::new();
    for c in &data {
        let id = format!("{} {}", c["file"].as_str().unwrap(), c["n"]);
        let want = c["want"].as_str().unwrap();
        let script = c["script"].as_str().unwrap();
        let got = std::panic::catch_unwind(|| run(script)).unwrap_or_else(|_| Some("<panic>".into()));
        if got.as_deref() == Some(want) {
            passing.insert(id);
        } else {
            if listed.contains(&id) {
                lost.push(format!("{id}: want {want:?}, got {got:?}"));
            }
            if show {
                println!("{id}: want {want:?}, got {got:?}\n{script}");
            }
        }
    }
    println!("{} of {} LuaLS type inference cases pass", passing.len(), data.len());
    if std::env::var_os("LUALS_TYPES_WRITE").is_some() {
        let out: Vec<&str> = passing.iter().map(String::as_str).collect();
        std::fs::write(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/luals_types_pass.txt"), out.join("\n") + "\n").unwrap();
        return;
    }
    assert!(lost.is_empty(), "listed cases that fail now:\n{}", lost.join("\n"));
    let new: Vec<&String> = passing.difference(&listed).collect();
    assert!(new.is_empty(), "cases that pass now but are not listed (LUALS_TYPES_WRITE=1): {new:?}");
}
