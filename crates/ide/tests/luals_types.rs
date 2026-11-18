//! LuaLS type inference cases, extracted to `data/luals_types.json` by
//! `tools/extract_luals_types.py`: the type at `<?x?>` against LuaLS's view.
//!
//! `data/luals_types_pass.txt` lists the cases that pass (`file n`, one per
//! line). A listed case that fails, or a passing case not listed, fails the test,
//! so the list stays true. `LUALS_TYPES_WRITE=1` rewrites the list;
//! `LUALS_TYPES_SHOW=1` prints every failing case.

use cfx_lua_ide::{TextSize, TypeSettings, names, parse, type_at_with};
use serde_json::Value;
use std::collections::BTreeSet;

fn run(script: &str, settings: &TypeSettings) -> Option<String> {
    let at = script.find("<?")?;
    let text = script.replacen("<?", "", 1).replacen("?>", "", 1);
    let text = text.replace("<?", "").replace("?>", "");
    let p = parse(&text);
    let n = names(&p, &text);
    type_at_with(&p, &n, &text, TextSize::from(at as u32), settings)
}

/// The case's LuaLS settings; None for one not read here.
fn settings_of(c: &Value) -> Option<TypeSettings> {
    let mut s = TypeSettings::default();
    for (k, v) in c["settings"].as_object()? {
        match k.as_str() {
            "Lua.hover.expandAlias" => s.expand_alias = v.as_bool()?,
            "Lua.hover.enumsLimit" => s.enums_limit = v.as_u64()? as usize,
            "Lua.type.inferParamType" => s.infer_param_type = v.as_bool()?,
            _ => return None,
        }
    }
    Some(s)
}

#[test]
fn luals_type_inference() {
    let data: Vec<Value> = serde_json::from_str(include_str!("data/luals_types.json")).unwrap();
    let listed: BTreeSet<String> = include_str!("data/luals_types_pass.txt").lines().map(str::to_string).filter(|l| !l.is_empty()).collect();
    let show = std::env::var_os("LUALS_TYPES_SHOW").is_some();
    let mut passing = BTreeSet::new();
    let mut lost = Vec::new();
    let mut unscored = Vec::new();
    for c in &data {
        let id = format!("{} {}", c["file"].as_str().unwrap(), c["n"]);
        let want = c["want"].as_str().unwrap();
        let script = c["script"].as_str().unwrap();
        let Some(settings) = settings_of(c) else {
            unscored.push(id);
            continue;
        };
        let got = std::panic::catch_unwind(|| run(script, &settings)).unwrap_or_else(|_| Some("<panic>".into()));
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
    println!("{} of {} LuaLS type inference cases pass; not scored: {unscored:?}", passing.len(), data.len() - unscored.len());
    if std::env::var_os("LUALS_TYPES_WRITE").is_some() {
        let out: Vec<&str> = passing.iter().map(String::as_str).collect();
        std::fs::write(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/luals_types_pass.txt"), out.join("\n") + "\n").unwrap();
        return;
    }
    assert!(lost.is_empty(), "listed cases that fail now:\n{}", lost.join("\n"));
    let new: Vec<&String> = passing.difference(&listed).collect();
    assert!(new.is_empty(), "cases that pass now but are not listed (LUALS_TYPES_WRITE=1): {new:?}");
}
