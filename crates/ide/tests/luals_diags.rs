//! LuaLS diagnostic cases, extracted to `data/luals_diags.json` by
//! `tools/extract_luals_diags.py`: the ranges a rule reports against LuaLS's
//! `<!x!>` markers, scored as LuaLS scores them (no hit: no marker may be
//! there; hits: exactly the marked ranges).
//!
//! `data/luals_diags_pass.txt` lists the cases that pass (`code n`). A listed
//! case that fails, or a passing case not listed, fails the test.
//! `LUALS_DIAGS_WRITE=1` rewrites the list, `LUALS_DIAGS_SHOW=1` prints every
//! failing case of the rules this server has, `LUALS_DIAGS_RULE=code` only that
//! rule's. A case run with LuaLS switches (`settings`) runs with the same
//! settings here. A case that fails is run again with `cfx.lualsCompat` (LuaLS's
//! own behaviour where this server is deliberately narrower by default) and
//! listed as `code n compat` when it passes that way. A setting this server
//! does not read keeps the case unscored.
//!
//! Rules with an `extra` count report something LuaLS does not: those are the
//! false alarms that matter, unlike missed markers.

use cfx_lua_ide::lint::{LintConfig, RULES, Severity};
use cfx_lua_ide::workspace::{Workspace, lint_file};
use cfx_lua_ide::{names, parse};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// The text without markers and the marked byte ranges.
fn markers(script: &str) -> (String, BTreeSet<(usize, usize)>) {
    let mut text = String::new();
    let mut open = Vec::new();
    let mut ranges = BTreeSet::new();
    let mut rest = script;
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix("<!") {
            open.push(text.len());
            rest = r;
        } else if let Some(r) = rest.strip_prefix("!>").filter(|_| !open.is_empty()) {
            ranges.insert((open.pop().unwrap(), text.len()));
            rest = r;
        } else {
            let c = rest.chars().next().unwrap();
            text.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    (text, ranges)
}

enum Outcome {
    Pass,
    Missed,
    Extra,
}

fn run(ws: &Workspace, path: &std::path::Path, config: &LintConfig, code: &str, script: &str) -> (Outcome, String) {
    let (text, want) = markers(script);
    let p = parse(&text);
    let n = names(&p, &text);
    let got: BTreeSet<(usize, usize)> = lint_file(ws, path, &p, &text, &n, config)
        .into_iter()
        .filter(|l| l.code == code)
        .map(|l| (usize::from(l.range.start()), usize::from(l.range.end())))
        .collect();
    let shown = |s: &BTreeSet<(usize, usize)>| s.iter().map(|&(a, b)| format!("{:?}", &text[a..b])).collect::<Vec<_>>().join(", ");
    let detail = format!("want [{}], got [{}]", shown(&want), shown(&got));
    let outcome = if got == want || (got.is_empty() && want.is_empty()) {
        Outcome::Pass
    } else if got.iter().any(|g| !want.contains(g)) {
        Outcome::Extra
    } else {
        Outcome::Missed
    };
    (outcome, detail)
}

/// The case's LuaLS settings on top of `base`; None for a setting not read here.
fn with_settings(base: &LintConfig, settings: &serde_json::Map<String, Value>) -> Option<LintConfig> {
    let mut c = base.clone();
    let strings = |v: &Value| -> Vec<String> { v.as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect()).unwrap_or_default() };
    for (k, v) in settings {
        match k.as_str() {
            "Lua.diagnostics.globals" => c.globals.extend(strings(v)),
            "Lua.diagnostics.unusedLocalExclude" => c.unused_local_exclude.extend(strings(v)),
            "Lua.doc.privateName" => c.private_names = strings(v),
            "Lua.doc.protectedName" => c.protected_names = strings(v),
            "Lua.doc.regengine" => c.lua_name_patterns = v.as_str() == Some("lua"),
            "Lua.type.weakNilCheck" => c.weak_nil_check = v.as_bool(),
            "Lua.type.weakUnionCheck" => c.weak_union_check = v.as_bool() == Some(true),
            "Lua.type.castNumberToInteger" => c.cast_number_to_integer = v.as_bool() == Some(true),
            "Lua.type.checkTableShape" => c.check_table_shape = v.as_bool() == Some(true),
            _ => return None,
        }
    }
    Some(c)
}

#[test]
fn luals_diagnostics() {
    let data: Vec<Value> = serde_json::from_str(include_str!("data/luals_diags.json")).unwrap();
    let listed: BTreeSet<String> = include_str!("data/luals_diags_pass.txt").lines().map(str::to_string).filter(|l| !l.is_empty()).collect();
    let show = std::env::var_os("LUALS_DIAGS_SHOW").is_some();
    let only = std::env::var("LUALS_DIAGS_RULE").ok();
    let root = std::env::temp_dir().join(format!("cfx-luals-diags-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let ws = Workspace::scan(std::slice::from_ref(&root));
    let path = root.join("test.lua");
    // Every rule on, as LuaLS's test turns every file status to `Any!`.
    let mut config = LintConfig::default();
    for (code, _) in RULES {
        config.severity.insert(code.to_string(), Some(Severity::Warning));
    }
    let ours: BTreeSet<&str> = RULES.iter().map(|(c, _)| *c).collect();
    let mut compat = config.clone();
    compat.luals_compat = true;
    let mut passing = BTreeSet::new();
    let mut unscored = Vec::new();
    let mut details: BTreeMap<String, String> = BTreeMap::new();
    // Every case again with `cfx.lualsCompat` alone, no fallback.
    let mut compat_only = 0;
    let mut compat_fails = Vec::new();
    // Rule -> (cases, passed, extra).
    let mut per_rule: BTreeMap<&str, (usize, usize, usize)> = BTreeMap::new();
    for c in &data {
        let code = c["code"].as_str().unwrap();
        let id = format!("{code} {}", c["n"]);
        let script = c["script"].as_str().unwrap();
        let settings = c["settings"].as_object().unwrap();
        let (Some(config), Some(compat)) = (with_settings(&config, settings), with_settings(&compat, settings)) else {
            unscored.push(id);
            continue;
        };
        let attempt = |config: &LintConfig| {
            std::panic::catch_unwind(|| run(&ws, &path, config, code, script)).unwrap_or_else(|_| (Outcome::Extra, "<panic>".to_string()))
        };
        let (mut outcome, detail) = attempt(&config);
        let compat_pass = matches!(attempt(&compat).0, Outcome::Pass);
        if compat_pass {
            compat_only += 1;
        } else {
            compat_fails.push(id.clone());
        }
        let mut id = id;
        if !matches!(outcome, Outcome::Pass) && compat_pass {
            outcome = Outcome::Pass;
            id.push_str(" compat");
        }
        let entry = per_rule.entry(code).or_default();
        entry.0 += 1;
        match outcome {
            Outcome::Pass => {
                entry.1 += 1;
                passing.insert(id);
            }
            failed => {
                if matches!(failed, Outcome::Extra) {
                    entry.2 += 1;
                }
                details.insert(id.clone(), detail.clone());
                if ours.contains(code) && (show || only.as_deref() == Some(code)) && only.as_deref().is_none_or(|o| o == code) {
                    println!("--- {id}: {detail}\n{script}");
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(&root);
    let with_compat = passing.iter().filter(|p| p.ends_with(" compat")).count();
    println!(
        "{} of {} LuaLS diagnostic cases pass ({} of them with cfx.lualsCompat; {} with cfx.lualsCompat on for every case); not scored: {:?}",
        passing.len(),
        per_rule.values().map(|r| r.0).sum::<usize>(),
        with_compat,
        compat_only,
        unscored
    );
    if !compat_fails.is_empty() {
        println!("  failing with cfx.lualsCompat on: {compat_fails:?}");
    }
    for (code, (cases, passed, extra)) in &per_rule {
        if ours.contains(code) {
            println!("  {code}: {passed}/{cases}{}", if *extra > 0 { format!(", {extra} extra") } else { String::new() });
        }
    }
    if std::env::var_os("LUALS_DIAGS_WRITE").is_some() {
        let out: Vec<&str> = passing.iter().map(String::as_str).collect();
        std::fs::write(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/luals_diags_pass.txt"), out.join("\n") + "\n").unwrap();
        return;
    }
    // A listed `code n compat` that no longer passes either way counts too.
    let lost: Vec<String> = listed
        .difference(&passing)
        .map(|l| format!("{l}: {}", details.get(l.trim_end_matches(" compat")).map_or("passes another way now", String::as_str)))
        .collect();
    assert!(lost.is_empty(), "listed cases that fail now:\n{}", lost.join("\n"));
    let new: Vec<&String> = passing.difference(&listed).collect();
    assert!(new.is_empty(), "cases that pass now but are not listed (LUALS_DIAGS_WRITE=1): {new:?}");
}
