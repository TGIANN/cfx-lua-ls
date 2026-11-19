//! LuaLS hover and completion cases, extracted to `data/luals_hover.json` and
//! `data/luals_completion.json` by `tools/extract_luals_cases.lua`.
//!
//! Hover: the text of our first ```lua block against LuaLS's, scored twice:
//! `exact` (the whole block) and `type` (for a value, the type after its
//! name: `local x: integer = 1` gives `integer`), which counts inference apart
//! from layout.
//!
//! Completion: the set of (label, kind) against LuaLS's, order ignored. Call
//! and keyword snippets (`f()`, kind `Snippet`) are dropped from LuaLS's list,
//! so the `callSnippet` switch and `IgnoreFunction` do not matter; our CFX
//! natives and runtime globals are dropped from our list; a case
//! whose list that leaves empty is not scored. An `include` case passes when
//! LuaLS's items are all among ours, an `<exists>` one when there are any.
//!
//! `data/luals_assist_pass.txt` lists the cases that pass (`hover-exact n`,
//! `hover-type n`, `completion file n`). A listed case that fails, or a passing
//! case not listed, fails the test. `LUALS_ASSIST_WRITE=1` rewrites the list,
//! `LUALS_ASSIST_SHOW=hover` / `completion` prints the failing cases. Cases run
//! with a non-default LuaLS switch (`config`) or a custom check are not scored.

use cfx_lua_ide::library::{Game, Origin, library};
use cfx_lua_ide::manifest::FileSide;
use cfx_lua_ide::workspace::Workspace;
use cfx_lua_ide::{CompleteOptions, CompletionKind, FileCtx, ShowWord, TextSize, complete_with, hover, names, parse};
use serde_json::Value;
use std::collections::BTreeSet;

/// The text without LuaLS's `<?x?>` / `<??>` markers and the first marked
/// range (a position is an empty range).
fn marked(script: &str) -> (String, Option<(usize, usize)>) {
    let mut text = String::new();
    let mut first = None;
    let mut open = None;
    let mut rest = script;
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix("<?") {
            open = Some(text.len());
            rest = r;
        } else if let Some(r) = rest.strip_prefix("?>").filter(|_| open.is_some()) {
            if first.is_none() {
                first = Some((open.unwrap(), text.len()));
            }
            open = None;
            rest = r;
        } else {
            let c = rest.chars().next().unwrap();
            text.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    (text, first)
}

fn kind_name(k: CompletionKind) -> &'static str {
    match k {
        CompletionKind::Keyword => "Keyword",
        CompletionKind::Local => "Variable",
        CompletionKind::Global => "Field",
        CompletionKind::Function => "Function",
        CompletionKind::Field => "Field",
        CompletionKind::Class => "Class",
        CompletionKind::Enum => "Enum",
        CompletionKind::EnumMember => "EnumMember",
        CompletionKind::Property => "Property",
        CompletionKind::Interface => "Interface",
        CompletionKind::Event => "Event",
        CompletionKind::Snippet => "Snippet",
        CompletionKind::Value => "Value",
        CompletionKind::Operator => "Operator",
        CompletionKind::Text => "Text",
    }
}

fn first_lua_block(markdown: &str) -> Option<String> {
    let rest = &markdown[markdown.find("```lua")? + 6..];
    let end = rest.find("```")?;
    Some(rest[..end].trim_matches(['\r', '\n']).to_string())
}

/// The type a value's hover gives: after the first `: `, up to ` = ` or ` {`.
/// None for functions and lines without a type.
fn value_type(line: &str) -> Option<&str> {
    if !["local ", "(global) ", "(field) ", "(parameter) "].iter().any(|p| line.starts_with(p)) {
        return None;
    }
    let rest = &line[line.find(": ")? + 2..];
    let end = [rest.find(" = "), rest.find(" {")].into_iter().flatten().min().unwrap_or(rest.len());
    Some(rest[..end].trim())
}

struct Scores {
    passing: BTreeSet<String>,
    shown: Vec<String>,
}

fn hover_cases(ws: &Workspace, path: &std::path::Path, show: bool, s: &mut Scores) -> (usize, usize, usize, usize) {
    let data: Vec<Value> = serde_json::from_str(include_str!("data/luals_hover.json")).unwrap();
    let (mut total, mut exact, mut typed, mut typed_ok) = (0, 0, 0, 0);
    for c in &data {
        if !c["config"].as_array().unwrap().is_empty() {
            continue;
        }
        let Some(want) = c["expect"].as_str() else { continue };
        let want = want.trim_matches(['\r', '\n']).replace("\r\n", "\n");
        let (text, Some((at, _))) = marked(c["script"].as_str().unwrap()) else { continue };
        total += 1;
        let n = c["n"].as_u64().unwrap();
        let got = std::panic::catch_unwind(|| {
            let p = parse(&text);
            let names = names(&p, &text);
            let cx = FileCtx { game: Game::Any, side: FileSide::Unknown, ws: Some((ws, path)) };
            hover(&p, &names, &text, TextSize::from(at as u32), cx).and_then(|h| first_lua_block(&h.markdown))
        })
        .unwrap_or_else(|_| Some("<panic>".to_string()))
        .unwrap_or_default();
        if got == want {
            exact += 1;
            s.passing.insert(format!("hover-exact {n}"));
        }
        if let Some(w) = want.lines().next().and_then(value_type) {
            typed += 1;
            if got.lines().next().and_then(value_type) == Some(w) {
                typed_ok += 1;
                s.passing.insert(format!("hover-type {n}"));
            }
        }
        if got != want && show {
            s.shown.push(format!("--- hover {n}\nwant: {want}\ngot:  {got}\n{text}"));
        }
    }
    (total, exact, typed, typed_ok)
}

fn completion_cases(ws: &Workspace, path: &std::path::Path, show: bool, s: &mut Scores) -> (usize, usize, usize) {
    let data: Vec<Value> = serde_json::from_str(include_str!("data/luals_completion.json")).unwrap();
    let (mut total, mut passed, mut nonempty) = (0, 0, 0);
    for c in &data {
        if !c["config"].as_array().unwrap().is_empty() || c["flags"].as_array().unwrap().iter().any(|f| f == "ContinueTyping") {
            continue;
        }
        let expect = &c["expect"];
        if expect.as_str() == Some("<function>") {
            continue;
        }
        let (text, Some((_, at))) = marked(c["script"].as_str().unwrap()) else { continue };
        let want: Option<BTreeSet<(String, String)>> = expect["items"].as_array().map(|items| {
            items
                .iter()
                // An item LuaLS builds at run time has no label here: left out.
                .filter_map(|i| Some((i["label"].as_str()?.to_string(), i["kind"].as_str()?.to_string())))
                .filter(|(label, kind)| kind != "Snippet" && !label.contains('('))
                .collect()
        });
        if want.as_ref().is_some_and(|w| w.is_empty()) {
            continue;
        }
        total += 1;
        let id = format!("completion {} {}", c["file"].as_str().unwrap(), c["n"]);
        let got: Option<BTreeSet<(String, String)>> = std::panic::catch_unwind(|| {
            let p = parse(&text);
            let names = names(&p, &text);
            let cx = FileCtx { game: Game::Any, side: FileSide::Unknown, ws: Some((ws, path)) };
            // CFX natives and runtime globals: not in LuaLS's world.
            let cfx = |label: &str| {
                let items = library().get(label);
                !items.is_empty() && items.iter().all(|i| i.origin != Origin::Lua)
            };
            // LuaLS's test passes the character before the cursor as the trigger.
            let trigger = text[..at].chars().last().filter(|c| *c != '\n' && !c.is_alphanumeric() && *c != '_');
            // LuaLS's completion tests run with `showWord = "Enable"`.
            let opts = CompleteOptions { trigger, show_word: ShowWord::Enable };
            complete_with(&p, &names, &text, TextSize::from(at as u32), cx, opts)
                .map(|r| r.items.into_iter().filter(|i| !cfx(&i.label)).map(|i| (i.label, kind_name(i.kind).to_string())).collect())
        })
        // A panic fails the case (it is no "no result").
        .unwrap_or_else(|_| Some([("<panic>".to_string(), "<panic>".to_string())].into()));
        let got_set = got.clone().unwrap_or_default();
        let any = !got_set.is_empty();
        // Snippets and functions are left out on both sides (LuaLS's are built
        // at run time, its function items are labelled `f(a, b)`).
        let got_set: BTreeSet<(String, String)> = got_set.into_iter().filter(|(_, k)| k != "Snippet" && k != "Function").collect();
        let ok = match &want {
            None if expect.as_str() == Some("<exists>") => any,
            None => got_set.is_empty(),
            Some(want) if expect["include"].as_bool().unwrap() => want.is_subset(&got_set),
            Some(want) => *want == got_set,
        };
        if ok {
            passed += 1;
            if want.is_some() {
                nonempty += 1;
            }
            s.passing.insert(id);
        } else if show {
            s.shown.push(format!("--- {id}\nwant: {expect}\ngot:  {got:?}\n{text}"));
        }
    }
    (total, passed, nonempty)
}

#[test]
fn luals_hover_and_completion() {
    let listed: BTreeSet<String> = include_str!("data/luals_assist_pass.txt").lines().map(str::to_string).filter(|l| !l.is_empty()).collect();
    let show = std::env::var("LUALS_ASSIST_SHOW").unwrap_or_default();
    let root = std::env::temp_dir().join(format!("cfx-luals-assist-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let ws = Workspace::scan(std::slice::from_ref(&root));
    let path = root.join("test.lua");
    let mut s = Scores { passing: BTreeSet::new(), shown: Vec::new() };
    let (ht, he, tt, tp) = hover_cases(&ws, &path, show == "hover", &mut s);
    let (ct, cp, cn) = completion_cases(&ws, &path, show == "completion", &mut s);
    let _ = std::fs::remove_dir_all(&root);
    for line in &s.shown {
        println!("{line}");
    }
    println!("hover: {he}/{ht} exact, types {tp}/{tt}; completion: {cp}/{ct} ({cn} with a list of items)");
    if std::env::var_os("LUALS_ASSIST_WRITE").is_some() {
        let out: Vec<&str> = s.passing.iter().map(String::as_str).collect();
        std::fs::write(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/luals_assist_pass.txt"), out.join("\n") + "\n").unwrap();
        return;
    }
    let lost: Vec<&String> = listed.difference(&s.passing).collect();
    assert!(lost.is_empty(), "listed cases that fail now: {lost:?}");
    let new: Vec<&String> = s.passing.difference(&listed).collect();
    assert!(new.is_empty(), "cases that pass now but are not listed (LUALS_ASSIST_WRITE=1): {new:?}");
}
